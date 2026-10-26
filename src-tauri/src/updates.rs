//! Controllo degli aggiornamenti: abbina gli addon installati ai progetti delle fonti (D12,
//! D33) e sceglie, per ognuno, la versione più recente per il gioco dell'installazione.
//!
//! [`check_updates_with`] scansiona l'installazione e offre le cartelle alle fonti nell'ordine di D33;
//! [`update_addons_with`] rifà il controllo e chiede a ogni fonte di aggiornare i suoi progetti.
//! CurseForge è [`CurseForgeSource`]: [`evaluate`] è pura, riceve la scansione, la risposta dei
//! fingerprint e i mod, e non usa né rete né disco.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde::Serialize;

use crate::addons::cache::FingerprintCache;
use crate::addons::scan::{self, InstalledAddon, ListedAddon};
use crate::install;
use crate::installations::Installation;
use crate::progress::{Phase, ProgressSink, Reporter};
use crate::sources::curseforge::{
    CurseForgeClient, CurseForgeError, File, FileModule, FingerprintMatch, FingerprintMatches, Mod,
    Transport, pick_file,
};
use crate::sources::github_mirror::{self, MirrorSource, MirrorStatus, Outcome, Reason};
use crate::sources::{ProjectKey, RecordWriter, Source, SourceKind, UpdateTarget};
use crate::store::{InstalledRecords, ProjectRecords, SourceRecord, SourceRecords};

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

/// Il file installato o disponibile di un progetto. `id` è l'id del file CurseForge, che serve a
/// scaricarlo; le fonti senza id numerico (GitHub, WoWInterface, Tukui) mettono `0` e la
/// versione o il tag in `name`, la stessa stringa che scrivono in `SourceRecord::file`.
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
    /// L'id CurseForge; `0` per le altre fonti, dove il progetto si identifica con `source` e
    /// `key`.
    pub id: u32,
    pub source: SourceKind,
    /// L'id del progetto nella fonte, come stringa (`61284`, `owner/repo`, `-2`).
    pub key: String,
    pub name: String,
    pub url: Option<String>,
    /// Se la fonte permette di scaricare i file da un'app di terzi (per CurseForge
    /// `allowModDistribution`; sconosciuto vale `false`).
    pub allow_mod_distribution: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum UpdateStatus {
    NotMatched,
    UpToDate,
    UpdateAvailable {
        latest: FileSummary,
    },
    /// Per la versione non c'è nessuna Release, ma una Beta più recente del file installato sì, e
    /// l'opzione "Beta quando non c'è una Release" è spenta (issue #141). Se una Release esiste,
    /// la Beta non conta nemmeno con l'opzione accesa, quindi lo stato resta `UpToDate`.
    BetaOnly {
        latest: FileSummary,
    },
    Error {
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AddonUpdate {
    pub folder: String,
    /// La fonte che ha reclamato la cartella, se c'è.
    pub source: Option<SourceKind>,
    /// Il progetto; più cartelle possono avere lo stesso progetto (per esempio DBM).
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
    /// Il file che la regola unica [`pick_file`] sceglie per il progetto, anche quando è quello
    /// installato: la reinstallazione (issue #148) scarica questo. `None` se la regola non
    /// sceglie niente o la fonte non lo usa. Resta nel backend: il JSON non cambia.
    #[serde(skip)]
    pub reinstall_file: Option<FileSummary>,
    /// Il mirror GitHub di un progetto CurseForge bloccato con un aggiornamento (D34, issue
    /// #163): `githubMirror` e `githubMirrorReason`, `null` quando non è stato calcolato.
    #[serde(flatten)]
    pub mirror: MirrorStatus,
}

/// Abbina e valuta. `version_type` è il `gameVersionTypeId` dell'installazione; `overrides`
/// lo sostituisce per i progetti che hanno una versione forzata. Senza Beta (vedi
/// [`evaluate_with`]).
#[cfg(test)]
pub fn evaluate(
    addons: &[InstalledAddon],
    matches: &FingerprintMatches,
    mods: &[Mod],
    version_type: u32,
    overrides: &VersionOverrides,
) -> Vec<AddonUpdate> {
    evaluate_with(addons, matches, mods, version_type, overrides, false)
}

/// Come `evaluate`; con `allow_beta` il file più recente può essere una Beta quando la versione
/// non ha una Release (issue #141).
pub fn evaluate_with(
    addons: &[InstalledAddon],
    matches: &FingerprintMatches,
    mods: &[Mod],
    version_type: u32,
    overrides: &VersionOverrides,
    allow_beta: bool,
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
                source: None,
                project: None,
                installed_file: None,
                status: UpdateStatus::NotMatched,
                version_override: None,
                project_folders: Vec::new(),
                installed_modules: Vec::new(),
                reinstall_file: None,
                mirror: MirrorStatus::default(),
            };
            let Some(fingerprint) = addon.fingerprint else {
                update.source = Some(SourceKind::Curseforge);
                update.status = UpdateStatus::Error {
                    message: "Unreadable folder: it cannot be recognized.".to_owned(),
                };
                return update;
            };
            let Some(found) = by_fingerprint.get(&u64::from(fingerprint)) else {
                return update;
            };
            let project = mods_by_id.get(&found.id);
            update.source = Some(SourceKind::Curseforge);
            update.project = Some(Project {
                id: found.id,
                source: SourceKind::Curseforge,
                key: found.id.to_string(),
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
            let picked = newest_file(found, target, allow_beta);
            update.reinstall_file = picked.map(FileSummary::from);
            update.status = match picked {
                Some(latest) if is_newer(latest, &found.file) => UpdateStatus::UpdateAvailable {
                    latest: FileSummary::from(latest),
                },
                // Con una Release per la versione la Beta non si usa mai (`pick_file`), con
                // l'opzione o senza: niente da suggerire.
                Some(_) => UpdateStatus::UpToDate,
                // Senza Release: se una Beta più recente c'è, lo si dice invece di "aggiornato".
                None => match newest_file(found, target, true) {
                    Some(beta) if !allow_beta && is_newer(beta, &found.file) => {
                        UpdateStatus::BetaOnly {
                            latest: FileSummary::from(beta),
                        }
                    }
                    _ => UpdateStatus::UpToDate,
                },
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

/// Il file più recente per il tipo di versione, tra quelli che CurseForge indica come ultimi per
/// il progetto, con la regola di [`pick_file`] (una Beta solo con `allow_beta`).
fn newest_file(found: &FingerprintMatch, version_type: u32, allow_beta: bool) -> Option<&File> {
    let candidates = found
        .latest_files
        .iter()
        .filter(|f| f.is_available)
        .filter(|f| for_version_type(f, version_type));
    pick_file(
        candidates,
        |f| f.release_type,
        |f| (f.file_date.as_str(), f.id),
        allow_beta,
    )
}

/// Le date sono ISO 8601 in UTC, quindi il confronto come testo segue il tempo; a parità di
/// data decide l'id, che CurseForge assegna in ordine crescente.
fn is_newer(candidate: &File, installed: &File) -> bool {
    candidate.id != installed.id
        && (candidate.file_date.as_str(), candidate.id)
            > (installed.file_date.as_str(), installed.id)
}

/// Scansiona l'installazione e offre le cartelle alle fonti di `sources`, nell'ordine (D33):
/// una cartella registrata in `records` va solo alla fonte del record; le altre a ogni fonte
/// che non le ha ancora reclamate. Una voce per cartella, nell'ordine della scansione; le
/// cartelle che nessuna fonte riconosce restano `notMatched` senza fonte. Reclama una cartella
/// solo la fonte che le dà un progetto: un errore su una cartella (per esempio CurseForge che non
/// la legge) non la toglie alle fonti successive e si mostra solo se nessuna la riconosce. Una
/// fonte che fallisce non ferma le altre: le cartelle che le erano state offerte e che nessuno
/// ha reclamato riportano il suo errore. Se falliscono tutte, l'errore è quello della prima.
/// Bloccante: va chiamata fuori dal thread principale.
#[cfg(test)]
pub fn check_updates(
    installation: &Installation,
    sources: &[&dyn Source],
    records: &SourceRecords,
) -> Result<Vec<AddonUpdate>, String> {
    check_updates_with(installation, None, sources, records)
}

/// Come `check_updates`, con i fingerprint della scansione presi da `cache` (issue #125).
pub fn check_updates_with(
    installation: &Installation,
    cache: Option<&FingerprintCache>,
    sources: &[&dyn Source],
    records: &SourceRecords,
) -> Result<Vec<AddonUpdate>, String> {
    scan_and_check(installation, cache, sources, records).map(|(_, updates)| updates)
}

/// Come `check_updates`, ma restituisce anche la scansione.
fn scan_and_check(
    installation: &Installation,
    cache: Option<&FingerprintCache>,
    sources: &[&dyn Source],
    records: &SourceRecords,
) -> Result<(Vec<InstalledAddon>, Vec<AddonUpdate>), String> {
    let addons = scan::scan_addons_with(&installation.path, &installation.flavor, cache)?;
    let updates = check_scanned(installation, &addons, sources, records)?;
    Ok((addons, updates))
}

/// La lista degli addon e i loro stati da una sola scansione (`refresh_addons`, issue #126).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Refresh {
    /// Come `list_addons`.
    pub addons: Vec<ListedAddon>,
    /// Come `check_updates`; vuoto se il controllo è fallito.
    pub updates: Vec<AddonUpdate>,
    /// Perché il controllo è fallito per intero: la lista arriva comunque.
    pub check_error: Option<String>,
}

/// Scansiona una volta l'installazione e ne ricava sia la lista per la tab Installed (con i
/// progetti di `installed`) sia gli stati di aggiornamento (D33, con `records`). Un controllo
/// fallito non toglie la lista: finisce in `check_error`. Solo una scansione fallita è `Err`.
/// Bloccante.
pub fn refresh_addons(
    installation: &Installation,
    cache: Option<&FingerprintCache>,
    sources: &[&dyn Source],
    records: &SourceRecords,
    installed: &InstalledRecords,
) -> Result<Refresh, String> {
    let addons = scan::scan_addons_with(&installation.path, &installation.flavor, cache)?;
    let (updates, check_error) = match check_scanned(installation, &addons, sources, records) {
        Ok(updates) => (updates, None),
        Err(error) => (Vec::new(), Some(error)),
    };
    Ok(Refresh {
        addons: scan::with_installed_projects(addons, installed),
        updates,
        check_error,
    })
}

/// Il controllo di `check_updates` su una scansione già fatta.
fn check_scanned(
    installation: &Installation,
    addons: &[InstalledAddon],
    sources: &[&dyn Source],
    records: &SourceRecords,
) -> Result<Vec<AddonUpdate>, String> {
    let no_records = ProjectRecords::new();
    // Cartella (in minuscolo) → la fonte del record che la nomina. Una cartella nei record di
    // più fonti non appartiene a nessuna.
    let mut owners: HashMap<String, Option<SourceKind>> = HashMap::new();
    for (source, projects) in records {
        for folder in projects.values().flat_map(|record| &record.folders) {
            owners
                .entry(folder.to_lowercase())
                .and_modify(|owner| {
                    if *owner != Some(*source) {
                        *owner = None;
                    }
                })
                .or_insert(Some(*source));
        }
    }
    let owner_of =
        |addon: &InstalledAddon| owners.get(&addon.folder.to_lowercase()).copied().flatten();

    let mut results: Vec<Option<AddonUpdate>> = vec![None; addons.len()];
    // Per cartella, il primo errore di una fonte a cui era stata offerta: della fonte intera o
    // della sola cartella. Vale solo se nessuna fonte la reclama.
    let mut failures: Vec<Option<AddonUpdate>> = vec![None; addons.len()];
    let mut first_error: Option<String> = None;
    let mut succeeded = false;
    for source in sources {
        let kind = source.kind();
        let offered_indexes: Vec<usize> = (0..addons.len())
            .filter(|&i| results[i].is_none())
            .filter(|&i| owner_of(&addons[i]).is_none_or(|owner| owner == kind))
            .collect();
        let offered: Vec<InstalledAddon> =
            offered_indexes.iter().map(|&i| addons[i].clone()).collect();
        let found = match source.check(
            installation,
            &offered,
            records.get(&kind).unwrap_or(&no_records),
        ) {
            Ok(found) => found,
            Err(error) => {
                for &i in &offered_indexes {
                    failures[i].get_or_insert_with(|| {
                        unclaimed(
                            &addons[i],
                            UpdateStatus::Error {
                                message: format!("{} check failed: {error}", kind.label()),
                            },
                        )
                    });
                }
                first_error.get_or_insert(error);
                continue;
            }
        };
        succeeded = true;
        for update in found {
            let Some(&i) = offered_indexes
                .iter()
                .find(|&&i| addons[i].folder == update.folder)
            else {
                continue;
            };
            // Solo un progetto reclama la cartella; un errore resta in serbo per il caso in cui
            // nessuna fonte la riconosca.
            if update.project.is_some() {
                results[i].get_or_insert(update);
            } else if matches!(update.status, UpdateStatus::Error { .. }) {
                failures[i].get_or_insert(update);
            }
        }
    }
    if !succeeded && let Some(error) = first_error {
        return Err(error);
    }

    let mut updates: Vec<AddonUpdate> = addons
        .iter()
        .zip(results)
        .zip(failures)
        .map(|((addon, result), failure)| {
            result
                .or(failure)
                .unwrap_or_else(|| unclaimed(addon, UpdateStatus::NotMatched))
        })
        .collect();
    // Il lavoro che una fonte rimanda a dopo il controllo di tutte (il mirror GitHub di
    // CurseForge, D34): nell'ordine delle fonti.
    for source in sources {
        source.finish(installation, &mut updates);
    }
    Ok(updates)
}

/// La voce di una cartella che nessuna fonte ha reclamato.
fn unclaimed(addon: &InstalledAddon, status: UpdateStatus) -> AddonUpdate {
    AddonUpdate {
        folder: addon.folder.clone(),
        source: None,
        project: None,
        installed_file: None,
        status,
        version_override: None,
        project_folders: Vec::new(),
        installed_modules: Vec::new(),
        reinstall_file: None,
        mirror: MirrorStatus::default(),
    }
}

/// CurseForge come fonte: fingerprint delle cartelle, versioni del gioco forzate per progetto
/// (D6) e `allowModDistribution` (D13).
pub struct CurseForgeSource<'a, T: Transport> {
    pub client: &'a CurseForgeClient<T>,
    pub overrides: &'a VersionOverrides,
    /// L'opzione "Beta quando non c'è una Release" (issue #141).
    pub allow_beta: bool,
    /// GitHub per i progetti che CurseForge non lascia scaricare (D34, issue #163). Senza, quei
    /// progetti si aggiornano solo dal sito.
    pub github: Option<&'a dyn MirrorSource>,
}

impl<T: Transport> Source for CurseForgeSource<'_, T> {
    fn kind(&self) -> SourceKind {
        SourceKind::Curseforge
    }

    fn check(
        &self,
        installation: &Installation,
        offered: &[InstalledAddon],
        _records: &ProjectRecords,
    ) -> Result<Vec<AddonUpdate>, String> {
        let version_type = version_type_for(&installation.flavor).ok_or_else(|| {
            format!(
                "The update check does not support the game version \"{}\" yet.",
                installation.name
            )
        })?;
        let fingerprints: Vec<u32> = offered.iter().filter_map(|a| a.fingerprint).collect();
        let matches = self
            .client
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
        let mods = self.client.get_mods(&mod_ids).map_err(|e| e.to_string())?;
        let mut updates = evaluate_with(
            offered,
            &matches,
            &mods,
            version_type,
            self.overrides,
            self.allow_beta,
        );
        if self.github.is_some() {
            mark_mirrors(&mut updates, &matches, &mods);
        }
        Ok(updates)
    }

    fn finish(&self, installation: &Installation, updates: &mut [AddonUpdate]) {
        if let Some(github) = self.github {
            resolve_mirrors(updates, github, &installation.flavor);
        }
    }

    fn update(&self, target: &UpdateTarget<'_>, record: &mut RecordWriter<'_>) -> UpdateOutcome {
        // Senza chiave il controllo è fallito e il progetto non è tra quelli riconosciuti: il
        // motivo vero è la chiave, non "non trovato".
        if !self.client.has_key() {
            return UpdateOutcome::error(
                SourceKind::Curseforge,
                target.key,
                "",
                CurseForgeError::MissingKey.to_string(),
            );
        }
        let Ok(mod_id) = target.key.parse::<u32>() else {
            return target.not_found(SourceKind::Curseforge);
        };
        update_project(
            target,
            self.client,
            self.github,
            mod_id,
            self.allow_beta,
            record,
        )
    }
}

/// Segna per il mirror GitHub (D34) i progetti bloccati con un aggiornamento disponibile, con
/// il progetto e il file che servono a [`resolve_mirrors`]. Nessuna rete: il mirror si calcola
/// dopo che tutte le fonti hanno fatto il controllo (`Source::finish`), così le richieste dei
/// progetti GitHub registrati passano prima e non trovano il limite consumato dai mirror.
/// Niente per i progetti aggiornati, con una sola Beta o che CurseForge lascia scaricare.
fn mark_mirrors(updates: &mut [AddonUpdate], matches: &FingerprintMatches, mods: &[Mod]) {
    for update in updates {
        let (Some(project), UpdateStatus::UpdateAvailable { latest }) =
            (&update.project, &update.status)
        else {
            continue;
        };
        if project.allow_mod_distribution {
            continue;
        }
        let Some(found) = mods.iter().find(|m| m.id == project.id) else {
            continue;
        };
        let Some(file) = matches
            .exact_matches
            .iter()
            .chain(&matches.partial_matches)
            .filter(|m| m.id == project.id)
            .flat_map(|m| &m.latest_files)
            .find(|f| f.id == latest.id)
        else {
            continue;
        };
        update.mirror.pending = Some(Box::new(github_mirror::Pending {
            project: found.clone(),
            file: file.clone(),
        }));
    }
}

/// Calcola il mirror delle voci segnate da [`mark_mirrors`], una volta per progetto e file.
fn resolve_mirrors(updates: &mut [AddonUpdate], github: &dyn MirrorSource, flavor: &str) {
    let mut resolved: HashMap<(u32, u32), MirrorStatus> = HashMap::new();
    for update in updates {
        let Some(pending) = update.mirror.pending.take() else {
            continue;
        };
        update.mirror = resolved
            .entry((pending.project.id, pending.file.id))
            .or_insert_with(|| {
                MirrorStatus::of(&github.resolve(&pending.project, &pending.file, flavor))
            })
            .clone();
    }
}

/// Il motivo di oggi per un progetto bloccato senza GitHub.
const BLOCKED: &str = "The author does not allow downloads from third-party apps: update it from the CurseForge website.";

// ── Aggiornamento ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum OutcomeStatus {
    Updated,
    Skipped,
    Error,
}

/// L'esito dell'aggiornamento di un progetto. I messaggi sono in inglese (D27).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateOutcome {
    /// L'id CurseForge; `0` per le altre fonti (vedi `source` e `key`).
    pub mod_id: u32,
    pub source: SourceKind,
    /// L'id del progetto nella fonte, come stringa.
    pub key: String,
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
    fn new(source: SourceKind, key: &str, name: &str, status: OutcomeStatus) -> Self {
        let mod_id = match source {
            SourceKind::Curseforge => key.parse().unwrap_or(0),
            _ => 0,
        };
        Self {
            mod_id,
            source,
            key: key.to_owned(),
            name: name.to_owned(),
            status,
            message: None,
            file: None,
        }
    }

    pub(crate) fn skipped(source: SourceKind, key: &str, name: &str, message: &str) -> Self {
        Self {
            message: Some(message.to_owned()),
            ..Self::new(source, key, name, OutcomeStatus::Skipped)
        }
    }

    pub(crate) fn error(source: SourceKind, key: &str, name: &str, message: String) -> Self {
        Self {
            message: Some(message),
            ..Self::new(source, key, name, OutcomeStatus::Error)
        }
    }

    pub(crate) fn updated(
        source: SourceKind,
        key: &str,
        name: &str,
        file: FileSummary,
        message: Option<String>,
    ) -> Self {
        Self {
            message,
            file: Some(file),
            ..Self::new(source, key, name, OutcomeStatus::Updated)
        }
    }
}

/// Aggiorna i progetti `projects` di un'installazione. Rifà il controllo con le stesse fonti di
/// `check_updates` invece di fidarsi del frontend, poi passa ogni progetto alla sua fonte. Un
/// errore su un progetto non ferma gli altri; un progetto ripetuto si aggiorna una volta sola.
/// Bloccante.
///
/// Restituisce `Err` solo se il controllo iniziale fallisce: in quel caso non si tocca niente.
#[cfg(test)]
pub fn update_addons(
    installation: &Installation,
    sources: &[&dyn Source],
    records: &SourceRecords,
    projects: &[ProjectKey],
    record: &mut RecordWriter<'_>,
    progress: &ProgressSink<'_>,
) -> Result<Vec<UpdateOutcome>, String> {
    update_addons_with(
        installation,
        None,
        sources,
        records,
        projects,
        false,
        record,
        progress,
    )
}

/// Come `update_addons`, con i fingerprint del controllo iniziale presi da `cache`. Con
/// `reinstall` (issue #148) un progetto già aggiornato si riscarica comunque: ogni fonte installa
/// il file che sceglierebbe per un aggiornamento, anche se è la versione installata.
#[allow(clippy::too_many_arguments)]
pub fn update_addons_with(
    installation: &Installation,
    cache: Option<&FingerprintCache>,
    sources: &[&dyn Source],
    records: &SourceRecords,
    projects: &[ProjectKey],
    reinstall: bool,
    record: &mut RecordWriter<'_>,
    progress: &ProgressSink<'_>,
) -> Result<Vec<UpdateOutcome>, String> {
    // Il controllo vale per tutti i progetti richiesti: ognuno è in `checking` finché non tocca
    // a lui. Un progetto saltato o fallito prima del download non riceve altre fasi: l'esito
    // restituito resta la risposta definitiva, e chi ascolta l'evento chiude lì l'avanzamento.
    let mut announced = HashSet::new();
    for project in projects.iter().filter(|p| announced.insert(*p)) {
        Reporter::new(progress, Some(project.source), &project.key, "").phase(Phase::Checking);
    }
    let (addons, updates) = scan_and_check(installation, cache, sources, records)?;
    let addons_dir = installation.path.join("Interface").join("AddOns");
    let mut seen = HashSet::new();
    Ok(projects
        .iter()
        .filter(|project| seen.insert(*project))
        .map(|project| {
            let Some(source) = sources.iter().find(|s| s.kind() == project.source) else {
                return UpdateOutcome::skipped(
                    project.source,
                    &project.key,
                    "",
                    "This source is not available.",
                );
            };
            let target = UpdateTarget {
                key: &project.key,
                addons_dir: &addons_dir,
                flavor: &installation.flavor,
                addons: &addons,
                updates: &updates,
                progress,
                reinstall,
            };
            source.update(&target, record)
        })
        .collect())
}

fn update_project<T: Transport>(
    target: &UpdateTarget<'_>,
    client: &CurseForgeClient<T>,
    github: Option<&dyn MirrorSource>,
    mod_id: u32,
    allow_beta: bool,
    record: &mut RecordWriter<'_>,
) -> UpdateOutcome {
    const CF: SourceKind = SourceKind::Curseforge;
    let key = target.key;
    let folders: Vec<&AddonUpdate> = target
        .updates
        .iter()
        .filter(|u| {
            u.project
                .as_ref()
                .is_some_and(|p| p.source == CF && p.id == mod_id)
        })
        .collect();
    let Some(project) = folders.first().and_then(|u| u.project.as_ref()) else {
        return target.not_found(CF);
    };
    let name = project.name.as_str();
    let available = folders.iter().find_map(|u| match &u.status {
        UpdateStatus::UpdateAvailable { latest } => Some((*u, latest)),
        _ => None,
    });
    // Con `reinstall` un progetto aggiornato riscarica il file della regola unica, anche se è
    // quello installato; con un aggiornamento disponibile resta quello.
    let chosen = available.or_else(|| {
        target
            .reinstall
            .then(|| {
                folders
                    .iter()
                    .find_map(|u| u.reinstall_file.as_ref().map(|file| (*u, file)))
            })
            .flatten()
    });
    let Some((installed, latest)) = chosen else {
        if !target.reinstall {
            return UpdateOutcome::skipped(CF, key, name, "Already up to date.");
        }
        // La regola non sceglie niente: con l'opzione spenta il motivo può essere una Beta più
        // recente (`betaOnly`), e lo si dice; altrimenti non c'è un file da reinstallare.
        let beta_only = folders
            .iter()
            .any(|u| matches!(u.status, UpdateStatus::BetaOnly { .. }));
        let reason = match (allow_beta, beta_only) {
            (false, true) => {
                "No Release for this game version: turn on the Beta option to reinstall the Beta."
            }
            (false, false) => "No Release for this game version: nothing to reinstall.",
            (true, _) => "No Release or Beta for this game version: nothing to reinstall.",
        };
        return UpdateOutcome::skipped(CF, key, name, reason);
    };
    // Senza un aggiornamento la scelta della regola non è più recente dell'installato: se è un
    // altro file (per esempio un'Alpha installata a mano, o una Beta con l'opzione ora spenta)
    // reinstallarla sarebbe un ritorno a una versione precedente, e non si fa in silenzio.
    if available.is_none()
        && installed
            .installed_file
            .as_ref()
            .is_some_and(|file| file.id != latest.id)
    {
        return UpdateOutcome::skipped(
            CF,
            key,
            name,
            "The installed file is newer than the one the app would install: reinstalling would downgrade it.",
        );
    }
    if !project.allow_mod_distribution {
        return match github {
            Some(github) => update_from_mirror(
                target, client, github, mod_id, name, installed, latest, record,
            ),
            None => UpdateOutcome::skipped(CF, key, name, BLOCKED),
        };
    }

    let file = match client.get_file(mod_id, latest.id) {
        Ok(file) => file,
        Err(error) => return UpdateOutcome::error(CF, key, name, error.to_string()),
    };
    if file.download_url.is_none() {
        return UpdateOutcome::skipped(
            CF,
            key,
            name,
            "CurseForge does not provide a download link for this file: update it from the CurseForge website.",
        );
    }
    let old_modules = removable_modules(target.addons, target.updates, installed);
    let reporter = Reporter::new(target.progress, Some(CF), key, name);
    let folders = match install_file(
        client,
        &file,
        target.addons_dir,
        &old_modules,
        &|_| Ok(()),
        &reporter,
    ) {
        Ok(folders) => folders,
        Err(message) => return UpdateOutcome::error(CF, key, name, message),
    };
    record_curseforge(record, key, name, &file, folders, None)
}

/// Un progetto CurseForge bloccato (D13) aggiornato o reinstallato dal suo mirror GitHub (D34,
/// issue #163): il file CurseForge `latest` si rilegge con il progetto, il mirror si ricalcola
/// (la release è già nella cache di GitHub se il controllo l'ha letta) e lo zip di GitHub
/// sostituisce le cartelle. Il record resta CurseForge, con l'id del file che combacia.
#[allow(clippy::too_many_arguments)]
fn update_from_mirror<T: Transport>(
    target: &UpdateTarget<'_>,
    client: &CurseForgeClient<T>,
    github: &dyn MirrorSource,
    mod_id: u32,
    name: &str,
    installed: &AddonUpdate,
    latest: &FileSummary,
    record: &mut RecordWriter<'_>,
) -> UpdateOutcome {
    const CF: SourceKind = SourceKind::Curseforge;
    let key = target.key;
    let file = match client.get_file(mod_id, latest.id) {
        Ok(file) => file,
        Err(error) => return UpdateOutcome::error(CF, key, name, error.to_string()),
    };
    let project = match client.get_mods(&[mod_id]) {
        Ok(mods) => mods.into_iter().find(|m| m.id == mod_id),
        Err(error) => return UpdateOutcome::error(CF, key, name, error.to_string()),
    };
    let Some(project) = project else {
        return UpdateOutcome::skipped(CF, key, name, BLOCKED);
    };
    let (repo, tag, asset) = match github.resolve(&project, &file, target.flavor) {
        Outcome::Mirror { repo, tag, asset } => (repo, tag, asset),
        Outcome::Unavailable { reason } => {
            let message = format!(
                "The author does not allow downloads from third-party apps and the GitHub copy cannot be used. {}",
                reason.message()
            );
            return match reason {
                Reason::Failed(_) => UpdateOutcome::error(CF, key, name, message),
                _ => UpdateOutcome::skipped(CF, key, name, &message),
            };
        }
    };
    let old_modules = removable_modules(target.addons, target.updates, installed);
    let reporter = Reporter::new(target.progress, Some(CF), key, name);
    let folders = match github_mirror::install(
        github,
        &asset,
        target.addons_dir,
        &old_modules,
        &|_| Ok(()),
        &reporter,
    ) {
        Ok(folders) => folders,
        Err(message) => return UpdateOutcome::error(CF, key, name, message),
    };
    let note = github_mirror::installed_message(&repo, &tag);
    record_curseforge(record, key, name, &file, folders, Some(note))
}

/// Registra il file CurseForge `file` installato nelle cartelle `folders` e restituisce l'esito
/// `updated`, con `note` nel messaggio. Le cartelle sono già sostituite: se il registro non si
/// salva l'installazione resta riuscita, e il messaggio lo dice. Lo usano il download da
/// CurseForge e quello dal mirror GitHub (issue #163, poi l'installazione dalla Search).
pub(crate) fn record_curseforge(
    record: &mut RecordWriter<'_>,
    key: &str,
    name: &str,
    file: &File,
    folders: Vec<String>,
    note: Option<String>,
) -> UpdateOutcome {
    const CF: SourceKind = SourceKind::Curseforge;
    let failed = record(
        CF,
        key,
        SourceRecord {
            file: file.id.to_string(),
            folders,
        },
    )
    .err()
    .map(|error| format!("Installed, but the app could not remember its folders: {error}"));
    let message = match (note, failed) {
        (Some(note), Some(failed)) => Some(format!("{note} {failed}")),
        (note, failed) => note.or(failed),
    };
    UpdateOutcome::updated(CF, key, name, FileSummary::from(file), message)
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
    let project_of = |u: &AddonUpdate| u.project.as_ref().map(|p| (p.source, p.key.clone()));
    let this_project = project_of(installed);
    let own: HashSet<String> = installed
        .installed_modules
        .iter()
        .map(|m| lower(&m.name))
        .collect();
    let shared: HashSet<String> = updates
        .iter()
        .filter(|u| project_of(u) != this_project)
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
    reporter: &Reporter<'_>,
) -> Result<Vec<String>, String> {
    reporter.download(0, None);
    let bytes = client
        .download_with_progress(file, &mut |received, total| {
            reporter.download(received, total)
        })
        .map_err(|e| e.to_string())?;
    reporter.downloaded(bytes.len() as u64);
    install_bytes(&bytes, addons_dir, old_modules, check, reporter)
}

/// Estrae lo zip `bytes` in staging e sostituisce le cartelle, come [`install_file`] dopo il
/// download: lo usano anche le fonti che scaricano con un proprio trasporto.
pub(crate) fn install_bytes(
    bytes: &[u8],
    addons_dir: &Path,
    old_modules: &[String],
    check: &dyn Fn(&[String]) -> Result<(), String>,
    reporter: &Reporter<'_>,
) -> Result<Vec<String>, String> {
    reporter.phase(Phase::Extracting);
    let staging = install::WorkDir::staging(addons_dir)?;
    let folders = install::extract(bytes, staging.path())?;
    check(&folders)?;
    reporter.phase(Phase::Installing);
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
    use crate::progress;
    use crate::store::InstalledRecord;

    /// `check_updates` con la sola fonte CurseForge e senza record, come prima di D12.
    fn cf_check_updates<T: Transport>(
        installation: &Installation,
        client: &CurseForgeClient<T>,
        overrides: &VersionOverrides,
    ) -> Result<Vec<AddonUpdate>, String> {
        let source = CurseForgeSource {
            client,
            overrides,
            allow_beta: false,
            github: None,
        };
        check_updates(installation, &[&source], &SourceRecords::new())
    }

    /// `update_addons` con la sola fonte CurseForge, i progetti per `mod_id` e il registro nella
    /// forma CurseForge.
    fn cf_update_addons<T: Transport>(
        installation: &Installation,
        client: &CurseForgeClient<T>,
        overrides: &VersionOverrides,
        mod_ids: &[u32],
        record: &mut dyn FnMut(u32, InstalledRecord) -> Result<(), String>,
    ) -> Result<Vec<UpdateOutcome>, String> {
        let plain = Mode {
            allow_beta: false,
            reinstall: false,
        };
        cf_update_addons_with(
            installation,
            client,
            overrides,
            mod_ids,
            plain,
            record,
            &progress::ignore,
        )
    }

    /// L'opzione Beta e la reinstallazione (issue #148) per [`cf_update_addons_with`].
    #[derive(Clone, Copy)]
    struct Mode {
        allow_beta: bool,
        reinstall: bool,
    }

    /// Come [`cf_update_addons`], con `mode` e l'avanzamento in `progress`.
    fn cf_update_addons_with<T: Transport>(
        installation: &Installation,
        client: &CurseForgeClient<T>,
        overrides: &VersionOverrides,
        mod_ids: &[u32],
        mode: Mode,
        record: &mut dyn FnMut(u32, InstalledRecord) -> Result<(), String>,
        progress: &ProgressSink<'_>,
    ) -> Result<Vec<UpdateOutcome>, String> {
        let source = CurseForgeSource {
            client,
            overrides,
            allow_beta: mode.allow_beta,
            github: None,
        };
        let projects: Vec<ProjectKey> = mod_ids
            .iter()
            .map(|id| ProjectKey {
                source: SourceKind::Curseforge,
                key: id.to_string(),
            })
            .collect();
        update_addons_with(
            installation,
            None,
            &[&source],
            &SourceRecords::new(),
            &projects,
            mode.reinstall,
            &mut |source, key, written| {
                assert_eq!(source, SourceKind::Curseforge);
                record(
                    key.parse().unwrap(),
                    InstalledRecord {
                        file_id: written.file.parse().unwrap(),
                        folders: written.folders,
                    },
                )
            },
            progress,
        )
    }
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
            dependencies: Vec::new(),
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
                source_url: None,
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
    fn betas_and_other_game_versions_are_not_updates() {
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
        // Senza l'opzione la Beta non è un aggiornamento, ma lo stato la nomina (issue #141).
        assert!(
            matches!(&result[0].status, UpdateStatus::BetaOnly { latest } if latest.id == 13),
            "{:?}",
            result[0].status
        );
    }

    fn beta_match(latest_files: Vec<File>) -> FingerprintMatch {
        FingerprintMatch {
            id: 7,
            // Il file installato è una Beta.
            file: file(10, "2026-09-01T00:00:00Z", 2, FOREVER, &[100]),
            latest_files,
        }
    }

    fn evaluate_beta(found: FingerprintMatch, allow_beta: bool) -> UpdateStatus {
        evaluate_with(
            &[addon("Foo", Some(100))],
            &matches(vec![found]),
            &[project(7, Some(true))],
            FOREVER,
            &VersionOverrides::new(),
            allow_beta,
        )
        .remove(0)
        .status
    }

    #[test]
    fn with_the_beta_option_a_newer_beta_is_an_update_and_a_release_still_wins() {
        let only_beta = || beta_match(vec![file(13, "2026-10-02T00:00:00Z", 2, FOREVER, &[])]);
        assert!(matches!(
            evaluate_beta(only_beta(), true),
            UpdateStatus::UpdateAvailable { latest } if latest.id == 13
        ));
        // Appena esiste una Release, anche più vecchia della Beta, l'aggiornamento è quella.
        let with_release = beta_match(vec![
            file(13, "2026-10-02T00:00:00Z", 2, FOREVER, &[]),
            file(12, "2026-09-20T00:00:00Z", 1, FOREVER, &[]),
        ]);
        assert!(matches!(
            evaluate_beta(with_release, true),
            UpdateStatus::UpdateAvailable { latest } if latest.id == 12
        ));
        // L'Alpha non conta mai.
        let alpha = beta_match(vec![file(15, "2026-10-05T00:00:00Z", 3, FOREVER, &[])]);
        assert_eq!(evaluate_beta(alpha, true), UpdateStatus::UpToDate);
        // Con una Release per la versione la Beta non conta, nemmeno senza l'opzione: nessun
        // suggerimento, perché accenderla non cambierebbe niente.
        let release_installed = FingerprintMatch {
            id: 7,
            file: file(10, "2026-09-01T00:00:00Z", 1, FOREVER, &[100]),
            latest_files: vec![
                file(10, "2026-09-01T00:00:00Z", 1, FOREVER, &[]),
                file(13, "2026-10-02T00:00:00Z", 2, FOREVER, &[]),
            ],
        };
        assert_eq!(
            evaluate_beta(release_installed, false),
            UpdateStatus::UpToDate
        );
        // La Beta installata è già la più recente.
        let same = beta_match(vec![file(10, "2026-09-01T00:00:00Z", 2, FOREVER, &[])]);
        assert_eq!(evaluate_beta(same, true), UpdateStatus::UpToDate);
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
            source: None,
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
            reinstall_file: Some(FileSummary {
                id: 1,
                name: "x".to_owned(),
                date: "d".to_owned(),
            }),
            mirror: MirrorStatus::default(),
        };
        // Senza mirror calcolato i due campi nuovi ci sono, entrambi `null`.
        let json = serde_json::to_value(&update).unwrap();
        assert_eq!(json["githubMirror"], serde_json::Value::Null);
        assert_eq!(json["githubMirrorReason"], serde_json::Value::Null);
        let mirror = AddonUpdate {
            mirror: MirrorStatus {
                github_mirror: Some(github_mirror::GithubMirror {
                    repo: "Nevcairiel/Bartender4".to_owned(),
                    tag: "4.18.3.1".to_owned(),
                }),
                github_mirror_reason: None,
                pending: None,
            },
            ..update.clone()
        };
        let json = serde_json::to_value(&mirror).unwrap();
        assert_eq!(
            json["githubMirror"],
            serde_json::json!({"repo": "Nevcairiel/Bartender4", "tag": "4.18.3.1"})
        );
        assert_eq!(json["githubMirrorReason"], serde_json::Value::Null);
        let unavailable = AddonUpdate {
            mirror: MirrorStatus {
                github_mirror: None,
                github_mirror_reason: Some("why".to_owned()),
                pending: None,
            },
            ..update.clone()
        };
        let json = serde_json::to_value(&unavailable).unwrap();
        assert_eq!(json["githubMirror"], serde_json::Value::Null);
        assert_eq!(json["githubMirrorReason"], "why");

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
        use super::{Mode, cf_check_updates, cf_update_addons, cf_update_addons_with};
        use crate::sources::curseforge::{ApiKey, BytesResponse, HttpResponse, TransportError};
        use crate::store::InstalledRecord;
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
            cf_update_addons(installation, client, overrides, mod_ids, &mut |_, _| Ok(()))
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
                    source: SourceKind::Curseforge,
                    key: "1".to_owned(),
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

            let checked = cf_check_updates(&fx.installation, &client, &no_overrides()).unwrap();
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
            let outcomes = cf_update_addons(
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

        // ── Reinstallazione (issue #148) ────────────────────────────────────

        const REINSTALL: Mode = Mode {
            allow_beta: false,
            reinstall: true,
        };

        /// Un progetto aggiornato con il file `id` installato, la sua risposta e il suo zip.
        fn up_to_date(fx: &Fixture, mod_id: u32, folder: &str, release_type: u8) -> (Value, Value) {
            let mut current = file_json(
                mod_id,
                mod_id * 10,
                "2026-09-01T00:00:00Z",
                &[(folder, fx.fingerprints[folder])],
            );
            current["releaseType"] = json!(release_type);
            let found = json!({"id": mod_id, "file": current, "latestFiles": [current.clone()]});
            (found, current)
        }

        fn reinstall_routes(found: Value, project: Value, file: &Value, zip: Vec<u8>) -> Routes {
            let id = file["id"].as_u64().unwrap();
            let mod_id = file["modId"].as_u64().unwrap();
            Routes {
                fingerprints: json!({"data": {"exactMatches": [found]}}).to_string(),
                mods: json!({"data": [project]}).to_string(),
                files: HashMap::from([(
                    format!("{API}/v1/mods/{mod_id}/files/{id}"),
                    json!({ "data": file }).to_string(),
                )]),
                zips: HashMap::from([(download_url(id as u32), zip)]),
                ..Routes::default()
            }
        }

        #[test]
        fn reinstall_downloads_the_installed_file_again() {
            let fx = fixture(&[("Alpha", "alpha v1"), ("Other", "other")]);
            let (found, current) = up_to_date(&fx, 1, "Alpha", 1);
            let routes = reinstall_routes(
                found,
                project(1, "Alpha", true),
                &current,
                zip_of(&[
                    ("Alpha/Alpha.toc", "## Title: Alpha\nAlpha.lua\n"),
                    ("Alpha/Alpha.lua", "alpha v1"),
                ]),
            );
            let log = Arc::clone(&routes.log);
            let client = client(routes);
            // Un file estraneo, fuori dal fingerprint: la reinstallazione rimette la cartella com'è
            // nello zip.
            fs::write(fx.addons.join("Alpha").join("Junk.txt"), "junk").unwrap();

            // Senza reinstall: già aggiornato, niente scaricato.
            let outcomes =
                update_without_record(&fx.installation, &client, &no_overrides(), &[1]).unwrap();
            assert_eq!(outcomes[0].status, OutcomeStatus::Skipped);
            assert_eq!(outcomes[0].message.as_deref(), Some("Already up to date."));
            assert!(log.downloads.lock().unwrap().is_empty());

            let mut records = Vec::new();
            let phases = Mutex::new(Vec::new());
            let outcomes = cf_update_addons_with(
                &fx.installation,
                &client,
                &no_overrides(),
                &[1],
                REINSTALL,
                &mut |mod_id, record| {
                    records.push((mod_id, record));
                    Ok(())
                },
                &|p: crate::progress::Progress| {
                    phases.lock().unwrap().push((p.key, p.phase));
                },
            )
            .unwrap();

            assert_eq!(outcomes[0].status, OutcomeStatus::Updated);
            assert_eq!(outcomes[0].file.as_ref().map(|f| f.id), Some(10));
            assert_eq!(*log.downloads.lock().unwrap(), [download_url(10)]);
            assert_eq!(
                records,
                [(
                    1,
                    InstalledRecord {
                        file_id: 10,
                        folders: vec!["Alpha".to_owned()],
                    }
                )]
            );
            // La cartella è quella dello zip: il file estraneo non c'è più.
            assert_eq!(names(&fx.addons.join("Alpha")), ["Alpha.lua", "Alpha.toc"]);
            assert_eq!(names(&fx.addons), ["Alpha", "Other"]);
            // L'avanzamento è quello di un aggiornamento.
            let phases: Vec<Phase> = phases
                .into_inner()
                .unwrap()
                .into_iter()
                .filter(|(key, _)| key == "1")
                .map(|(_, phase)| phase)
                .collect();
            assert_eq!(phases.first(), Some(&Phase::Checking));
            assert!(phases.contains(&Phase::Downloading));
            assert_eq!(phases.last(), Some(&Phase::Installing));
        }

        #[test]
        fn reinstall_with_an_update_available_installs_the_update() {
            let fx = fixture(&[("Alpha", "alpha v1")]);
            let new_file = file_json(1, 11, "2026-10-01T00:00:00Z", &[]);
            let routes = reinstall_routes(
                installed(&fx, 1, &["Alpha"], true),
                project(1, "Alpha", true),
                &new_file,
                zip_of(&[("Alpha/Alpha.lua", "alpha v2")]),
            );
            let log = Arc::clone(&routes.log);
            let client = client(routes);

            let outcomes = cf_update_addons_with(
                &fx.installation,
                &client,
                &no_overrides(),
                &[1],
                REINSTALL,
                &mut |_, _| Ok(()),
                &crate::progress::ignore,
            )
            .unwrap();

            assert_eq!(outcomes[0].status, OutcomeStatus::Updated);
            assert_eq!(outcomes[0].file.as_ref().map(|f| f.id), Some(11));
            assert_eq!(*log.downloads.lock().unwrap(), [download_url(11)]);
        }

        #[test]
        fn reinstall_keeps_the_download_rules() {
            let fx = fixture(&[("Alpha", "alpha"), ("Beta", "beta")]);
            let (alpha, _) = up_to_date(&fx, 1, "Alpha", 1);
            let (beta, mut no_link) = up_to_date(&fx, 2, "Beta", 1);
            no_link["downloadUrl"] = Value::Null;
            let routes = Routes {
                fingerprints: json!({"data": {"exactMatches": [alpha, beta]}}).to_string(),
                mods: json!({"data": [project(1, "Alpha", false), project(2, "Beta", true)]})
                    .to_string(),
                files: HashMap::from([(
                    format!("{API}/v1/mods/2/files/20"),
                    json!({ "data": no_link }).to_string(),
                )]),
                ..Routes::default()
            };
            let log = Arc::clone(&routes.log);
            let client = client(routes);

            let outcomes = cf_update_addons_with(
                &fx.installation,
                &client,
                &no_overrides(),
                &[1, 2, 99],
                REINSTALL,
                &mut |_, _| panic!("nothing is installed"),
                &crate::progress::ignore,
            )
            .unwrap();

            let summary: Vec<(u32, OutcomeStatus, &str)> = outcomes
                .iter()
                .map(|o| (o.mod_id, o.status, o.message.as_deref().unwrap_or("")))
                .collect();
            assert_eq!(
                summary,
                [
                    (
                        1,
                        OutcomeStatus::Skipped,
                        "The author does not allow downloads from third-party apps: update it from the CurseForge website."
                    ),
                    (
                        2,
                        OutcomeStatus::Skipped,
                        "CurseForge does not provide a download link for this file: update it from the CurseForge website."
                    ),
                    (
                        99,
                        OutcomeStatus::Skipped,
                        "Not linked to a source: cannot reinstall."
                    ),
                ]
            );
            assert!(log.downloads.lock().unwrap().is_empty());
        }

        #[test]
        fn reinstall_of_a_beta_follows_the_beta_option() {
            let fx = fixture(&[("Alpha", "alpha")]);
            let (found, current) = up_to_date(&fx, 1, "Alpha", 2);
            let routes = reinstall_routes(
                found,
                project(1, "Alpha", true),
                &current,
                zip_of(&[("Alpha/Alpha.lua", "alpha")]),
            );
            let log = Arc::clone(&routes.log);
            let client = client(routes);
            let reinstall = |allow_beta| {
                cf_update_addons_with(
                    &fx.installation,
                    &client,
                    &no_overrides(),
                    &[1],
                    Mode {
                        allow_beta,
                        reinstall: true,
                    },
                    &mut |_, _| Ok(()),
                    &crate::progress::ignore,
                )
                .unwrap()
                .remove(0)
            };

            // Senza l'opzione la regola unica non sceglie la Beta: niente da reinstallare.
            let off = reinstall(false);
            assert_eq!(off.status, OutcomeStatus::Skipped);
            assert_eq!(
                off.message.as_deref(),
                Some("No Release for this game version: nothing to reinstall.")
            );
            assert!(log.downloads.lock().unwrap().is_empty());

            let on = reinstall(true);
            assert_eq!(on.status, OutcomeStatus::Updated);
            assert_eq!(on.file.as_ref().map(|f| f.id), Some(10));
            assert_eq!(*log.downloads.lock().unwrap(), [download_url(10)]);
        }

        /// Una Beta installata con l'opzione accesa, poi spenta, e una Release più vecchia: la
        /// regola sceglie la Release, ma reinstallarla tornerebbe indietro.
        #[test]
        fn reinstall_never_downgrades() {
            let fx = fixture(&[("Alpha", "alpha")]);
            let (mut found, current) = up_to_date(&fx, 1, "Alpha", 2);
            let older = file_json(1, 9, "2026-08-01T00:00:00Z", &[]);
            found["latestFiles"] = json!([current, older.clone()]);
            let routes = reinstall_routes(
                found,
                project(1, "Alpha", true),
                &older,
                zip_of(&[("Alpha/Alpha.lua", "old")]),
            );
            let log = Arc::clone(&routes.log);
            let client = client(routes);

            let outcome = cf_update_addons_with(
                &fx.installation,
                &client,
                &no_overrides(),
                &[1],
                REINSTALL,
                &mut |_, _| panic!("nothing is installed"),
                &crate::progress::ignore,
            )
            .unwrap()
            .remove(0);

            assert_eq!(outcome.status, OutcomeStatus::Skipped);
            assert_eq!(
                outcome.message.as_deref(),
                Some(
                    "The installed file is newer than the one the app would install: reinstalling would downgrade it."
                )
            );
            assert!(log.downloads.lock().unwrap().is_empty());
        }

        /// Con l'opzione spenta e una Beta più recente dell'installato il motivo la nomina.
        #[test]
        fn reinstall_names_a_newer_beta_when_the_option_is_off() {
            let fx = fixture(&[("Alpha", "alpha")]);
            let (mut found, mut newer) = up_to_date(&fx, 1, "Alpha", 2);
            newer["id"] = json!(12);
            newer["fileDate"] = json!("2026-10-01T00:00:00Z");
            found["latestFiles"] = json!([newer]);
            let routes = reinstall_routes(
                found,
                project(1, "Alpha", true),
                &newer,
                zip_of(&[("Alpha/Alpha.lua", "beta")]),
            );
            let client = client(routes);

            let outcome = cf_update_addons_with(
                &fx.installation,
                &client,
                &no_overrides(),
                &[1],
                REINSTALL,
                &mut |_, _| panic!("nothing is installed"),
                &crate::progress::ignore,
            )
            .unwrap()
            .remove(0);

            assert_eq!(
                outcome.message.as_deref(),
                Some(
                    "No Release for this game version: turn on the Beta option to reinstall the Beta."
                )
            );
        }

        #[test]
        fn without_reinstall_a_missing_project_keeps_its_message() {
            let fx = fixture(&[("Alpha", "alpha")]);
            let (found, _) = up_to_date(&fx, 1, "Alpha", 1);
            let routes = Routes {
                fingerprints: json!({"data": {"exactMatches": [found]}}).to_string(),
                mods: json!({"data": [project(1, "Alpha", true)]}).to_string(),
                ..Routes::default()
            };
            let client = client(routes);

            let outcomes =
                update_without_record(&fx.installation, &client, &no_overrides(), &[99]).unwrap();

            assert_eq!(
                outcomes[0].message.as_deref(),
                Some("Not found among the installed addons.")
            );
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
            let result = cf_update_addons(
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

            let outcomes = cf_update_addons(
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
                    cf_check_updates(&installation, &client, &VersionOverrides::new()).unwrap()
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
            let json = serde_json::to_value(UpdateOutcome::skipped(
                SourceKind::Curseforge,
                "7",
                "Foo",
                "why",
            ))
            .unwrap();
            assert_eq!(
                json,
                json!({"modId": 7, "source": "curseforge", "key": "7", "name": "Foo", "status": "skipped", "message": "why"})
            );
            // Le altre fonti non hanno un id CurseForge.
            let json = serde_json::to_value(UpdateOutcome::skipped(
                SourceKind::Github,
                "owner/repo",
                "Foo",
                "why",
            ))
            .unwrap();
            assert_eq!(json["modId"], 0);
            assert_eq!(json["source"], "github");
            assert_eq!(json["key"], "owner/repo");
            let json = serde_json::to_value(UpdateOutcome {
                mod_id: 7,
                source: SourceKind::Curseforge,
                key: "7".to_owned(),
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

        /// Una fonte finta: reclama le cartelle di `claims` con un progetto proprio, oppure
        /// fallisce; ricorda le cartelle che le sono state offerte.
        struct FakeSource {
            kind: SourceKind,
            claims: Vec<&'static str>,
            fails: Option<&'static str>,
            offered: Mutex<Vec<String>>,
        }

        impl FakeSource {
            fn new(kind: SourceKind, claims: &[&'static str]) -> Self {
                Self {
                    kind,
                    claims: claims.to_vec(),
                    fails: None,
                    offered: Mutex::new(Vec::new()),
                }
            }

            fn failing(kind: SourceKind, error: &'static str) -> Self {
                Self {
                    fails: Some(error),
                    ..Self::new(kind, &[])
                }
            }

            fn offered(&self) -> Vec<String> {
                self.offered.lock().unwrap().clone()
            }
        }

        impl Source for FakeSource {
            fn kind(&self) -> SourceKind {
                self.kind
            }

            fn check(
                &self,
                _installation: &Installation,
                offered: &[InstalledAddon],
                _records: &ProjectRecords,
            ) -> Result<Vec<AddonUpdate>, String> {
                self.offered
                    .lock()
                    .unwrap()
                    .extend(offered.iter().map(|a| a.folder.clone()));
                if let Some(error) = self.fails {
                    return Err(error.to_owned());
                }
                Ok(offered
                    .iter()
                    .filter(|a| self.claims.contains(&a.folder.as_str()))
                    .map(|a| AddonUpdate {
                        folder: a.folder.clone(),
                        source: Some(self.kind),
                        project: Some(Project {
                            id: 0,
                            source: self.kind,
                            key: format!("{}-key", a.folder),
                            name: a.folder.clone(),
                            url: None,
                            allow_mod_distribution: true,
                        }),
                        installed_file: None,
                        status: UpdateStatus::UpToDate,
                        version_override: None,
                        project_folders: vec![a.folder.clone()],
                        installed_modules: Vec::new(),
                        reinstall_file: None,
                        mirror: Default::default(),
                    })
                    .collect())
            }

            fn update(
                &self,
                target: &UpdateTarget<'_>,
                record: &mut RecordWriter<'_>,
            ) -> UpdateOutcome {
                record(
                    self.kind,
                    target.key,
                    SourceRecord {
                        file: "v2".to_owned(),
                        folders: Vec::new(),
                    },
                )
                .unwrap();
                UpdateOutcome::skipped(self.kind, target.key, "", "fake")
            }
        }

        fn sources_of(updates: &[AddonUpdate]) -> Vec<(&str, Option<SourceKind>)> {
            updates
                .iter()
                .map(|u| (u.folder.as_str(), u.source))
                .collect()
        }

        fn record_of(source: SourceKind, key: &str, folders: &[&str]) -> SourceRecords {
            SourceRecords::from([(
                source,
                ProjectRecords::from([(
                    key.to_owned(),
                    SourceRecord {
                        file: "1".to_owned(),
                        folders: folders.iter().map(|f| (*f).to_owned()).collect(),
                    },
                )]),
            )])
        }

        #[test]
        fn sources_claim_folders_in_order() {
            let fx = fixture(&[("Alpha", "a"), ("Beta", "b"), ("Gamma", "c")]);
            let first = FakeSource::new(SourceKind::Curseforge, &["Alpha"]);
            let second = FakeSource::new(SourceKind::Tukui, &["Alpha", "Beta"]);
            let updates =
                check_updates(&fx.installation, &[&first, &second], &SourceRecords::new()).unwrap();
            assert_eq!(
                sources_of(&updates),
                [
                    ("Alpha", Some(SourceKind::Curseforge)),
                    ("Beta", Some(SourceKind::Tukui)),
                    ("Gamma", None),
                ]
            );
            assert_eq!(updates[2].status, UpdateStatus::NotMatched);
            // Una cartella reclamata non si offre alla fonte successiva.
            assert_eq!(first.offered(), ["Alpha", "Beta", "Gamma"]);
            assert_eq!(second.offered(), ["Beta", "Gamma"]);
        }

        #[test]
        fn a_refresh_gives_the_list_and_the_states_from_one_scan() {
            let fx = fixture(&[("Alpha", "a"), ("Beta", "b")]);
            let first = FakeSource::new(SourceKind::Curseforge, &["Alpha"]);
            let installed = InstalledRecords::from([(
                7,
                crate::store::InstalledRecord {
                    file_id: 1,
                    folders: vec!["Alpha".to_owned()],
                },
            )]);
            let refresh = refresh_addons(
                &fx.installation,
                None,
                &[&first],
                &SourceRecords::new(),
                &installed,
            )
            .unwrap();
            let folders: Vec<&str> = refresh
                .addons
                .iter()
                .map(|a| a.addon.folder.as_str())
                .collect();
            assert_eq!(folders, ["Alpha", "Beta"]);
            assert_eq!(
                refresh.addons[0].installed_project.as_ref().map(|p| p.id),
                Some(7)
            );
            assert_eq!(
                sources_of(&refresh.updates),
                [("Alpha", Some(SourceKind::Curseforge)), ("Beta", None)]
            );
            assert_eq!(refresh.check_error, None);
            // Le cartelle sono state offerte una volta sola: una scansione, un controllo.
            assert_eq!(first.offered(), ["Alpha", "Beta"]);
        }

        #[test]
        fn a_failed_check_keeps_the_list_with_the_reason() {
            let fx = fixture(&[("Alpha", "a")]);
            let failing = FakeSource::failing(SourceKind::Curseforge, "network down");
            let refresh = refresh_addons(
                &fx.installation,
                None,
                &[&failing],
                &SourceRecords::new(),
                &InstalledRecords::new(),
            )
            .unwrap();
            assert_eq!(refresh.addons.len(), 1);
            assert!(refresh.updates.is_empty());
            assert_eq!(refresh.check_error.as_deref(), Some("network down"));
        }

        #[test]
        fn a_refresh_of_a_missing_installation_is_an_error() {
            let fx = fixture(&[("Alpha", "a")]);
            let mut gone = fx.installation.clone();
            gone.path = fx.installation.path.join("missing");
            let first = FakeSource::new(SourceKind::Curseforge, &[]);
            assert!(
                refresh_addons(
                    &gone,
                    None,
                    &[&first],
                    &SourceRecords::new(),
                    &InstalledRecords::new()
                )
                .is_err()
            );
        }

        #[test]
        fn a_recorded_folder_goes_only_to_its_source() {
            let fx = fixture(&[("Alpha", "a"), ("Beta", "b")]);
            let first = FakeSource::new(SourceKind::Curseforge, &["Alpha", "Beta"]);
            let second = FakeSource::new(SourceKind::Github, &["Alpha"]);
            // Il record nomina la cartella con maiuscole diverse da quelle del disco.
            let records = record_of(SourceKind::Github, "owner/alpha", &["ALPHA"]);
            let updates = check_updates(&fx.installation, &[&first, &second], &records).unwrap();
            assert_eq!(
                sources_of(&updates),
                [
                    ("Alpha", Some(SourceKind::Github)),
                    ("Beta", Some(SourceKind::Curseforge)),
                ]
            );
            assert_eq!(first.offered(), ["Beta"]);
        }

        #[test]
        fn a_folder_recorded_by_two_sources_belongs_to_neither() {
            let fx = fixture(&[("Libs", "l")]);
            let first = FakeSource::new(SourceKind::Curseforge, &["Libs"]);
            let mut records = record_of(SourceKind::Github, "owner/a", &["Libs"]);
            records.extend(record_of(SourceKind::Tukui, "-2", &["Libs"]));
            let updates = check_updates(&fx.installation, &[&first], &records).unwrap();
            assert_eq!(
                sources_of(&updates),
                [("Libs", Some(SourceKind::Curseforge))]
            );
        }

        #[test]
        fn a_failing_source_does_not_stop_the_others() {
            let fx = fixture(&[("Alpha", "a"), ("Beta", "b")]);
            let failing = FakeSource::failing(SourceKind::Curseforge, "network down");
            let working = FakeSource::new(SourceKind::Tukui, &["Beta"]);
            let updates = check_updates(
                &fx.installation,
                &[&failing, &working],
                &SourceRecords::new(),
            )
            .unwrap();
            assert_eq!(updates[1].source, Some(SourceKind::Tukui));
            assert_eq!(updates[0].source, None);
            assert_eq!(
                updates[0].status,
                UpdateStatus::Error {
                    message: "CurseForge check failed: network down".to_owned()
                }
            );
        }

        /// Una fonte che segna ogni cartella offerta come illeggibile, come fa CurseForge con una
        /// cartella senza fingerprint.
        struct ErrorPerFolder;

        impl Source for ErrorPerFolder {
            fn kind(&self) -> SourceKind {
                SourceKind::Curseforge
            }

            fn check(
                &self,
                _installation: &Installation,
                offered: &[InstalledAddon],
                _records: &ProjectRecords,
            ) -> Result<Vec<AddonUpdate>, String> {
                Ok(offered
                    .iter()
                    .map(|a| {
                        unclaimed(
                            a,
                            UpdateStatus::Error {
                                message: "Unreadable folder".to_owned(),
                            },
                        )
                    })
                    .collect())
            }

            fn update(&self, target: &UpdateTarget<'_>, _: &mut RecordWriter<'_>) -> UpdateOutcome {
                UpdateOutcome::skipped(SourceKind::Curseforge, target.key, "", "unused")
            }
        }

        #[test]
        fn a_folder_error_does_not_hide_the_folder_from_later_sources() {
            let fx = fixture(&[("ElvUI", "e"), ("Other", "o")]);
            let tukui = FakeSource::new(SourceKind::Tukui, &["ElvUI"]);
            let updates = check_updates(
                &fx.installation,
                &[&ErrorPerFolder, &tukui],
                &SourceRecords::new(),
            )
            .unwrap();
            assert_eq!(tukui.offered(), ["ElvUI", "Other"]);
            assert_eq!(updates[0].source, Some(SourceKind::Tukui));
            // Nessuno riconosce `Other`: resta l'errore della prima fonte.
            assert_eq!(updates[1].source, None);
            assert_eq!(
                updates[1].status,
                UpdateStatus::Error {
                    message: "Unreadable folder".to_owned()
                }
            );
        }

        #[test]
        fn when_every_source_fails_the_check_fails() {
            let fx = fixture(&[("Alpha", "a")]);
            let first = FakeSource::failing(SourceKind::Curseforge, "first");
            let second = FakeSource::failing(SourceKind::Tukui, "second");
            assert_eq!(
                check_updates(&fx.installation, &[&first, &second], &SourceRecords::new()),
                Err("first".to_owned())
            );
        }

        /// CurseForge senza chiave, come nell'app senza `CURSEFORGE_API_KEY` (#123).
        fn keyless() -> CurseForgeClient<Routes> {
            CurseForgeClient::new(Routes::default(), None)
        }

        fn missing_key() -> String {
            CurseForgeError::MissingKey.to_string()
        }

        #[test]
        fn without_a_key_the_free_sources_still_check() {
            let fx = fixture(&[("Alpha", "a"), ("ElvUI", "e")]);
            let client = keyless();
            let overrides = no_overrides();
            let curseforge = CurseForgeSource {
                client: &client,
                overrides: &overrides,
                allow_beta: false,
                github: None,
            };
            let tukui = FakeSource::new(SourceKind::Tukui, &["ElvUI"]);
            let updates = check_updates(
                &fx.installation,
                &[&curseforge, &tukui],
                &SourceRecords::new(),
            )
            .unwrap();
            assert_eq!(updates[1].source, Some(SourceKind::Tukui));
            assert_eq!(updates[1].status, UpdateStatus::UpToDate);
            // La cartella che nessuna fonte libera riconosce dice perché.
            assert_eq!(updates[0].source, None);
            assert_eq!(
                updates[0].status,
                UpdateStatus::Error {
                    message: format!("CurseForge check failed: {}", missing_key())
                }
            );
        }

        #[test]
        fn without_a_key_free_projects_update_and_curseforge_ones_say_why() {
            let fx = fixture(&[("Alpha", "a"), ("ElvUI", "e")]);
            let client = keyless();
            let overrides = no_overrides();
            let curseforge = CurseForgeSource {
                client: &client,
                overrides: &overrides,
                allow_beta: false,
                github: None,
            };
            let tukui = FakeSource::new(SourceKind::Tukui, &["ElvUI"]);
            let mut written = Vec::new();
            let outcomes = update_addons(
                &fx.installation,
                &[&curseforge, &tukui],
                &SourceRecords::new(),
                &[
                    ProjectKey {
                        source: SourceKind::Curseforge,
                        key: "1".to_owned(),
                    },
                    ProjectKey {
                        source: SourceKind::Tukui,
                        key: "ElvUI-key".to_owned(),
                    },
                ],
                &mut |source, key, _| {
                    written.push((source, key.to_owned()));
                    Ok(())
                },
                &crate::progress::ignore,
            )
            .unwrap();
            assert_eq!(written, [(SourceKind::Tukui, "ElvUI-key".to_owned())]);
            assert_eq!(outcomes[0].status, OutcomeStatus::Error);
            assert_eq!(outcomes[0].message, Some(missing_key()));
            assert_eq!(outcomes[1].message.as_deref(), Some("fake"));
        }

        #[test]
        fn without_a_key_a_folder_recorded_for_curseforge_stays_with_curseforge() {
            // D33: il record dice da dove l'app ha installato la cartella, e vince anche
            // quando quella fonte non può controllare. Darla a Tukui farebbe aggiornare da
            // un'altra fonte un progetto installato da CurseForge.
            let fx = fixture(&[("ElvUI", "e")]);
            let client = keyless();
            let overrides = no_overrides();
            let curseforge = CurseForgeSource {
                client: &client,
                overrides: &overrides,
                allow_beta: false,
                github: None,
            };
            let tukui = FakeSource::new(SourceKind::Tukui, &["ElvUI"]);
            let records = record_of(SourceKind::Curseforge, "1", &["ElvUI"]);
            let updates =
                check_updates(&fx.installation, &[&curseforge, &tukui], &records).unwrap();
            assert!(tukui.offered().is_empty());
            assert_eq!(updates[0].source, None);
            assert_eq!(
                updates[0].status,
                UpdateStatus::Error {
                    message: format!("CurseForge check failed: {}", missing_key())
                }
            );
        }

        #[test]
        fn without_a_key_and_with_tukui_down_the_check_fails() {
            let fx = fixture(&[("Alpha", "a")]);
            let client = keyless();
            let overrides = no_overrides();
            let curseforge = CurseForgeSource {
                client: &client,
                overrides: &overrides,
                allow_beta: false,
                github: None,
            };
            let tukui = FakeSource::failing(SourceKind::Tukui, "network down");
            assert_eq!(
                check_updates(
                    &fx.installation,
                    &[&curseforge, &tukui],
                    &SourceRecords::new()
                ),
                Err(missing_key())
            );
        }

        #[test]
        fn updates_go_to_the_project_source() {
            let fx = fixture(&[("Alpha", "a")]);
            let tukui = FakeSource::new(SourceKind::Tukui, &["Alpha"]);
            let mut written = Vec::new();
            let outcomes = update_addons(
                &fx.installation,
                &[&tukui],
                &SourceRecords::new(),
                &[
                    ProjectKey {
                        source: SourceKind::Tukui,
                        key: "-2".to_owned(),
                    },
                    ProjectKey {
                        source: SourceKind::Tukui,
                        key: "-2".to_owned(),
                    },
                    ProjectKey {
                        source: SourceKind::Github,
                        key: "owner/repo".to_owned(),
                    },
                ],
                &mut |source, key, _| {
                    written.push((source, key.to_owned()));
                    Ok(())
                },
                &crate::progress::ignore,
            )
            .unwrap();
            // Il progetto ripetuto si aggiorna una volta; la fonte assente salta.
            assert_eq!(written, [(SourceKind::Tukui, "-2".to_owned())]);
            let summary: Vec<(SourceKind, &str, Option<&str>)> = outcomes
                .iter()
                .map(|o| (o.source, o.key.as_str(), o.message.as_deref()))
                .collect();
            assert_eq!(
                summary,
                [
                    (SourceKind::Tukui, "-2", Some("fake")),
                    (
                        SourceKind::Github,
                        "owner/repo",
                        Some("This source is not available.")
                    ),
                ]
            );
        }

        // ── Mirror GitHub dei progetti bloccati (D34, issue #163) ───────────

        mod mirror {
            use super::*;
            use crate::sources::github::GithubSource;
            use crate::sources::http::{ApiResponse, PublicTransport};

            const REPO: &str = "owner/Alpha";
            const SOURCE_URL: &str = "https://github.com/owner/Alpha";

            /// GitHub finto: le release per URL dell'API, gli zip per URL, e le richieste fatte.
            #[derive(Default)]
            struct Hub {
                /// URL dell'API → (stato, corpo, richieste rimaste).
                api: HashMap<String, (u16, String, u64)>,
                zips: HashMap<String, Vec<u8>>,
                calls: Arc<Mutex<Vec<String>>>,
            }

            impl PublicTransport for Hub {
                fn get(&self, url: &str) -> Result<HttpResponse, TransportError> {
                    self.calls.lock().unwrap().push(url.to_owned());
                    reply(None)
                }

                fn get_api(&self, url: &str, _accept: &str) -> Result<ApiResponse, TransportError> {
                    self.calls.lock().unwrap().push(url.to_owned());
                    let (status, body, remaining) =
                        self.api.get(url).cloned().ok_or(TransportError::Connect)?;
                    Ok(ApiResponse {
                        status,
                        body,
                        ratelimit_remaining: Some(remaining),
                        ratelimit_reset: Some(1_793_025_120),
                    })
                }

                fn get_bytes(
                    &self,
                    url: &str,
                    _max_len: u64,
                ) -> Result<BytesResponse, TransportError> {
                    self.calls.lock().unwrap().push(url.to_owned());
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

            fn latest_url() -> String {
                format!("https://api.github.com/repos/{REPO}/releases/latest")
            }

            fn asset_url(tag: &str) -> String {
                format!("https://github.com/{REPO}/releases/download/{tag}/Alpha-{tag}.zip")
            }

            /// Un GitHub con l'ultima release `tag` e il suo unico zip `zip`.
            fn hub(tag: &str, zip: Vec<u8>) -> Hub {
                let body = json!({"tag_name": tag, "assets": [
                    {"name": format!("Alpha-{tag}.zip"), "browser_download_url": asset_url(tag)}
                ]});
                Hub {
                    api: HashMap::from([(latest_url(), (200, body.to_string(), 59))]),
                    zips: HashMap::from([(asset_url(tag), zip)]),
                    ..Hub::default()
                }
            }

            /// Un progetto bloccato che dichiara il repository GitHub.
            fn blocked(mod_id: u32, name: &str) -> Value {
                json!({"id": mod_id, "name": name, "allowModDistribution": false,
                       "links": {"sourceUrl": SOURCE_URL}})
            }

            /// Il file CurseForge `id` senza link di download, come per i progetti bloccati.
            fn blocked_file(mod_id: u32, id: u32, date: &str, modules: &[(&str, u32)]) -> Value {
                let mut file = file_json(mod_id, id, date, modules);
                file["downloadUrl"] = Value::Null;
                file
            }

            /// Le risposte CurseForge di un progetto installato (file 10) con l'aggiornamento 11.
            fn routes(fx: &Fixture, project: Value) -> Routes {
                Routes {
                    fingerprints: json!({"data": {"exactMatches": [
                        installed(fx, 1, &["Alpha"], true)
                    ]}})
                    .to_string(),
                    mods: json!({ "data": [project] }).to_string(),
                    files: HashMap::from([(
                        format!("{API}/v1/mods/1/files/11"),
                        json!({"data": blocked_file(1, 11, "2026-10-01T00:00:00Z", &[])})
                            .to_string(),
                    )]),
                    ..Routes::default()
                }
            }

            fn source<'a>(
                client: &'a CurseForgeClient<Routes>,
                overrides: &'a VersionOverrides,
                github: &'a GithubSource<Hub>,
            ) -> CurseForgeSource<'a, Routes> {
                CurseForgeSource {
                    client,
                    overrides,
                    allow_beta: false,
                    github: Some(github),
                }
            }

            fn check(
                fx: &Fixture,
                client: &CurseForgeClient<Routes>,
                github: &GithubSource<Hub>,
            ) -> Vec<AddonUpdate> {
                let overrides = no_overrides();
                let cf = source(client, &overrides, github);
                check_updates(&fx.installation, &[&cf], &SourceRecords::new()).unwrap()
            }

            type Written = Vec<(SourceKind, String, SourceRecord)>;

            fn update(
                fx: &Fixture,
                client: &CurseForgeClient<Routes>,
                github: &GithubSource<Hub>,
                reinstall: bool,
                written: &mut Written,
            ) -> Vec<UpdateOutcome> {
                let overrides = no_overrides();
                let cf = source(client, &overrides, github);
                update_addons_with(
                    &fx.installation,
                    None,
                    &[&cf],
                    &SourceRecords::new(),
                    &[ProjectKey {
                        source: SourceKind::Curseforge,
                        key: "1".to_owned(),
                    }],
                    reinstall,
                    &mut |source, key, record| {
                        written.push((source, key.to_owned(), record));
                        Ok(())
                    },
                    &crate::progress::ignore,
                )
                .unwrap()
            }

            fn alpha_zip(content: &str) -> Vec<u8> {
                zip_of(&[
                    ("Alpha/Alpha.toc", "## Title: Alpha\nAlpha.lua\n"),
                    ("Alpha/Alpha.lua", content),
                ])
            }

            #[test]
            fn a_blocked_project_with_a_mirror_updates_from_github() {
                let fx = fixture(&[("Alpha", "alpha v1")]);
                let routes = routes(&fx, blocked(1, "Alpha"));
                let cf_log = Arc::clone(&routes.log);
                let client = client(routes);
                let hub = hub("11", alpha_zip("alpha v2"));
                let calls = Arc::clone(&hub.calls);
                let github = GithubSource::new(hub);

                let updates = check(&fx, &client, &github);
                assert_eq!(
                    updates[0].mirror,
                    MirrorStatus {
                        github_mirror: Some(github_mirror::GithubMirror {
                            repo: REPO.to_owned(),
                            tag: "11".to_owned(),
                        }),
                        github_mirror_reason: None,
                        pending: None,
                    }
                );
                let json = serde_json::to_value(&updates[0]).unwrap();
                assert_eq!(json["githubMirror"], json!({"repo": REPO, "tag": "11"}));
                assert_eq!(json["githubMirrorReason"], Value::Null);

                let mut written = Written::new();
                let outcomes = update(&fx, &client, &github, false, &mut written);
                assert_eq!(outcomes[0].status, OutcomeStatus::Updated, "{outcomes:?}");
                assert_eq!(outcomes[0].source, SourceKind::Curseforge);
                assert_eq!(outcomes[0].file.as_ref().map(|f| f.id), Some(11));
                assert_eq!(
                    outcomes[0].message.as_deref(),
                    Some(
                        "Installed from GitHub (owner/Alpha 11): CurseForge blocks third-party downloads."
                    )
                );
                // Il record resta CurseForge, con l'id del file che combacia.
                assert_eq!(
                    written,
                    [(
                        SourceKind::Curseforge,
                        "1".to_owned(),
                        SourceRecord {
                            file: "11".to_owned(),
                            folders: vec!["Alpha".to_owned()],
                        }
                    )]
                );
                assert_eq!(
                    fs::read_to_string(fx.addons.join("Alpha").join("Alpha.lua")).unwrap(),
                    "alpha v2"
                );
                // Niente dalla CDN di CurseForge; da GitHub una lettura della release (riusata
                // tra controllo e aggiornamento) e lo zip.
                assert!(cf_log.downloads.lock().unwrap().is_empty());
                let calls = calls.lock().unwrap().clone();
                assert_eq!(calls, [latest_url(), asset_url("11")]);
            }

            #[test]
            fn mirrors_are_resolved_after_the_github_source_checks() {
                // Un progetto GitHub registrato e un progetto CurseForge bloccato: la release del
                // primo si legge prima di quella del mirror, così il limite di 60 richieste l'ora
                // non lo consumano i mirror (finding della review su PR #173).
                let fx = fixture(&[("Alpha", "alpha v1"), ("Gamma", "gamma")]);
                let client = client(routes(&fx, blocked(1, "Alpha")));
                let mut hub = hub("11", alpha_zip("x"));
                let gamma = "https://api.github.com/repos/owner/Gamma/releases/latest";
                let body = json!({"tag_name": "2", "assets": []});
                hub.api
                    .insert(gamma.to_owned(), (200, body.to_string(), 58));
                let calls = Arc::clone(&hub.calls);
                let github = GithubSource::new(hub);
                let overrides = no_overrides();
                let cf = source(&client, &overrides, &github);
                let records = SourceRecords::from([(
                    SourceKind::Github,
                    ProjectRecords::from([(
                        "owner/Gamma".to_owned(),
                        SourceRecord {
                            file: "1".to_owned(),
                            folders: vec!["Gamma".to_owned()],
                        },
                    )]),
                )]);

                let updates = check_updates(&fx.installation, &[&cf, &github], &records).unwrap();
                assert_eq!(*calls.lock().unwrap(), [gamma.to_owned(), latest_url()]);
                let alpha = updates.iter().find(|u| u.folder == "Alpha").unwrap();
                assert_eq!(
                    alpha.mirror.github_mirror.as_ref().map(|m| m.tag.as_str()),
                    Some("11")
                );
                assert!(updates.iter().all(|u| u.mirror.pending.is_none()));
            }

            #[test]
            fn without_a_matching_release_the_update_is_skipped_with_the_reason() {
                let fx = fixture(&[("Alpha", "alpha v1")]);
                let client = client(routes(&fx, blocked(1, "Alpha")));
                let hub = hub("9", alpha_zip("old"));
                let calls = Arc::clone(&hub.calls);
                let github = GithubSource::new(hub);

                let updates = check(&fx, &client, &github);
                let reason =
                    "The latest GitHub release (9) is not the CurseForge file (file-11.zip).";
                assert_eq!(updates[0].mirror.github_mirror, None);
                assert_eq!(
                    updates[0].mirror.github_mirror_reason.as_deref(),
                    Some(reason)
                );
                assert!(matches!(
                    updates[0].status,
                    UpdateStatus::UpdateAvailable { .. }
                ));

                let mut written = Written::new();
                let outcomes = update(&fx, &client, &github, false, &mut written);
                assert_eq!(outcomes[0].status, OutcomeStatus::Skipped);
                let message = outcomes[0].message.as_deref().unwrap();
                assert!(message.contains("third-party"), "{message}");
                assert!(message.ends_with(reason), "{message}");
                assert!(written.is_empty());
                assert!(!calls.lock().unwrap().contains(&asset_url("9")));
                assert_eq!(
                    fs::read_to_string(fx.addons.join("Alpha").join("Alpha.lua")).unwrap(),
                    "alpha v1"
                );
            }

            #[test]
            fn projects_that_need_no_mirror_ask_github_nothing() {
                let fx = fixture(&[("Alpha", "alpha"), ("Beta", "beta")]);
                let (beta, _) = up_to_date(&fx, 2, "Beta", 1);
                let mut free = blocked(1, "Alpha");
                free["allowModDistribution"] = json!(true);
                let routes = Routes {
                    fingerprints: json!({"data": {"exactMatches": [
                        installed(&fx, 1, &["Alpha"], true),
                        beta,
                    ]}})
                    .to_string(),
                    mods: json!({"data": [free, blocked(2, "Beta")]}).to_string(),
                    ..Routes::default()
                };
                let client = client(routes);
                let hub = hub("11", alpha_zip("x"));
                let calls = Arc::clone(&hub.calls);
                let github = GithubSource::new(hub);

                let updates = check(&fx, &client, &github);
                assert_eq!(updates.len(), 2);
                for update in &updates {
                    assert_eq!(update.mirror, MirrorStatus::default(), "{update:?}");
                }
                assert!(calls.lock().unwrap().is_empty());
            }

            #[test]
            fn a_blocked_project_without_a_github_source_says_why() {
                let fx = fixture(&[("Alpha", "alpha v1")]);
                let mut project = blocked(1, "Alpha");
                project["links"] = json!({"sourceUrl": "https://repos.curseforge.com/wow/alpha"});
                let client = client(routes(&fx, project));
                let hub = Hub::default();
                let calls = Arc::clone(&hub.calls);
                let github = GithubSource::new(hub);

                let updates = check(&fx, &client, &github);
                assert_eq!(
                    updates[0].mirror.github_mirror_reason.as_deref(),
                    Some("The CurseForge project does not link a GitHub repository.")
                );
                let outcomes = update(&fx, &client, &github, false, &mut Written::new());
                assert_eq!(outcomes[0].status, OutcomeStatus::Skipped);
                assert!(calls.lock().unwrap().is_empty());
            }

            #[test]
            fn reinstall_of_a_blocked_project_uses_the_mirror() {
                let fx = fixture(&[("Alpha", "alpha v1")]);
                let (found, _) = up_to_date(&fx, 1, "Alpha", 1);
                let routes = Routes {
                    fingerprints: json!({"data": {"exactMatches": [found]}}).to_string(),
                    mods: json!({"data": [blocked(1, "Alpha")]}).to_string(),
                    files: HashMap::from([(
                        format!("{API}/v1/mods/1/files/10"),
                        json!({"data": blocked_file(1, 10, "2026-09-01T00:00:00Z", &[])})
                            .to_string(),
                    )]),
                    ..Routes::default()
                };
                let client = client(routes);
                let hub = hub("10", alpha_zip("alpha v1 again"));
                let calls = Arc::clone(&hub.calls);
                let github = GithubSource::new(hub);

                // Aggiornato: il controllo non chiede niente a GitHub.
                let updates = check(&fx, &client, &github);
                assert_eq!(updates[0].status, UpdateStatus::UpToDate);
                assert_eq!(updates[0].mirror, MirrorStatus::default());
                assert!(calls.lock().unwrap().is_empty());

                let mut written = Written::new();
                let outcomes = update(&fx, &client, &github, true, &mut written);
                assert_eq!(outcomes[0].status, OutcomeStatus::Updated, "{outcomes:?}");
                assert_eq!(
                    outcomes[0].message.as_deref(),
                    Some(
                        "Installed from GitHub (owner/Alpha 10): CurseForge blocks third-party downloads."
                    )
                );
                assert_eq!(written.len(), 1);
                assert_eq!(written[0].0, SourceKind::Curseforge);
                assert_eq!(written[0].2.file, "10");
                assert_eq!(
                    fs::read_to_string(fx.addons.join("Alpha").join("Alpha.lua")).unwrap(),
                    "alpha v1 again"
                );
                assert_eq!(*calls.lock().unwrap(), [latest_url(), asset_url("10")]);
            }

            #[test]
            fn the_rate_limit_is_a_reason_not_an_error() {
                let fx = fixture(&[("Alpha", "alpha v1")]);
                let client = client(routes(&fx, blocked(1, "Alpha")));
                let hub = Hub {
                    api: HashMap::from([(latest_url(), (403, String::new(), 0))]),
                    ..Hub::default()
                };
                let github = GithubSource::new(hub);

                let updates = check(&fx, &client, &github);
                assert!(matches!(
                    updates[0].status,
                    UpdateStatus::UpdateAvailable { .. }
                ));
                let limit = "GitHub's limit of 60 requests an hour has been reached: try again after 14:32 UTC.";
                assert_eq!(
                    updates[0].mirror.github_mirror_reason.as_deref(),
                    Some(limit)
                );

                let outcomes = update(&fx, &client, &github, false, &mut Written::new());
                assert_eq!(outcomes[0].status, OutcomeStatus::Skipped);
                assert!(
                    outcomes[0].message.as_deref().unwrap().ends_with(limit),
                    "{outcomes:?}"
                );
            }

            #[test]
            fn a_download_outside_github_is_refused() {
                let fx = fixture(&[("Alpha", "alpha v1")]);
                let client = client(routes(&fx, blocked(1, "Alpha")));
                let evil = "https://evil.example/Alpha-11.zip";
                let body = json!({"tag_name": "11", "assets": [
                    {"name": "Alpha-11.zip", "browser_download_url": evil}
                ]});
                let hub = Hub {
                    api: HashMap::from([(latest_url(), (200, body.to_string(), 59))]),
                    zips: HashMap::from([(evil.to_owned(), alpha_zip("evil"))]),
                    ..Hub::default()
                };
                let calls = Arc::clone(&hub.calls);
                let github = GithubSource::new(hub);

                let mut written = Written::new();
                let outcomes = update(&fx, &client, &github, false, &mut written);
                assert_eq!(outcomes[0].status, OutcomeStatus::Error, "{outcomes:?}");
                assert!(written.is_empty());
                assert!(!calls.lock().unwrap().iter().any(|u| u == evil));
                assert_eq!(
                    fs::read_to_string(fx.addons.join("Alpha").join("Alpha.lua")).unwrap(),
                    "alpha v1"
                );
            }
        }
    }
}
