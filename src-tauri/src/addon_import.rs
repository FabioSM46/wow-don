//! Import of an addon string (issue #134): the preview and the execution, with the two modes
//! chosen by the user. *Keep my addons* installs only the missing projects; *Replace all*
//! installs the missing ones and then removes from `Interface/AddOns` the folders that no
//! imported project claims.
//!
//! The order "install first, then remove" is deliberate: an import that fails halfway does not
//! leave the installation empty, and if nothing gets installed (and something was missing)
//! nothing is removed. *Replace all* makes no backup (the user's choice): it removes only the
//! folders the frontend showed and the user confirmed (`confirmed`), and only if they still need
//! removing after the installations. A folder declared as a dependency in the `.toc` of an addon
//! that stays also stays. `WTF`, the SavedVariables and everything outside `AddOns` are never
//! touched.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::addon_list::{Decoded, InvalidEntry, ListEntry};
use crate::install;
use crate::store::{SourceKind, SourceRecords};
use crate::updates::Refresh;

/// A project: source and key.
type ProjectId = (SourceKind, String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImportMode {
    /// Adds the missing projects, removes nothing.
    Keep,
    /// Adds the missing projects and removes the folders none of them claims.
    Replace,
}

/// Whether a project in the string can be installed, before trying.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Availability {
    Available,
    /// The author does not allow downloads from third-party apps, the source does not list it or
    /// is not supported yet: the reason and, if there is one, the addon's page.
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

/// The preview of an import: it writes nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Preview {
    /// The flavor of the installation the string comes from.
    pub flavor: String,
    pub entries: Vec<PreviewEntry>,
    pub invalid: Vec<InvalidEntry>,
    /// The folders *Replace all* would remove now, with the name they have on disk.
    pub removals: Vec<String>,
    /// Why the check of the installed addons did not succeed: without it, a project that is
    /// installed but not recognised shows up as new.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub check_error: Option<String>,
    /// Why it could not be determined whether the new projects can be downloaded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lookup_error: Option<String>,
}

/// The projects present in the installation: the ones the check recognises and the ones the app
/// recorded with at least one folder still on disk.
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

/// The folders *Replace all* would remove if only the `keep` projects stayed: all those on disk
/// that none of them claims (from the check, from the recognised files or from the records),
/// neither directly nor as a dependency declared in the `.toc` of a folder that stays. In scan
/// order, with the name they have on disk.
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
    // The dependencies of what stays also stay, including the dependencies' dependencies.
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

/// Lowercase folder → position in the scan.
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

/// The preview of `decoded` for an installation read in `refresh`. `availability` says whether a
/// new project can be downloaded.
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

/// The outcome of a project in the string.
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

/// A folder that *Replace all* was supposed to remove and did not.
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
    /// The folders removed by *Replace all*.
    pub removed: Vec<String>,
    /// Folders that would need removing after the installations but that the user had not
    /// confirmed (for example ones that appeared after the preview): they stay.
    pub not_confirmed: Vec<String>,
    pub removal_errors: Vec<RemovalError>,
    /// Why *Replace all* removed nothing, when it did not even try.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub removal_skipped: Option<String>,
    /// Why it could not be determined beforehand whether the projects can be downloaded: the
    /// outcomes of the individual projects then say what happened.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lookup_error: Option<String>,
}

/// How many entries are prepared before installing them: the zips of a group stay in memory
/// until their turn comes, so a long list is done in groups.
pub const ENTRIES_PER_GROUP: usize = 4 * crate::parallel::DOWNLOADS_AT_ONCE;

/// What installs the projects of the string, in two phases (issue #180). In the command it is the
/// real sources; in the tests a fake one.
pub trait Installer {
    /// The first phase for a group of entries to install: resolve and download, possibly in
    /// parallel, without touching `AddOns` or the records. What fails, `install` will report.
    fn prepare(&mut self, entries: &[&ListEntry]) {
        let _ = entries;
    }

    /// The second phase for one entry: install it, with what `prepare` already downloaded.
    /// It is called one entry at a time, in list order.
    fn install(&mut self, entry: &ListEntry) -> EntryOutcome;
}

/// Runs the import. `before` and `records` are the state before the installations; `rescan`
/// rereads disk and records afterwards, for *Replace all*. The removed folders must then be
/// forgotten by the caller (records and fingerprint cache).
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
    let list = &decoded.list.addons;
    let mut outcomes: Vec<Option<EntryOutcome>> = list
        .iter()
        .map(|entry| {
            installed
                .contains(&id_of(entry))
                .then(|| EntryOutcome::new(entry, EntryStatus::AlreadyInstalled, None))
        })
        .collect();
    let pending: Vec<usize> = (0..list.len()).filter(|i| outcomes[*i].is_none()).collect();
    for group in pending.chunks(ENTRIES_PER_GROUP) {
        let entries: Vec<&ListEntry> = group.iter().map(|i| &list[*i]).collect();
        installer.prepare(&entries);
        for (index, entry) in group.iter().zip(entries) {
            outcomes[*index] = Some(installer.install(entry));
        }
    }
    let entries: Vec<EntryOutcome> = outcomes.into_iter().flatten().collect();
    let mut result = ImportResult {
        entries,
        invalid: decoded.invalid.clone(),
        removed: Vec::new(),
        not_confirmed: Vec::new(),
        removal_errors: Vec::new(),
        removal_skipped: None,
        lookup_error: None,
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
            reinstall_file: None,
            mirror: Default::default(),
        }
    }

    /// An installation on disk with the folders of `addons`, and the state read as the scan would
    /// give it: `updates` recognises the projects.
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

    /// Installs by writing the folder `Addon<key>`; the keys in `fail` fail. `calls` are the
    /// installations, `prepared` the prepared groups, in order.
    struct FakeInstaller<'a> {
        addons_dir: &'a Path,
        fail: Vec<&'static str>,
        calls: Vec<String>,
        prepared: Vec<Vec<String>>,
    }

    impl<'a> FakeInstaller<'a> {
        fn new(addons_dir: &'a Path, fail: Vec<&'static str>) -> Self {
            Self {
                addons_dir,
                fail,
                calls: Vec::new(),
                prepared: Vec::new(),
            }
        }
    }

    impl Installer for FakeInstaller<'_> {
        fn prepare(&mut self, entries: &[&ListEntry]) {
            // The first phase does not touch the disk.
            assert!(entries.iter().all(|e| !self.calls.contains(&e.key)));
            self.prepared
                .push(entries.iter().map(|e| e.key.clone()).collect());
        }

        fn install(&mut self, entry: &ListEntry) -> EntryOutcome {
            assert!(
                self.prepared.iter().flatten().any(|key| *key == entry.key),
                "installed before it was prepared"
            );
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
        // DBM stays with its library and its declared dependency; Details and the zip do not.
        assert_eq!(preview.removals, ["Details", "ZipAddon"]);
    }

    #[test]
    fn a_required_dependency_from_the_toc_stays() {
        // From the real text of the `.toc`: `## RequiredDeps` is an alias of `## Dependencies`.
        let mut core = listed("DBM-Core", &[]);
        core.addon.toc = crate::addons::toc::parse_toc(
            "## Title: DBM\n## RequiredDeps: LibFoo\n## OptionalDeps: LibBar\n",
        );
        let refresh = state(
            vec![
                core,
                listed("LibFoo", &[]),
                listed("LibBar", &[]),
                listed("Other", &[]),
            ],
            vec![matched("DBM-Core", "3358", &[])],
        );
        let keep = HashSet::from([(SourceKind::Curseforge, "3358".to_owned())]);
        assert_eq!(
            removal_candidates(&refresh, &SourceRecords::new(), &keep),
            ["Other"]
        );
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
        let mut installer = FakeInstaller::new(&disk.addons_dir, Vec::new());
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
    fn entries_are_prepared_in_groups_then_installed_in_list_order() {
        let disk = disk(&["Details"]);
        let before = state(
            vec![listed("Details", &[])],
            vec![matched("Details", "61284", &[])],
        );
        let count = ENTRIES_PER_GROUP + 2;
        let keys: Vec<String> = (1..=count).map(|n| n.to_string()).collect();
        let mut list: Vec<ListEntry> = keys
            .iter()
            .map(|key| entry(SourceKind::Curseforge, key))
            .collect();
        // One already installed in the middle: it is neither prepared nor installed.
        list.insert(3, entry(SourceKind::Curseforge, "61284"));
        let mut installer = FakeInstaller::new(&disk.addons_dir, vec!["2"]);
        let result = run_import(
            &disk.addons_dir,
            &decoded(list.clone()),
            ImportMode::Keep,
            &[],
            &before,
            &SourceRecords::new(),
            &mut installer,
            &mut || panic!("keep does not read the disk again"),
        );
        assert_eq!(installer.calls, keys);
        assert_eq!(
            installer.prepared,
            [
                keys[..ENTRIES_PER_GROUP].to_vec(),
                keys[ENTRIES_PER_GROUP..].to_vec()
            ]
        );
        // The outcomes follow the list, with the error in its place.
        let outcomes: Vec<(&str, EntryStatus)> = result
            .entries
            .iter()
            .map(|e| (e.key.as_str(), e.status))
            .collect();
        let expected: Vec<(&str, EntryStatus)> = list
            .iter()
            .map(|e| {
                let status = match e.key.as_str() {
                    "61284" => EntryStatus::AlreadyInstalled,
                    "2" => EntryStatus::Error,
                    _ => EntryStatus::Installed,
                };
                (e.key.as_str(), status)
            })
            .collect();
        assert_eq!(outcomes, expected);
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
        let mut installer = FakeInstaller::new(&disk.addons_dir, Vec::new());
        let addons_dir = disk.addons_dir.clone();
        let result = run_import(
            &disk.addons_dir,
            &decoded(vec![
                entry(SourceKind::Curseforge, "61284"),
                entry(SourceKind::Curseforge, "5"),
            ]),
            ImportMode::Replace,
            // "Late" appeared after the preview; "Dep" is confirmed but is now a dependency.
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
        let mut installer = FakeInstaller::new(&disk.addons_dir, vec!["5", "6"]);
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
        let mut installer = FakeInstaller::new(&disk.addons_dir, Vec::new());
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
        // A rigged scan that returns hostile names as folders to remove.
        let hostile = state(
            vec![
                listed("..", &[]),
                listed("../Keep", &[]),
                listed("a/b", &[]),
                listed("Details", &[]),
            ],
            vec![matched("Details", "61284", &[])],
        );
        let mut installer = FakeInstaller::new(&disk.addons_dir, Vec::new());
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
        let mut installer = FakeInstaller::new(&disk.addons_dir, Vec::new());
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
