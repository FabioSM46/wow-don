//! Import di una stringa di addon (issue #134): l'anteprima e l'esecuzione, con le due modalità
//! scelte dall'utente. *Keep my addons* installa solo i progetti che mancano; *Replace all*
//! installa i mancanti e poi toglie da `Interface/AddOns` le cartelle che nessun progetto
//! importato reclama.
//!
//! L'ordine "prima installa, poi togli" è voluto: un import fallito a metà non lascia
//! l'installazione vuota, e se non si installa niente (e qualcosa mancava) non si toglie niente.
//! *Replace all* non fa un backup (scelta dell'utente): toglie solo le cartelle che il frontend ha
//! mostrato e l'utente ha confermato (`confirmed`), e solo se dopo le installazioni sono ancora
//! da togliere. Una cartella dichiarata come dipendenza nel `.toc` di un addon che resta resta
//! anche lei. `WTF`, le SavedVariables e tutto quello fuori da `AddOns` non si toccano mai.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::addon_list::{Decoded, InvalidEntry, ListEntry};
use crate::install;
use crate::store::{SourceKind, SourceRecords};
use crate::updates::Refresh;

/// Un progetto: fonte e chiave.
type ProjectId = (SourceKind, String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImportMode {
    /// Aggiunge i progetti che mancano, non toglie niente.
    Keep,
    /// Aggiunge i progetti che mancano e toglie le cartelle che nessuno di loro reclama.
    Replace,
}

/// Se un progetto della stringa si può installare, prima di provarci.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Availability {
    Available,
    /// L'autore non permette i download da app di terzi, la fonte non lo elenca o non è ancora
    /// supportata: il motivo e, se c'è, la pagina dell'addon.
    NotDownloadable {
        message: String,
        url: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum EntryState {
    New,
    Installed,
    NotDownloadable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewEntry {
    pub source: SourceKind,
    pub key: String,
    pub name: String,
    pub state: EntryState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// L'anteprima di un import: non scrive niente.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Preview {
    /// Il flavor dell'installazione da cui viene la stringa.
    pub flavor: String,
    pub entries: Vec<PreviewEntry>,
    pub invalid: Vec<InvalidEntry>,
    /// Le cartelle che *Replace all* toglierebbe ora, con il nome che hanno sul disco.
    pub removals: Vec<String>,
    /// Perché il controllo degli addon installati non è riuscito: senza, un progetto installato
    /// ma non riconosciuto risulta nuovo.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub check_error: Option<String>,
    /// Perché non si è potuto sapere se i progetti nuovi si possono scaricare.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lookup_error: Option<String>,
}

/// I progetti presenti nell'installazione: quelli che il controllo riconosce e quelli che l'app
/// ha registrato con almeno una cartella ancora sul disco.
pub fn installed_projects(refresh: &Refresh, records: &SourceRecords) -> HashSet<ProjectId> {
    let on_disk = disk_folders(refresh);
    let recognized = refresh
        .updates
        .iter()
        .filter_map(|update| update.project.as_ref())
        .map(|project| (project.source, project.key.clone()));
    let recorded = records.iter().flat_map(|(source, projects)| {
        projects
            .iter()
            .filter(|(_, record)| {
                record
                    .folders
                    .iter()
                    .any(|folder| on_disk.contains_key(&folder.to_lowercase()))
            })
            .map(|(key, _)| (*source, key.clone()))
    });
    recognized.chain(recorded).collect()
}

/// Le cartelle che *Replace all* toglierebbe se restassero solo i progetti `keep`: tutte quelle
/// sul disco che nessuno di loro reclama (dal controllo, dai file riconosciuti o dai record), né
/// direttamente né come dipendenza dichiarata nel `.toc` di una cartella che resta. Nell'ordine
/// della scansione, con il nome che hanno sul disco.
pub fn removal_candidates(
    refresh: &Refresh,
    records: &SourceRecords,
    keep: &HashSet<ProjectId>,
) -> Vec<String> {
    let on_disk = disk_folders(refresh);
    let mut kept: HashSet<String> = HashSet::new();
    for update in &refresh.updates {
        let Some(project) = &update.project else {
            continue;
        };
        if keep.contains(&(project.source, project.key.clone())) {
            kept.insert(update.folder.to_lowercase());
            kept.extend(update.project_folders.iter().map(|f| f.to_lowercase()));
        }
    }
    for (source, projects) in records {
        for (key, record) in projects {
            if keep.contains(&(*source, key.clone())) {
                kept.extend(record.folders.iter().map(|f| f.to_lowercase()));
            }
        }
    }
    // Le dipendenze di quello che resta restano, anche quelle delle dipendenze.
    let mut queue: Vec<String> = kept.iter().cloned().collect();
    while let Some(folder) = queue.pop() {
        let Some(&index) = on_disk.get(&folder) else {
            continue;
        };
        let toc = &refresh.addons[index].addon.toc;
        for dependency in toc.dependencies.iter().chain(&toc.optional_deps) {
            let dependency = dependency.trim().to_lowercase();
            if !dependency.is_empty() && kept.insert(dependency.clone()) {
                queue.push(dependency);
            }
        }
    }
    refresh
        .addons
        .iter()
        .map(|listed| &listed.addon.folder)
        .filter(|folder| !kept.contains(&folder.to_lowercase()))
        .cloned()
        .collect()
}

/// Cartella in minuscolo → posizione nella scansione.
fn disk_folders(refresh: &Refresh) -> HashMap<String, usize> {
    refresh
        .addons
        .iter()
        .enumerate()
        .map(|(index, listed)| (listed.addon.folder.to_lowercase(), index))
        .collect()
}

fn id_of(entry: &ListEntry) -> ProjectId {
    (entry.source, entry.key.clone())
}

/// L'anteprima di `decoded` per un'installazione letta in `refresh`. `availability` dice se un
/// progetto nuovo si può scaricare.
pub fn preview(
    decoded: &Decoded,
    refresh: &Refresh,
    records: &SourceRecords,
    availability: &dyn Fn(&ListEntry) -> Availability,
    lookup_error: Option<String>,
) -> Preview {
    let installed = installed_projects(refresh, records);
    let entries = decoded
        .list
        .addons
        .iter()
        .map(|entry| {
            let (state, message, url) = if installed.contains(&id_of(entry)) {
                (EntryState::Installed, None, None)
            } else {
                match availability(entry) {
                    Availability::Available => (EntryState::New, None, None),
                    Availability::NotDownloadable { message, url } => {
                        (EntryState::NotDownloadable, Some(message), url)
                    }
                }
            };
            PreviewEntry {
                source: entry.source,
                key: entry.key.clone(),
                name: entry.name.clone(),
                state,
                message,
                url,
            }
        })
        .collect();
    let keep: HashSet<ProjectId> = decoded.list.addons.iter().map(id_of).collect();
    let removals = if keep.is_empty() {
        Vec::new()
    } else {
        removal_candidates(refresh, records, &keep)
    };
    Preview {
        flavor: decoded.list.flavor.clone(),
        entries,
        invalid: decoded.invalid.clone(),
        removals,
        check_error: refresh.check_error.clone(),
        lookup_error,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum EntryStatus {
    Installed,
    AlreadyInstalled,
    Skipped,
    Error,
}

/// L'esito di un progetto della stringa.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EntryOutcome {
    pub source: SourceKind,
    pub key: String,
    pub name: String,
    pub status: EntryStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl EntryOutcome {
    pub fn new(entry: &ListEntry, status: EntryStatus, message: Option<String>) -> Self {
        Self {
            source: entry.source,
            key: entry.key.clone(),
            name: entry.name.clone(),
            status,
            message,
        }
    }
}

/// Una cartella che *Replace all* doveva togliere e non ha tolto.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemovalError {
    pub folder: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportResult {
    pub entries: Vec<EntryOutcome>,
    pub invalid: Vec<InvalidEntry>,
    /// Le cartelle tolte da *Replace all*.
    pub removed: Vec<String>,
    /// Cartelle che dopo le installazioni sarebbero da togliere ma che l'utente non aveva
    /// confermato (per esempio comparse dopo l'anteprima): restano.
    pub not_confirmed: Vec<String>,
    pub removal_errors: Vec<RemovalError>,
    /// Perché *Replace all* non ha tolto niente, quando non ci ha nemmeno provato.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub removal_skipped: Option<String>,
}

/// Chi installa un progetto della stringa. Nel comando sono le fonti vere; nei test una finta.
pub trait Installer {
    fn install(&mut self, entry: &ListEntry) -> EntryOutcome;
}

/// Esegue l'import. `before` e `records` sono lo stato prima delle installazioni; `rescan`
/// rilegge disco e record dopo, per *Replace all*. Le cartelle tolte vanno poi dimenticate dal
/// chiamante (record e cache dei fingerprint).
#[allow(clippy::too_many_arguments)]
pub fn run_import(
    addons_dir: &Path,
    decoded: &Decoded,
    mode: ImportMode,
    confirmed: &[String],
    before: &Refresh,
    records: &SourceRecords,
    installer: &mut dyn Installer,
    rescan: &mut dyn FnMut() -> Result<(Refresh, SourceRecords), String>,
) -> ImportResult {
    let installed = installed_projects(before, records);
    let entries: Vec<EntryOutcome> = decoded
        .list
        .addons
        .iter()
        .map(|entry| {
            if installed.contains(&id_of(entry)) {
                EntryOutcome::new(entry, EntryStatus::AlreadyInstalled, None)
            } else {
                installer.install(entry)
            }
        })
        .collect();
    let mut result = ImportResult {
        entries,
        invalid: decoded.invalid.clone(),
        removed: Vec::new(),
        not_confirmed: Vec::new(),
        removal_errors: Vec::new(),
        removal_skipped: None,
    };
    if mode == ImportMode::Keep {
        return result;
    }
    if result.entries.is_empty() {
        result.removal_skipped =
            Some("The list has no addons, so no folder was removed.".to_owned());
        return result;
    }
    let any_installed = result
        .entries
        .iter()
        .any(|e| e.status == EntryStatus::Installed);
    let all_present = result
        .entries
        .iter()
        .all(|e| e.status == EntryStatus::AlreadyInstalled);
    if !any_installed && !all_present {
        result.removal_skipped =
            Some("No addon could be installed, so no folder was removed.".to_owned());
        return result;
    }
    let (after, records_after) = match rescan() {
        Ok(state) => state,
        Err(error) => {
            result.removal_skipped = Some(format!(
                "The addons could not be read again, so no folder was removed: {error}"
            ));
            return result;
        }
    };
    let keep: HashSet<ProjectId> = decoded.list.addons.iter().map(id_of).collect();
    let confirmed: HashSet<String> = confirmed.iter().map(|f| f.to_lowercase()).collect();
    for folder in removal_candidates(&after, &records_after, &keep) {
        if !confirmed.contains(&folder.to_lowercase()) {
            result.not_confirmed.push(folder);
            continue;
        }
        match install::remove_folder(addons_dir, &folder) {
            Ok(()) => result.removed.push(folder),
            Err(message) => result.removal_errors.push(RemovalError { folder, message }),
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::addon_list::AddonList;
    use crate::addons::scan::{InstalledAddon, ListedAddon};
    use crate::addons::toc::TocInfo;
    use crate::store::{ProjectRecords, SourceRecord};
    use crate::test_support::TempDir;
    use crate::updates::{AddonUpdate, Project, UpdateStatus};
    use std::fs;
    use std::path::PathBuf;

    fn entry(source: SourceKind, key: &str) -> ListEntry {
        ListEntry {
            source,
            key: key.to_owned(),
            name: format!("Addon {key}"),
            version_type: None,
        }
    }

    fn decoded(entries: Vec<ListEntry>) -> Decoded {
        Decoded {
            list: AddonList {
                v: 1,
                flavor: "retail".to_owned(),
                addons: entries,
            },
            invalid: Vec::new(),
        }
    }

    fn listed(folder: &str, deps: &[&str]) -> ListedAddon {
        ListedAddon {
            addon: InstalledAddon {
                folder: folder.to_owned(),
                toc_file: Some(format!("{folder}.toc")),
                toc: TocInfo {
                    dependencies: deps.iter().map(|d| (*d).to_owned()).collect(),
                    ..TocInfo::default()
                },
                modified_ms: None,
                fingerprint: Some(1),
                error: None,
            },
            installed_project: None,
        }
    }

    fn matched(folder: &str, key: &str, also: &[&str]) -> AddonUpdate {
        AddonUpdate {
            folder: folder.to_owned(),
            source: Some(SourceKind::Curseforge),
            project: Some(Project {
                id: key.parse().unwrap(),
                source: SourceKind::Curseforge,
                key: key.to_owned(),
                name: format!("Addon {key}"),
                url: None,
                allow_mod_distribution: true,
            }),
            installed_file: None,
            status: UpdateStatus::UpToDate,
            version_override: None,
            project_folders: also.iter().map(|f| (*f).to_owned()).collect(),
            installed_modules: Vec::new(),
        }
    }

    /// Un'installazione su disco con le cartelle di `addons`, e lo stato letto come lo darebbe
    /// la scansione: `updates` riconosce i progetti.
    struct Disk {
        _tmp: TempDir,
        addons_dir: PathBuf,
    }

    fn disk(folders: &[&str]) -> Disk {
        let tmp = TempDir::new();
        let addons_dir = tmp.path().join("Interface").join("AddOns");
        for folder in folders {
            fs::create_dir_all(addons_dir.join(folder)).unwrap();
            fs::write(addons_dir.join(folder).join(format!("{folder}.toc")), "").unwrap();
        }
        Disk {
            _tmp: tmp,
            addons_dir,
        }
    }

    fn on_disk(disk: &Disk) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(&disk.addons_dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    /// Installa scrivendo la cartella `Addon<key>`; le chiavi in `fail` falliscono.
    struct FakeInstaller<'a> {
        addons_dir: &'a Path,
        fail: Vec<&'static str>,
        calls: Vec<String>,
    }

    impl Installer for FakeInstaller<'_> {
        fn install(&mut self, entry: &ListEntry) -> EntryOutcome {
            self.calls.push(entry.key.clone());
            if self.fail.contains(&entry.key.as_str()) {
                return EntryOutcome::new(entry, EntryStatus::Error, Some("boom".to_owned()));
            }
            fs::create_dir_all(self.addons_dir.join(format!("Addon{}", entry.key))).unwrap();
            EntryOutcome::new(entry, EntryStatus::Installed, None)
        }
    }

    fn state(addons: Vec<ListedAddon>, updates: Vec<AddonUpdate>) -> Refresh {
        Refresh {
            addons,
            updates,
            check_error: None,
        }
    }

    #[test]
    fn the_preview_sorts_entries_and_lists_what_replace_would_remove() {
        let refresh = state(
            vec![
                listed("Details", &[]),
                listed("DBM-Core", &["!BugGrabber"]),
                listed("!BugGrabber", &[]),
                listed("ZipAddon", &[]),
                listed("Library", &[]),
                listed("Recorded", &[]),
            ],
            vec![
                matched("Details", "61284", &[]),
                matched("DBM-Core", "3358", &["Library"]),
            ],
        );
        let records = SourceRecords::from([(
            SourceKind::Tukui,
            ProjectRecords::from([(
                "-2".to_owned(),
                SourceRecord {
                    file: "16.00".to_owned(),
                    folders: vec!["recorded".to_owned()],
                },
            )]),
        )]);
        let list = decoded(vec![
            entry(SourceKind::Curseforge, "3358"),
            entry(SourceKind::Tukui, "-2"),
            entry(SourceKind::Curseforge, "1"),
            entry(SourceKind::Curseforge, "2"),
        ]);
        let preview = preview(
            &list,
            &refresh,
            &records,
            &|entry| {
                if entry.key == "2" {
                    Availability::NotDownloadable {
                        message: "not allowed".to_owned(),
                        url: Some("https://www.curseforge.com/wow/addons/two".to_owned()),
                    }
                } else {
                    Availability::Available
                }
            },
            None,
        );
        let states: Vec<(&str, EntryState)> = preview
            .entries
            .iter()
            .map(|e| (e.key.as_str(), e.state))
            .collect();
        assert_eq!(
            states,
            [
                ("3358", EntryState::Installed),
                ("-2", EntryState::Installed),
                ("1", EntryState::New),
                ("2", EntryState::NotDownloadable),
            ]
        );
        assert_eq!(
            preview.entries[3].url.as_deref(),
            Some("https://www.curseforge.com/wow/addons/two")
        );
        // DBM resta con la sua libreria e la sua dipendenza dichiarata; Details e lo zip no.
        assert_eq!(preview.removals, ["Details", "ZipAddon"]);
    }

    #[test]
    fn an_empty_list_never_proposes_removals() {
        let refresh = state(vec![listed("Details", &[])], Vec::new());
        let preview = preview(
            &decoded(Vec::new()),
            &refresh,
            &SourceRecords::new(),
            &|_| Availability::Available,
            None,
        );
        assert!(preview.removals.is_empty());
    }

    #[test]
    fn keep_installs_what_is_missing_and_touches_nothing_else() {
        let disk = disk(&["Details", "Other"]);
        let before = state(
            vec![listed("Details", &[]), listed("Other", &[])],
            vec![matched("Details", "61284", &[])],
        );
        let mut installer = FakeInstaller {
            addons_dir: &disk.addons_dir,
            fail: Vec::new(),
            calls: Vec::new(),
        };
        let result = run_import(
            &disk.addons_dir,
            &decoded(vec![
                entry(SourceKind::Curseforge, "61284"),
                entry(SourceKind::Curseforge, "5"),
            ]),
            ImportMode::Keep,
            &["Other".to_owned()],
            &before,
            &SourceRecords::new(),
            &mut installer,
            &mut || panic!("keep does not read the disk again"),
        );
        assert_eq!(installer.calls, ["5"]);
        let statuses: Vec<EntryStatus> = result.entries.iter().map(|e| e.status).collect();
        assert_eq!(
            statuses,
            [EntryStatus::AlreadyInstalled, EntryStatus::Installed]
        );
        assert!(result.removed.is_empty());
        assert_eq!(on_disk(&disk), ["Addon5", "Details", "Other"]);
    }

    #[test]
    fn replace_removes_only_confirmed_and_unclaimed_folders() {
        let disk = disk(&["Details", "Other", "Late", "Dep"]);
        let before = state(
            vec![
                listed("Details", &[]),
                listed("Other", &[]),
                listed("Dep", &[]),
            ],
            vec![matched("Details", "61284", &[])],
        );
        let mut installer = FakeInstaller {
            addons_dir: &disk.addons_dir,
            fail: Vec::new(),
            calls: Vec::new(),
        };
        let addons_dir = disk.addons_dir.clone();
        let result = run_import(
            &disk.addons_dir,
            &decoded(vec![
                entry(SourceKind::Curseforge, "61284"),
                entry(SourceKind::Curseforge, "5"),
            ]),
            ImportMode::Replace,
            // "Late" è comparsa dopo l'anteprima; "Dep" è confermata ma ora è una dipendenza.
            &["other".to_owned(), "Dep".to_owned(), "Details".to_owned()],
            &before,
            &SourceRecords::new(),
            &mut installer,
            &mut || {
                assert!(addons_dir.join("Addon5").is_dir());
                Ok((
                    state(
                        vec![
                            listed("Addon5", &["Dep"]),
                            listed("Dep", &[]),
                            listed("Details", &[]),
                            listed("Late", &[]),
                            listed("Other", &[]),
                        ],
                        vec![
                            matched("Details", "61284", &[]),
                            matched("Addon5", "5", &[]),
                        ],
                    ),
                    SourceRecords::new(),
                ))
            },
        );
        assert_eq!(result.removed, ["Other"]);
        assert_eq!(result.not_confirmed, ["Late"]);
        assert!(result.removal_errors.is_empty());
        assert_eq!(on_disk(&disk), ["Addon5", "Dep", "Details", "Late"]);
    }

    #[test]
    fn replace_removes_nothing_when_every_install_fails() {
        let disk = disk(&["Other"]);
        let before = state(vec![listed("Other", &[])], Vec::new());
        let mut installer = FakeInstaller {
            addons_dir: &disk.addons_dir,
            fail: vec!["5", "6"],
            calls: Vec::new(),
        };
        let result = run_import(
            &disk.addons_dir,
            &decoded(vec![
                entry(SourceKind::Curseforge, "5"),
                entry(SourceKind::Curseforge, "6"),
            ]),
            ImportMode::Replace,
            &["Other".to_owned()],
            &before,
            &SourceRecords::new(),
            &mut installer,
            &mut || panic!("nothing was installed: no rescan"),
        );
        assert!(result.removed.is_empty());
        assert_eq!(
            result.removal_skipped.as_deref(),
            Some("No addon could be installed, so no folder was removed.")
        );
        assert_eq!(on_disk(&disk), ["Other"]);
    }

    #[test]
    fn replace_with_everything_present_still_removes_the_rest() {
        let disk = disk(&["Details", "Other"]);
        let before = state(
            vec![listed("Details", &[]), listed("Other", &[])],
            vec![matched("Details", "61284", &[])],
        );
        let after = before.clone();
        let mut installer = FakeInstaller {
            addons_dir: &disk.addons_dir,
            fail: Vec::new(),
            calls: Vec::new(),
        };
        let result = run_import(
            &disk.addons_dir,
            &decoded(vec![entry(SourceKind::Curseforge, "61284")]),
            ImportMode::Replace,
            &["Other".to_owned()],
            &before,
            &SourceRecords::new(),
            &mut installer,
            &mut || Ok((after.clone(), SourceRecords::new())),
        );
        assert!(installer.calls.is_empty());
        assert_eq!(result.removed, ["Other"]);
        assert_eq!(on_disk(&disk), ["Details"]);
    }

    #[test]
    fn hostile_confirmed_names_never_reach_outside_addons() {
        let disk = disk(&["Details"]);
        let outside = disk.addons_dir.parent().unwrap().join("Keep");
        fs::create_dir_all(&outside).unwrap();
        // Una scansione falsata che restituisce nomi ostili come cartelle da togliere.
        let hostile = state(
            vec![
                listed("..", &[]),
                listed("../Keep", &[]),
                listed("a/b", &[]),
                listed("Details", &[]),
            ],
            vec![matched("Details", "61284", &[])],
        );
        let mut installer = FakeInstaller {
            addons_dir: &disk.addons_dir,
            fail: Vec::new(),
            calls: Vec::new(),
        };
        let result = run_import(
            &disk.addons_dir,
            &decoded(vec![entry(SourceKind::Curseforge, "61284")]),
            ImportMode::Replace,
            &["..".to_owned(), "../Keep".to_owned(), "a/b".to_owned()],
            &hostile,
            &SourceRecords::new(),
            &mut installer,
            &mut || Ok((hostile.clone(), SourceRecords::new())),
        );
        assert!(result.removed.is_empty());
        assert_eq!(result.removal_errors.len(), 3);
        assert!(outside.is_dir());
        assert!(disk.addons_dir.join("Details").is_dir());
    }

    #[test]
    fn an_empty_list_with_replace_removes_nothing() {
        let disk = disk(&["Other"]);
        let before = state(vec![listed("Other", &[])], Vec::new());
        let mut installer = FakeInstaller {
            addons_dir: &disk.addons_dir,
            fail: Vec::new(),
            calls: Vec::new(),
        };
        let result = run_import(
            &disk.addons_dir,
            &decoded(Vec::new()),
            ImportMode::Replace,
            &["Other".to_owned()],
            &before,
            &SourceRecords::new(),
            &mut installer,
            &mut || panic!("no rescan"),
        );
        assert!(result.removal_skipped.is_some());
        assert_eq!(on_disk(&disk), ["Other"]);
    }
}
