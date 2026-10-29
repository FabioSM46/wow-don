//! The previous version from the last automatic round and its Undo (D41, issue #201).
//!
//! Only in automatic rounds in `install` mode are the folders an update replaces not deleted:
//! [`NextGeneration`] collects them (with `install::keeping_replaced`) in a work folder next to
//! `AddOns` and, if the round installed at least one project, renames it to
//! `Interface/.wow-don-previous/` in place of the previous generation. There is a single
//! generation per installation: inside, one numbered subfolder (the "slot") per project.
//! Manual updates do not go through here and delete as before.
//!
//! The manifest [`PREVIOUS_FILE`] in the app's config folder (D22, atomic write) records, per
//! installation, the time of the round and, per project, source, key, previous record, record
//! written by the round, slot and saved folders. [`undo`] uses it to put back the folders and
//! the records; a project whose record is no longer the one written by the round is skipped.
//!
//! Disk and manifest are written only under `InstallLock` (the round and
//! `undo_last_auto_update`): the manifest is re-read on every use and has no in-memory state.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::addons::cache::FingerprintCache;
use crate::install::{self, WorkDir};
use crate::installations::Installation;
use crate::sources::RecordWriter;
use crate::store::{self, InstalledAddonsStore, SourceKind, SourceRecord, SourceRecords};
use crate::updates::UpdateOutcome;

/// The generations manifest, in the app's config folder.
pub const PREVIOUS_FILE: &str = "auto-update-previous.json";

/// The generation folder, in `Interface` (same disk as `AddOns`).
pub const GENERATION_DIR: &str = ".wow-don-previous";

const VERSION: u32 = 1;

/// A project updated by an automatic round, with what is needed to put it back as it was.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviousProject {
    pub source: SourceKind,
    pub key: String,
    pub name: String,
    /// The record before the round; `null` if the project had none (recognised only by
    /// fingerprint): Undo then removes the record.
    pub previous: Option<SourceRecord>,
    /// The record written by the round: Undo proceeds only if it is still the current one.
    pub current: SourceRecord,
    /// The generation subfolder holding the saved folders.
    pub slot: String,
    /// The saved folders, with the name they had in `AddOns`.
    pub saved: Vec<String>,
}

/// The last undoable automatic round of an installation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviousRound {
    /// The time of the round, ISO 8601 UTC.
    pub round_at: String,
    /// The projects, in the order the round updated them.
    pub projects: Vec<PreviousProject>,
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredPrevious {
    version: u32,
    installations: BTreeMap<String, PreviousRound>,
}

/// The [`PREVIOUS_FILE`] manifest. It is re-read on every use; an unreadable file blocks Undo
/// instead of being overwritten, and counts as "nothing to undo" for the status.
#[derive(Debug)]
pub struct PreviousManifest {
    file: PathBuf,
}

impl PreviousManifest {
    pub fn new(file: PathBuf) -> Self {
        Self { file }
    }

    /// The generations per installation; empty without a file.
    pub fn read(&self) -> Result<BTreeMap<String, PreviousRound>, String> {
        let damaged = || {
            format!(
                "{PREVIOUS_FILE} is damaged: move it away to forget the automatic update that could be undone."
            )
        };
        let text = match store::read_text(&self.file) {
            Ok(Some(text)) => text,
            Ok(None) => return Ok(BTreeMap::new()),
            Err(error) => {
                return Err(format!(
                    "Could not read {PREVIOUS_FILE} ({}).",
                    error.kind()
                ));
            }
        };
        let stored: StoredPrevious = serde_json::from_str(&text).map_err(|_| damaged())?;
        if stored.version != VERSION {
            return Err(format!(
                "{PREVIOUS_FILE} has an unsupported version ({}): update the app or move the file away.",
                stored.version
            ));
        }
        Ok(stored.installations)
    }

    /// Replaces (`Some`) or removes (`None`) the generation of an installation.
    pub fn set(&self, installation_id: &str, round: Option<PreviousRound>) -> Result<(), String> {
        let mut installations = self.read()?;
        match round {
            Some(round) => {
                installations.insert(installation_id.to_owned(), round);
            }
            None => {
                if installations.remove(installation_id).is_none() {
                    return Ok(());
                }
            }
        }
        let stored = StoredPrevious {
            version: VERSION,
            installations,
        };
        store::write_atomically(&self.file, &stored).map_err(|error| {
            format!(
                "Could not save the automatic update that can be undone ({}).",
                error.kind()
            )
        })
    }
}

/// An undoable round, for `get_auto_update_status` (`undoable`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UndoableRound {
    pub installation_id: String,
    /// The time of the round, ISO 8601 UTC.
    pub round_at: String,
    /// How many projects (addons) Undo would put back.
    pub addons: usize,
}

/// The undoable rounds of the installations `installation_ids`, in their order. An unreadable
/// manifest counts as "nothing to undo": Undo reports the error.
pub fn undoable(manifest: &PreviousManifest, installation_ids: &[String]) -> Vec<UndoableRound> {
    let Ok(rounds) = manifest.read() else {
        return Vec::new();
    };
    installation_ids
        .iter()
        .filter_map(|id| {
            let round = rounds.get(id).filter(|round| !round.projects.is_empty())?;
            Some(UndoableRound {
                installation_id: id.clone(),
                round_at: round.round_at.clone(),
                addons: round.projects.len(),
            })
        })
        .collect()
}

fn interface_dir(installation: &Installation) -> PathBuf {
    installation.path.join("Interface")
}

fn addons_dir(installation: &Installation) -> PathBuf {
    interface_dir(installation).join("AddOns")
}

// ── Round: the new generation ───────────────────────────────────────────────

/// The generation an automatic round prepares for an installation.
pub struct NextGeneration {
    addons_dir: PathBuf,
    generation: PathBuf,
    /// Created on the first [`run`](Self::run); deletes itself if the round installs nothing.
    dir: Option<WorkDir>,
    error: Option<String>,
    projects: Vec<PreviousProject>,
    /// The updated projects whose previous version was not kept in full (or did not reach this
    /// thread): they cannot be undone, and [`commit`](Self::commit) says so.
    not_kept: Vec<(SourceKind, String)>,
}

impl NextGeneration {
    pub fn new(installation: &Installation) -> Self {
        Self {
            addons_dir: addons_dir(installation),
            generation: interface_dir(installation).join(GENERATION_DIR),
            dir: None,
            error: None,
            projects: Vec::new(),
            not_kept: Vec::new(),
        }
    }

    /// Runs `run` (the round's update) keeping the replaced folders. `record` is the real
    /// recorder; `before` are the installation's records before the round. A project enters
    /// the generation only if its record was saved and all its replaced folders were kept.
    /// If the work folder cannot be created the update proceeds like a manual update, and
    /// [`commit`](Self::commit) returns the error.
    pub fn run<R>(
        &mut self,
        before: &SourceRecords,
        record: &mut RecordWriter<'_>,
        run: impl FnOnce(&mut RecordWriter<'_>) -> R,
    ) -> R {
        if self.dir.is_none() && self.error.is_none() {
            match WorkDir::create(&self.addons_dir, "next") {
                Ok(dir) => self.dir = Some(dir),
                Err(error) => {
                    self.error = Some(format!("Could not keep the previous versions: {error}"))
                }
            }
        }
        let Some(dir) = &self.dir else {
            return run(record);
        };
        let projects = &mut self.projects;
        let not_kept = &mut self.not_kept;
        install::keeping_replaced(dir.path(), || {
            let mut keeping =
                |source: SourceKind, key: &str, new: SourceRecord| -> Result<(), String> {
                    let kept = install::take_kept();
                    let previous = before.get(&source).and_then(|p| p.get(key)).cloned();
                    record(source, key, new.clone())?;
                    match kept.filter(|kept| kept.complete) {
                        Some(kept) => projects.push(PreviousProject {
                            source,
                            key: key.to_owned(),
                            name: String::new(),
                            previous,
                            current: new,
                            slot: kept.slot,
                            saved: kept.folders,
                        }),
                        None => not_kept.push((source, key.to_owned())),
                    }
                    Ok(())
                };
            run(&mut keeping)
        })
    }

    /// At the end of the round: if at least one project entered the generation, it replaces the
    /// previous one on disk and in the manifest, with the time `round_at` and the names taken
    /// from `updated`. Otherwise the previous generation stays as it is.
    pub fn commit(
        self,
        manifest: &PreviousManifest,
        installation_id: &str,
        round_at: &str,
        updated: &[UpdateOutcome],
    ) -> Result<(), String> {
        let name_of = |source: SourceKind, key: &str| {
            updated
                .iter()
                .find(|o| o.source == source && o.key == key)
                .map_or_else(|| key.to_owned(), |o| o.name.clone())
        };
        // A project updated without its previous version is not lost silently.
        let not_kept = (!self.not_kept.is_empty()).then(|| {
            let names: Vec<String> = self
                .not_kept
                .iter()
                .map(|(source, key)| name_of(*source, key))
                .collect();
            format!(
                "Could not keep the previous version of {}: undo will not restore it.",
                names.join(", ")
            )
        });
        let Some(dir) = self.dir else {
            return self.error.map_or(Ok(()), Err);
        };
        if self.projects.is_empty() {
            return not_kept.map_or(Ok(()), Err);
        }
        let projects = self
            .projects
            .into_iter()
            .map(|mut project| {
                if let Some(outcome) = updated
                    .iter()
                    .find(|o| o.source == project.source && o.key == project.key)
                {
                    project.name = outcome.name.clone();
                }
                project
            })
            .collect();
        let failed = |error: io::Error| {
            format!(
                "Could not keep the previous versions for Undo ({}).",
                error.kind()
            )
        };
        if fs::symlink_metadata(&self.generation).is_ok() {
            // The old generation moves into a scratch folder that deletes itself: the name stays
            // free for the new one even if the deletion does not finish.
            let trash = WorkDir::create(&self.addons_dir, "old").map_err(|error| {
                format!("Could not keep the previous versions for Undo: {error}")
            })?;
            fs::rename(&self.generation, trash.path().join("generation")).map_err(failed)?;
            // The folders are gone: the manifest entry is no longer valid.
            manifest.set(installation_id, None)?;
        }
        // After the `rename` the work folder is gone and its `Drop` touches nothing.
        fs::rename(dir.path(), &self.generation).map_err(failed)?;
        let round = PreviousRound {
            round_at: round_at.to_owned(),
            projects,
        };
        if let Err(error) = manifest.set(installation_id, Some(round)) {
            let _ = fs::remove_dir_all(&self.generation);
            return Err(error);
        }
        not_kept.map_or(Ok(()), Err)
    }
}

// ── Undo ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum UndoStatus {
    /// Folders and record are back to the version from before the round.
    Restored,
    /// The project changed after the round: nothing was touched.
    Skipped,
    /// The folders could not be put back (if the previous version is still on disk Undo can be
    /// retried), or they came back but the record did not: then folders and record do not
    /// match and the message says so.
    Error,
}

/// The outcome of a project's Undo. Messages are in English (D27).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UndoOutcome {
    pub source: SourceKind,
    pub key: String,
    pub name: String,
    pub status: UndoStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

const CHANGED: &str = "Changed after the automatic update: left as it is.";
const MISSING: &str = "The previous version is no longer on disk.";
const NOTHING: &str = "There is no automatic update to undo for this installation.";

/// Undoes the last automatic round of `installation`: for each project it removes the new
/// folders, puts back the saved ones and restores the previous record, then deletes the
/// generation and the manifest entry. Must be called under `InstallLock`. A project that cannot
/// be put back because of a failed `rename` stays in the generation and in the manifest, and
/// Undo can be retried; the others leave it anyway. `Err` only if the manifest or the records
/// cannot be read, or if there is nothing to undo.
pub fn undo(
    installation: &Installation,
    manifest: &PreviousManifest,
    installed: &Mutex<InstalledAddonsStore>,
    cache: Option<&FingerprintCache>,
) -> Result<Vec<UndoOutcome>, String> {
    undo_with(installation, manifest, installed, cache, |from, to| {
        fs::rename(from, to)
    })
}

fn locked(
    installed: &Mutex<InstalledAddonsStore>,
) -> Result<std::sync::MutexGuard<'_, InstalledAddonsStore>, String> {
    installed
        .lock()
        .map_err(|_| "The installed addons list is not available: restart the app.".to_owned())
}

pub(crate) fn undo_with(
    installation: &Installation,
    manifest: &PreviousManifest,
    installed: &Mutex<InstalledAddonsStore>,
    cache: Option<&FingerprintCache>,
    mut rename: impl FnMut(&Path, &Path) -> io::Result<()>,
) -> Result<Vec<UndoOutcome>, String> {
    let round = manifest
        .read()?
        .remove(&installation.id)
        .filter(|round| !round.projects.is_empty())
        .ok_or_else(|| NOTHING.to_owned())?;
    let records = locked(installed)?.source_records_for(&installation.id)?;
    let addons = addons_dir(installation);
    let generation = interface_dir(installation).join(GENERATION_DIR);

    let mut outcomes = Vec::new();
    let mut left = Vec::new();
    // From last to first: if two projects in the round touched the same folder, each one puts
    // back what was there before it.
    for project in round.projects.iter().rev() {
        let outcome = |status, message: Option<String>| UndoOutcome {
            source: project.source,
            key: project.key.clone(),
            name: project.name.clone(),
            status,
            message,
        };
        let now = records
            .get(&project.source)
            .and_then(|p| p.get(&project.key));
        if now != Some(&project.current) {
            outcomes.push(outcome(UndoStatus::Skipped, Some(CHANGED.to_owned())));
            continue;
        }
        let slot = generation.join(&project.slot);
        let present = project
            .saved
            .iter()
            .all(|folder| fs::symlink_metadata(slot.join(folder)).is_ok());
        if !present {
            outcomes.push(outcome(UndoStatus::Error, Some(MISSING.to_owned())));
            continue;
        }
        if let Err(message) = put_back(&addons, &slot, project, &mut rename) {
            outcomes.push(outcome(UndoStatus::Error, Some(message)));
            left.push(project.clone());
            continue;
        }
        if let Some(cache) = cache {
            let touched: Vec<String> = project
                .current
                .folders
                .iter()
                .chain(&project.saved)
                .cloned()
                .collect();
            cache.forget(&addons, &touched);
        }
        let restored = locked(installed).and_then(|mut store| match &project.previous {
            Some(previous) => store.record_source(
                &installation.id,
                project.source,
                &project.key,
                previous.clone(),
            ),
            None => store.forget_project(&installation.id, project.source, &project.key),
        });
        // The folders came back but the record did not: it is not a successful Undo, and it
        // cannot be repeated (the slot is empty). The status says so, not just the message.
        outcomes.push(match restored {
            Ok(()) => outcome(UndoStatus::Restored, None),
            Err(error) => outcome(
                UndoStatus::Error,
                Some(format!(
                    "Restored the previous folders, but the app could not update its records: {error}"
                )),
            ),
        });
    }
    outcomes.reverse();
    left.reverse();

    if left.is_empty() {
        manifest.set(&installation.id, None)?;
        let _ = fs::remove_dir_all(&generation);
    } else {
        let keep: HashSet<&str> = left.iter().map(|p| p.slot.as_str()).collect();
        for project in &round.projects {
            if !keep.contains(project.slot.as_str()) {
                let _ = fs::remove_dir_all(generation.join(&project.slot));
            }
        }
        manifest.set(
            &installation.id,
            Some(PreviousRound {
                round_at: round.round_at,
                projects: left,
            }),
        )?;
    }
    Ok(outcomes)
}

/// Puts the folders saved in `slot` back into `addons` in place of those in the round's record.
/// With `replace_with`: the new folders (matched case-insensitively) are moved aside and
/// deleted only after all the saved ones are back; if a `rename` fails everything goes back to
/// how it was, and the previous version stays in the slot.
fn put_back(
    addons: &Path,
    slot: &Path,
    project: &PreviousProject,
    rename: &mut impl FnMut(&Path, &Path) -> io::Result<()>,
) -> Result<(), String> {
    if !project.saved.is_empty() {
        return install::replace_with(addons, slot, &project.current.folders, rename);
    }
    // The round replaced nothing (the new folders were not there): removing them is enough.
    let wanted: HashSet<String> = project
        .current
        .folders
        .iter()
        .map(|f| f.to_lowercase())
        .collect();
    for name in install::folder_names(addons)? {
        if wanted.contains(&name.to_lowercase()) {
            install::remove_folder(addons, &name)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;

    const CF: SourceKind = SourceKind::Curseforge;
    const GH: SourceKind = SourceKind::Github;

    struct Fixture {
        _tmp: TempDir,
        installation: Installation,
        addons: PathBuf,
        manifest: PreviousManifest,
        installed: Mutex<InstalledAddonsStore>,
    }

    fn fixture() -> Fixture {
        let tmp = TempDir::new();
        let path = tmp.path().join("World of Warcraft").join("_retail_");
        let addons = path.join("Interface").join("AddOns");
        fs::create_dir_all(&addons).unwrap();
        let config = tmp.path().join("config");
        Fixture {
            installation: Installation {
                id: "test".to_owned(),
                path,
                flavor: "retail".to_owned(),
                name: "Retail".to_owned(),
            },
            addons,
            manifest: PreviousManifest::new(config.join(PREVIOUS_FILE)),
            installed: Mutex::new(InstalledAddonsStore::load(
                config.join(store::INSTALLED_ADDONS_FILE),
            )),
            _tmp: tmp,
        }
    }

    impl Fixture {
        fn write(&self, folder: &str, text: &str) {
            let dir = self.addons.join(folder);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("file.lua"), text).unwrap();
        }

        fn read(&self, folder: &str) -> String {
            fs::read_to_string(self.addons.join(folder).join("file.lua")).unwrap()
        }

        fn listing(&self) -> Vec<String> {
            install::folder_names(&self.addons).unwrap()
        }

        fn generation(&self) -> PathBuf {
            self.installation
                .path
                .join("Interface")
                .join(GENERATION_DIR)
        }

        /// The folders of `Interface`, without `AddOns`.
        fn interface_extras(&self) -> Vec<String> {
            install::folder_names(&self.installation.path.join("Interface"))
                .unwrap()
                .into_iter()
                .filter(|name| name != "AddOns")
                .collect()
        }

        fn records(&self) -> SourceRecords {
            self.installed
                .lock()
                .unwrap()
                .source_records_for("test")
                .unwrap()
        }

        fn record(&self, source: SourceKind, key: &str) -> Option<SourceRecord> {
            self.records()
                .get(&source)
                .and_then(|p| p.get(key))
                .cloned()
        }

        fn set_record(&self, source: SourceKind, key: &str, record: SourceRecord) {
            self.installed
                .lock()
                .unwrap()
                .record_source("test", source, key, record)
                .unwrap();
        }

        /// An automatic round: for each `(source, key, file, folders)` it replaces the folders
        /// with a `new <file>` content (as a source does) and writes the record. Returns the
        /// `updated` outcomes and the one from `commit`.
        fn round(
            &self,
            updates: &[(SourceKind, &str, &str, &[&str])],
        ) -> (Vec<UpdateOutcome>, Result<(), String>) {
            let before = self.records();
            let mut next = NextGeneration::new(&self.installation);
            let mut writer = |source, key: &str, record| {
                self.installed
                    .lock()
                    .unwrap()
                    .record_source("test", source, key, record)
            };
            let updated = next.run(&before, &mut writer, |record| {
                let mut updated = Vec::new();
                for (source, key, file, folders) in updates {
                    let staging = WorkDir::staging(&self.addons).unwrap();
                    for folder in *folders {
                        let dir = staging.path().join(folder);
                        fs::create_dir_all(&dir).unwrap();
                        fs::write(dir.join("file.lua"), format!("new {file}")).unwrap();
                    }
                    install::replace(&self.addons, staging.path(), &[]).unwrap();
                    record(
                        *source,
                        key,
                        SourceRecord {
                            file: (*file).to_owned(),
                            folders: folders.iter().map(|f| (*f).to_owned()).collect(),
                        },
                    )
                    .unwrap();
                    updated.push(UpdateOutcome::updated(
                        *source,
                        key,
                        &format!("{key} name"),
                        crate::updates::FileSummary {
                            id: 0,
                            name: (*file).to_owned(),
                            date: String::new(),
                        },
                        None,
                    ));
                }
                updated
            });
            let committed = next.commit(&self.manifest, "test", "2026-10-27T10:00:00Z", &updated);
            (updated, committed)
        }

        fn undo(&self) -> Result<Vec<UndoOutcome>, String> {
            undo(&self.installation, &self.manifest, &self.installed, None)
        }
    }

    fn record(file: &str, folders: &[&str]) -> SourceRecord {
        SourceRecord {
            file: file.to_owned(),
            folders: folders.iter().map(|f| (*f).to_owned()).collect(),
        }
    }

    fn statuses(outcomes: &[UndoOutcome]) -> Vec<(&str, UndoStatus)> {
        outcomes
            .iter()
            .map(|o| (o.key.as_str(), o.status))
            .collect()
    }

    /// Two projects: DBM (CurseForge, with a record) and BigWigs (GitHub, recognised without one).
    fn two_addons() -> Fixture {
        let fx = fixture();
        fx.write("DBM-Core", "old dbm core");
        fx.write("DBM-GUI", "old dbm gui");
        fx.write("BigWigs", "old bigwigs");
        fx.write("Other", "other");
        fx.set_record(CF, "3358", record("1", &["DBM-Core", "DBM-GUI"]));
        fx
    }

    #[test]
    fn an_automatic_round_keeps_the_replaced_folders_and_undo_puts_them_back() {
        let fx = two_addons();
        let (_, committed) = fx.round(&[
            (CF, "3358", "2", &["DBM-Core", "DBM-GUI"]),
            (GH, "owner/bigwigs", "v2", &["BigWigs"]),
        ]);
        committed.unwrap();
        assert_eq!(fx.read("DBM-Core"), "new 2");
        assert_eq!(fx.read("BigWigs"), "new v2");
        // Only the generation next to AddOns: staging, scratch folders and work folder are
        // gone.
        assert_eq!(fx.interface_extras(), [GENERATION_DIR]);
        let rounds = fx.manifest.read().unwrap();
        let round = &rounds["test"];
        assert_eq!(round.round_at, "2026-10-27T10:00:00Z");
        assert_eq!(round.projects.len(), 2);
        let dbm = &round.projects[0];
        assert_eq!(dbm.name, "3358 name");
        assert_eq!(dbm.previous, Some(record("1", &["DBM-Core", "DBM-GUI"])));
        assert_eq!(dbm.current, record("2", &["DBM-Core", "DBM-GUI"]));
        assert_eq!(dbm.saved, ["DBM-Core", "DBM-GUI"]);
        assert_eq!(round.projects[1].previous, None);
        assert_eq!(
            undoable(&fx.manifest, &["test".to_owned(), "other".to_owned()]),
            [UndoableRound {
                installation_id: "test".to_owned(),
                round_at: "2026-10-27T10:00:00Z".to_owned(),
                addons: 2,
            }]
        );

        let outcomes = fx.undo().unwrap();
        assert_eq!(
            statuses(&outcomes),
            [
                ("3358", UndoStatus::Restored),
                ("owner/bigwigs", UndoStatus::Restored)
            ]
        );
        assert!(outcomes.iter().all(|o| o.message.is_none()));
        assert_eq!(fx.read("DBM-Core"), "old dbm core");
        assert_eq!(fx.read("DBM-GUI"), "old dbm gui");
        assert_eq!(fx.read("BigWigs"), "old bigwigs");
        assert_eq!(fx.read("Other"), "other");
        assert_eq!(fx.listing(), ["BigWigs", "DBM-Core", "DBM-GUI", "Other"]);
        // The previous record comes back; the one that was not there before disappears.
        assert_eq!(
            fx.record(CF, "3358"),
            Some(record("1", &["DBM-Core", "DBM-GUI"]))
        );
        assert_eq!(fx.record(GH, "owner/bigwigs"), None);
        assert!(fx.interface_extras().is_empty());
        assert!(fx.manifest.read().unwrap().is_empty());
        assert!(undoable(&fx.manifest, &["test".to_owned()]).is_empty());
        assert_eq!(fx.undo().unwrap_err(), NOTHING);
    }

    #[test]
    fn obsolete_folders_removed_by_the_round_come_back() {
        let fx = fixture();
        fx.write("Addon", "old");
        fx.write("Addon-Obsolete", "obsolete");
        fx.set_record(CF, "1", record("1", &["Addon", "Addon-Obsolete"]));
        let before = fx.records();
        let mut next = NextGeneration::new(&fx.installation);
        let mut writer = |source, key: &str, record| {
            fx.installed
                .lock()
                .unwrap()
                .record_source("test", source, key, record)
        };
        next.run(&before, &mut writer, |record| {
            let staging = WorkDir::staging(&fx.addons).unwrap();
            fs::create_dir_all(staging.path().join("Addon")).unwrap();
            fs::write(staging.path().join("Addon").join("file.lua"), "new").unwrap();
            install::replace(&fx.addons, staging.path(), &["Addon-Obsolete".to_owned()]).unwrap();
            record(CF, "1", record_of("2", &["Addon"])).unwrap();
        });
        next.commit(&fx.manifest, "test", "t", &[]).unwrap();
        assert_eq!(fx.listing(), ["Addon"]);

        fx.undo().unwrap();
        assert_eq!(fx.listing(), ["Addon", "Addon-Obsolete"]);
        assert_eq!(fx.read("Addon"), "old");
        assert_eq!(fx.read("Addon-Obsolete"), "obsolete");
    }

    fn record_of(file: &str, folders: &[&str]) -> SourceRecord {
        record(file, folders)
    }

    #[test]
    fn a_project_changed_after_the_round_is_skipped_without_touching_it() {
        let fx = two_addons();
        fx.round(&[
            (CF, "3358", "2", &["DBM-Core", "DBM-GUI"]),
            (GH, "owner/bigwigs", "v2", &["BigWigs"]),
        ])
        .1
        .unwrap();
        // DBM updated by hand after the round.
        fx.write("DBM-Core", "manual 3");
        fx.set_record(CF, "3358", record("3", &["DBM-Core", "DBM-GUI"]));

        let outcomes = fx.undo().unwrap();
        assert_eq!(
            statuses(&outcomes),
            [
                ("3358", UndoStatus::Skipped),
                ("owner/bigwigs", UndoStatus::Restored)
            ]
        );
        assert_eq!(outcomes[0].message.as_deref(), Some(CHANGED));
        assert_eq!(fx.read("DBM-Core"), "manual 3");
        assert_eq!(fx.read("DBM-GUI"), "new 2");
        assert_eq!(
            fx.record(CF, "3358"),
            Some(record("3", &["DBM-Core", "DBM-GUI"]))
        );
        assert_eq!(fx.read("BigWigs"), "old bigwigs");
        // The skipped project can never be undone: the generation disappears anyway.
        assert!(fx.interface_extras().is_empty());
        assert!(fx.manifest.read().unwrap().is_empty());
    }

    #[test]
    fn a_removed_project_is_skipped_too() {
        let fx = two_addons();
        fx.round(&[(GH, "owner/bigwigs", "v2", &["BigWigs"])])
            .1
            .unwrap();
        fx.installed
            .lock()
            .unwrap()
            .forget_project("test", GH, "owner/bigwigs")
            .unwrap();
        fs::remove_dir_all(fx.addons.join("BigWigs")).unwrap();
        let outcomes = fx.undo().unwrap();
        assert_eq!(
            statuses(&outcomes),
            [("owner/bigwigs", UndoStatus::Skipped)]
        );
        assert!(!fx.addons.join("BigWigs").exists());
    }

    #[test]
    fn a_second_round_replaces_the_generation_and_an_empty_one_leaves_it() {
        let fx = two_addons();
        fx.round(&[(CF, "3358", "2", &["DBM-Core", "DBM-GUI"])])
            .1
            .unwrap();
        fx.round(&[(GH, "owner/bigwigs", "v2", &["BigWigs"])])
            .1
            .unwrap();
        let rounds = fx.manifest.read().unwrap();
        assert_eq!(rounds["test"].projects.len(), 1);
        assert_eq!(rounds["test"].projects[0].key, "owner/bigwigs");
        assert_eq!(fx.interface_extras(), [GENERATION_DIR]);
        assert_eq!(
            install::folder_names(&fx.generation()).unwrap(),
            [rounds["test"].projects[0].slot.clone()]
        );

        // A round that installs nothing leaves the generation and the manifest.
        let (_, committed) = fx.round(&[]);
        committed.unwrap();
        assert_eq!(fx.manifest.read().unwrap(), rounds);
        assert_eq!(fx.interface_extras(), [GENERATION_DIR]);

        // Undo puts back only the last round: DBM stays at the first round's version.
        fx.undo().unwrap();
        assert_eq!(fx.read("BigWigs"), "old bigwigs");
        assert_eq!(fx.read("DBM-Core"), "new 2");
        assert_eq!(
            fx.record(CF, "3358"),
            Some(record("2", &["DBM-Core", "DBM-GUI"]))
        );
    }

    #[test]
    fn a_failed_rename_keeps_the_rest_of_the_generation_for_another_try() {
        let fx = two_addons();
        fx.round(&[
            (CF, "3358", "2", &["DBM-Core", "DBM-GUI"]),
            (GH, "owner/bigwigs", "v2", &["BigWigs"]),
        ])
        .1
        .unwrap();
        // Undo goes from last to first: BigWigs (2 moves) succeeds, then DBM's third move (the
        // first saved folder coming back) fails.
        let mut calls = 0;
        let outcomes = undo_with(
            &fx.installation,
            &fx.manifest,
            &fx.installed,
            None,
            |from, to| {
                calls += 1;
                if calls == 5 {
                    Err(io::Error::from(io::ErrorKind::PermissionDenied))
                } else {
                    fs::rename(from, to)
                }
            },
        )
        .unwrap();
        assert_eq!(
            statuses(&outcomes),
            [
                ("3358", UndoStatus::Error),
                ("owner/bigwigs", UndoStatus::Restored)
            ]
        );
        assert!(
            outcomes[0]
                .message
                .as_deref()
                .unwrap()
                .contains("Could not move")
        );
        // BigWigs is back with its record; DBM is still the round's version, consistent with
        // its record.
        assert_eq!(fx.read("BigWigs"), "old bigwigs");
        assert_eq!(fx.record(GH, "owner/bigwigs"), None);
        assert_eq!(fx.read("DBM-Core"), "new 2");
        assert_eq!(fx.read("DBM-GUI"), "new 2");
        assert_eq!(
            fx.record(CF, "3358"),
            Some(record("2", &["DBM-Core", "DBM-GUI"]))
        );
        let rounds = fx.manifest.read().unwrap();
        assert_eq!(rounds["test"].projects.len(), 1);
        let dbm = rounds["test"].projects[0].clone();
        assert_eq!(dbm.key, "3358");
        assert_eq!(
            install::folder_names(&fx.generation()).unwrap(),
            [dbm.slot.as_str()]
        );

        // The second attempt finishes the job.
        let outcomes = fx.undo().unwrap();
        assert_eq!(statuses(&outcomes), [("3358", UndoStatus::Restored)]);
        assert_eq!(fx.read("DBM-Core"), "old dbm core");
        assert!(fx.interface_extras().is_empty());
    }

    #[test]
    fn folder_names_are_matched_without_case() {
        // The record and the generation write "dbm-core", the disk has "DBM-Core": no two names
        // differing only in case ever exist together on disk.
        let fx = fixture();
        fx.write("DBM-Core", "old");
        fx.round(&[(CF, "3358", "2", &["dbm-core"])]).1.unwrap();
        assert_eq!(fx.listing(), ["dbm-core"]);
        let rounds = fx.manifest.read().unwrap();
        assert_eq!(rounds["test"].projects[0].saved, ["DBM-Core"]);
        // The current record still uses the round's name, the disk another spelling.
        fs::rename(fx.addons.join("dbm-core"), fx.addons.join("tmp")).unwrap();
        fs::rename(fx.addons.join("tmp"), fx.addons.join("Dbm-Core")).unwrap();

        let outcomes = fx.undo().unwrap();
        assert_eq!(statuses(&outcomes), [("3358", UndoStatus::Restored)]);
        assert_eq!(fx.listing(), ["DBM-Core"]);
        assert_eq!(fx.read("DBM-Core"), "old");
    }

    #[test]
    fn a_project_without_replaced_folders_is_undone_by_removing_the_new_ones() {
        let fx = fixture();
        fx.write("Other", "other");
        fx.round(&[(GH, "owner/new", "v1", &["NewAddon"])])
            .1
            .unwrap();
        assert_eq!(
            fx.manifest.read().unwrap()["test"].projects[0].saved,
            Vec::<String>::new()
        );
        let outcomes = fx.undo().unwrap();
        assert_eq!(statuses(&outcomes), [("owner/new", UndoStatus::Restored)]);
        assert_eq!(fx.listing(), ["Other"]);
    }

    #[test]
    fn an_update_whose_previous_version_was_not_kept_is_reported() {
        let fx = two_addons();
        let before = fx.records();
        let mut next = NextGeneration::new(&fx.installation);
        let mut writer = |_, _: &str, _| Ok(());
        // A record without a kept `replace` before it (for example on another thread).
        next.run(&before, &mut writer, |record| {
            record(GH, "owner/bigwigs", record_of("v2", &["BigWigs"])).unwrap();
        });
        let updated = [UpdateOutcome::updated(
            GH,
            "owner/bigwigs",
            "BigWigs",
            crate::updates::FileSummary {
                id: 0,
                name: "v2".to_owned(),
                date: String::new(),
            },
            None,
        )];
        let error = next
            .commit(&fx.manifest, "test", "t", &updated)
            .unwrap_err();
        assert_eq!(
            error,
            "Could not keep the previous version of BigWigs: undo will not restore it."
        );
        assert!(fx.manifest.read().unwrap().is_empty());
    }

    #[test]
    fn a_damaged_manifest_blocks_undo_and_hides_the_status() {
        let fx = fixture();
        fs::create_dir_all(fx.manifest.file.parent().unwrap()).unwrap();
        fs::write(&fx.manifest.file, "{").unwrap();
        assert!(fx.undo().unwrap_err().contains("damaged"));
        assert!(undoable(&fx.manifest, &["test".to_owned()]).is_empty());
        assert!(fx.manifest.set("test", None).is_err());
    }

    #[test]
    fn the_undo_payloads_are_camel_case() {
        let outcome = UndoOutcome {
            source: GH,
            key: "owner/repo".to_owned(),
            name: "Addon".to_owned(),
            status: UndoStatus::Restored,
            message: None,
        };
        let json = serde_json::to_value(&outcome).unwrap();
        assert_eq!(json["status"], "restored");
        assert_eq!(json["source"], "github");
        assert!(json.get("message").is_none());
        let round = UndoableRound {
            installation_id: "id".to_owned(),
            round_at: "t".to_owned(),
            addons: 3,
        };
        let json = serde_json::to_value(&round).unwrap();
        assert_eq!(json["installationId"], "id");
        assert_eq!(json["roundAt"], "t");
        assert_eq!(json["addons"], 3);
    }
}
