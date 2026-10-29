//! Removal of one or more addons from the Installed tab (issue #147).
//!
//! Each folder is removed with [`install::remove_folder`], the same function as the import's
//! *Replace all*: a single valid component inside `AddOns`, symbolic links are removed without
//! following them, a file that is not a folder stays where it is. A folder that cannot be removed
//! gives an error outcome and does not stop the others. No backup (the user's choice); `WTF`, the
//! SavedVariables and everything outside `AddOns` are never touched.
//!
//! The removed folders leave the sources registry ([`InstalledAddonsStore::forget_folders`])
//! and the fingerprint cache ([`FingerprintCache::forget`]).

use std::collections::HashSet;
use std::path::Path;
use std::sync::Mutex;

use serde::Serialize;

use crate::addons::cache::FingerprintCache;
use crate::install;
use crate::store::InstalledAddonsStore;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RemovalStatus {
    Removed,
    Error,
}

/// The outcome of a requested folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderRemoval {
    pub folder: String,
    pub status: RemovalStatus,
    /// Why the folder was not removed (in English), only with `error`.
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoveResult {
    /// One outcome per folder, in the requested order; a repeated name counts only once. The name
    /// is the one from the scan, so it is compared exactly: on Linux `Alpha` and `alpha` can be
    /// two folders.
    pub folders: Vec<FolderRemoval>,
    /// The folders are removed but the registry was not updated: a warning, not a failure.
    pub warning: Option<String>,
}

/// Removes the folders `folders` from `addons_dir`, then updates registry and cache for the removed
/// ones. The registry is locked only after the work on disk.
pub fn remove_addons(
    addons_dir: &Path,
    installation_id: &str,
    folders: &[String],
    installed: &Mutex<InstalledAddonsStore>,
    cache: &FingerprintCache,
) -> RemoveResult {
    let mut seen = HashSet::new();
    let outcomes: Vec<FolderRemoval> = folders
        .iter()
        .filter(|folder| seen.insert(folder.as_str()))
        .map(|folder| match install::remove_folder(addons_dir, folder) {
            Ok(()) => FolderRemoval {
                folder: folder.clone(),
                status: RemovalStatus::Removed,
                message: None,
            },
            Err(message) => FolderRemoval {
                folder: folder.clone(),
                status: RemovalStatus::Error,
                message: Some(message),
            },
        })
        .collect();
    let removed: Vec<String> = outcomes
        .iter()
        .filter(|o| o.status == RemovalStatus::Removed)
        .map(|o| o.folder.clone())
        .collect();
    let mut warning = None;
    if !removed.is_empty() {
        cache.forget(addons_dir, &removed);
        let forgotten = installed
            .lock()
            .map_err(|_| "Installed addons are not available: restart the app.".to_owned())
            .and_then(|mut store| store.forget_folders(installation_id, &removed));
        if let Err(error) = forgotten {
            warning = Some(format!(
                "Removed, but the app could not update its records: {error}"
            ));
        }
    }
    RemoveResult {
        folders: outcomes,
        warning,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{INSTALLED_ADDONS_FILE, SourceKind, SourceRecord};
    use crate::test_support::TempDir;
    use std::fs;
    use std::path::PathBuf;

    const ID: &str = "inst-1";

    fn addon(addons: &Path, folder: &str) -> PathBuf {
        let dir = addons.join(folder);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(format!("{folder}.toc")), "core.lua\n").unwrap();
        fs::write(dir.join("core.lua"), folder).unwrap();
        dir
    }

    struct Setup {
        tmp: TempDir,
        installed: Mutex<InstalledAddonsStore>,
        cache: FingerprintCache,
    }

    impl Setup {
        fn new() -> Self {
            let tmp = TempDir::new();
            fs::create_dir_all(addons_dir(&tmp)).unwrap();
            let installed = Mutex::new(InstalledAddonsStore::load(
                tmp.path().join("config").join(INSTALLED_ADDONS_FILE),
            ));
            Self {
                tmp,
                installed,
                cache: FingerprintCache::default(),
            }
        }

        fn addons(&self) -> PathBuf {
            addons_dir(&self.tmp)
        }

        fn record(&self, key: &str, folders: &[&str]) {
            self.installed
                .lock()
                .unwrap()
                .record_source(
                    ID,
                    SourceKind::Curseforge,
                    key,
                    SourceRecord {
                        folders: folders.iter().map(|f| (*f).to_owned()).collect(),
                        file: format!("{key}00"),
                    },
                )
                .unwrap();
        }

        fn recorded_keys(&self) -> Vec<String> {
            let records = self
                .installed
                .lock()
                .unwrap()
                .source_records_for(ID)
                .unwrap();
            let mut keys: Vec<String> = records
                .values()
                .flat_map(|projects| projects.keys().cloned())
                .collect();
            keys.sort();
            keys
        }

        fn remove(&self, folders: &[&str]) -> RemoveResult {
            let folders: Vec<String> = folders.iter().map(|f| (*f).to_owned()).collect();
            remove_addons(&self.addons(), ID, &folders, &self.installed, &self.cache)
        }
    }

    fn addons_dir(tmp: &TempDir) -> PathBuf {
        tmp.path().join("Interface").join("AddOns")
    }

    fn statuses(result: &RemoveResult) -> Vec<(&str, RemovalStatus)> {
        result
            .folders
            .iter()
            .map(|o| (o.folder.as_str(), o.status))
            .collect()
    }

    #[test]
    fn removes_several_folders_and_keeps_the_rest() {
        let setup = Setup::new();
        let addons = setup.addons();
        for folder in ["Alpha", "Beta", "Gamma"] {
            addon(&addons, folder);
        }
        let result = setup.remove(&["Alpha", "Gamma"]);
        assert_eq!(
            statuses(&result),
            [
                ("Alpha", RemovalStatus::Removed),
                ("Gamma", RemovalStatus::Removed)
            ]
        );
        assert!(result.folders.iter().all(|o| o.message.is_none()));
        assert_eq!(result.warning, None);
        assert!(!addons.join("Alpha").exists());
        assert!(addons.join("Beta").join("Beta.toc").is_file());
        assert!(!addons.join("Gamma").exists());
    }

    #[test]
    fn invalid_names_fail_without_stopping_the_others() {
        let setup = Setup::new();
        let addons = setup.addons();
        addon(&addons, "Alpha");
        addon(&addons, "Omega");
        // A folder next to AddOns that a hostile name would try to reach.
        let outside = addon(&setup.tmp.path().join("Interface"), "Outside");
        let result = setup.remove(&[
            "",
            "..",
            ".",
            "../Outside",
            "Alpha/../Omega",
            "Alpha\\core.lua",
            "Alpha ",
            "C:Alpha",
            "Alpha",
        ]);
        let errors: Vec<&str> = result
            .folders
            .iter()
            .filter(|o| o.status == RemovalStatus::Error)
            .map(|o| o.folder.as_str())
            .collect();
        assert_eq!(
            errors,
            [
                "",
                "..",
                ".",
                "../Outside",
                "Alpha/../Omega",
                "Alpha\\core.lua",
                "Alpha ",
                "C:Alpha"
            ]
        );
        assert!(
            result
                .folders
                .iter()
                .filter(|o| o.status == RemovalStatus::Error)
                .all(|o| o.message.as_deref().is_some_and(|m| !m.is_empty()))
        );
        assert_eq!(
            result.folders.last().unwrap().status,
            RemovalStatus::Removed
        );
        assert!(!addons.join("Alpha").exists());
        assert!(addons.join("Omega").join("Omega.toc").is_file());
        assert!(outside.join("Outside.toc").is_file());
        assert!(addons.is_dir());
    }

    #[test]
    fn a_missing_folder_or_a_file_is_an_error() {
        let setup = Setup::new();
        let addons = setup.addons();
        fs::write(addons.join("Notes.txt"), "keep me").unwrap();
        addon(&addons, "Alpha");
        let result = setup.remove(&["Missing", "Notes.txt", "Alpha"]);
        assert_eq!(
            statuses(&result),
            [
                ("Missing", RemovalStatus::Error),
                ("Notes.txt", RemovalStatus::Error),
                ("Alpha", RemovalStatus::Removed)
            ]
        );
        assert_eq!(
            fs::read_to_string(addons.join("Notes.txt")).unwrap(),
            "keep me"
        );
        assert!(!addons.join("Alpha").exists());
    }

    #[test]
    fn wtf_stays_untouched() {
        let setup = Setup::new();
        let addons = setup.addons();
        addon(&addons, "Alpha");
        let saved = setup
            .tmp
            .path()
            .join("WTF")
            .join("Account")
            .join("SavedVariables");
        fs::create_dir_all(&saved).unwrap();
        fs::write(saved.join("Alpha.lua"), "AlphaDB = {}").unwrap();
        let result = setup.remove(&["Alpha"]);
        assert_eq!(statuses(&result), [("Alpha", RemovalStatus::Removed)]);
        assert_eq!(
            fs::read_to_string(saved.join("Alpha.lua")).unwrap(),
            "AlphaDB = {}"
        );
    }

    #[test]
    fn a_repeated_name_counts_once() {
        let setup = Setup::new();
        addon(&setup.addons(), "Alpha");
        let result = setup.remove(&["Alpha", "Alpha"]);
        assert_eq!(statuses(&result), [("Alpha", RemovalStatus::Removed)]);
    }

    #[test]
    fn removed_folders_leave_the_records_and_the_cache() {
        let setup = Setup::new();
        let addons = setup.addons();
        let alpha = addon(&addons, "Alpha");
        let beta = addon(&addons, "Beta");
        setup.record("1", &["Alpha", "Alpha_Options"]);
        setup.record("2", &["Beta"]);
        setup.cache.fingerprint(&alpha);
        setup.cache.fingerprint(&beta);
        assert_eq!(setup.cache.len(), 2);

        // A missing folder does not remove the record that names it.
        let result = setup.remove(&["Alpha", "Missing"]);
        assert_eq!(
            statuses(&result),
            [
                ("Alpha", RemovalStatus::Removed),
                ("Missing", RemovalStatus::Error)
            ]
        );
        assert_eq!(setup.recorded_keys(), ["2"]);
        assert_eq!(setup.cache.len(), 1);
        setup.cache.fingerprint(&beta);
        assert_eq!(setup.cache.len(), 1, "Beta stays cached");
    }

    #[test]
    fn nothing_removed_leaves_records_alone() {
        let setup = Setup::new();
        setup.record("1", &["Alpha"]);
        let result = setup.remove(&["Alpha"]);
        assert_eq!(statuses(&result), [("Alpha", RemovalStatus::Error)]);
        assert_eq!(setup.recorded_keys(), ["1"]);
        assert_eq!(result.warning, None);
    }

    #[test]
    fn the_result_is_camel_case() {
        let result = RemoveResult {
            folders: vec![FolderRemoval {
                folder: "Alpha".to_owned(),
                status: RemovalStatus::Removed,
                message: None,
            }],
            warning: None,
        };
        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "folders": [{"folder": "Alpha", "status": "removed", "message": null}],
                "warning": null
            })
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_is_removed_without_following_it() {
        let setup = Setup::new();
        let addons = setup.addons();
        let target = addon(setup.tmp.path(), "Elsewhere");
        std::os::unix::fs::symlink(&target, addons.join("Linked")).unwrap();
        let result = setup.remove(&["Linked"]);
        assert_eq!(statuses(&result), [("Linked", RemovalStatus::Removed)]);
        assert!(fs::symlink_metadata(addons.join("Linked")).is_err());
        assert!(target.join("Elsewhere.toc").is_file());
    }
}
