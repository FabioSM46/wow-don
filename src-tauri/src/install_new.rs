//! Installazione di un addon nuovo dal catalogo (`install_addon`, issue #70), con le sue
//! dipendenze obbligatorie (issue #71).
//!
//! Segue il flusso di `updates::update_addons` e ne riusa `install_file`: download, estrazione
//! sicura in staging, sostituzione con ripristino. In più, prima di toccare il disco:
//! - un progetto già presente (stessa regola del flag `installed` della ricerca,
//!   [`catalog::installed_projects`]) si salta senza nessuna chiamata di rete;
//! - una cartella dello zip che esiste già e appartiene a un altro progetto (nei record di
//!   `installed-addons.json` o, senza record, per il `## X-Curse-Project-ID` del suo `.toc`)
//!   fa fallire l'installazione; una cartella esistente senza record né id di progetto
//!   (installata a mano) si sostituisce.
//!
//! Come in WowUp, dopo un'installazione riuscita si installano le dipendenze del file con
//! `relationType = 3` (RequiredDependency), e ricorsivamente le loro, con le stesse regole.
//!
//! Un progetto che CurseForge non lascia scaricare (D13) si installa dal suo mirror GitHub
//! (D34, issue #164) quando la release è la stessa del file CurseForge scelto: si scarica lo zip
//! di GitHub con le stesse regole e lo stesso controllo delle cartelle, e il record resta
//! **CurseForge**, con l'id del file che combacia. [`github_mirror`] dà lo stesso esito su
//! richiesta, senza installare: la ricerca non interroga GitHub.
//!
//! Bloccante: va chiamato fuori dal thread principale.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::Path;

use serde::Serialize;

use crate::catalog;
use crate::installations::Installation;
use crate::progress::{Phase, ProgressSink, Reporter};
use crate::sources::SourceKind;
use crate::sources::curseforge::{
    CurseForgeClient, File, Mod, RELATION_REQUIRED_DEPENDENCY, RELEASE_TYPE_BETA, Transport,
    pick_file,
};
use crate::sources::github::Asset;
use crate::sources::github_mirror::{self, MirrorSource, Outcome, Reason};
use crate::store::{InstalledRecord, InstalledRecords};
use crate::updates::{self, FileSummary, VersionOverrides};

/// Quanti livelli di dipendenze si seguono: il progetto scelto è il livello 0, le sue
/// dipendenze il livello 1. Le dipendenze più profonde non si installano e l'esito lo dice.
pub const MAX_DEPENDENCY_DEPTH: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum InstallStatus {
    Installed,
    Skipped,
    Error,
}

/// L'esito dell'installazione di un progetto, con la stessa forma di `UpdateOutcome` più
/// `dependencyOf` per le dipendenze.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallOutcome {
    pub mod_id: u32,
    /// Il nome del progetto; vuoto se non è stato letto da CurseForge (per esempio quando il
    /// progetto è già installato e non serve chiederlo).
    pub name: String,
    pub status: InstallStatus,
    /// Perché è stato saltato, cosa è andato storto o un avviso su un'installazione riuscita.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Il file installato, solo per `installed`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<FileSummary>,
    /// Per una dipendenza, l'id del progetto che la richiede; assente per il progetto scelto.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dependency_of: Option<u32>,
}

impl InstallOutcome {
    fn skipped(mod_id: u32, name: &str, message: &str) -> Self {
        Self {
            mod_id,
            name: name.to_owned(),
            status: InstallStatus::Skipped,
            message: Some(message.to_owned()),
            file: None,
            dependency_of: None,
        }
    }

    fn error(mod_id: u32, name: &str, message: String) -> Self {
        Self {
            mod_id,
            name: name.to_owned(),
            status: InstallStatus::Error,
            message: Some(message),
            file: None,
            dependency_of: None,
        }
    }
}

/// Installa il progetto `mod_id` nell'installazione: il file `file_id` se indicato (purché sia
/// di quel progetto), altrimenti l'ultimo file Release per la versione del gioco forzata per il
/// progetto (`overrides`) o per quella dell'installazione. `records` sono i record di
/// `installed-addons.json` dell'installazione; `record` salva quello appena installato.
///
/// Se il progetto si installa, si installano anche le sue dipendenze obbligatorie, un livello
/// alla volta fino a [`MAX_DEPENDENCY_DEPTH`] livelli: ognuna con l'ultimo Release per la
/// stessa versione del gioco del progetto scelto e con le stesse regole, ogni progetto al
/// massimo una volta (cicli e dipendenze condivise). Le dipendenze di un progetto già
/// installato o non installato non si seguono. Per ogni livello i progetti si leggono con una
/// sola `get_mods`, che dà il nome anche a quelli già installati o troppo profondi. Dopo ogni
/// installazione i record locali e l'insieme dei progetti installati includono quella appena
/// fatta, così una dipendenza appena installata conta come installata e le sue cartelle come
/// registrate.
///
/// Un progetto bloccato da CurseForge (anche una dipendenza) si installa dal mirror GitHub con
/// `github` (D34); senza `github`, o senza un mirror utilizzabile, resta saltato con il motivo.
///
/// Restituisce l'esito del progetto scelto seguito da uno per ogni dipendenza visitata, mai
/// `Err`: un errore su una dipendenza non tocca il progetto scelto né le altre dipendenze.
#[allow(clippy::too_many_arguments)]
pub fn install_addon<T: Transport>(
    installation: &Installation,
    client: &CurseForgeClient<T>,
    github: Option<&dyn MirrorSource>,
    overrides: &VersionOverrides,
    records: &InstalledRecords,
    mod_id: u32,
    file_id: Option<u32>,
    allow_beta: bool,
    record: &mut dyn FnMut(u32, InstalledRecord) -> Result<(), String>,
    progress: &ProgressSink<'_>,
) -> Vec<InstallOutcome> {
    // Un'installazione aggiunge progetti e non ne toglie: la cartella di un altro progetto non
    // si sostituisce, quindi basta leggere il disco una volta e aggiungere quelli installati.
    let installed = catalog::installed_projects(installation, records);
    if installed.contains(&mod_id) {
        return vec![InstallOutcome::skipped(mod_id, "", "Already installed.")];
    }
    // La versione del gioco del progetto scelto vale anche per le sue dipendenze.
    let version_type = overrides
        .get(&mod_id)
        .copied()
        .or_else(|| updates::version_type_for(&installation.flavor));
    let mut installer = Installer {
        installation,
        client,
        github,
        version_type,
        allow_beta,
        records: records.clone(),
        installed,
        record,
        progress,
    };

    let key = mod_id.to_string();
    Reporter::new(progress, Some(SourceKind::Curseforge), &key, "").phase(Phase::Checking);
    let project = match client.get_mods(&[mod_id]) {
        Ok(mods) => mods.into_iter().find(|m| m.id == mod_id),
        Err(error) => return vec![InstallOutcome::error(mod_id, "", error.to_string())],
    };
    let Some(project) = project else {
        return vec![InstallOutcome::error(
            mod_id,
            "",
            "Addon not found on CurseForge.".to_owned(),
        )];
    };
    let (outcome, dependencies) = installer.install(&project, file_id);
    let mut outcomes = vec![outcome];

    let mut visited = HashSet::from([mod_id]);
    // (dipendenza, progetto che la richiede) del livello corrente.
    let mut level = Vec::new();
    enqueue(&mut level, &mut visited, &dependencies, mod_id);
    let mut depth = 1;
    while !level.is_empty() {
        let ids: Vec<u32> = level.iter().map(|(dependency, _)| *dependency).collect();
        let projects: Result<HashMap<u32, Mod>, String> = client
            .get_mods(&ids)
            .map(|mods| mods.into_iter().map(|m| (m.id, m)).collect())
            .map_err(|error| error.to_string());
        let mut next = Vec::new();
        for (dependency, parent) in level {
            let project = projects.as_ref().ok().and_then(|p| p.get(&dependency));
            let name = project.map_or("", |p| p.name.as_str());
            let (mut outcome, dependencies) = if installer.installed.contains(&dependency) {
                (
                    InstallOutcome::skipped(dependency, name, "Already installed."),
                    Vec::new(),
                )
            } else if depth > MAX_DEPENDENCY_DEPTH {
                let message = format!(
                    "Not installed: it is more than {MAX_DEPENDENCY_DEPTH} levels deep in the dependency chain. Install it on its own."
                );
                (
                    InstallOutcome::skipped(dependency, name, &message),
                    Vec::new(),
                )
            } else {
                match (&projects, project) {
                    (Err(error), _) => (
                        InstallOutcome::error(dependency, "", error.clone()),
                        Vec::new(),
                    ),
                    (Ok(_), None) => (
                        InstallOutcome::error(
                            dependency,
                            "",
                            "Addon not found on CurseForge.".to_owned(),
                        ),
                        Vec::new(),
                    ),
                    (Ok(_), Some(project)) => installer.install(project, None),
                }
            };
            outcome.dependency_of = Some(parent);
            outcomes.push(outcome);
            enqueue(&mut next, &mut visited, &dependencies, dependency);
        }
        level = next;
        depth += 1;
    }
    outcomes
}

/// Aggiunge al livello le dipendenze non ancora visitate, una volta sola ciascuna.
fn enqueue(
    level: &mut Vec<(u32, u32)>,
    visited: &mut HashSet<u32>,
    dependencies: &[u32],
    parent: u32,
) {
    for &dependency in dependencies {
        if visited.insert(dependency) {
            level.push((dependency, parent));
        }
    }
}

/// Lo stato condiviso dalle installazioni di una chiamata a [`install_addon`].
struct Installer<'a, T: Transport> {
    installation: &'a Installation,
    client: &'a CurseForgeClient<T>,
    /// Il mirror GitHub dei progetti bloccati (D34); `None` li salta come prima.
    github: Option<&'a dyn MirrorSource>,
    /// La versione del gioco del progetto scelto, per i progetti senza `file_id`.
    version_type: Option<u32>,
    /// Una Beta quando la versione non ha una Release (issue #141), anche per le dipendenze.
    allow_beta: bool,
    /// I record di partenza più quelli delle installazioni già fatte.
    records: InstalledRecords,
    /// I progetti installati, aggiornato dopo ogni installazione riuscita.
    installed: HashSet<u32>,
    record: &'a mut dyn FnMut(u32, InstalledRecord) -> Result<(), String>,
    /// Dove riportare l'avanzamento di ogni progetto installato (issue #127).
    progress: &'a ProgressSink<'a>,
}

impl<T: Transport> Installer<'_, T> {
    /// Installa un progetto letto da CurseForge e non ancora installato. Restituisce l'esito
    /// e, se è stato installato, le dipendenze obbligatorie del file.
    fn install(&mut self, project: &Mod, file_id: Option<u32>) -> (InstallOutcome, Vec<u32>) {
        let key = project.id.to_string();
        let reporter = Reporter::new(
            self.progress,
            Some(SourceKind::Curseforge),
            &key,
            &project.name,
        );
        reporter.phase(Phase::Checking);
        let (file, download) = match find_file(
            self.installation,
            self.client,
            self.github,
            project,
            self.version_type,
            self.allow_beta,
            file_id,
        ) {
            Ok(found) => found,
            Err(outcome) => return (outcome, Vec::new()),
        };
        let mut result = install_checked(
            self.installation,
            self.client,
            &mut self.records,
            &file,
            &download,
            &project.name,
            self.record,
            &reporter,
        );
        if result.0.status == InstallStatus::Installed {
            self.installed.insert(project.id);
            // Una Beta scelta dall'app, non dall'utente nel dettaglio: l'esito lo dice.
            if file_id.is_none() && file.release_type == RELEASE_TYPE_BETA {
                result.0.message.get_or_insert_with(|| {
                    "Installed the beta: there is no release for this game version.".to_owned()
                });
            }
        }
        result
    }
}

/// Da dove si scarica il file scelto.
enum Download<'a> {
    /// Il `downloadUrl` del file CurseForge.
    Curseforge,
    /// Lo zip della release GitHub che combacia con il file CurseForge (D34).
    Mirror {
        github: &'a dyn MirrorSource,
        repo: String,
        tag: String,
        asset: Asset,
    },
}

/// Il messaggio di un progetto bloccato senza mirror GitHub da chiedere.
const BLOCKED: &str = "The author does not allow downloads from third-party apps: install it from the CurseForge website.";

/// Il file da installare per il progetto, già controllato: file del progetto, e scaricabile da
/// CurseForge (distribuzione permessa e un `downloadUrl`) o dal mirror GitHub per un progetto
/// bloccato. Altrimenti l'esito che spiega perché no.
fn find_file<'a, T: Transport>(
    installation: &Installation,
    client: &CurseForgeClient<T>,
    github: Option<&'a dyn MirrorSource>,
    project: &Mod,
    version_type: Option<u32>,
    allow_beta: bool,
    file_id: Option<u32>,
) -> Result<(File, Download<'a>), InstallOutcome> {
    let mod_id = project.id;
    let name = project.name.as_str();
    // D13: sconosciuto vale `false`.
    let blocked = project.allow_mod_distribution != Some(true);
    let mirror = match (blocked, github) {
        (false, _) => None,
        (true, Some(github)) => Some(github),
        (true, None) => return Err(InstallOutcome::skipped(mod_id, name, BLOCKED)),
    };
    let file = chosen_file(
        installation,
        client,
        project,
        version_type,
        allow_beta,
        file_id,
    )
    .map_err(|error| match error {
        FileError::Skipped(message) => InstallOutcome::skipped(mod_id, name, &message),
        FileError::Unsupported(message) | FileError::Error(message) => {
            InstallOutcome::error(mod_id, name, message)
        }
    })?;
    if let Some(github) = mirror {
        return match github.resolve(project, &file, &installation.flavor) {
            Outcome::Mirror { repo, tag, asset } => Ok((
                file,
                Download::Mirror {
                    github,
                    repo,
                    tag,
                    asset,
                },
            )),
            Outcome::Unavailable { reason } => Err(unavailable(mod_id, name, &reason)),
        };
    }
    if file.download_url.is_none() {
        return Err(InstallOutcome::skipped(
            mod_id,
            name,
            "CurseForge does not provide a download link for this file: install it from the CurseForge website.",
        ));
    }
    Ok((file, Download::Curseforge))
}

/// L'esito di un progetto bloccato senza un mirror utilizzabile: saltato con il motivo, oppure
/// un errore quando GitHub non ha risposto (come negli aggiornamenti).
fn unavailable(mod_id: u32, name: &str, reason: &Reason) -> InstallOutcome {
    let message = github_mirror::unavailable_message(reason);
    match reason {
        Reason::Failed(_) => InstallOutcome::error(mod_id, name, message),
        _ => InstallOutcome::skipped(mod_id, name, &message),
    }
}

/// Perché non c'è un file da installare: un esito `skipped` o un errore.
enum FileError {
    Skipped(String),
    /// La versione del gioco dell'installazione non è supportata: un errore dell'installazione,
    /// un motivo per il controllo del mirror.
    Unsupported(String),
    Error(String),
}

/// Il file CurseForge `file_id` del progetto se indicato, altrimenti l'ultimo per la versione del
/// gioco (con la regola della Beta), letto da CurseForge e controllato come file del progetto.
/// Non guarda se il file si può scaricare.
fn chosen_file<T: Transport>(
    installation: &Installation,
    client: &CurseForgeClient<T>,
    project: &Mod,
    version_type: Option<u32>,
    allow_beta: bool,
    file_id: Option<u32>,
) -> Result<File, FileError> {
    let mod_id = project.id;
    let file_id = match file_id {
        Some(id) => id,
        None => {
            let Some(version_type) = version_type else {
                return Err(FileError::Unsupported(format!(
                    "Installing addons does not support the game version \"{}\" yet.",
                    installation.name
                )));
            };
            latest_file(project, version_type, allow_beta).ok_or_else(|| {
                FileError::Skipped("There is no release file for this game version.".to_owned())
            })?
        }
    };
    let file = client
        .get_file(mod_id, file_id)
        .map_err(|error| FileError::Error(error.to_string()))?;
    if file.mod_id != mod_id {
        return Err(FileError::Error(
            "The requested file does not belong to this addon.".to_owned(),
        ));
    }
    Ok(file)
}

/// L'esito del mirror GitHub di un progetto CurseForge (comando `github_mirror`, issue #164).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MirrorAvailability {
    /// Il progetto si installa dallo zip di GitHub.
    pub available: bool,
    /// `owner/repo`, solo con `available`.
    pub repo: Option<String>,
    /// Il tag della release, solo con `available`.
    pub tag: Option<String>,
    /// Perché il mirror non si usa (D27, in inglese); `null` per un progetto che CurseForge
    /// lascia scaricare, e quando il mirror c'è.
    pub reason: Option<String>,
}

impl MirrorAvailability {
    fn unavailable(reason: Option<String>) -> Self {
        Self {
            available: false,
            repo: None,
            tag: None,
            reason,
        }
    }
}

/// Il mirror GitHub del progetto `mod_id` per l'installazione, con lo stesso file e la stessa
/// regola di [`install_addon`] con lo stesso `file_id` (il file indicato, oppure l'ultimo per la
/// versione del gioco): si chiama su richiesta (dettaglio o Install), mai durante la ricerca, per
/// stare nel limite di 60 richieste l'ora. Un progetto che CurseForge lascia scaricare non fa
/// nessuna richiesta a GitHub. `Err` solo quando CurseForge non risponde, non conosce il
/// progetto o dà un file di un altro progetto; una versione del gioco non supportata, un file
/// mancante e i problemi di GitHub sono un `reason`.
pub fn github_mirror<T: Transport>(
    installation: &Installation,
    client: &CurseForgeClient<T>,
    github: &dyn MirrorSource,
    overrides: &VersionOverrides,
    mod_id: u32,
    file_id: Option<u32>,
    allow_beta: bool,
) -> Result<MirrorAvailability, String> {
    let project = client
        .get_mods(&[mod_id])
        .map_err(|error| error.to_string())?
        .into_iter()
        .find(|m| m.id == mod_id)
        .ok_or_else(|| "Addon not found on CurseForge.".to_owned())?;
    if project.allow_mod_distribution == Some(true) {
        return Ok(MirrorAvailability::unavailable(None));
    }
    let version_type = overrides
        .get(&mod_id)
        .copied()
        .or_else(|| updates::version_type_for(&installation.flavor));
    let file = match chosen_file(
        installation,
        client,
        &project,
        version_type,
        allow_beta,
        file_id,
    ) {
        Ok(file) => file,
        Err(FileError::Skipped(message) | FileError::Unsupported(message)) => {
            return Ok(MirrorAvailability::unavailable(Some(message)));
        }
        Err(FileError::Error(message)) => return Err(message),
    };
    Ok(
        match github.resolve(&project, &file, &installation.flavor) {
            Outcome::Mirror { repo, tag, .. } => MirrorAvailability {
                available: true,
                repo: Some(repo),
                tag: Some(tag),
                reason: None,
            },
            Outcome::Unavailable { reason } => {
                MirrorAvailability::unavailable(Some(reason.message()))
            }
        },
    )
}

/// Il file da installare per la versione tra quelli che CurseForge indica come ultimi del
/// progetto, con la regola di [`pick_file`] (una Beta solo con `allow_beta`). L'indice non ha la
/// data: decide l'id, che CurseForge assegna in ordine crescente (come l'ultimo file della
/// ricerca).
fn latest_file(project: &Mod, version_type: u32, allow_beta: bool) -> Option<u32> {
    let for_version = project
        .latest_files_indexes
        .iter()
        .filter(|index| index.game_version_type_id == Some(version_type));
    pick_file(
        for_version,
        |index| index.release_type,
        |index| index.file_id,
        allow_beta,
    )
    .map(|index| index.file_id)
}

/// Scarica e installa `file` (da CurseForge o, per un progetto bloccato, lo zip del suo mirror
/// GitHub), rifiutando le cartelle di altri progetti, poi lo registra sia
/// con `record` sia nei `records` locali (anche se `record` fallisce: le cartelle sono sul
/// disco e appartengono al progetto). Su un'installazione riuscita restituisce anche le
/// dipendenze obbligatorie del file.
#[allow(clippy::too_many_arguments)]
fn install_checked<T: Transport>(
    installation: &Installation,
    client: &CurseForgeClient<T>,
    records: &mut InstalledRecords,
    file: &File,
    download: &Download<'_>,
    name: &str,
    record: &mut dyn FnMut(u32, InstalledRecord) -> Result<(), String>,
    reporter: &Reporter<'_>,
) -> (InstallOutcome, Vec<u32>) {
    let mod_id = file.mod_id;
    let addons_dir = installation.path.join("Interface").join("AddOns");
    let flavor = installation.flavor.as_str();
    let check =
        |folders: &[String]| check_foreign_folders(&addons_dir, flavor, records, mod_id, folders);
    let (installed, note) = match download {
        Download::Curseforge => (
            updates::install_file(client, file, &addons_dir, &[], &check, reporter),
            None,
        ),
        Download::Mirror {
            github,
            repo,
            tag,
            asset,
        } => (
            github_mirror::install(*github, asset, &addons_dir, &[], &check, reporter),
            Some(github_mirror::installed_message(repo, tag)),
        ),
    };
    let folders = match installed {
        Ok(folders) => folders,
        Err(message) => return (InstallOutcome::error(mod_id, name, message), Vec::new()),
    };
    let installed = InstalledRecord {
        file_id: file.id,
        folders,
    };
    // Le cartelle sono già al loro posto: se il registro non si salva l'installazione resta
    // riuscita, e il messaggio lo dice (come in `update_addons`).
    let failed = record(mod_id, installed.clone())
        .err()
        .map(|error| format!("Installed, but the app could not remember its folders: {error}"));
    // Il record resta CurseForge anche dal mirror: l'esito dice che lo zip viene da GitHub.
    let message = match (note, failed) {
        (Some(note), Some(failed)) => Some(format!("{note} {failed}")),
        (note, failed) => note.or(failed),
    };
    records.insert(mod_id, installed);
    let dependencies = file
        .dependencies
        .iter()
        .filter(|dependency| dependency.relation_type == RELATION_REQUIRED_DEPENDENCY)
        .map(|dependency| dependency.mod_id)
        .collect();
    (
        InstallOutcome {
            mod_id,
            name: name.to_owned(),
            status: InstallStatus::Installed,
            message,
            file: Some(FileSummary::from(file)),
            dependency_of: None,
        },
        dependencies,
    )
}

/// Rifiuta lo zip se una delle sue cartelle esiste già in `AddOns` e appartiene a un altro
/// progetto: lo dice un record, oppure, per una cartella senza record, il
/// `## X-Curse-Project-ID` del suo `.toc` per il flavor (la stessa regola di
/// [`catalog::installed_projects`]). Una cartella senza record né id si sostituisce. Nomi
/// confrontati senza distinguere maiuscole e minuscole, come fa il gioco. Una cartella `AddOns`
/// che non esiste vale come vuota; una illeggibile fa fallire l'installazione, perché non si
/// potrebbe sapere cosa si sostituisce.
fn check_foreign_folders(
    addons_dir: &Path,
    flavor: &str,
    records: &InstalledRecords,
    mod_id: u32,
    folders: &[String],
) -> Result<(), String> {
    let on_disk = folder_names(addons_dir)?;
    for folder in folders {
        let lower = folder.to_lowercase();
        let Some(disk_name) = on_disk.get(&lower) else {
            continue;
        };
        let recorded: Vec<u32> = records
            .iter()
            .filter(|(_, record)| record.folders.iter().any(|f| f.to_lowercase() == lower))
            .map(|(id, _)| *id)
            .collect();
        // Il `.toc` conta solo per le cartelle che nessun record conosce, come nella ricerca.
        let owner = if recorded.is_empty() {
            catalog::declared_project(&addons_dir.join(disk_name), disk_name, flavor)
                .filter(|id| *id != mod_id)
        } else {
            recorded.into_iter().find(|id| *id != mod_id)
        };
        if let Some(owner) = owner {
            return Err(format!(
                "The folder \"{folder}\" belongs to another installed addon (CurseForge project {owner}): nothing was changed."
            ));
        }
    }
    Ok(())
}

/// Le cartelle in `AddOns`: nome in minuscolo → nome sul disco.
pub(crate) fn folder_names(addons_dir: &Path) -> Result<HashMap<String, String>, String> {
    let entries = match fs::read_dir(addons_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(HashMap::new()),
        Err(error) => {
            return Err(format!(
                "Could not read the AddOns folder ({}).",
                error.kind()
            ));
        }
    };
    let mut names = HashMap::new();
    for entry in entries {
        let entry = entry
            .map_err(|error| format!("Could not read the AddOns folder ({}).", error.kind()))?;
        if entry.path().is_dir() {
            let name = entry.file_name().to_string_lossy().into_owned();
            names.insert(name.to_lowercase(), name);
        }
    }
    Ok(names)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::io::{Cursor, Write};
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    use serde_json::{Value, json};
    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;

    use super::*;
    use crate::progress;
    use crate::sources::curseforge::{ApiKey, BytesResponse, HttpResponse, TransportError};
    use crate::test_support::TempDir;
    use crate::updates::{CLASSIC_ERA, RETAIL};

    const API: &str = "https://api.curseforge.com";

    /// Trasporto finto che risponde per URL e registra le richieste.
    #[derive(Default)]
    struct Routes {
        /// I progetti di `POST /v1/mods`: la risposta contiene quelli chiesti nel corpo.
        projects: Vec<Value>,
        files: HashMap<String, String>,
        zips: HashMap<String, Vec<u8>>,
        /// Gli indirizzi per cui `get` fallisce come senza rete.
        broken: Vec<String>,
        /// Condiviso con il test, che non può rileggere il trasporto dal client.
        log: Arc<Log>,
    }

    #[derive(Default)]
    struct Log {
        requests: Mutex<Vec<String>>,
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
            self.log.requests.lock().unwrap().push(url.to_owned());
            if self.broken.iter().any(|broken| broken == url) {
                return Err(TransportError::Connect);
            }
            reply(self.files.get(url))
        }

        fn post_json(
            &self,
            url: &str,
            _key: &ApiKey,
            body: String,
        ) -> Result<HttpResponse, TransportError> {
            self.log.requests.lock().unwrap().push(url.to_owned());
            if url != format!("{API}/v1/mods") {
                return reply(None);
            }
            let ids: Value = serde_json::from_str::<Value>(&body).unwrap()["modIds"].clone();
            let ids: Vec<u64> = ids
                .as_array()
                .unwrap()
                .iter()
                .map(|id| id.as_u64().unwrap())
                .collect();
            let data: Vec<&Value> = self
                .projects
                .iter()
                .filter(|p| ids.contains(&p["id"].as_u64().unwrap()))
                .collect();
            reply(Some(&json!({ "data": data }).to_string()))
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

    fn download_url(id: u32) -> String {
        format!("https://edge.forgecdn.net/files/{id}/file-{id}.zip")
    }

    fn file_json(mod_id: u32, id: u32, with_url: bool) -> Value {
        let mut file = json!({
            "id": id, "modId": mod_id, "displayName": format!("File {id}"),
            "fileName": format!("file-{id}.zip"), "fileDate": "2026-10-01T00:00:00Z",
            "releaseType": 1,
        });
        if with_url {
            file["downloadUrl"] = json!(download_url(id));
        }
        file
    }

    /// Un progetto con un file Release per Retail (`id·10`) e uno per Classic Era (`id·10+1`),
    /// più una beta Retail più recente che non va scelta.
    fn project(mod_id: u32, allow: Option<bool>) -> Value {
        let mut project = json!({
            "id": mod_id, "name": format!("Project {mod_id}"),
            "latestFilesIndexes": [
                {"fileId": mod_id * 10, "releaseType": 1, "gameVersionTypeId": RETAIL},
                {"fileId": mod_id * 10 + 1, "releaseType": 1, "gameVersionTypeId": CLASSIC_ERA},
                {"fileId": mod_id * 10 + 2, "releaseType": 2, "gameVersionTypeId": RETAIL},
            ],
        });
        if let Some(allow) = allow {
            project["allowModDistribution"] = json!(allow);
        }
        project
    }

    struct Fixture {
        _tmp: TempDir,
        installation: Installation,
        addons: PathBuf,
    }

    fn fixture() -> Fixture {
        let tmp = TempDir::new();
        let path = tmp.path().join("World of Warcraft").join("_retail_");
        let addons = path.join("Interface").join("AddOns");
        fs::create_dir_all(&addons).unwrap();
        Fixture {
            _tmp: tmp,
            installation: Installation {
                id: "test".to_owned(),
                path,
                flavor: "retail".to_owned(),
                name: "Retail".to_owned(),
            },
            addons,
        }
    }

    fn write_folder(addons: &Path, folder: &str, toc: &str) {
        let dir = addons.join(folder);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(format!("{folder}.toc")), toc).unwrap();
        fs::write(dir.join(format!("{folder}.lua")), "old").unwrap();
    }

    /// Le rotte di un progetto installabile: file `file_id` con lo zip `zip`.
    fn routes(mod_id: u32, file_id: u32, zip: &[(&str, &str)]) -> Routes {
        Routes {
            projects: vec![project(mod_id, Some(true))],
            files: HashMap::from([(
                format!("{API}/v1/mods/{mod_id}/files/{file_id}"),
                json!({"data": file_json(mod_id, file_id, true)}).to_string(),
            )]),
            zips: HashMap::from([(download_url(file_id), zip_of(zip))]),
            ..Routes::default()
        }
    }

    /// Le rotte di più progetti installabili, con le loro dipendenze `(id, relationType)`.
    /// Ogni progetto ha i file Retail e Classic Era di [`project`], e lo zip di entrambi
    /// contiene la cartella `P<id>`.
    fn graph(nodes: &[(u32, &[(u32, u8)])]) -> Routes {
        let mut routes = Routes::default();
        for (mod_id, dependencies) in nodes {
            routes.projects.push(project(*mod_id, Some(true)));
            let dependencies: Vec<Value> = dependencies
                .iter()
                .map(|(id, relation)| json!({"modId": id, "relationType": relation}))
                .collect();
            for file_id in [mod_id * 10, mod_id * 10 + 1] {
                let mut file = file_json(*mod_id, file_id, true);
                file["dependencies"] = json!(dependencies);
                routes.files.insert(
                    format!("{API}/v1/mods/{mod_id}/files/{file_id}"),
                    json!({ "data": file }).to_string(),
                );
                let folder = format!("P{mod_id}");
                routes.zips.insert(
                    download_url(file_id),
                    zip_of(&[(&format!("{folder}/{folder}.toc"), "")]),
                );
            }
        }
        routes
    }

    /// `(modId, status, dependencyOf)` di ogni esito, nell'ordine.
    fn summary(outcomes: &[InstallOutcome]) -> Vec<(u32, InstallStatus, Option<u32>)> {
        outcomes
            .iter()
            .map(|o| (o.mod_id, o.status, o.dependency_of))
            .collect()
    }

    fn client(routes: Routes) -> CurseForgeClient<Routes> {
        CurseForgeClient::new(routes, Some("test-key".to_owned()))
    }

    fn record(file_id: u32, folders: &[&str]) -> InstalledRecord {
        InstalledRecord {
            file_id,
            folders: folders.iter().map(|f| (*f).to_owned()).collect(),
        }
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }

    /// Installa senza versioni forzate un progetto senza dipendenze, raccogliendo i record
    /// scritti.
    fn install(
        fx: &Fixture,
        routes: Routes,
        records: &InstalledRecords,
        mod_id: u32,
        file_id: Option<u32>,
    ) -> (InstallOutcome, Vec<(u32, InstalledRecord)>) {
        let (mut outcomes, written) = install_all(fx, routes, records, mod_id, file_id);
        assert_eq!(outcomes.len(), 1, "{outcomes:?}");
        (outcomes.remove(0), written)
    }

    /// Installa senza versioni forzate, con le dipendenze, raccogliendo i record scritti.
    fn install_all(
        fx: &Fixture,
        routes: Routes,
        records: &InstalledRecords,
        mod_id: u32,
        file_id: Option<u32>,
    ) -> (Vec<InstallOutcome>, Vec<(u32, InstalledRecord)>) {
        let mut written = Vec::new();
        let outcomes = install_addon(
            &fx.installation,
            &client(routes),
            None,
            &VersionOverrides::new(),
            records,
            mod_id,
            file_id,
            false,
            &mut |id, r| {
                written.push((id, r));
                Ok(())
            },
            &progress::ignore,
        );
        (outcomes, written)
    }

    #[test]
    fn installs_the_latest_release_and_records_it() {
        let fx = fixture();
        let routes = routes(
            1,
            10,
            &[
                ("Alpha/Alpha.toc", "## Title: Alpha"),
                ("Alpha_Opt/x.lua", ""),
            ],
        );
        let log = Arc::clone(&routes.log);

        let (outcome, written) = install(&fx, routes, &InstalledRecords::new(), 1, None);

        assert_eq!(outcome.status, InstallStatus::Installed, "{outcome:?}");
        assert_eq!(outcome.name, "Project 1");
        assert_eq!(outcome.message, None);
        assert_eq!(outcome.file.as_ref().unwrap().id, 10);
        assert_eq!(names(&fx.addons), ["Alpha", "Alpha_Opt"]);
        assert_eq!(written, [(1, record(10, &["Alpha", "Alpha_Opt"]))]);
        assert_eq!(*log.downloads.lock().unwrap(), [download_url(10)]);
        // Niente rimane accanto ad AddOns.
        assert_eq!(names(fx.addons.parent().unwrap()), ["AddOns"]);
    }

    #[test]
    fn the_project_override_picks_the_file() {
        let fx = fixture();
        let routes = routes(1, 11, &[("Alpha/Alpha.toc", "")]);
        let outcome = install_addon(
            &fx.installation,
            &client(routes),
            None,
            &VersionOverrides::from([(1, CLASSIC_ERA)]),
            &InstalledRecords::new(),
            1,
            None,
            false,
            &mut |_, _| Ok(()),
            &progress::ignore,
        )
        .remove(0);
        assert_eq!(outcome.status, InstallStatus::Installed, "{outcome:?}");
        assert_eq!(outcome.file.unwrap().id, 11);
    }

    #[test]
    fn an_explicit_file_id_is_installed() {
        let fx = fixture();
        let (outcome, written) = install(
            &fx,
            routes(1, 99, &[("Alpha/Alpha.toc", "")]),
            &InstalledRecords::new(),
            1,
            Some(99),
        );
        assert_eq!(outcome.status, InstallStatus::Installed, "{outcome:?}");
        assert_eq!(written[0].1.file_id, 99);
    }

    #[test]
    fn a_file_of_another_project_is_refused() {
        let fx = fixture();
        let mut routes = routes(1, 99, &[("Alpha/Alpha.toc", "")]);
        routes.files.insert(
            format!("{API}/v1/mods/1/files/99"),
            json!({"data": file_json(2, 99, true)}).to_string(),
        );
        let log = Arc::clone(&routes.log);

        let (outcome, written) = install(&fx, routes, &InstalledRecords::new(), 1, Some(99));

        assert_eq!(outcome.status, InstallStatus::Error);
        assert_eq!(
            outcome.message.as_deref(),
            Some("The requested file does not belong to this addon.")
        );
        assert!(written.is_empty());
        assert!(log.downloads.lock().unwrap().is_empty());
        assert!(names(&fx.addons).is_empty());
    }

    #[test]
    fn distribution_not_allowed_or_unknown_is_skipped_without_downloading() {
        for allow in [Some(false), None] {
            let fx = fixture();
            let mut routes = routes(1, 10, &[("Alpha/Alpha.toc", "")]);
            routes.projects = vec![project(1, allow)];
            let log = Arc::clone(&routes.log);

            let (outcome, written) = install(&fx, routes, &InstalledRecords::new(), 1, None);

            assert_eq!(outcome.status, InstallStatus::Skipped, "{allow:?}");
            assert!(
                outcome
                    .message
                    .unwrap()
                    .contains("install it from the CurseForge website")
            );
            assert!(written.is_empty());
            assert!(log.downloads.lock().unwrap().is_empty());
            assert!(names(&fx.addons).is_empty());
        }
    }

    #[test]
    fn a_file_without_download_url_is_skipped() {
        let fx = fixture();
        let mut routes = routes(1, 10, &[("Alpha/Alpha.toc", "")]);
        routes.files.insert(
            format!("{API}/v1/mods/1/files/10"),
            json!({"data": file_json(1, 10, false)}).to_string(),
        );
        let log = Arc::clone(&routes.log);

        let (outcome, _) = install(&fx, routes, &InstalledRecords::new(), 1, None);

        assert_eq!(outcome.status, InstallStatus::Skipped);
        assert!(
            outcome
                .message
                .unwrap()
                .contains("does not provide a download link")
        );
        assert!(log.downloads.lock().unwrap().is_empty());
    }

    #[test]
    fn no_release_for_the_game_version_is_skipped() {
        let mut fx = fixture();
        fx.installation.flavor = "classic".to_owned();
        let (outcome, _) = install(
            &fx,
            routes(1, 10, &[("Alpha/Alpha.toc", "")]),
            &InstalledRecords::new(),
            1,
            None,
        );
        assert_eq!(outcome.status, InstallStatus::Skipped);
        assert_eq!(
            outcome.message.as_deref(),
            Some("There is no release file for this game version.")
        );
    }

    /// Un progetto che per WoW Forever ha solo una Beta (come RareScanner, issue #141).
    fn beta_only_routes() -> Routes {
        let mut routes = routes(1, 12, &[("Alpha/Alpha.toc", "")]);
        routes.projects = vec![json!({
            "id": 1, "name": "Project 1", "allowModDistribution": true,
            "latestFilesIndexes": [
                {"fileId": 12, "releaseType": 2, "gameVersionTypeId": crate::updates::FOREVER},
                {"fileId": 13, "releaseType": 3, "gameVersionTypeId": crate::updates::FOREVER},
            ],
        })];
        let mut file = file_json(1, 12, true);
        file["releaseType"] = json!(2);
        routes.files.insert(
            format!("{API}/v1/mods/1/files/12"),
            json!({"data": file}).to_string(),
        );
        routes
    }

    #[test]
    fn a_beta_is_installed_only_with_the_option_and_says_so() {
        let mut fx = fixture();
        fx.installation.flavor = "classic_beta".to_owned();
        let run = |allow_beta| {
            install_addon(
                &fx.installation,
                &client(beta_only_routes()),
                None,
                &VersionOverrides::new(),
                &InstalledRecords::new(),
                1,
                None,
                allow_beta,
                &mut |_, _| Ok(()),
                &progress::ignore,
            )
            .remove(0)
        };
        let off = run(false);
        assert_eq!(off.status, InstallStatus::Skipped);
        assert_eq!(
            off.message.as_deref(),
            Some("There is no release file for this game version.")
        );
        let on = run(true);
        assert_eq!(on.status, InstallStatus::Installed, "{on:?}");
        assert_eq!(on.file.as_ref().map(|f| f.id), Some(12));
        assert_eq!(
            on.message.as_deref(),
            Some("Installed the beta: there is no release for this game version.")
        );
    }

    #[test]
    fn a_folder_of_another_project_leaves_the_disk_unchanged() {
        let fx = fixture();
        write_folder(&fx.addons, "SharedLib", "## Title: Shared\n");
        let records = InstalledRecords::from([(2, record(20, &["sharedlib"]))]);
        let routes = routes(
            1,
            10,
            &[("Alpha/Alpha.toc", ""), ("SharedLib/SharedLib.toc", "new")],
        );

        let (outcome, written) = install(&fx, routes, &records, 1, None);

        assert_eq!(outcome.status, InstallStatus::Error);
        let message = outcome.message.unwrap();
        assert!(message.contains("\"SharedLib\""), "{message}");
        assert!(message.contains("project 2"), "{message}");
        assert!(written.is_empty());
        assert_eq!(names(&fx.addons), ["SharedLib"]);
        assert_eq!(
            fs::read_to_string(fx.addons.join("SharedLib").join("SharedLib.lua")).unwrap(),
            "old"
        );
        assert_eq!(names(fx.addons.parent().unwrap()), ["AddOns"]);
    }

    #[test]
    fn an_unrecorded_folder_of_another_project_leaves_the_disk_unchanged() {
        let fx = fixture();
        // Nessun record, ma il `.toc` dice che `SharedLib` è del progetto 2.
        write_folder(&fx.addons, "sharedlib", "## X-Curse-Project-ID: 2\n");
        let routes = routes(
            1,
            10,
            &[("Alpha/Alpha.toc", ""), ("SharedLib/SharedLib.toc", "new")],
        );

        let (outcome, written) = install(&fx, routes, &InstalledRecords::new(), 1, None);

        assert_eq!(outcome.status, InstallStatus::Error);
        let message = outcome.message.unwrap();
        assert!(message.contains("\"SharedLib\""), "{message}");
        assert!(message.contains("project 2"), "{message}");
        assert!(written.is_empty());
        assert_eq!(names(&fx.addons), ["sharedlib"]);
        assert_eq!(
            fs::read_to_string(fx.addons.join("sharedlib").join("sharedlib.lua")).unwrap(),
            "old"
        );
    }

    #[test]
    fn a_recorded_folder_that_is_gone_does_not_block() {
        let fx = fixture();
        // Il progetto 2 ha registrato `Alpha`, ma la cartella non c'è più.
        let records = InstalledRecords::from([(2, record(20, &["Alpha"]))]);
        let (outcome, _) = install(
            &fx,
            routes(1, 10, &[("Alpha/Alpha.toc", "")]),
            &records,
            1,
            None,
        );
        assert_eq!(outcome.status, InstallStatus::Installed, "{outcome:?}");
    }

    #[test]
    fn a_manually_installed_folder_is_replaced() {
        let fx = fixture();
        write_folder(&fx.addons, "alpha", "## Title: Alpha\n");
        let (outcome, written) = install(
            &fx,
            routes(1, 10, &[("Alpha/Alpha.lua", "new")]),
            &InstalledRecords::new(),
            1,
            None,
        );
        assert_eq!(outcome.status, InstallStatus::Installed, "{outcome:?}");
        assert_eq!(names(&fx.addons), ["Alpha"]);
        assert_eq!(
            fs::read_to_string(fx.addons.join("Alpha").join("Alpha.lua")).unwrap(),
            "new"
        );
        assert_eq!(written[0].1.folders, ["Alpha"]);
    }

    #[test]
    fn an_installed_project_is_skipped_without_network() {
        let fx = fixture();
        write_folder(&fx.addons, "Beta", "## X-Curse-Project-ID: 3\n");
        write_folder(&fx.addons, "Alpha", "## Title: Alpha\n");
        let records = InstalledRecords::from([(1, record(10, &["ALPHA"]))]);
        for mod_id in [1, 3] {
            let routes = routes(mod_id, mod_id * 10, &[("Alpha/Alpha.toc", "")]);
            let log = Arc::clone(&routes.log);

            let (outcome, written) = install(&fx, routes, &records, mod_id, None);

            assert_eq!(outcome.status, InstallStatus::Skipped);
            assert_eq!(outcome.message.as_deref(), Some("Already installed."));
            assert!(written.is_empty());
            assert!(log.requests.lock().unwrap().is_empty());
            assert!(log.downloads.lock().unwrap().is_empty());
        }
    }

    #[test]
    fn a_record_that_cannot_be_saved_leaves_the_install_done() {
        let fx = fixture();
        let outcome = install_addon(
            &fx.installation,
            &client(routes(1, 10, &[("Alpha/Alpha.toc", "")])),
            None,
            &VersionOverrides::new(),
            &InstalledRecords::new(),
            1,
            None,
            false,
            &mut |_, _| Err("disk full".to_owned()),
            &progress::ignore,
        )
        .remove(0);
        assert_eq!(outcome.status, InstallStatus::Installed);
        assert!(outcome.file.is_some());
        assert_eq!(
            outcome.message.as_deref(),
            Some("Installed, but the app could not remember its folders: disk full")
        );
        assert_eq!(names(&fx.addons), ["Alpha"]);
    }

    #[test]
    fn network_errors_and_unknown_projects_are_errors() {
        let fx = fixture();
        let mut unknown = routes(1, 10, &[]);
        unknown.projects = Vec::new();
        let (outcome, _) = install(&fx, unknown, &InstalledRecords::new(), 1, None);
        assert_eq!(outcome.status, InstallStatus::Error);
        assert_eq!(
            outcome.message.as_deref(),
            Some("Addon not found on CurseForge.")
        );

        // Il file indicato non esiste: 404 dall'API.
        let (outcome, _) = install(
            &fx,
            routes(1, 10, &[]),
            &InstalledRecords::new(),
            1,
            Some(55),
        );
        assert_eq!(outcome.status, InstallStatus::Error);
        assert!(!outcome.message.unwrap().contains("test-key"));
    }

    const REQUIRED: u8 = RELATION_REQUIRED_DEPENDENCY;
    use InstallStatus::{Error as Failed, Installed, Skipped};

    #[test]
    fn required_dependencies_are_installed_recursively() {
        let fx = fixture();
        let routes = graph(&[(1, &[(2, REQUIRED)]), (2, &[(3, REQUIRED)]), (3, &[])]);
        let log = Arc::clone(&routes.log);

        let (outcomes, written) = install_all(&fx, routes, &InstalledRecords::new(), 1, None);

        assert_eq!(
            summary(&outcomes),
            [
                (1, Installed, None),
                (2, Installed, Some(1)),
                (3, Installed, Some(2))
            ]
        );
        assert_eq!(outcomes[1].name, "Project 2");
        assert_eq!(names(&fx.addons), ["P1", "P2", "P3"]);
        assert_eq!(
            written,
            [
                (1, record(10, &["P1"])),
                (2, record(20, &["P2"])),
                (3, record(30, &["P3"]))
            ]
        );
        assert_eq!(
            *log.downloads.lock().unwrap(),
            [download_url(10), download_url(20), download_url(30)]
        );
    }

    #[test]
    fn optional_dependencies_tools_and_libraries_are_ignored() {
        let fx = fixture();
        let routes = graph(&[
            (1, &[(2, 1), (3, 2), (4, 4), (5, 5), (6, 6), (7, REQUIRED)]),
            (7, &[]),
        ]);

        let (outcomes, _) = install_all(&fx, routes, &InstalledRecords::new(), 1, None);

        assert_eq!(
            summary(&outcomes),
            [(1, Installed, None), (7, Installed, Some(1))]
        );
    }

    #[test]
    fn an_installed_dependency_is_skipped_without_downloading() {
        let fx = fixture();
        write_folder(&fx.addons, "Lib", "## X-Curse-Project-ID: 2\n");
        let routes = graph(&[(1, &[(2, REQUIRED)]), (2, &[(3, REQUIRED)]), (3, &[])]);
        let log = Arc::clone(&routes.log);

        let (outcomes, written) = install_all(&fx, routes, &InstalledRecords::new(), 1, None);

        // Le dipendenze di una dipendenza già installata non si seguono.
        assert_eq!(
            summary(&outcomes),
            [(1, Installed, None), (2, Skipped, Some(1))]
        );
        assert_eq!(outcomes[1].message.as_deref(), Some("Already installed."));
        assert_eq!(outcomes[1].name, "Project 2");
        assert_eq!(*log.downloads.lock().unwrap(), [download_url(10)]);
        assert_eq!(written.len(), 1);
    }

    #[test]
    fn cycles_and_shared_dependencies_are_visited_once() {
        let fx = fixture();
        // 1 → 2 → 1 è un ciclo; 3 è richiesto da 1 e da 2.
        let routes = graph(&[
            (1, &[(2, REQUIRED), (3, REQUIRED)]),
            (2, &[(1, REQUIRED), (3, REQUIRED)]),
            (3, &[(3, REQUIRED)]),
        ]);
        let log = Arc::clone(&routes.log);

        let (outcomes, _) = install_all(&fx, routes, &InstalledRecords::new(), 1, None);

        assert_eq!(
            summary(&outcomes),
            [
                (1, Installed, None),
                (2, Installed, Some(1)),
                (3, Installed, Some(1))
            ]
        );
        assert_eq!(log.downloads.lock().unwrap().len(), 3);
    }

    #[test]
    fn dependencies_beyond_the_depth_limit_are_not_installed() {
        let fx = fixture();
        let nodes: Vec<(u32, Vec<(u32, u8)>)> =
            (1..=8).map(|id| (id, vec![(id + 1, REQUIRED)])).collect();
        let nodes: Vec<(u32, &[(u32, u8)])> = nodes
            .iter()
            .map(|(id, deps)| (*id, deps.as_slice()))
            .collect();
        let routes = graph(&nodes);
        let log = Arc::clone(&routes.log);

        let (outcomes, _) = install_all(&fx, routes, &InstalledRecords::new(), 1, None);

        let mut expected = vec![(1, Installed, None)];
        expected.extend((2..=6).map(|id| (id, Installed, Some(id - 1))));
        expected.push((7, Skipped, Some(6)));
        assert_eq!(summary(&outcomes), expected);
        let message = outcomes[6].message.as_deref().unwrap();
        assert!(message.contains("more than 5 levels deep"), "{message}");
        // Il nome arriva dalla `get_mods` del livello, anche senza installare.
        assert_eq!(outcomes[6].name, "Project 7");
        assert_eq!(log.downloads.lock().unwrap().len(), 6);
        assert!(
            !log.requests
                .lock()
                .unwrap()
                .iter()
                .any(|url| url.contains("/7"))
        );
    }

    #[test]
    fn an_installed_dependency_beyond_the_depth_limit_is_already_installed() {
        let fx = fixture();
        write_folder(&fx.addons, "Deep", "## X-Curse-Project-ID: 7\n");
        let nodes: Vec<(u32, Vec<(u32, u8)>)> =
            (1..=7).map(|id| (id, vec![(id + 1, REQUIRED)])).collect();
        let nodes: Vec<(u32, &[(u32, u8)])> = nodes
            .iter()
            .map(|(id, deps)| (*id, deps.as_slice()))
            .collect();

        let (outcomes, _) = install_all(&fx, graph(&nodes), &InstalledRecords::new(), 1, None);

        assert_eq!(outcomes.len(), 7);
        assert_eq!((outcomes[6].mod_id, outcomes[6].status), (7, Skipped));
        assert_eq!(outcomes[6].message.as_deref(), Some("Already installed."));
    }

    #[test]
    fn each_level_of_dependencies_is_read_with_one_call() {
        let fx = fixture();
        let routes = graph(&[
            (1, &[(2, REQUIRED), (3, REQUIRED)]),
            (2, &[(4, REQUIRED)]),
            (3, &[(5, REQUIRED)]),
            (4, &[]),
            (5, &[]),
        ]);
        let log = Arc::clone(&routes.log);

        let (outcomes, _) = install_all(&fx, routes, &InstalledRecords::new(), 1, None);

        assert_eq!(outcomes.len(), 5);
        let mods_calls = log
            .requests
            .lock()
            .unwrap()
            .iter()
            .filter(|url| *url == &format!("{API}/v1/mods"))
            .count();
        // Il progetto scelto, poi un livello [2, 3] e un livello [4, 5].
        assert_eq!(mods_calls, 3);
    }

    #[test]
    fn a_blocked_dependency_is_skipped_and_the_others_continue() {
        let fx = fixture();
        let mut routes = graph(&[(1, &[(2, REQUIRED), (3, REQUIRED)]), (2, &[]), (3, &[])]);
        routes.projects[1] = project(2, Some(false));
        let log = Arc::clone(&routes.log);

        let (outcomes, _) = install_all(&fx, routes, &InstalledRecords::new(), 1, None);

        assert_eq!(
            summary(&outcomes),
            [
                (1, Installed, None),
                (2, Skipped, Some(1)),
                (3, Installed, Some(1))
            ]
        );
        assert!(
            outcomes[1]
                .message
                .as_deref()
                .unwrap()
                .contains("install it from the CurseForge website")
        );
        assert!(!log.downloads.lock().unwrap().contains(&download_url(20)));
    }

    #[test]
    fn a_network_error_on_a_dependency_keeps_the_rest() {
        let fx = fixture();
        let mut routes = graph(&[(1, &[(2, REQUIRED), (3, REQUIRED)]), (2, &[]), (3, &[])]);
        routes.broken.push(format!("{API}/v1/mods/2/files/20"));

        let (outcomes, written) = install_all(&fx, routes, &InstalledRecords::new(), 1, None);

        assert_eq!(
            summary(&outcomes),
            [
                (1, Installed, None),
                (2, Failed, Some(1)),
                (3, Installed, Some(1))
            ]
        );
        assert!(!outcomes[1].message.as_deref().unwrap().contains("test-key"));
        assert_eq!(names(&fx.addons), ["P1", "P3"]);
        assert_eq!(
            written.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
            [1, 3]
        );
    }

    #[test]
    fn dependencies_use_the_game_version_of_the_chosen_project() {
        let fx = fixture();
        let routes = graph(&[(1, &[(2, REQUIRED)]), (2, &[])]);
        let mut written = Vec::new();
        let collected = progress::tests::Collected::default();

        // La versione forzata del progetto scelto vale per la dipendenza, non la sua.
        let outcomes = install_addon(
            &fx.installation,
            &client(routes),
            None,
            &VersionOverrides::from([(1, CLASSIC_ERA), (2, RETAIL)]),
            &InstalledRecords::new(),
            1,
            None,
            false,
            &mut |id, r| {
                written.push((id, r.file_id));
                Ok(())
            },
            &collected.sink(),
        );

        assert_eq!(
            summary(&outcomes),
            [(1, Installed, None), (2, Installed, Some(1))]
        );
        assert_eq!(written, [(1, 11), (2, 21)]);
        // Ogni progetto, dipendenza compresa, riporta le sue fasi con il suo id.
        use progress::Phase::*;
        let steps = |key: &str| {
            [Checking, Downloading, Extracting, Installing].map(|phase| (key.to_owned(), phase))
        };
        assert_eq!(collected.phases(), [steps("1"), steps("2")].concat());
        assert!(
            collected
                .0
                .borrow()
                .iter()
                .all(|p| p.source == Some(SourceKind::Curseforge))
        );
    }

    #[test]
    fn a_just_installed_dependency_owns_its_folders() {
        let fx = fixture();
        let mut routes = graph(&[(1, &[(2, REQUIRED), (3, REQUIRED)]), (2, &[]), (3, &[])]);
        // 2 e 3 portano la stessa cartella: dopo 2, `Lib` è registrata come sua.
        for id in [2, 3] {
            routes
                .zips
                .insert(download_url(id * 10), zip_of(&[("Lib/Lib.toc", "")]));
        }

        let (outcomes, written) = install_all(&fx, routes, &InstalledRecords::new(), 1, None);

        assert_eq!(
            summary(&outcomes),
            [
                (1, Installed, None),
                (2, Installed, Some(1)),
                (3, Failed, Some(1))
            ]
        );
        let message = outcomes[2].message.as_deref().unwrap();
        assert!(message.contains("project 2"), "{message}");
        assert_eq!(written.len(), 2);
    }

    #[test]
    fn no_dependency_is_followed_when_the_project_is_not_installed() {
        let fx = fixture();
        let mut routes = graph(&[(1, &[(2, REQUIRED)]), (2, &[])]);
        routes.projects[0] = project(1, Some(false));
        let log = Arc::clone(&routes.log);

        let (outcomes, _) = install_all(&fx, routes, &InstalledRecords::new(), 1, None);

        assert_eq!(summary(&outcomes), [(1, Skipped, None)]);
        assert!(log.downloads.lock().unwrap().is_empty());
    }

    #[test]
    fn the_outcome_payload_is_camel_case() {
        let json = serde_json::to_value(InstallOutcome::skipped(7, "Foo", "why")).unwrap();
        assert_eq!(
            json,
            json!({"modId": 7, "name": "Foo", "status": "skipped", "message": "why"})
        );
        let json = serde_json::to_value(InstallOutcome {
            mod_id: 7,
            name: "Foo".to_owned(),
            status: InstallStatus::Installed,
            message: None,
            file: Some(FileSummary {
                id: 1,
                name: "x".to_owned(),
                date: "d".to_owned(),
            }),
            dependency_of: Some(3),
        })
        .unwrap();
        assert_eq!(json["status"], "installed");
        assert_eq!(json["file"]["id"], 1);
        assert!(json.get("message").is_none());
        assert_eq!(json["dependencyOf"], 3);
        assert_eq!(serde_json::to_value(InstallStatus::Error).unwrap(), "error");
    }

    // ── Mirror GitHub dei progetti bloccati (D34, issue #164) ───────────

    const REPO: &str = "owner/alpha";

    /// Il mirror GitHub finto: l'esito di `resolve` per progetto e gli zip delle release, con le
    /// richieste fatte.
    #[derive(Default)]
    struct FakeMirror {
        outcomes: HashMap<u32, Outcome>,
        zips: HashMap<String, Vec<u8>>,
        /// `(progetto, file CurseForge)` di ogni `resolve`.
        resolved: Mutex<Vec<(u32, u32)>>,
        downloads: Mutex<Vec<String>>,
    }

    impl MirrorSource for FakeMirror {
        fn resolve(&self, project: &Mod, file: &File, _flavor: &str) -> Outcome {
            self.resolved.lock().unwrap().push((project.id, file.id));
            self.outcomes
                .get(&project.id)
                .cloned()
                .unwrap_or(Outcome::Unavailable {
                    reason: Reason::NoGithubSource,
                })
        }

        fn download(
            &self,
            asset: &Asset,
            _progress: &mut crate::sources::http::DownloadProgress<'_>,
        ) -> Result<Vec<u8>, String> {
            let url = asset.browser_download_url.clone();
            self.downloads.lock().unwrap().push(url.clone());
            self.zips
                .get(&url)
                .cloned()
                .ok_or_else(|| "Could not download the file.".to_owned())
        }
    }

    fn asset_url(mod_id: u32) -> String {
        format!("https://github.com/{REPO}/releases/download/v1/P{mod_id}.zip")
    }

    impl FakeMirror {
        /// Una release `v1` che combacia per il progetto `mod_id`, con lo zip `zip`.
        fn with_release(mut self, mod_id: u32, zip: &[(&str, &str)]) -> Self {
            let asset = Asset {
                name: format!("P{mod_id}.zip"),
                browser_download_url: asset_url(mod_id),
            };
            self.zips.insert(asset_url(mod_id), zip_of(zip));
            self.outcomes.insert(
                mod_id,
                Outcome::Mirror {
                    repo: REPO.to_owned(),
                    tag: "v1".to_owned(),
                    asset,
                },
            );
            self
        }

        fn without(mut self, mod_id: u32, reason: Reason) -> Self {
            self.outcomes
                .insert(mod_id, Outcome::Unavailable { reason });
            self
        }
    }

    /// Le rotte di un progetto bloccato: il file `file_id` esiste ma senza `downloadUrl`.
    fn blocked_routes(mod_id: u32, file_id: u32) -> Routes {
        Routes {
            projects: vec![project(mod_id, Some(false))],
            files: HashMap::from([(
                format!("{API}/v1/mods/{mod_id}/files/{file_id}"),
                json!({"data": file_json(mod_id, file_id, false)}).to_string(),
            )]),
            ..Routes::default()
        }
    }

    /// Installa con il mirror, con le dipendenze, raccogliendo i record scritti.
    fn install_mirrored(
        fx: &Fixture,
        routes: Routes,
        mirror: &FakeMirror,
        records: &InstalledRecords,
        mod_id: u32,
    ) -> (Vec<InstallOutcome>, Vec<(u32, InstalledRecord)>) {
        let mut written = Vec::new();
        let outcomes = install_addon(
            &fx.installation,
            &client(routes),
            Some(mirror),
            &VersionOverrides::new(),
            records,
            mod_id,
            None,
            false,
            &mut |id, r| {
                written.push((id, r));
                Ok(())
            },
            &progress::ignore,
        );
        (outcomes, written)
    }

    #[test]
    fn a_blocked_project_installs_from_its_mirror_with_a_curseforge_record() {
        let fx = fixture();
        let routes = blocked_routes(1, 10);
        let log = Arc::clone(&routes.log);
        let mirror = FakeMirror::default().with_release(1, &[("Alpha/Alpha.toc", "")]);

        let (outcomes, written) =
            install_mirrored(&fx, routes, &mirror, &InstalledRecords::new(), 1);

        assert_eq!(summary(&outcomes), [(1, Installed, None)]);
        let outcome = &outcomes[0];
        assert_eq!(
            outcome.message.as_deref(),
            Some(
                "Installed from GitHub (owner/alpha v1): CurseForge blocks third-party downloads."
            )
        );
        // Il file dell'esito e del record è quello di CurseForge che combacia.
        assert_eq!(outcome.file.as_ref().unwrap().id, 10);
        assert_eq!(written, [(1, record(10, &["Alpha"]))]);
        assert_eq!(*mirror.resolved.lock().unwrap(), [(1, 10)]);
        assert_eq!(*mirror.downloads.lock().unwrap(), [asset_url(1)]);
        assert!(log.downloads.lock().unwrap().is_empty());
        assert_eq!(names(&fx.addons), ["Alpha"]);
        assert_eq!(names(fx.addons.parent().unwrap()), ["AddOns"]);
    }

    #[test]
    fn a_blocked_project_without_a_usable_mirror_is_skipped_with_the_reason() {
        let fx = fixture();
        let mirror = FakeMirror::default().without(
            1,
            Reason::DifferentVersion {
                github: "v0".to_owned(),
                curseforge: "file-10.zip".to_owned(),
            },
        );

        let (outcomes, written) = install_mirrored(
            &fx,
            blocked_routes(1, 10),
            &mirror,
            &InstalledRecords::new(),
            1,
        );

        assert_eq!(summary(&outcomes), [(1, Skipped, None)]);
        assert_eq!(
            outcomes[0].message.as_deref(),
            Some(
                "The author does not allow downloads from third-party apps and the GitHub copy cannot be used. The latest GitHub release (v0) is not the CurseForge file (file-10.zip)."
            )
        );
        assert!(written.is_empty());
        assert!(mirror.downloads.lock().unwrap().is_empty());
        assert!(names(&fx.addons).is_empty());

        // GitHub che non risponde è un errore, non un salto.
        let mirror = FakeMirror::default().without(1, Reason::Failed("offline".to_owned()));
        let (outcomes, _) = install_mirrored(
            &fx,
            blocked_routes(1, 10),
            &mirror,
            &InstalledRecords::new(),
            1,
        );
        assert_eq!(summary(&outcomes), [(1, Failed, None)]);
        assert!(outcomes[0].message.as_deref().unwrap().ends_with("offline"));
    }

    #[test]
    fn an_allowed_project_asks_github_nothing() {
        let fx = fixture();
        let mirror = FakeMirror::default().with_release(1, &[("Other/Other.toc", "")]);

        let (outcomes, written) = install_mirrored(
            &fx,
            routes(1, 10, &[("Alpha/Alpha.toc", "")]),
            &mirror,
            &InstalledRecords::new(),
            1,
        );

        assert_eq!(summary(&outcomes), [(1, Installed, None)]);
        assert_eq!(outcomes[0].message, None);
        assert_eq!(written, [(1, record(10, &["Alpha"]))]);
        assert!(mirror.resolved.lock().unwrap().is_empty());
        assert!(mirror.downloads.lock().unwrap().is_empty());
    }

    #[test]
    fn a_mirror_zip_with_a_folder_of_another_project_leaves_the_disk_unchanged() {
        let fx = fixture();
        write_folder(&fx.addons, "SharedLib", "## Title: Shared\n");
        let records = InstalledRecords::from([(2, record(20, &["SharedLib"]))]);
        let mirror = FakeMirror::default().with_release(
            1,
            &[("Alpha/Alpha.toc", ""), ("SharedLib/SharedLib.toc", "new")],
        );

        let (outcomes, written) =
            install_mirrored(&fx, blocked_routes(1, 10), &mirror, &records, 1);

        assert_eq!(summary(&outcomes), [(1, Failed, None)]);
        let message = outcomes[0].message.as_deref().unwrap();
        assert!(message.contains("project 2"), "{message}");
        assert!(written.is_empty());
        assert_eq!(names(&fx.addons), ["SharedLib"]);
        assert_eq!(
            fs::read_to_string(fx.addons.join("SharedLib").join("SharedLib.lua")).unwrap(),
            "old"
        );
        assert_eq!(names(fx.addons.parent().unwrap()), ["AddOns"]);
    }

    #[test]
    fn blocked_dependencies_follow_the_same_mirror_rule() {
        let fx = fixture();
        let mut routes = graph(&[
            (1, &[(2, REQUIRED), (3, REQUIRED), (4, REQUIRED)]),
            (2, &[]),
            (3, &[]),
            (4, &[]),
        ]);
        routes.projects[1] = project(2, Some(false));
        routes.projects[2] = project(3, Some(false));
        let log = Arc::clone(&routes.log);
        let mirror = FakeMirror::default().with_release(2, &[("P2/P2.toc", "")]);

        let (outcomes, written) =
            install_mirrored(&fx, routes, &mirror, &InstalledRecords::new(), 1);

        assert_eq!(
            summary(&outcomes),
            [
                (1, Installed, None),
                (2, Installed, Some(1)),
                (3, Skipped, Some(1)),
                (4, Installed, Some(1)),
            ]
        );
        assert!(
            outcomes[1]
                .message
                .as_deref()
                .unwrap()
                .starts_with("Installed from GitHub")
        );
        assert!(
            outcomes[2]
                .message
                .as_deref()
                .unwrap()
                .contains("does not link a GitHub repository")
        );
        assert!(written.contains(&(2, record(20, &["P2"]))));
        assert_eq!(*mirror.resolved.lock().unwrap(), [(2, 20), (3, 30)]);
        let downloads = log.downloads.lock().unwrap();
        assert!(!downloads.contains(&download_url(20)));
        assert!(!downloads.contains(&download_url(30)));
        assert_eq!(names(&fx.addons), ["P1", "P2", "P4"]);
    }

    fn check_mirror(routes: Routes, mirror: &FakeMirror) -> Result<MirrorAvailability, String> {
        check_mirror_file(&fixture(), routes, mirror, None)
    }

    fn check_mirror_file(
        fx: &Fixture,
        routes: Routes,
        mirror: &FakeMirror,
        file_id: Option<u32>,
    ) -> Result<MirrorAvailability, String> {
        github_mirror(
            &fx.installation,
            &client(routes),
            mirror,
            &VersionOverrides::new(),
            1,
            file_id,
            false,
        )
    }

    #[test]
    fn the_mirror_check_uses_the_requested_file_like_the_install() {
        // Il file indicato (per esempio la Beta scelta nel dettaglio), anche senza un Release
        // per la versione del gioco.
        let mut routes = blocked_routes(1, 12);
        routes.projects[0]["latestFilesIndexes"] = json!([]);
        let mirror = FakeMirror::default().with_release(1, &[]);
        let result = check_mirror_file(&fixture(), routes, &mirror, Some(12)).unwrap();
        assert!(result.available, "{result:?}");
        assert_eq!(*mirror.resolved.lock().unwrap(), [(1, 12)]);
    }

    #[test]
    fn an_unsupported_game_version_is_a_reason_not_an_error() {
        let mut fx = fixture();
        fx.installation.flavor = "unknown_flavor".to_owned();
        fx.installation.name = "Unknown".to_owned();
        let mirror = FakeMirror::default().with_release(1, &[]);
        let result = check_mirror_file(&fx, blocked_routes(1, 10), &mirror, None).unwrap();
        assert!(!result.available);
        assert_eq!(
            result.reason.as_deref(),
            Some("Installing addons does not support the game version \"Unknown\" yet.")
        );
        assert!(mirror.resolved.lock().unwrap().is_empty());
    }

    #[test]
    fn the_mirror_check_is_available_unavailable_or_not_needed() {
        let mirror = FakeMirror::default().with_release(1, &[("Alpha/Alpha.toc", "")]);
        assert_eq!(
            check_mirror(blocked_routes(1, 10), &mirror),
            Ok(MirrorAvailability {
                available: true,
                repo: Some(REPO.to_owned()),
                tag: Some("v1".to_owned()),
                reason: None,
            })
        );
        assert_eq!(*mirror.resolved.lock().unwrap(), [(1, 10)]);
        // Il controllo non scarica niente.
        assert!(mirror.downloads.lock().unwrap().is_empty());

        let mirror = FakeMirror::default();
        assert_eq!(
            check_mirror(blocked_routes(1, 10), &mirror),
            Ok(MirrorAvailability {
                available: false,
                repo: None,
                tag: None,
                reason: Some(Reason::NoGithubSource.message()),
            })
        );

        // Un progetto che CurseForge lascia scaricare: nessuna richiesta a GitHub.
        let mirror = FakeMirror::default().with_release(1, &[("Alpha/Alpha.toc", "")]);
        assert_eq!(
            check_mirror(routes(1, 10, &[]), &mirror),
            Ok(MirrorAvailability {
                available: false,
                repo: None,
                tag: None,
                reason: None,
            })
        );
        assert!(mirror.resolved.lock().unwrap().is_empty());
    }

    #[test]
    fn the_mirror_check_without_a_file_or_project_asks_github_nothing() {
        let mirror = FakeMirror::default().with_release(1, &[]);
        // Nessun file per la versione del gioco: il motivo è quello dell'installazione.
        let mut routes = blocked_routes(1, 10);
        routes.projects[0]["latestFilesIndexes"] = json!([]);
        let result = check_mirror(routes, &mirror).unwrap();
        assert!(!result.available);
        assert_eq!(
            result.reason.as_deref(),
            Some("There is no release file for this game version.")
        );
        // Un progetto che CurseForge non conosce, o CurseForge che non risponde, è un errore.
        assert_eq!(
            check_mirror(Routes::default(), &mirror),
            Err("Addon not found on CurseForge.".to_owned())
        );
        let mut routes = blocked_routes(1, 10);
        routes.broken.push(format!("{API}/v1/mods/1/files/10"));
        assert!(check_mirror(routes, &mirror).is_err());
        assert!(mirror.resolved.lock().unwrap().is_empty());
    }

    #[test]
    fn the_mirror_payload_is_camel_case() {
        let json = serde_json::to_value(MirrorAvailability {
            available: true,
            repo: Some(REPO.to_owned()),
            tag: Some("v1".to_owned()),
            reason: None,
        })
        .unwrap();
        assert_eq!(
            json,
            json!({"available": true, "repo": REPO, "tag": "v1", "reason": null})
        );
    }
}
