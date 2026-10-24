//! Controllo degli aggiornamenti: abbina gli addon installati ai progetti di CurseForge e
//! sceglie, per ognuno, il file più recente per la versione del gioco dell'installazione.
//!
//! [`evaluate`] è pura: riceve la scansione, la risposta dei fingerprint e i mod, e non usa né
//! rete né disco. [`check_updates`] la collega al client e alla scansione; [`update_addons`]
//! rifà il controllo e installa i file nuovi con il modulo `install`.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde::Serialize;

use crate::addons::scan::{self, InstalledAddon};
use crate::install;
use crate::installations::Installation;
use crate::sources::curseforge::{
    CurseForgeClient, CurseForgeError, File, FileModule, FingerprintMatch, FingerprintMatches, Mod,
    RELEASE_TYPE_RELEASE, Transport,
};
use crate::store::InstalledRecord;

/// Tipi di versione del gioco di CurseForge (`gameVersionTypeId`), verificati il 2026-10-22 con
/// il test `live_version_types` di `sources::curseforge` sull'API reale.
pub const RETAIL: u32 = 517;
pub const CLASSIC_ERA: u32 = 67408;
/// "WoW Mists of Pandaria Classic". **Cambia con l'espansione**: quando la Classic progressiva
/// passa all'espansione successiva, questo id va aggiornato con il tipo nuovo (il test
/// `the_progressive_classic_is_mists` lo rende visibile nel diff).
pub const CLASSIC_PROGRESSIVE: u32 = 79434;
/// "WoW Burning Crusade Classic". L'elenco dei tipi di CurseForge non ha un tipo per la Classic
/// Anniversary, e i `.toc` dell'Anniversary usano il suffisso `_TBC` (warcraft.wiki.gg, "TOC
/// format": "Burning Crusade Classic and Classic Anniversary Edition"): gli addon Anniversary
/// sono pubblicati sotto questo tipo. Se CurseForge aggiungerà un tipo dedicato, va cambiato.
pub const ANNIVERSARY: u32 = 73246;
/// "WoW Forever".
pub const FOREVER: u32 = 88568;

/// I tipi di versione che l'app conosce, con il nome mostrato nel menu del frontend (in
/// inglese, come tutta l'interfaccia). Sono anche gli unici ammessi come versione forzata di
/// un addon (D6).
pub const VERSION_TYPES: [(u32, &str); 5] = [
    (RETAIL, "Retail"),
    (CLASSIC_ERA, "Classic Era"),
    (CLASSIC_PROGRESSIVE, "Classic"),
    (ANNIVERSARY, "Anniversary"),
    (FOREVER, "WoW Forever"),
];

/// Un tipo di versione del gioco per il frontend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VersionType {
    pub id: u32,
    pub name: &'static str,
}

/// I tipi noti, nell'ordine di [`VERSION_TYPES`]. Nessuna chiamata di rete.
pub fn version_types() -> Vec<VersionType> {
    VERSION_TYPES
        .iter()
        .map(|&(id, name)| VersionType { id, name })
        .collect()
}

pub fn is_known_version_type(id: u32) -> bool {
    VERSION_TYPES.iter().any(|&(known, _)| known == id)
}

/// Versioni del gioco forzate: id del progetto CurseForge → `gameVersionTypeId`.
pub type VersionOverrides = HashMap<u32, u32>;

/// Tipo di versione del gioco per il flavor di un'installazione.
///
/// - `_retail_` → Retail; `_classic_era_` → Classic Era;
/// - `_classic_` → la Classic progressiva attuale (Mists of Pandaria);
/// - `_anniversary_` → Burning Crusade Classic (vedi [`ANNIVERSARY`]);
/// - `_classic_beta_` → WoW Forever: è il client che usa quella cartella nell'installazione
///   reale descritta in `docs/architecture.md`. Se un'altra beta di Classic tornasse a usarla,
///   questa riga andrebbe rivista.
///
/// PTR, beta di Retail e Titan Reforged (81212) restano senza tipo finché non si sa con
/// certezza quale cartella usano: meglio un errore chiaro che un aggiornamento per la versione
/// sbagliata. Un seguito possibile è leggere il prodotto installato da `.build.info` invece di
/// dedurlo dal nome della cartella.
pub fn version_type_for(flavor: &str) -> Option<u32> {
    match flavor.trim_matches('_').to_ascii_lowercase().as_str() {
        "retail" => Some(RETAIL),
        "classic_era" => Some(CLASSIC_ERA),
        "classic" => Some(CLASSIC_PROGRESSIVE),
        "anniversary" => Some(ANNIVERSARY),
        "classic_beta" => Some(FOREVER),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileSummary {
    pub id: u32,
    pub name: String,
    pub date: String,
}

impl From<&File> for FileSummary {
    fn from(file: &File) -> Self {
        Self {
            id: file.id,
            name: if file.display_name.is_empty() {
                file.file_name.clone().unwrap_or_default()
            } else {
                file.display_name.clone()
            },
            date: file.file_date.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: u32,
    pub name: String,
    pub url: Option<String>,
    /// Se CurseForge permette di scaricare i file da un'app di terzi. Sconosciuto vale `false`.
    pub allow_mod_distribution: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum UpdateStatus {
    NotMatched,
    UpToDate,
    UpdateAvailable { latest: FileSummary },
    Error { message: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AddonUpdate {
    pub folder: String,
    /// Progetto CurseForge; più cartelle possono avere lo stesso progetto (per esempio DBM).
    pub project: Option<Project>,
    pub installed_file: Option<FileSummary>,
    #[serde(flatten)]
    pub status: UpdateStatus,
    /// La versione del gioco forzata per il progetto, se c'è: è quella usata per cercare il
    /// file più recente al posto di quella dell'installazione.
    pub version_override: Option<u32>,
    /// I nomi delle cartelle del file riconosciuto per questa cartella (vuoto se non
    /// riconosciuta): il frontend li usa per raggruppare le cartelle dello stesso file, anche
    /// quelle che CurseForge non riconosce una per una.
    pub project_folders: Vec<String>,
    /// Le cartelle (`modules`) del file installato, con il loro fingerprint: servono
    /// all'aggiornamento per togliere quelle che la versione nuova non ha più. Restano nel
    /// backend.
    #[serde(skip)]
    pub installed_modules: Vec<FileModule>,
}

/// Abbina e valuta. `version_type` è il `gameVersionTypeId` dell'installazione; `overrides`
/// lo sostituisce per i progetti che hanno una versione forzata.
pub fn evaluate(
    addons: &[InstalledAddon],
    matches: &FingerprintMatches,
    mods: &[Mod],
    version_type: u32,
    overrides: &VersionOverrides,
) -> Vec<AddonUpdate> {
    // Ogni fingerprint di cartella → il match del progetto che la contiene. Prima i match
    // parziali (il primo file che contiene il fingerprint), poi gli esatti, che li
    // sostituiscono: un match esatto vince sempre su uno parziale.
    let mut by_fingerprint: HashMap<u64, &FingerprintMatch> = HashMap::new();
    for found in &matches.partial_matches {
        for module in &found.file.modules {
            by_fingerprint.entry(module.fingerprint).or_insert(found);
        }
    }
    for found in &matches.exact_matches {
        for module in &found.file.modules {
            by_fingerprint.insert(module.fingerprint, found);
        }
    }
    let mods_by_id: HashMap<u32, &Mod> = mods.iter().map(|m| (m.id, m)).collect();

    addons
        .iter()
        .map(|addon| {
            let mut update = AddonUpdate {
                folder: addon.folder.clone(),
                project: None,
                installed_file: None,
                status: UpdateStatus::NotMatched,
                version_override: None,
                project_folders: Vec::new(),
                installed_modules: Vec::new(),
            };
            let Some(fingerprint) = addon.fingerprint else {
                update.status = UpdateStatus::Error {
                    message: "Unreadable folder: it cannot be recognized.".to_owned(),
                };
                return update;
            };
            let Some(found) = by_fingerprint.get(&u64::from(fingerprint)) else {
                return update;
            };
            let project = mods_by_id.get(&found.id);
            update.project = Some(Project {
                id: found.id,
                name: project.map(|m| m.name.clone()).unwrap_or_default(),
                url: project.and_then(|m| m.links.website_url.clone()),
                allow_mod_distribution: project
                    .and_then(|m| m.allow_mod_distribution)
                    .unwrap_or(false),
            });
            update.installed_file = Some(FileSummary::from(&found.file));
            update.installed_modules = found.file.modules.clone();
            update.project_folders = found.file.modules.iter().map(|m| m.name.clone()).collect();
            update.version_override = overrides.get(&found.id).copied();
            let target = update.version_override.unwrap_or(version_type);
            update.status = match newest_release(found, target) {
                Some(latest) if is_newer(latest, &found.file) => UpdateStatus::UpdateAvailable {
                    latest: FileSummary::from(latest),
                },
                _ => UpdateStatus::UpToDate,
            };
            update
        })
        .collect()
}

fn for_version_type(file: &File, version_type: u32) -> bool {
    file.sortable_game_versions
        .iter()
        .any(|v| v.game_version_type_id == Some(version_type))
}

/// Il file Release più recente per il tipo di versione, tra quelli che CurseForge indica come
/// ultimi per il progetto.
fn newest_release(found: &FingerprintMatch, version_type: u32) -> Option<&File> {
    found
        .latest_files
        .iter()
        .filter(|f| f.release_type == RELEASE_TYPE_RELEASE && f.is_available)
        .filter(|f| for_version_type(f, version_type))
        .max_by(|a, b| a.file_date.cmp(&b.file_date).then(a.id.cmp(&b.id)))
}

/// Le date sono ISO 8601 in UTC, quindi il confronto come testo segue il tempo; a parità di
/// data decide l'id, che CurseForge assegna in ordine crescente.
fn is_newer(candidate: &File, installed: &File) -> bool {
    candidate.id != installed.id
        && (candidate.file_date.as_str(), candidate.id)
            > (installed.file_date.as_str(), installed.id)
}

/// Scansiona l'installazione, chiede a CurseForge e valuta. Bloccante: va chiamata fuori dal
/// thread principale.
pub fn check_updates<T: Transport>(
    installation: &Installation,
    client: &CurseForgeClient<T>,
    overrides: &VersionOverrides,
) -> Result<Vec<AddonUpdate>, String> {
    scan_and_evaluate(installation, client, overrides).map(|(_, updates)| updates)
}

/// Come [`check_updates`], ma restituisce anche la scansione.
fn scan_and_evaluate<T: Transport>(
    installation: &Installation,
    client: &CurseForgeClient<T>,
    overrides: &VersionOverrides,
) -> Result<(Vec<InstalledAddon>, Vec<AddonUpdate>), String> {
    let version_type = version_type_for(&installation.flavor).ok_or_else(|| {
        format!(
            "The update check does not support the game version \"{}\" yet.",
            installation.name
        )
    })?;
    let addons = scan::scan_addons(&installation.path, &installation.flavor)?;
    let fingerprints: Vec<u32> = addons.iter().filter_map(|a| a.fingerprint).collect();
    let matches = client
        .match_fingerprints(&fingerprints)
        .map_err(|e: CurseForgeError| e.to_string())?;
    let mut mod_ids: Vec<u32> = matches
        .exact_matches
        .iter()
        .chain(&matches.partial_matches)
        .map(|m| m.id)
        .collect();
    mod_ids.sort_unstable();
    mod_ids.dedup();
    let mods = client.get_mods(&mod_ids).map_err(|e| e.to_string())?;
    let updates = evaluate(&addons, &matches, &mods, version_type, overrides);
    Ok((addons, updates))
}

// ── Aggiornamento ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum OutcomeStatus {
    Updated,
    Skipped,
    Error,
}

/// L'esito dell'aggiornamento di un progetto. I messaggi sono in inglese: l'interfaccia
/// sarà solo in inglese (issue #45).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateOutcome {
    pub mod_id: u32,
    /// Il nome del progetto; vuoto se il progetto non è tra quelli installati.
    pub name: String,
    pub status: OutcomeStatus,
    /// Perché è stato saltato o cosa è andato storto.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Il file installato, solo per `updated`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<FileSummary>,
}

impl UpdateOutcome {
    fn skipped(mod_id: u32, name: &str, message: &str) -> Self {
        Self {
            mod_id,
            name: name.to_owned(),
            status: OutcomeStatus::Skipped,
            message: Some(message.to_owned()),
            file: None,
        }
    }

    fn error(mod_id: u32, name: &str, message: String) -> Self {
        Self {
            mod_id,
            name: name.to_owned(),
            status: OutcomeStatus::Error,
            message: Some(message),
            file: None,
        }
    }
}

/// Aggiorna i progetti `mod_ids` di un'installazione. Rifà il controllo, con le stesse versioni
/// forzate di [`check_updates`], invece di fidarsi del frontend; per ogni progetto con un aggiornamento chiede il file a CurseForge, lo scarica,
/// lo estrae in staging e sostituisce le cartelle, togliendo quelle del file installato che la
/// versione nuova non ha più. Un errore su un progetto non ferma gli altri. Bloccante.
///
/// Restituisce `Err` solo se il controllo iniziale fallisce: in quel caso non si tocca niente.
pub fn update_addons<T: Transport>(
    installation: &Installation,
    client: &CurseForgeClient<T>,
    overrides: &VersionOverrides,
    mod_ids: &[u32],
    record: &mut dyn FnMut(u32, InstalledRecord) -> Result<(), String>,
) -> Result<Vec<UpdateOutcome>, String> {
    let (addons, updates) = scan_and_evaluate(installation, client, overrides)?;
    let addons_dir = installation.path.join("Interface").join("AddOns");
    let mut seen = HashSet::new();
    Ok(mod_ids
        .iter()
        .copied()
        .filter(|id| seen.insert(*id))
        .map(|mod_id| update_project(&addons, &updates, &addons_dir, client, mod_id, record))
        .collect())
}

fn update_project<T: Transport>(
    addons: &[InstalledAddon],
    updates: &[AddonUpdate],
    addons_dir: &Path,
    client: &CurseForgeClient<T>,
    mod_id: u32,
    record: &mut dyn FnMut(u32, InstalledRecord) -> Result<(), String>,
) -> UpdateOutcome {
    let folders: Vec<&AddonUpdate> = updates
        .iter()
        .filter(|u| u.project.as_ref().is_some_and(|p| p.id == mod_id))
        .collect();
    let Some(project) = folders.first().and_then(|u| u.project.as_ref()) else {
        return UpdateOutcome::skipped(mod_id, "", "Not found among the installed addons.");
    };
    let name = project.name.as_str();
    let available = folders.iter().find_map(|u| match &u.status {
        UpdateStatus::UpdateAvailable { latest } => Some((u, latest)),
        _ => None,
    });
    let Some((installed, latest)) = available else {
        return UpdateOutcome::skipped(mod_id, name, "Already up to date.");
    };
    if !project.allow_mod_distribution {
        return UpdateOutcome::skipped(
            mod_id,
            name,
            "The author does not allow downloads from third-party apps: update it from the CurseForge website.",
        );
    }

    let file = match client.get_file(mod_id, latest.id) {
        Ok(file) => file,
        Err(error) => return UpdateOutcome::error(mod_id, name, error.to_string()),
    };
    if file.download_url.is_none() {
        return UpdateOutcome::skipped(
            mod_id,
            name,
            "CurseForge does not provide a download link for this file: update it from the CurseForge website.",
        );
    }
    let old_modules = removable_modules(addons, updates, installed);
    let folders = match install_file(client, &file, addons_dir, &old_modules, &|_| Ok(())) {
        Ok(folders) => folders,
        Err(message) => return UpdateOutcome::error(mod_id, name, message),
    };
    // Le cartelle sono già sostituite: se il registro non si salva l'aggiornamento resta
    // riuscito, e il messaggio lo dice.
    let message = record(
        mod_id,
        InstalledRecord {
            file_id: file.id,
            folders,
        },
    )
    .err()
    .map(|error| format!("Installed, but the app could not remember its folders: {error}"));
    UpdateOutcome {
        mod_id,
        name: name.to_owned(),
        status: OutcomeStatus::Updated,
        message,
        file: Some(FileSummary::from(&file)),
    }
}

/// Le cartelle del file installato che l'aggiornamento può togliere se il nuovo zip non le ha
/// più. I nomi restituiti sono quelli letti dal disco, mai quelli della risposta di CurseForge.
/// Una cartella resta se:
/// - non è più sul disco, o il suo fingerprint non è quello del modulo installato (è stata
///   modificata o sostituita da un'altra cosa);
/// - è un modulo del file installato di un altro progetto riconosciuto (una libreria
///   distribuita da più addon);
/// - un addon che non fa parte di questo progetto la dichiara tra le dipendenze del `.toc`,
///   obbligatorie o facoltative (protegge anche gli addon non riconosciuti da CurseForge).
fn removable_modules(
    addons: &[InstalledAddon],
    updates: &[AddonUpdate],
    installed: &AddonUpdate,
) -> Vec<String> {
    let lower = |name: &str| name.to_lowercase();
    let mod_id = installed.project.as_ref().map(|p| p.id);
    let own: HashSet<String> = installed
        .installed_modules
        .iter()
        .map(|m| lower(&m.name))
        .collect();
    let shared: HashSet<String> = updates
        .iter()
        .filter(|u| u.project.as_ref().map(|p| p.id) != mod_id)
        .flat_map(|u| u.installed_modules.iter().map(|m| lower(&m.name)))
        .collect();
    let needed: HashSet<String> = addons
        .iter()
        .filter(|a| !own.contains(&lower(&a.folder)))
        .flat_map(|a| a.toc.dependencies.iter().chain(&a.toc.optional_deps))
        .map(|name| lower(name))
        .collect();
    installed
        .installed_modules
        .iter()
        .filter_map(|module| {
            addons.iter().find(|a| {
                lower(&a.folder) == lower(&module.name)
                    && a.fingerprint.map(u64::from) == Some(module.fingerprint)
            })
        })
        .filter(|a| !shared.contains(&lower(&a.folder)) && !needed.contains(&lower(&a.folder)))
        .map(|a| a.folder.clone())
        .collect()
}

/// Scarica `file`, lo estrae in staging e sostituisce le cartelle. Le cartelle di `old_modules`
/// che il nuovo zip non contiene vengono tolte nello stesso `replace`. Restituisce le cartelle
/// dello zip, ora installate.
///
/// `check` riceve le cartelle dello zip dopo l'estrazione e prima di `replace`: se rifiuta,
/// `AddOns` resta com'era e lo staging si cancella. L'aggiornamento non rifiuta niente;
/// l'installazione di un addon nuovo (`install_new`) lo usa per le cartelle di altri progetti.
pub(crate) fn install_file<T: Transport>(
    client: &CurseForgeClient<T>,
    file: &File,
    addons_dir: &Path,
    old_modules: &[String],
    check: &dyn Fn(&[String]) -> Result<(), String>,
) -> Result<Vec<String>, String> {
    let bytes = client.download(file).map_err(|e| e.to_string())?;
    let staging = install::WorkDir::staging(addons_dir)?;
    let folders = install::extract(&bytes, staging.path())?;
    check(&folders)?;
    let new: HashSet<String> = folders.iter().map(|f| f.to_lowercase()).collect();
    let obsolete: Vec<String> = old_modules
        .iter()
        .filter(|m| !new.contains(&m.to_lowercase()))
        .cloned()
        .collect();
    install::replace(addons_dir, staging.path(), &obsolete)?;
    Ok(folders)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::addons::toc::TocInfo;
    use crate::sources::curseforge::{FileModule, ModLinks, SortableGameVersion};

    const ERA: u32 = CLASSIC_ERA;

    fn addon(folder: &str, fingerprint: Option<u32>) -> InstalledAddon {
        InstalledAddon {
            folder: folder.to_owned(),
            toc_file: None,
            toc: TocInfo::default(),
            modified_ms: None,
            fingerprint,
            error: None,
        }
    }

    fn file(id: u32, date: &str, release_type: u8, version_type: u32, modules: &[u64]) -> File {
        File {
            id,
            mod_id: 1,
            display_name: format!("File {id}"),
            file_name: Some(format!("file-{id}.zip")),
            file_date: date.to_owned(),
            release_type,
            game_versions: Vec::new(),
            sortable_game_versions: vec![SortableGameVersion {
                game_version_name: String::new(),
                game_version_type_id: Some(version_type),
            }],
            file_fingerprint: None,
            modules: modules
                .iter()
                .map(|f| FileModule {
                    name: String::new(),
                    fingerprint: *f,
                })
                .collect(),
            is_available: true,
            download_url: None,
        }
    }

    fn matches(found: Vec<FingerprintMatch>) -> FingerprintMatches {
        FingerprintMatches {
            exact_matches: found,
            ..FingerprintMatches::default()
        }
    }

    fn project(id: u32, allow: Option<bool>) -> Mod {
        Mod {
            id,
            name: format!("Project {id}"),
            slug: String::new(),
            allow_mod_distribution: allow,
            links: ModLinks {
                website_url: Some(format!("https://www.curseforge.com/wow/addons/p{id}")),
            },
            latest_files_indexes: Vec::new(),
        }
    }

    #[test]
    fn unmatched_and_unreadable_folders() {
        let result = evaluate(
            &[addon("Nope", Some(1)), addon("Broken", None)],
            &matches(Vec::new()),
            &[],
            RETAIL,
            &VersionOverrides::new(),
        );
        assert_eq!(result[0].status, UpdateStatus::NotMatched);
        assert!(result[0].project.is_none());
        assert!(matches!(result[1].status, UpdateStatus::Error { .. }));
    }

    #[test]
    fn up_to_date_when_the_installed_file_is_the_newest() {
        let installed = file(10, "2026-10-01T00:00:00Z", 1, RETAIL, &[100]);
        let found = FingerprintMatch {
            id: 7,
            file: installed.clone(),
            latest_files: vec![installed],
        };
        let result = evaluate(
            &[addon("Foo", Some(100))],
            &matches(vec![found]),
            &[project(7, Some(true))],
            RETAIL,
            &VersionOverrides::new(),
        );
        assert_eq!(result[0].status, UpdateStatus::UpToDate);
        let project = result[0].project.as_ref().unwrap();
        assert_eq!(project.name, "Project 7");
        assert!(project.allow_mod_distribution);
        assert_eq!(result[0].installed_file.as_ref().unwrap().id, 10);
    }

    #[test]
    fn update_available_for_a_newer_release() {
        let found = FingerprintMatch {
            id: 7,
            file: file(10, "2026-09-01T00:00:00Z", 1, RETAIL, &[100]),
            latest_files: vec![file(12, "2026-10-01T00:00:00Z", 1, RETAIL, &[])],
        };
        let result = evaluate(
            &[addon("Foo", Some(100))],
            &matches(vec![found]),
            &[project(7, None)],
            RETAIL,
            &VersionOverrides::new(),
        );
        assert_eq!(
            result[0].status,
            UpdateStatus::UpdateAvailable {
                latest: FileSummary {
                    id: 12,
                    name: "File 12".to_owned(),
                    date: "2026-10-01T00:00:00Z".to_owned()
                }
            }
        );
        // allowModDistribution assente vale false.
        assert!(!result[0].project.as_ref().unwrap().allow_mod_distribution);
    }

    #[test]
    fn betas_and_other_game_versions_are_ignored() {
        let found = FingerprintMatch {
            id: 7,
            file: file(10, "2026-09-01T00:00:00Z", 1, RETAIL, &[100]),
            latest_files: vec![
                file(13, "2026-10-02T00:00:00Z", 2, RETAIL, &[]),
                file(14, "2026-10-03T00:00:00Z", 1, ERA, &[]),
            ],
        };
        let result = evaluate(
            &[addon("Foo", Some(100))],
            &matches(vec![found]),
            &[project(7, Some(true))],
            RETAIL,
            &VersionOverrides::new(),
        );
        assert_eq!(result[0].status, UpdateStatus::UpToDate);
    }

    #[test]
    fn folders_of_the_same_project_share_it() {
        let found = FingerprintMatch {
            id: 3,
            file: file(20, "2026-10-01T00:00:00Z", 1, RETAIL, &[1, 2]),
            latest_files: Vec::new(),
        };
        let result = evaluate(
            &[addon("DBM-Core", Some(1)), addon("DBM-GUI", Some(2))],
            &matches(vec![found]),
            &[project(3, Some(true))],
            RETAIL,
            &VersionOverrides::new(),
        );
        assert_eq!(result[0].project, result[1].project);
        assert_eq!(result[0].project.as_ref().unwrap().id, 3);
    }

    #[test]
    fn project_folders_come_from_the_recognised_file() {
        let mut published = file(20, "2026-10-01T00:00:00Z", 1, RETAIL, &[1, 2]);
        published.modules[0].name = "Details".to_owned();
        published.modules[1].name = "Details_Compare2".to_owned();
        let found = FingerprintMatch {
            id: 61284,
            file: published,
            latest_files: Vec::new(),
        };
        let result = evaluate(
            &[
                addon("Details", Some(1)),
                addon("Details_Streamer", Some(99)),
                addon("Broken", None),
            ],
            &matches(vec![found]),
            &[project(61284, Some(true))],
            RETAIL,
            &VersionOverrides::new(),
        );
        assert_eq!(result[0].project_folders, ["Details", "Details_Compare2"]);
        // Non riconosciuta o non leggibile: nessuna cartella del progetto.
        assert!(result[1].project_folders.is_empty());
        assert!(result[2].project_folders.is_empty());
    }

    #[test]
    fn a_folder_only_in_partial_matches_is_recognised() {
        let mut published = file(20, "2026-09-01T00:00:00Z", 1, RETAIL, &[1, 2]);
        published.modules[0].name = "DBM-Core".to_owned();
        published.modules[1].name = "DBM-GUI".to_owned();
        let found = FingerprintMatch {
            id: 3,
            file: published,
            latest_files: vec![file(21, "2026-10-01T00:00:00Z", 1, RETAIL, &[])],
        };
        let partial = FingerprintMatches {
            partial_matches: vec![found],
            ..FingerprintMatches::default()
        };
        // DBM-GUI modificata: solo DBM-Core combacia con il file pubblicato.
        let result = evaluate(
            &[addon("DBM-Core", Some(1)), addon("DBM-GUI", Some(99))],
            &partial,
            &[project(3, Some(true))],
            RETAIL,
            &VersionOverrides::new(),
        );
        assert_eq!(result[0].project.as_ref().unwrap().id, 3);
        assert_eq!(result[0].installed_file.as_ref().unwrap().id, 20);
        assert_eq!(result[0].project_folders, ["DBM-Core", "DBM-GUI"]);
        assert_eq!(result[0].installed_modules.len(), 2);
        assert!(
            matches!(&result[0].status, UpdateStatus::UpdateAvailable { latest } if latest.id == 21)
        );
        // Il fingerprint della cartella modificata non è tra i `modules`: resta sconosciuta.
        assert_eq!(result[1].status, UpdateStatus::NotMatched);
    }

    #[test]
    fn an_exact_match_wins_over_a_partial_one() {
        let exact = FingerprintMatch {
            id: 7,
            file: file(10, "2026-10-01T00:00:00Z", 1, RETAIL, &[100]),
            latest_files: Vec::new(),
        };
        let partial = FingerprintMatch {
            id: 8,
            file: file(30, "2026-10-01T00:00:00Z", 1, RETAIL, &[100, 200]),
            latest_files: Vec::new(),
        };
        // L'ordine nella risposta non conta: l'esatto vince in entrambi i casi.
        let both = FingerprintMatches {
            exact_matches: vec![exact],
            partial_matches: vec![partial],
            ..FingerprintMatches::default()
        };
        let result = evaluate(
            &[addon("Foo", Some(100)), addon("Bar", Some(200))],
            &both,
            &[project(7, Some(true)), project(8, Some(true))],
            RETAIL,
            &VersionOverrides::new(),
        );
        assert_eq!(result[0].project.as_ref().unwrap().id, 7);
        assert_eq!(result[0].installed_file.as_ref().unwrap().id, 10);
        // Una cartella presente solo nel match parziale usa quello.
        assert_eq!(result[1].project.as_ref().unwrap().id, 8);
    }

    #[test]
    fn an_override_picks_the_classic_era_file_on_a_forever_installation() {
        // Un addon che pubblica per Classic Era ma non per Forever.
        let found = || FingerprintMatch {
            id: 7,
            file: file(10, "2026-09-01T00:00:00Z", 1, ERA, &[100]),
            latest_files: vec![file(12, "2026-10-01T00:00:00Z", 1, ERA, &[])],
        };
        let addons = [addon("Foo", Some(100)), addon("Other", Some(1))];
        let mods = [project(7, Some(true))];

        let plain = evaluate(
            &addons,
            &matches(vec![found()]),
            &mods,
            FOREVER,
            &VersionOverrides::new(),
        );
        assert_eq!(plain[0].status, UpdateStatus::UpToDate);
        assert_eq!(plain[0].version_override, None);

        let overrides = VersionOverrides::from([(7, ERA), (99, RETAIL)]);
        let forced = evaluate(&addons, &matches(vec![found()]), &mods, FOREVER, &overrides);
        assert!(
            matches!(&forced[0].status, UpdateStatus::UpdateAvailable { latest } if latest.id == 12),
            "{:?}",
            forced[0].status
        );
        assert_eq!(forced[0].version_override, Some(ERA));
        // Una cartella non riconosciuta non ha una versione forzata.
        assert_eq!(forced[1].status, UpdateStatus::NotMatched);
        assert_eq!(forced[1].version_override, None);
    }

    #[test]
    fn known_version_types_have_english_names() {
        let types = version_types();
        let names: Vec<_> = types.iter().map(|t| t.name).collect();
        assert_eq!(
            names,
            [
                "Retail",
                "Classic Era",
                "Classic",
                "Anniversary",
                "WoW Forever"
            ]
        );
        for flavor in [
            "retail",
            "classic_era",
            "classic",
            "anniversary",
            "classic_beta",
        ] {
            let id = version_type_for(flavor).unwrap();
            assert!(is_known_version_type(id), "{flavor}");
        }
        assert!(!is_known_version_type(81212));
        assert_eq!(
            serde_json::to_value(&types[0]).unwrap(),
            serde_json::json!({"id": RETAIL, "name": "Retail"})
        );
    }

    /// Fissa l'espansione attuale della Classic progressiva: quando cambia, questo test va
    /// aggiornato insieme a `CLASSIC_PROGRESSIVE`, e il cambio si vede nel diff.
    #[test]
    fn the_progressive_classic_is_mists() {
        assert_eq!(CLASSIC_PROGRESSIVE, 79434);
    }

    #[test]
    fn version_types_by_flavor() {
        assert_eq!(version_type_for("retail"), Some(RETAIL));
        assert_eq!(version_type_for("_retail_"), Some(RETAIL));
        assert_eq!(version_type_for("classic_era"), Some(ERA));
        assert_eq!(version_type_for("classic"), Some(CLASSIC_PROGRESSIVE));
        assert_eq!(version_type_for("anniversary"), Some(ANNIVERSARY));
        assert_eq!(version_type_for("classic_beta"), Some(FOREVER));
        assert_eq!(version_type_for("forever"), None);
        assert_eq!(version_type_for("ptr"), None);
        assert_eq!(version_type_for("beta"), None);
    }

    #[test]
    fn the_payload_has_a_status_tag() {
        let update = AddonUpdate {
            folder: "Foo".to_owned(),
            project: None,
            installed_file: None,
            status: UpdateStatus::UpdateAvailable {
                latest: FileSummary {
                    id: 1,
                    name: "x".to_owned(),
                    date: "d".to_owned(),
                },
            },
            version_override: Some(FOREVER),
            project_folders: vec!["Foo".to_owned()],
            installed_modules: vec![FileModule {
                name: "Foo".to_owned(),
                fingerprint: 1,
            }],
        };
        let json = serde_json::to_value(&update).unwrap();
        assert_eq!(json["status"], "updateAvailable");
        assert_eq!(json["latest"]["id"], 1);
        assert_eq!(json["installedFile"], serde_json::Value::Null);
        assert_eq!(json["versionOverride"], FOREVER);
        assert!(json.get("installedModules").is_none());
        assert_eq!(json["projectFolders"], serde_json::json!(["Foo"]));
        let json = serde_json::to_value(UpdateStatus::NotMatched).unwrap();
        assert_eq!(json["status"], "notMatched");
    }

    // ── update_addons ───────────────────────────────────────────────────────

    mod update {
        use std::collections::HashMap;
        use std::fs;
        use std::io::{Cursor, Write};
        use std::path::PathBuf;
        use std::sync::{Arc, Mutex};

        use serde_json::{Value, json};
        use zip::ZipWriter;
        use zip::write::SimpleFileOptions;

        use super::super::*;
        use crate::sources::curseforge::{ApiKey, BytesResponse, HttpResponse, TransportError};
        use crate::test_support::TempDir;

        const API: &str = "https://api.curseforge.com";

        /// Trasporto finto che risponde per URL e registra le richieste.
        #[derive(Default)]
        struct Routes {
            fingerprints: String,
            mods: String,
            files: HashMap<String, String>,
            zips: HashMap<String, Vec<u8>>,
            /// Condiviso con il test, che non può rileggere il trasporto dal client.
            log: Arc<Log>,
        }

        #[derive(Default)]
        struct Log {
            gets: Mutex<Vec<String>>,
            downloads: Mutex<Vec<String>>,
        }

        fn reply(body: Option<&String>) -> Result<HttpResponse, TransportError> {
            Ok(match body {
                Some(body) => HttpResponse {
                    status: 200,
                    body: body.clone(),
                },
                None => HttpResponse {
                    status: 404,
                    body: String::new(),
                },
            })
        }

        impl Transport for Routes {
            fn get(&self, url: &str, _key: &ApiKey) -> Result<HttpResponse, TransportError> {
                self.log.gets.lock().unwrap().push(url.to_owned());
                reply(self.files.get(url))
            }

            fn post_json(
                &self,
                url: &str,
                _key: &ApiKey,
                _body: String,
            ) -> Result<HttpResponse, TransportError> {
                if url == format!("{API}/v1/fingerprints/1") {
                    reply(Some(&self.fingerprints))
                } else if url == format!("{API}/v1/mods") {
                    reply(Some(&self.mods))
                } else {
                    reply(None)
                }
            }

            fn get_bytes(&self, url: &str, _max: u64) -> Result<BytesResponse, TransportError> {
                self.log.downloads.lock().unwrap().push(url.to_owned());
                Ok(match self.zips.get(url) {
                    Some(body) => BytesResponse {
                        status: 200,
                        body: body.clone(),
                    },
                    None => BytesResponse {
                        status: 404,
                        body: Vec::new(),
                    },
                })
            }
        }

        fn zip_of(files: &[(&str, &str)]) -> Vec<u8> {
            let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
            for (name, data) in files {
                writer
                    .start_file(*name, SimpleFileOptions::default())
                    .unwrap();
                writer.write_all(data.as_bytes()).unwrap();
            }
            writer.finish().unwrap().into_inner()
        }

        /// Un addon con `.toc` e un `.lua`: il contenuto lo rende riconoscibile.
        fn write_addon(addons: &Path, folder: &str, content: &str) {
            let dir = addons.join(folder);
            fs::create_dir_all(&dir).unwrap();
            fs::write(
                dir.join(format!("{folder}.toc")),
                format!("## Interface: 120000\n## Title: {folder}\n{folder}.lua\n"),
            )
            .unwrap();
            fs::write(dir.join(format!("{folder}.lua")), content).unwrap();
        }

        fn file_json(mod_id: u32, id: u32, date: &str, modules: &[(&str, u32)]) -> Value {
            json!({
                "id": id, "modId": mod_id, "displayName": format!("File {id}"),
                "fileName": format!("file-{id}.zip"), "fileDate": date, "releaseType": 1,
                "sortableGameVersions": [{"gameVersionName": "12.0.0", "gameVersionTypeId": RETAIL}],
                "modules": modules.iter()
                    .map(|(name, fp)| json!({"name": name, "fingerprint": fp}))
                    .collect::<Vec<_>>(),
                "downloadUrl": format!("https://edge.forgecdn.net/files/{id}/file-{id}.zip"),
            })
        }

        fn download_url(id: u32) -> String {
            format!("https://edge.forgecdn.net/files/{id}/file-{id}.zip")
        }

        struct Fixture {
            _tmp: TempDir,
            installation: Installation,
            addons: PathBuf,
            fingerprints: HashMap<String, u32>,
        }

        fn fixture(folders: &[(&str, &str)]) -> Fixture {
            let tmp = TempDir::new();
            let path = tmp.path().join("World of Warcraft").join("_retail_");
            let addons = path.join("Interface").join("AddOns");
            for (folder, content) in folders {
                write_addon(&addons, folder, content);
            }
            let fingerprints = scan::scan_addons(&path, "retail")
                .unwrap()
                .into_iter()
                .map(|a| (a.folder, a.fingerprint.expect("fingerprint")))
                .collect();
            Fixture {
                _tmp: tmp,
                installation: Installation {
                    id: "test".to_owned(),
                    path,
                    flavor: "retail".to_owned(),
                    name: "Retail".to_owned(),
                },
                addons,
                fingerprints,
            }
        }

        /// Un progetto installato (file 10·id) con un file più recente (11·id), se `newer`.
        fn installed(fx: &Fixture, mod_id: u32, folders: &[&str], newer: bool) -> Value {
            let modules: Vec<(&str, u32)> =
                folders.iter().map(|f| (*f, fx.fingerprints[*f])).collect();
            let current = file_json(mod_id, mod_id * 10, "2026-09-01T00:00:00Z", &modules);
            let latest = if newer {
                vec![file_json(
                    mod_id,
                    mod_id * 10 + 1,
                    "2026-10-01T00:00:00Z",
                    &[],
                )]
            } else {
                vec![current.clone()]
            };
            json!({"id": mod_id, "file": current, "latestFiles": latest})
        }

        fn project(mod_id: u32, name: &str, allow: bool) -> Value {
            json!({"id": mod_id, "name": name, "allowModDistribution": allow})
        }

        fn no_overrides() -> VersionOverrides {
            VersionOverrides::new()
        }

        /// `update_addons` senza registro, per i test che non lo guardano.
        fn update_without_record<T: Transport>(
            installation: &Installation,
            client: &CurseForgeClient<T>,
            overrides: &VersionOverrides,
            mod_ids: &[u32],
        ) -> Result<Vec<UpdateOutcome>, String> {
            update_addons(installation, client, overrides, mod_ids, &mut |_, _| Ok(()))
        }

        fn client(routes: Routes) -> CurseForgeClient<Routes> {
            CurseForgeClient::new(routes, Some("test-key".to_owned()))
        }

        fn names(dir: &Path) -> Vec<String> {
            let mut names: Vec<String> = fs::read_dir(dir)
                .unwrap()
                .map(|e| e.unwrap().file_name().into_string().unwrap())
                .collect();
            names.sort();
            names
        }

        #[test]
        fn updates_replaces_folders_and_removes_obsolete_ones() {
            let fx = fixture(&[
                ("Alpha", "alpha v1"),
                ("Alpha_Old", "alpha old"),
                ("Other", "other"),
            ]);
            let new_file = file_json(1, 11, "2026-10-01T00:00:00Z", &[]);
            let routes = Routes {
                fingerprints: json!({"data": {"exactMatches": [
                    installed(&fx, 1, &["Alpha", "Alpha_Old"], true)
                ]}})
                .to_string(),
                mods: json!({"data": [project(1, "Alpha", true)]}).to_string(),
                files: HashMap::from([(
                    format!("{API}/v1/mods/1/files/11"),
                    json!({ "data": new_file }).to_string(),
                )]),
                zips: HashMap::from([(
                    download_url(11),
                    zip_of(&[
                        ("Alpha/Alpha.toc", "## Title: Alpha\nAlpha.lua\n"),
                        ("Alpha/Alpha.lua", "alpha v2"),
                        ("Alpha_New/Alpha_New.toc", "## Title: Alpha_New\n"),
                    ]),
                )]),
                ..Routes::default()
            };
            let client = client(routes);

            let outcomes =
                update_without_record(&fx.installation, &client, &no_overrides(), &[1]).unwrap();

            assert_eq!(
                outcomes,
                [UpdateOutcome {
                    mod_id: 1,
                    name: "Alpha".to_owned(),
                    status: OutcomeStatus::Updated,
                    message: None,
                    file: Some(FileSummary {
                        id: 11,
                        name: "File 11".to_owned(),
                        date: "2026-10-01T00:00:00Z".to_owned(),
                    }),
                }]
            );
            assert_eq!(names(&fx.addons), ["Alpha", "Alpha_New", "Other"]);
            assert_eq!(
                fs::read_to_string(fx.addons.join("Alpha").join("Alpha.lua")).unwrap(),
                "alpha v2"
            );
            assert_eq!(names(&fx.addons.join("Alpha")), ["Alpha.lua", "Alpha.toc"]);
            assert_eq!(
                fs::read_to_string(fx.addons.join("Other").join("Other.lua")).unwrap(),
                "other"
            );
            // Nessuna cartella di lavoro rimasta accanto ad AddOns.
            assert_eq!(names(fx.addons.parent().unwrap()), ["AddOns"]);
        }

        /// Un addon installato in parte (manca una cartella del file pubblicato) arriva solo in
        /// `partialMatches`: si riconosce e si aggiorna come uno esatto.
        #[test]
        fn a_partially_installed_addon_is_updated() {
            let fx = fixture(&[("Beta", "beta v1"), ("Other", "other")]);
            let modules = [("Beta", fx.fingerprints["Beta"]), ("Beta_Extra", 4242)];
            let current = file_json(2, 20, "2026-09-01T00:00:00Z", &modules);
            let new_file = file_json(2, 21, "2026-10-01T00:00:00Z", &[]);
            let routes = Routes {
                fingerprints: json!({"data": {"exactMatches": [], "partialMatches": [
                    {"id": 2, "file": current, "latestFiles": [new_file.clone()]}
                ]}})
                .to_string(),
                mods: json!({"data": [project(2, "Beta", true)]}).to_string(),
                files: HashMap::from([(
                    format!("{API}/v1/mods/2/files/21"),
                    json!({ "data": new_file }).to_string(),
                )]),
                zips: HashMap::from([(
                    download_url(21),
                    zip_of(&[
                        ("Beta/Beta.toc", "## Title: Beta\nBeta.lua\n"),
                        ("Beta/Beta.lua", "beta v2"),
                        ("Beta_Extra/Beta_Extra.toc", "## Title: Beta_Extra\n"),
                    ]),
                )]),
                ..Routes::default()
            };
            let client = client(routes);

            let checked = check_updates(&fx.installation, &client, &no_overrides()).unwrap();
            let beta = checked.iter().find(|u| u.folder == "Beta").unwrap();
            assert_eq!(beta.project.as_ref().unwrap().name, "Beta");
            assert_eq!(beta.project_folders, ["Beta", "Beta_Extra"]);

            let outcomes =
                update_without_record(&fx.installation, &client, &no_overrides(), &[2]).unwrap();
            assert_eq!(outcomes[0].status, OutcomeStatus::Updated);
            assert_eq!(names(&fx.addons), ["Beta", "Beta_Extra", "Other"]);
            assert_eq!(
                fs::read_to_string(fx.addons.join("Beta").join("Beta.lua")).unwrap(),
                "beta v2"
            );
        }

        #[test]
        fn only_unchanged_unneeded_folders_on_disk_are_removed() {
            let fx = fixture(&[
                ("Alpha", "alpha v1"),
                ("Alpha_Gone", "gone"),
                ("Alpha_Edited", "edited"),
                ("Alpha_Lib", "lib"),
                ("Alpha_Plain", "plain"),
                ("Unknown", "unknown"),
            ]);
            let fp = |folder: &str| fx.fingerprints[folder];
            // Dopo l'installazione: una cartella tolta a mano, una modificata, e un addon non
            // riconosciuto che dipende da una libreria del progetto.
            fs::remove_dir_all(fx.addons.join("Alpha_Gone")).unwrap();
            fs::write(
                fx.addons.join("Alpha_Edited").join("Alpha_Edited.lua"),
                "mine",
            )
            .unwrap();
            fs::write(
                fx.addons.join("Unknown").join("Unknown.toc"),
                "## Interface: 120000\n## Dependencies: alpha_lib\nUnknown.lua\n",
            )
            .unwrap();
            let modules = [
                ("Alpha", fp("Alpha")),
                ("Alpha_Gone", fp("Alpha_Gone")),
                ("Alpha_Edited", fp("Alpha_Edited")),
                ("Alpha_Lib", fp("Alpha_Lib")),
                ("Alpha_Plain", fp("Alpha_Plain")),
                // Un nome che esce da AddOns non diventa mai un percorso da togliere.
                ("../Unknown", fp("Unknown")),
            ];
            let current = file_json(1, 10, "2026-09-01T00:00:00Z", &modules);
            let latest = file_json(1, 11, "2026-10-01T00:00:00Z", &[]);
            let routes = Routes {
                fingerprints: json!({"data": {"exactMatches": [
                    {"id": 1, "file": current, "latestFiles": [latest.clone()]}
                ]}})
                .to_string(),
                mods: json!({"data": [project(1, "Alpha", true)]}).to_string(),
                files: HashMap::from([(
                    format!("{API}/v1/mods/1/files/11"),
                    json!({ "data": latest }).to_string(),
                )]),
                zips: HashMap::from([(
                    download_url(11),
                    zip_of(&[("Alpha/Alpha.lua", "alpha v2")]),
                )]),
                ..Routes::default()
            };
            let client = client(routes);

            let outcomes =
                update_without_record(&fx.installation, &client, &no_overrides(), &[1]).unwrap();

            assert_eq!(outcomes[0].status, OutcomeStatus::Updated, "{outcomes:?}");
            assert_eq!(
                names(&fx.addons),
                ["Alpha", "Alpha_Edited", "Alpha_Lib", "Unknown"]
            );
            assert_eq!(names(fx.addons.parent().unwrap()), ["AddOns"]);
        }

        #[test]
        fn a_folder_shared_with_another_project_is_kept() {
            let fx = fixture(&[
                ("Alpha", "alpha v1"),
                ("Beta", "beta"),
                ("LibShared", "lib"),
            ]);
            let routes = Routes {
                fingerprints: json!({"data": {"exactMatches": [
                    installed(&fx, 1, &["Alpha", "LibShared"], true),
                    installed(&fx, 2, &["Beta", "LibShared"], false),
                ]}})
                .to_string(),
                mods: json!({"data": [project(1, "Alpha", true), project(2, "Beta", true)]})
                    .to_string(),
                files: HashMap::from([(
                    format!("{API}/v1/mods/1/files/11"),
                    json!({"data": file_json(1, 11, "2026-10-01T00:00:00Z", &[])}).to_string(),
                )]),
                zips: HashMap::from([(
                    download_url(11),
                    zip_of(&[("Alpha/Alpha.lua", "alpha v2")]),
                )]),
                ..Routes::default()
            };
            let client = client(routes);

            let outcomes =
                update_without_record(&fx.installation, &client, &no_overrides(), &[1]).unwrap();

            assert_eq!(outcomes[0].status, OutcomeStatus::Updated, "{outcomes:?}");
            assert_eq!(names(&fx.addons), ["Alpha", "Beta", "LibShared"]);
        }

        #[test]
        fn each_project_gets_its_own_outcome() {
            let fx = fixture(&[
                ("Alpha", "alpha v1"),
                ("Beta", "beta"),
                ("Gamma", "gamma"),
                ("Delta", "delta"),
                ("Eps", "eps"),
            ]);
            let mut no_link = file_json(5, 51, "2026-10-01T00:00:00Z", &[]);
            no_link["downloadUrl"] = Value::Null;
            let routes = Routes {
                fingerprints: json!({"data": {"exactMatches": [
                    installed(&fx, 1, &["Alpha"], true),
                    installed(&fx, 2, &["Beta"], true),
                    installed(&fx, 3, &["Gamma"], true),
                    installed(&fx, 4, &["Delta"], false),
                    installed(&fx, 5, &["Eps"], true),
                ]}})
                .to_string(),
                mods: json!({"data": [
                    project(1, "Alpha", true),
                    project(2, "Beta", false),
                    project(3, "Gamma", true),
                    project(4, "Delta", true),
                    project(5, "Eps", true),
                ]})
                .to_string(),
                files: HashMap::from([
                    (
                        format!("{API}/v1/mods/1/files/11"),
                        json!({"data": file_json(1, 11, "2026-10-01T00:00:00Z", &[])}).to_string(),
                    ),
                    (
                        format!("{API}/v1/mods/3/files/31"),
                        json!({"data": file_json(3, 31, "2026-10-01T00:00:00Z", &[])}).to_string(),
                    ),
                    (
                        format!("{API}/v1/mods/5/files/51"),
                        json!({ "data": no_link }).to_string(),
                    ),
                ]),
                // Gamma (31) non ha uno zip: la CDN risponde 404.
                zips: HashMap::from([(
                    download_url(11),
                    zip_of(&[("Alpha/Alpha.lua", "alpha v2")]),
                )]),
                ..Routes::default()
            };
            let log = Arc::clone(&routes.log);
            let client = client(routes);

            let mut records = Vec::new();
            let outcomes = update_addons(
                &fx.installation,
                &client,
                &no_overrides(),
                &[2, 3, 1, 4, 99, 5, 1],
                &mut |mod_id, record| {
                    records.push((mod_id, record));
                    Ok(())
                },
            )
            .unwrap();

            // Solo l'installazione riuscita è registrata, con le cartelle dello zip.
            assert_eq!(
                records,
                [(
                    1,
                    InstalledRecord {
                        file_id: 11,
                        folders: vec!["Alpha".to_owned()],
                    }
                )]
            );

            let summary: Vec<(u32, &str, OutcomeStatus)> = outcomes
                .iter()
                .map(|o| (o.mod_id, o.name.as_str(), o.status))
                .collect();
            assert_eq!(
                summary,
                [
                    (2, "Beta", OutcomeStatus::Skipped),
                    (3, "Gamma", OutcomeStatus::Error),
                    (1, "Alpha", OutcomeStatus::Updated),
                    (4, "Delta", OutcomeStatus::Skipped),
                    (99, "", OutcomeStatus::Skipped),
                    (5, "Eps", OutcomeStatus::Skipped),
                ]
            );
            for outcome in &outcomes {
                match outcome.status {
                    OutcomeStatus::Updated => assert!(outcome.message.is_none()),
                    _ => {
                        assert!(outcome.message.is_some(), "{outcome:?}");
                        assert!(outcome.file.is_none(), "{outcome:?}");
                    }
                }
            }
            assert!(
                outcomes[0]
                    .message
                    .as_deref()
                    .unwrap()
                    .contains("third-party")
            );
            assert!(
                outcomes[3]
                    .message
                    .as_deref()
                    .unwrap()
                    .contains("up to date")
            );
            assert!(
                outcomes[4]
                    .message
                    .as_deref()
                    .unwrap()
                    .contains("Not found")
            );
            assert!(
                outcomes[5]
                    .message
                    .as_deref()
                    .unwrap()
                    .contains("download link")
            );

            // Beta (non distribuibile) non è mai stato chiesto né scaricato; Eps (senza
            // link) non è stato scaricato.
            let gets = log.gets.lock().unwrap().clone();
            assert!(!gets.iter().any(|u| u.contains("/mods/2/")), "{gets:?}");
            let downloads = log.downloads.lock().unwrap().clone();
            assert_eq!(downloads, [download_url(31), download_url(11)]);

            // L'errore su Gamma non ha toccato niente; Alpha è aggiornato.
            assert_eq!(
                fs::read_to_string(fx.addons.join("Gamma").join("Gamma.lua")).unwrap(),
                "gamma"
            );
            assert_eq!(
                fs::read_to_string(fx.addons.join("Alpha").join("Alpha.lua")).unwrap(),
                "alpha v2"
            );
            assert_eq!(names(fx.addons.parent().unwrap()), ["AddOns"]);
        }

        #[test]
        fn update_follows_the_stored_override() {
            let fx = fixture(&[("Alpha", "alpha v1")]);
            // Installato un file Retail; il più recente è solo Classic Era.
            let mut current = installed(&fx, 1, &["Alpha"], false);
            let mut era = file_json(1, 11, "2026-10-01T00:00:00Z", &[]);
            era["sortableGameVersions"] =
                json!([{"gameVersionName": "1.15.7", "gameVersionTypeId": CLASSIC_ERA}]);
            current["latestFiles"] = json!([current["file"].clone(), era.clone()]);
            let routes = || Routes {
                fingerprints: json!({"data": {"exactMatches": [current.clone()]}}).to_string(),
                mods: json!({"data": [project(1, "Alpha", true)]}).to_string(),
                files: HashMap::from([(
                    format!("{API}/v1/mods/1/files/11"),
                    json!({ "data": era.clone() }).to_string(),
                )]),
                zips: HashMap::from([(
                    download_url(11),
                    zip_of(&[("Alpha/Alpha.lua", "alpha era")]),
                )]),
                ..Routes::default()
            };

            let outcomes =
                update_without_record(&fx.installation, &client(routes()), &no_overrides(), &[1])
                    .unwrap();
            assert_eq!(outcomes[0].status, OutcomeStatus::Skipped, "{outcomes:?}");

            let overrides = VersionOverrides::from([(1, CLASSIC_ERA)]);
            let outcomes =
                update_without_record(&fx.installation, &client(routes()), &overrides, &[1])
                    .unwrap();
            assert_eq!(outcomes[0].status, OutcomeStatus::Updated, "{outcomes:?}");
            assert_eq!(
                fs::read_to_string(fx.addons.join("Alpha").join("Alpha.lua")).unwrap(),
                "alpha era"
            );
        }

        #[test]
        fn a_failed_check_changes_nothing() {
            let fx = fixture(&[("Alpha", "alpha v1")]);
            let client = CurseForgeClient::new(Routes::default(), None);
            let mut recorded = false;
            let result = update_addons(
                &fx.installation,
                &client,
                &no_overrides(),
                &[1],
                &mut |_, _| {
                    recorded = true;
                    Ok(())
                },
            );
            assert!(result.is_err());
            assert!(!recorded);
            assert_eq!(names(&fx.addons), ["Alpha"]);
        }

        #[test]
        fn a_record_that_cannot_be_saved_leaves_the_update_done() {
            let fx = fixture(&[("Alpha", "alpha v1")]);
            let routes = Routes {
                fingerprints: json!({"data": {"exactMatches": [
                    installed(&fx, 1, &["Alpha"], true)
                ]}})
                .to_string(),
                mods: json!({"data": [project(1, "Alpha", true)]}).to_string(),
                files: HashMap::from([(
                    format!("{API}/v1/mods/1/files/11"),
                    json!({"data": file_json(1, 11, "2026-10-01T00:00:00Z", &[])}).to_string(),
                )]),
                zips: HashMap::from([(
                    download_url(11),
                    zip_of(&[("Alpha/Alpha.lua", "alpha v2")]),
                )]),
                ..Routes::default()
            };

            let outcomes = update_addons(
                &fx.installation,
                &client(routes),
                &no_overrides(),
                &[1],
                &mut |_, _| Err("disk full".to_owned()),
            )
            .unwrap();

            assert_eq!(outcomes[0].status, OutcomeStatus::Updated, "{outcomes:?}");
            assert!(outcomes[0].file.is_some());
            assert_eq!(
                outcomes[0].message.as_deref(),
                Some("Installed, but the app could not remember its folders: disk full")
            );
            assert_eq!(
                fs::read_to_string(fx.addons.join("Alpha").join("Alpha.lua")).unwrap(),
                "alpha v2"
            );
        }

        /// Aggiorna davvero gli addon di un'installazione reale: **modifica `Interface/AddOns`**,
        /// quindi conviene fare prima un backup. Lo lancia l'utente, a mano, con la chiave
        /// dell'ambiente (o di `.env` in debug), il percorso della cartella `_<flavor>_` e i
        /// progetti da aggiornare separati da virgole:
        ///
        /// `WOW_DON_LIVE_INSTALL=<cartella-_flavor_> WOW_DON_LIVE_MOD_IDS=61284 cargo test --manifest-path src-tauri/Cargo.toml live_update_addons -- --ignored --nocapture`
        ///
        /// Senza `WOW_DON_LIVE_MOD_IDS` stampa solo i progetti con un aggiornamento.
        #[test]
        #[ignore = "usa la rete, la chiave CurseForge reale e modifica un'installazione reale"]
        fn live_update_addons() {
            use crate::sources::curseforge::ReqwestTransport;

            let path = std::env::var("WOW_DON_LIVE_INSTALL").expect("WOW_DON_LIVE_INSTALL");
            let installation = crate::installations::validate(Path::new(&path)).unwrap();
            let config = crate::config::Config::load();
            let client = CurseForgeClient::new(
                ReqwestTransport::new().unwrap(),
                config.curseforge_api_key.clone(),
            );
            let mod_ids: Vec<u32> = std::env::var("WOW_DON_LIVE_MOD_IDS")
                .unwrap_or_default()
                .split(',')
                .filter_map(|id| id.trim().parse().ok())
                .collect();
            if mod_ids.is_empty() {
                for update in
                    check_updates(&installation, &client, &VersionOverrides::new()).unwrap()
                {
                    if let (Some(project), UpdateStatus::UpdateAvailable { latest }) =
                        (&update.project, &update.status)
                    {
                        println!("{:>8}  {:<30} -> {}", project.id, project.name, latest.name);
                    }
                }
                return;
            }
            for outcome in
                update_without_record(&installation, &client, &VersionOverrides::new(), &mod_ids)
                    .unwrap()
            {
                println!("{outcome:?}");
                assert_ne!(outcome.status, OutcomeStatus::Error, "{outcome:?}");
            }
        }

        #[test]
        fn the_outcome_payload_is_camel_case() {
            let json = serde_json::to_value(UpdateOutcome::skipped(7, "Foo", "why")).unwrap();
            assert_eq!(
                json,
                json!({"modId": 7, "name": "Foo", "status": "skipped", "message": "why"})
            );
            let json = serde_json::to_value(UpdateOutcome {
                mod_id: 7,
                name: "Foo".to_owned(),
                status: OutcomeStatus::Updated,
                message: None,
                file: Some(FileSummary {
                    id: 1,
                    name: "x".to_owned(),
                    date: "d".to_owned(),
                }),
            })
            .unwrap();
            assert_eq!(json["status"], "updated");
            assert_eq!(json["file"]["id"], 1);
            assert!(json.get("message").is_none());
            assert_eq!(serde_json::to_value(OutcomeStatus::Error).unwrap(), "error");
        }
    }
}
