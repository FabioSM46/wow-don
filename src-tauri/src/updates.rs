//! Update check: matches the installed addons to the sources' projects (D12,
//! D33) and picks, for each one, the most recent version for the installation's game.
//!
//! [`check_updates_with`] scans the installation and offers the folders to the sources in D33
//! order;
//! [`update_addons_with`] redoes the check and asks each source to update its projects.
//! CurseForge is [`CurseForgeSource`]: [`evaluate`] is pure, it receives the scan, the fingerprint
//! response and the mods, and uses neither network nor disk.

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

/// CurseForge game version types (`gameVersionTypeId`), verified on 2026-10-22 with the
/// `live_version_types` test of `sources::curseforge` against the real API.
pub const RETAIL: u32 = 517;
pub const CLASSIC_ERA: u32 = 67408;
/// "WoW Mists of Pandaria Classic". **Changes with the expansion**: when the progressive Classic
/// moves to the next expansion, this id must be updated to the new type (the
/// `the_progressive_classic_is_mists` test makes it visible in the diff).
pub const CLASSIC_PROGRESSIVE: u32 = 79434;
/// "WoW Burning Crusade Classic". CurseForge's list of types has no type for Classic
/// Anniversary, and Anniversary `.toc` files use the `_TBC` suffix (warcraft.wiki.gg, "TOC
/// format": "Burning Crusade Classic and Classic Anniversary Edition"): Anniversary addons
/// are published under this type. If CurseForge adds a dedicated type, this must change.
pub const ANNIVERSARY: u32 = 73246;
/// "WoW Forever".
pub const FOREVER: u32 = 88568;

/// The version types the app knows, with the name shown in the frontend's menu (in
/// English, like the whole interface). They are also the only ones allowed as an addon's
/// forced version (D6).
pub const VERSION_TYPES: [(u32, &str); 5] = [
    (RETAIL, "Retail"),
    (CLASSIC_ERA, "Classic Era"),
    (CLASSIC_PROGRESSIVE, "Classic"),
    (ANNIVERSARY, "Anniversary"),
    (FOREVER, "WoW Forever"),
];

/// A game version type for the frontend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VersionType {
    pub id: u32,
    pub name: &'static str,
}

/// The known types, in the order of [`VERSION_TYPES`]. No network call.
pub fn version_types() -> Vec<VersionType> {
    VERSION_TYPES
        .iter()
        .map(|&(id, name)| VersionType { id, name })
        .collect()
}

pub fn is_known_version_type(id: u32) -> bool {
    VERSION_TYPES.iter().any(|&(known, _)| known == id)
}

/// Forced game versions: CurseForge project id → `gameVersionTypeId`.
pub type VersionOverrides = HashMap<u32, u32>;

/// Game version type for an installation's flavor.
///
/// - `_retail_` → Retail; `_classic_era_` → Classic Era;
/// - `_classic_` → the current progressive Classic (Mists of Pandaria);
/// - `_anniversary_` → Burning Crusade Classic (see [`ANNIVERSARY`]);
/// - `_forever_` → WoW Forever: the official client's folder (product `wowf` in
///   `.build.info`, verified on a real installation);
/// - `_classic_beta_` → WoW Forever: the beta's folder (product `wow_classic_beta`). If
///   another Classic beta went back to using it, this line would need revisiting.
///
/// PTR, Retail beta and Titan Reforged (81212) stay without a type until it is known for
/// certain which folder they use: a clear error is better than an update for the wrong
/// version. A possible follow-up is reading the installed product from `.build.info` instead of
/// deducing it from the folder name.
pub fn version_type_for(flavor: &str) -> Option<u32> {
    match flavor.trim_matches('_').to_ascii_lowercase().as_str() {
        "retail" => Some(RETAIL),
        "classic_era" => Some(CLASSIC_ERA),
        "classic" => Some(CLASSIC_PROGRESSIVE),
        "anniversary" => Some(ANNIVERSARY),
        "classic_beta" | "forever" => Some(FOREVER),
        _ => None,
    }
}

/// A project's installed or available file. `id` is the CurseForge file id, needed to
/// download it; sources without a numeric id (GitHub, WoWInterface, Tukui) put `0` and the
/// version or tag in `name`, the same string they write in `SourceRecord::file`.
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
    /// The CurseForge id; `0` for the other sources, where the project is identified by `source`
    /// and `key`.
    pub id: u32,
    pub source: SourceKind,
    /// The project's id in the source, as a string (`61284`, `owner/repo`, `-2`).
    pub key: String,
    pub name: String,
    pub url: Option<String>,
    /// Whether the source allows downloading the files from a third-party app (for CurseForge
    /// `allowModDistribution`; unknown counts as `false`).
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
    /// For the version there is no Release, but there is a Beta newer than the installed file,
    /// and the "Beta when there is no Release" option is off (issue #141). If a Release exists,
    /// the Beta does not count even with the option on, so the status stays `UpToDate`.
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
    /// The source that claimed the folder, if any.
    pub source: Option<SourceKind>,
    /// The project; several folders can share the same project (for example DBM).
    pub project: Option<Project>,
    pub installed_file: Option<FileSummary>,
    #[serde(flatten)]
    pub status: UpdateStatus,
    /// The game version forced for the project, if any: it is the one used to look up the
    /// most recent file instead of the installation's.
    pub version_override: Option<u32>,
    /// The folder names of the file recognized for this folder (empty if not
    /// recognized): the frontend uses them to group the folders of the same file, including
    /// those CurseForge does not recognize one by one.
    pub project_folders: Vec<String>,
    /// The folders (`modules`) of the installed file, with their fingerprint: the update needs
    /// them to remove the ones the new version no longer has. They stay in the
    /// backend.
    #[serde(skip)]
    pub installed_modules: Vec<FileModule>,
    /// The file the single rule [`pick_file`] picks for the project, even when it is the
    /// installed one: the reinstall (issue #148) downloads this. `None` if the rule picks
    /// nothing or the source does not use it. Stays in the backend: the JSON does not change.
    #[serde(skip)]
    pub reinstall_file: Option<FileSummary>,
    /// The GitHub mirror of a blocked CurseForge project with an update (D34, issue
    /// #163): `githubMirror` and `githubMirrorReason`, `null` when it has not been computed.
    #[serde(flatten)]
    pub mirror: MirrorStatus,
}

/// Where [`MatchIndex::locate`] finds a folder.
enum Located<'a> {
    /// The match of the project that contains the folder.
    Found(&'a FingerprintMatch),
    /// No matched file contains it: CurseForge does not know it.
    NotMatched,
    /// The folder could not be listed (see [`InstalledAddon::is_unreadable`]).
    Unreadable,
}

/// A CurseForge project that an app record (`installed-addons.json`) names for folders
/// the fingerprint does not match (issue #216): the match is built from the record's installed
/// file (`get_files`) and the mod's latest files (`get_mods`), and applies to the record's
/// `folders`.
pub struct RecordedMatch {
    pub folders: Vec<String>,
    pub found: FingerprintMatch,
}

/// The matches of the fingerprint response, searchable by folder. First the partial matches (the
/// first file that contains the folder), then the exact ones, which replace them: an exact match
/// always wins over a partial one. The record matches are only the fallback.
struct MatchIndex<'a> {
    /// Fingerprint of a folder → the match of the file that contains it.
    by_fingerprint: HashMap<u64, &'a FingerprintMatch>,
    /// Name of a folder (`modules[].name`, lowercase) → the match of the file that lists it.
    by_folder: HashMap<String, &'a FingerprintMatch>,
    /// Name of a folder (lowercase) → the match of the record that names it; `None` if the
    /// records of several projects name it, which then do not decide.
    by_record: HashMap<String, Option<&'a FingerprintMatch>>,
}

impl<'a> MatchIndex<'a> {
    fn new(matches: &'a FingerprintMatches, recorded: &'a [RecordedMatch]) -> Self {
        let mut index = Self {
            by_fingerprint: HashMap::new(),
            by_folder: HashMap::new(),
            by_record: HashMap::new(),
        };
        for record in recorded {
            for folder in &record.folders {
                index
                    .by_record
                    .entry(folder.to_lowercase())
                    .and_modify(|owner| {
                        if owner.is_some_and(|found| found.id != record.found.id) {
                            *owner = None;
                        }
                    })
                    .or_insert(Some(&record.found));
            }
        }
        for found in &matches.partial_matches {
            for module in &found.file.modules {
                index
                    .by_fingerprint
                    .entry(module.fingerprint)
                    .or_insert(found);
                index
                    .by_folder
                    .entry(module.name.to_lowercase())
                    .or_insert(found);
            }
        }
        for found in &matches.exact_matches {
            for module in &found.file.modules {
                index.by_fingerprint.insert(module.fingerprint, found);
                index.by_folder.insert(module.name.to_lowercase(), found);
            }
        }
        index
    }

    /// A folder's project. With a fingerprint, only that decides. Without one, the folder can
    /// be readable but have no file that CurseForge's computation counts (a `.toc` with
    /// a suffix the computation does not know, like `_Camelot`, issue #212): then it belongs to
    /// the matched file that lists it among its `modules`, with the name compared
    /// case-insensitively as the game does, and if no file lists it, it is not
    /// recognized. Only a folder that could not be listed is an error.
    ///
    /// A readable folder that the fingerprint response matches in neither way
    /// belongs to the project of the record that names it, if any (issue #216): the record never
    /// changes an already matched folder.
    fn locate(&self, addon: &InstalledAddon) -> Located<'a> {
        let folder = addon.folder.to_lowercase();
        let found = match addon.fingerprint {
            Some(fingerprint) => self.by_fingerprint.get(&u64::from(fingerprint)),
            None if addon.is_unreadable() => return Located::Unreadable,
            None => self.by_folder.get(&folder),
        };
        found
            .or_else(|| self.by_record.get(&folder).and_then(Option::as_ref))
            .map_or(Located::NotMatched, |&found| Located::Found(found))
    }
}

/// Matches and evaluates. `version_type` is the installation's `gameVersionTypeId`; `overrides`
/// replaces it for the projects that have a forced version. Without Beta (see
/// [`evaluate_with`]).
#[cfg(test)]
pub fn evaluate(
    addons: &[InstalledAddon],
    matches: &FingerprintMatches,
    mods: &[Mod],
    version_type: u32,
    overrides: &VersionOverrides,
) -> Vec<AddonUpdate> {
    evaluate_with(addons, matches, &[], mods, version_type, overrides, false)
}

/// Like `evaluate`; with `allow_beta` the most recent file can be a Beta when the version
/// has no Release (issue #141). `recorded` are the record projects for the folders that
/// `matches` does not match (issue #216), evaluated with the same rules.
pub fn evaluate_with(
    addons: &[InstalledAddon],
    matches: &FingerprintMatches,
    recorded: &[RecordedMatch],
    mods: &[Mod],
    version_type: u32,
    overrides: &VersionOverrides,
    allow_beta: bool,
) -> Vec<AddonUpdate> {
    let index = MatchIndex::new(matches, recorded);
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
            let found = match index.locate(addon) {
                Located::Found(found) => found,
                Located::NotMatched => return update,
                Located::Unreadable => {
                    update.source = Some(SourceKind::Curseforge);
                    update.status = UpdateStatus::Error {
                        message: "Unreadable folder: it cannot be recognized.".to_owned(),
                    };
                    return update;
                }
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
                // With a Release for the version the Beta is never used (`pick_file`), with
                // the option or without: nothing to suggest.
                Some(_) => UpdateStatus::UpToDate,
                // No Release: if a newer Beta exists, say so instead of "up to date".
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

/// The most recent file for the version type, among those CurseForge reports as latest for
/// the project, with the rule of [`pick_file`] (a Beta only with `allow_beta`).
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

/// Dates are ISO 8601 in UTC, so comparing them as text follows time; with equal
/// dates the id decides, which CurseForge assigns in increasing order.
fn is_newer(candidate: &File, installed: &File) -> bool {
    candidate.id != installed.id
        && (candidate.file_date.as_str(), candidate.id)
            > (installed.file_date.as_str(), installed.id)
}

/// Scans the installation and offers the folders to the sources in `sources`, in order (D33):
/// a folder registered in `records` goes only to the record's source; the others to every source
/// that has not claimed them yet. One entry per folder, in scan order; folders
/// no source recognizes stay `notMatched` without a source. A folder is claimed only by
/// the source that gives it a project: an error on a folder (for example CurseForge failing to
/// read it) does not take it away from the following sources and shows only if none recognizes
/// it. A failing source does not stop the others: the folders that had been offered to it and that
/// nobody claimed report its error. If all fail, the error is the first one's.
/// Blocking: must be called off the main thread.
#[cfg(test)]
pub fn check_updates(
    installation: &Installation,
    sources: &[&dyn Source],
    records: &SourceRecords,
) -> Result<Vec<AddonUpdate>, String> {
    check_updates_with(installation, None, sources, records)
}

/// Like `check_updates`, with the scan's fingerprints taken from `cache` (issue #125).
pub fn check_updates_with(
    installation: &Installation,
    cache: Option<&FingerprintCache>,
    sources: &[&dyn Source],
    records: &SourceRecords,
) -> Result<Vec<AddonUpdate>, String> {
    scan_and_check(installation, cache, sources, records).map(|(_, updates)| updates)
}

/// Like `check_updates`, but also returns the scan.
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

/// The addon list and their statuses from a single scan (`refresh_addons`, issue #126).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Refresh {
    /// Like `list_addons`.
    pub addons: Vec<ListedAddon>,
    /// Like `check_updates`; empty if the check failed.
    pub updates: Vec<AddonUpdate>,
    /// Why the check failed entirely: the list arrives anyway.
    pub check_error: Option<String>,
}

/// Scans the installation once and derives from it both the list for the Installed tab (with the
/// projects of `installed`) and the update statuses (D33, with `records`). A failed check
/// does not take away the list: it ends up in `check_error`. Only a failed scan is `Err`.
/// Blocking.
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

/// The `check_updates` check on a scan already done.
fn check_scanned(
    installation: &Installation,
    addons: &[InstalledAddon],
    sources: &[&dyn Source],
    records: &SourceRecords,
) -> Result<Vec<AddonUpdate>, String> {
    let no_records = ProjectRecords::new();
    // Folder (lowercase) → the source of the record that names it. A folder in the records of
    // several sources belongs to none.
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
    // Per folder, the first error of a source it had been offered to: of the whole source or
    // of that folder alone. It applies only if no source claims it.
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
            // Only a project claims the folder; an error is kept in reserve for the case where
            // no source recognizes it.
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
    // The work a source postpones until after every source's check (the CurseForge GitHub
    // mirror, D34): in source order.
    for source in sources {
        source.finish(installation, &mut updates);
    }
    Ok(updates)
}

/// The entry for a folder no source claimed.
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

/// CurseForge as a source: folder fingerprints, forced game versions per project
/// (D6) and `allowModDistribution` (D13).
pub struct CurseForgeSource<'a, T: Transport> {
    pub client: &'a CurseForgeClient<T>,
    pub overrides: &'a VersionOverrides,
    /// The "Beta when there is no Release" option (issue #141).
    pub allow_beta: bool,
    /// GitHub for the projects CurseForge does not allow downloading (D34, issue #163). Without
    /// it, those projects update only from the website.
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
        records: &ProjectRecords,
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
        let candidates = record_candidates(offered, &matches, records);
        let mut mod_ids: Vec<u32> = matches
            .exact_matches
            .iter()
            .chain(&matches.partial_matches)
            .map(|m| m.id)
            .chain(candidates.iter().map(|c| c.mod_id))
            .collect();
        mod_ids.sort_unstable();
        mod_ids.dedup();
        let mods = self.client.get_mods(&mod_ids).map_err(|e| e.to_string())?;
        let recorded = self.recorded_matches(&candidates, &mods);
        let mut updates = evaluate_with(
            offered,
            &matches,
            &recorded,
            &mods,
            version_type,
            self.overrides,
            self.allow_beta,
        );
        if self.github.is_some() {
            let found: Vec<&FingerprintMatch> = matches
                .exact_matches
                .iter()
                .chain(&matches.partial_matches)
                .chain(recorded.iter().map(|r| &r.found))
                .collect();
            mark_mirrors(&mut updates, &found, &mods);
        }
        Ok(updates)
    }

    fn finish(&self, installation: &Installation, updates: &mut [AddonUpdate]) {
        if let Some(github) = self.github {
            resolve_mirrors(updates, github, &installation.flavor);
        }
    }

    fn update(&self, target: &UpdateTarget<'_>, record: &mut RecordWriter<'_>) -> UpdateOutcome {
        // Without a CurseForge address the check failed and the project is not among the
        // recognized ones: the real reason is the configuration, not "not found".
        if !self.client.is_configured() {
            return UpdateOutcome::error(
                SourceKind::Curseforge,
                target.key,
                "",
                CurseForgeError::NotConfigured.to_string(),
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

/// A CurseForge record to use as a fallback (issue #216): project, installed file and folders.
struct RecordCandidate<'a> {
    mod_id: u32,
    file_id: u32,
    folders: &'a [String],
}

/// The records that name at least one offered folder that the fingerprint response does not
/// match, neither directly nor through the `modules` of a matched file. Unreadable folders stay
/// out: they are a scan error, not an unknown addon. A record with a project or file id
/// that is not a number does not count.
fn record_candidates<'a>(
    offered: &[InstalledAddon],
    matches: &FingerprintMatches,
    records: &'a ProjectRecords,
) -> Vec<RecordCandidate<'a>> {
    let index = MatchIndex::new(matches, &[]);
    let unmatched: HashSet<String> = offered
        .iter()
        .filter(|addon| matches!(index.locate(addon), Located::NotMatched))
        .map(|addon| addon.folder.to_lowercase())
        .collect();
    if unmatched.is_empty() {
        return Vec::new();
    }
    records
        .iter()
        .filter(|(_, record)| {
            record
                .folders
                .iter()
                .any(|folder| unmatched.contains(&folder.to_lowercase()))
        })
        .filter_map(|(key, record)| {
            Some(RecordCandidate {
                mod_id: key.parse().ok()?,
                file_id: record.file.parse().ok()?,
                folders: &record.folders,
            })
        })
        .collect()
}

impl<T: Transport> CurseForgeSource<'_, T> {
    /// The matches of the `candidates` records: the installed file is read with a single
    /// `get_files`, the latest files from the mod already read in `mods`. A record whose project
    /// is not in `mods` or whose file CurseForge no longer returns (or returns for another
    /// project) gives no match, and its folders stay unrecognized. The fallback never worsens the
    /// check: if the `get_files` request fails, there is no record match (their folders
    /// stay `notMatched`, with no message) and the fingerprint matches apply as before.
    fn recorded_matches(
        &self,
        candidates: &[RecordCandidate<'_>],
        mods: &[Mod],
    ) -> Vec<RecordedMatch> {
        let project_of = |id: u32| mods.iter().find(|m| m.id == id);
        let mut file_ids: Vec<u32> = candidates
            .iter()
            .filter(|c| project_of(c.mod_id).is_some())
            .map(|c| c.file_id)
            .collect();
        file_ids.sort_unstable();
        file_ids.dedup();
        if file_ids.is_empty() {
            return Vec::new();
        }
        let Ok(files) = self.client.get_files(&file_ids) else {
            return Vec::new();
        };
        candidates
            .iter()
            .filter_map(|candidate| {
                let project = project_of(candidate.mod_id)?;
                let file = files
                    .iter()
                    .find(|f| f.id == candidate.file_id && f.mod_id == candidate.mod_id)?;
                Some(RecordedMatch {
                    folders: candidate.folders.to_vec(),
                    found: FingerprintMatch {
                        id: candidate.mod_id,
                        file: file.clone(),
                        latest_files: project.latest_files.clone(),
                    },
                })
            })
            .collect()
    }
}

/// Marks for the GitHub mirror (D34) the blocked projects with an available update, with
/// the project and file that [`resolve_mirrors`] needs. No network: the mirror is computed
/// after every source has done its check (`Source::finish`), so the requests of the
/// registered GitHub projects go first and do not find the limit used up by the mirrors.
/// Nothing for up-to-date projects, those with only a Beta, or those CurseForge allows downloading.
fn mark_mirrors(updates: &mut [AddonUpdate], matches: &[&FingerprintMatch], mods: &[Mod]) {
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
            .iter()
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

/// Computes the mirror of the entries marked by [`mark_mirrors`], once per project and file.
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

/// Today's reason for a blocked project without GitHub.
const BLOCKED: &str = "The author does not allow downloads from third-party apps: update it from the CurseForge website.";

// ── Update ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum OutcomeStatus {
    Updated,
    Skipped,
    Error,
}

/// The outcome of updating a project. The messages are in English (D27).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateOutcome {
    /// The CurseForge id; `0` for the other sources (see `source` and `key`).
    pub mod_id: u32,
    pub source: SourceKind,
    /// The project's id in the source, as a string.
    pub key: String,
    /// The project's name; empty if the project is not among the installed ones.
    pub name: String,
    pub status: OutcomeStatus,
    /// Why it was skipped or what went wrong.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// The installed file, only for `updated`.
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

/// Updates the projects `projects` of an installation. Redoes the check with the same sources as
/// `check_updates` instead of trusting the frontend, then hands each project to its source. An
/// error on one project does not stop the others; a repeated project is updated only once.
/// Blocking.
///
/// Returns `Err` only if the initial check fails: in that case nothing is touched.
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

/// Like `update_addons`, with the initial check's fingerprints taken from `cache`. With
/// `reinstall` (issue #148) an up-to-date project is downloaded again anyway: each source installs
/// the file it would pick for an update, even if it is the installed version.
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
    // The check applies to all requested projects: each one is in `checking` until its
    // turn. A project skipped or failed before the download gets no further phases: the returned
    // outcome stays the final answer, and whoever listens to the event closes the progress there.
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
    // With `reinstall` an up-to-date project downloads again the single rule's file, even if it
    // is the installed one; with an available update it stays that one.
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
        // The rule picks nothing: with the option off the reason can be a newer Beta
        // (`betaOnly`), and it is said; otherwise there is no file to reinstall.
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
    // Without an update the rule's pick is not newer than the installed one: if it is a
    // different file (for example an Alpha installed by hand, or a Beta with the option now off)
    // reinstalling it would be a return to an earlier version, and that is not done silently.
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

/// A blocked CurseForge project (D13) updated or reinstalled from its GitHub mirror (D34,
/// issue #163): the CurseForge `latest` file is read again with the project, the mirror is
/// recomputed (the release is already in GitHub's cache if the check read it) and the GitHub
/// zip replaces the folders. The record stays CurseForge, with the matching file id.
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
            let message = github_mirror::unavailable_message(&reason);
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

/// Records the CurseForge file `file` installed in the folders `folders` and returns the
/// `updated` outcome, with `note` in the message. The folders are already replaced: if the record
/// cannot be saved the installation stays successful, and the message says so. Used by the
/// CurseForge download and the GitHub mirror one (issue #163, then installing from Search).
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

/// The installed file's folders that the update can remove if the new zip no longer
/// has them. The returned names are the ones read from disk, never those of CurseForge's response.
/// A folder stays if:
/// - it is no longer on disk, or its fingerprint is not that of the installed module (it was
///   modified or replaced by something else);
/// - it is a module of another recognized project's installed file (a library
///   shipped by several addons);
/// - an addon that is not part of this project declares it among its `.toc` dependencies,
///   required or optional (this also protects addons CurseForge does not recognize).
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

/// Downloads `file`, extracts it into staging and replaces the folders. The folders of
/// `old_modules` the new zip does not contain are removed in the same `replace`. Returns the
/// zip's folders, now installed.
///
/// `check` receives the zip's folders after extraction and before `replace`: if it rejects,
/// `AddOns` stays as it was and the staging is deleted. The update rejects nothing;
/// installing a new addon (`install_new`) uses it for other projects' folders.
pub(crate) fn install_file<T: Transport>(
    client: &CurseForgeClient<T>,
    file: &File,
    addons_dir: &Path,
    old_modules: &[String],
    check: &dyn Fn(&[String]) -> Result<(), String>,
    reporter: &Reporter<'_>,
) -> Result<Vec<String>, String> {
    let bytes = download_file(client, file, reporter)?;
    install_bytes(&bytes, addons_dir, old_modules, check, reporter)
}

/// Downloads the zip of `file` reporting progress: the network part of [`install_file`].
pub(crate) fn download_file<T: Transport>(
    client: &CurseForgeClient<T>,
    file: &File,
    reporter: &Reporter<'_>,
) -> Result<Vec<u8>, String> {
    reporter.download(0, None);
    let bytes = client
        .download_with_progress(file, &mut |received, total| {
            reporter.download(received, total)
        })
        .map_err(|e| e.to_string())?;
    reporter.downloaded(bytes.len() as u64);
    Ok(bytes)
}

/// Extracts the zip `bytes` into staging and replaces the folders, like [`install_file`] after the
/// download: also used by the sources that download with their own transport.
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

    /// `check_updates` with only the CurseForge source and no records, as before D12.
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

    /// `update_addons` with only the CurseForge source, the projects by `mod_id` and the record in
    /// CurseForge form.
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

    /// The Beta option and the reinstall (issue #148) for [`cf_update_addons_with`].
    #[derive(Clone, Copy)]
    struct Mode {
        allow_beta: bool,
        reinstall: bool,
    }

    /// Like [`cf_update_addons`], with `mode` and the progress in `progress`.
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
            latest_files: Vec::new(),
        }
    }

    /// A folder the scan could not list, with the scan's real error.
    fn unreadable(folder: &str) -> InstalledAddon {
        let mut addon = addon(folder, None);
        addon.error = Some(scan::unreadable_folder(&std::io::Error::from(
            std::io::ErrorKind::PermissionDenied,
        )));
        addon
    }

    #[test]
    fn unmatched_and_unreadable_folders() {
        let result = evaluate(
            &[addon("Nope", Some(1)), unreadable("Broken")],
            &matches(Vec::new()),
            &[],
            RETAIL,
            &VersionOverrides::new(),
        );
        assert_eq!(result[0].status, UpdateStatus::NotMatched);
        assert!(result[0].project.is_none());
        assert!(matches!(result[1].status, UpdateStatus::Error { .. }));
    }

    /// The EllesmereUI file with the `_Camelot` folder the computation does not count (issue #212):
    /// the installed one is 20, 21 is newer.
    fn ellesmere(found_partially: bool) -> (FingerprintMatches, Vec<Mod>) {
        let mut published = file(20, "2026-09-01T00:00:00Z", 1, FOREVER, &[1, 2]);
        published.modules[0].name = "EllesmereUI".to_owned();
        // The name in the file and the folder's differ only in case: the game
        // does not distinguish it.
        published.modules[1].name = "ELLESMEREUIFOREVERESSENTIALS".to_owned();
        let found = FingerprintMatch {
            id: 1477613,
            file: published,
            latest_files: vec![file(21, "2026-10-01T00:00:00Z", 1, FOREVER, &[])],
        };
        let matches = if found_partially {
            FingerprintMatches {
                partial_matches: vec![found],
                ..FingerprintMatches::default()
            }
        } else {
            matches(vec![found])
        };
        (matches, vec![project(1477613, Some(true))])
    }

    #[test]
    fn a_folder_without_fingerprint_joins_the_project_of_the_file_that_lists_it() {
        for found_partially in [false, true] {
            let (matches, mods) = ellesmere(found_partially);
            let result = evaluate(
                &[
                    addon("EllesmereUI", Some(1)),
                    addon("EllesmereUIForeverEssentials", None),
                ],
                &matches,
                &mods,
                FOREVER,
                &VersionOverrides::new(),
            );
            let (main, essentials) = (&result[0], &result[1]);
            assert!(matches!(main.status, UpdateStatus::UpdateAvailable { .. }));
            // The same status, project and file as the project's other folders.
            assert_eq!(essentials.status, main.status, "partial: {found_partially}");
            assert_eq!(essentials.source, Some(SourceKind::Curseforge));
            assert_eq!(essentials.project, main.project);
            assert_eq!(essentials.installed_file, main.installed_file);
            assert_eq!(essentials.project_folders, main.project_folders);
            assert_eq!(essentials.reinstall_file, main.reinstall_file);
        }
    }

    #[test]
    fn a_folder_without_fingerprint_that_no_file_lists_is_not_matched() {
        let (matches, mods) = ellesmere(false);
        let result = evaluate(
            &[
                addon("SomethingElse_Forever", None),
                unreadable("EllesmereUI"),
            ],
            &matches,
            &mods,
            FOREVER,
            &VersionOverrides::new(),
        );
        assert_eq!(result[0].status, UpdateStatus::NotMatched);
        assert_eq!(result[0].source, None);
        assert!(result[0].project.is_none());
        // Truly unreadable: it stays an error even if a matched file lists the folder.
        assert!(matches!(result[1].status, UpdateStatus::Error { .. }));
        assert!(result[1].project.is_none());
    }

    #[test]
    fn a_readable_folder_with_only_a_toc_of_its_flavor_is_never_an_error() {
        use crate::addons::toc;
        use crate::test_support::TempDir;

        // Every flavor the app maps to a version type; every known type has at least one,
        // so a new type without a flavor here makes the test fail instead of staying uncovered.
        const FLAVORS: [&str; 6] = [
            "_retail_",
            "_classic_era_",
            "_classic_",
            "_anniversary_",
            "_forever_",
            "_classic_beta_",
        ];
        for (id, name) in VERSION_TYPES {
            assert!(
                FLAVORS.iter().any(|f| version_type_for(f) == Some(id)),
                "no flavor tested for {name}"
            );
        }
        let mut uncounted = 0;
        for flavor in FLAVORS {
            let version_type = version_type_for(flavor).unwrap();
            for suffix in toc::toc_suffixes(flavor) {
                let tmp = TempDir::new();
                let folder = format!("Guard{suffix}");
                let dir = tmp.path().join("Interface").join("AddOns").join(&folder);
                std::fs::create_dir_all(&dir).unwrap();
                std::fs::write(
                    dir.join(format!("{folder}_{suffix}.toc")),
                    "## Interface: 11508\ncore.lua\n",
                )
                .unwrap();
                std::fs::write(dir.join("core.lua"), "-- guard\n").unwrap();
                let scanned = scan::scan_addons(tmp.path(), flavor).unwrap();
                assert!(!scanned[0].is_unreadable(), "{flavor} {suffix}");

                let mut published = file(20, "2026-10-01T00:00:00Z", 1, version_type, &[1, 2]);
                published.modules[0].name = "GuardCore".to_owned();
                published.modules[1].name = folder.clone();
                let listed = matches(vec![FingerprintMatch {
                    id: 9,
                    file: published,
                    latest_files: Vec::new(),
                }]);
                for found in [&matches(Vec::new()), &listed] {
                    let result = evaluate(
                        &scanned,
                        found,
                        &[project(9, Some(true))],
                        version_type,
                        &VersionOverrides::new(),
                    );
                    assert!(
                        !matches!(result[0].status, UpdateStatus::Error { .. }),
                        "{flavor} {suffix}: {:?}",
                        result[0].status
                    );
                }
                if scanned[0].fingerprint.is_none() {
                    uncounted += 1;
                    let result = evaluate(
                        &scanned,
                        &listed,
                        &[project(9, Some(true))],
                        version_type,
                        &VersionOverrides::new(),
                    );
                    assert_eq!(result[0].project.as_ref().map(|p| p.id), Some(9));
                }
            }
        }
        // The case of issue #212 really happens: `_Camelot` does not go into the fingerprint.
        assert!(uncounted > 0);
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
        // A missing allowModDistribution counts as false.
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
        // Without the option the Beta is not an update, but the status names it (issue #141).
        assert!(
            matches!(&result[0].status, UpdateStatus::BetaOnly { latest } if latest.id == 13),
            "{:?}",
            result[0].status
        );
    }

    fn beta_match(latest_files: Vec<File>) -> FingerprintMatch {
        FingerprintMatch {
            id: 7,
            // The installed file is a Beta.
            file: file(10, "2026-09-01T00:00:00Z", 2, FOREVER, &[100]),
            latest_files,
        }
    }

    fn evaluate_beta(found: FingerprintMatch, allow_beta: bool) -> UpdateStatus {
        evaluate_with(
            &[addon("Foo", Some(100))],
            &matches(vec![found]),
            &[],
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
        // As soon as a Release exists, even one older than the Beta, the update is that one.
        let with_release = beta_match(vec![
            file(13, "2026-10-02T00:00:00Z", 2, FOREVER, &[]),
            file(12, "2026-09-20T00:00:00Z", 1, FOREVER, &[]),
        ]);
        assert!(matches!(
            evaluate_beta(with_release, true),
            UpdateStatus::UpdateAvailable { latest } if latest.id == 12
        ));
        // The Alpha never counts.
        let alpha = beta_match(vec![file(15, "2026-10-05T00:00:00Z", 3, FOREVER, &[])]);
        assert_eq!(evaluate_beta(alpha, true), UpdateStatus::UpToDate);
        // With a Release for the version the Beta does not count, not even without the option: no
        // suggestion, because turning it on would change nothing.
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
        // The installed Beta is already the most recent.
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
                unreadable("Broken"),
            ],
            &matches(vec![found]),
            &[project(61284, Some(true))],
            RETAIL,
            &VersionOverrides::new(),
        );
        assert_eq!(result[0].project_folders, ["Details", "Details_Compare2"]);
        // Not recognized or not readable: no project folder.
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
        // DBM-GUI modified: only DBM-Core matches the published file.
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
        // The modified folder's fingerprint is not among the `modules`: it stays unknown.
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
        // The order in the response does not matter: the exact one wins in both cases.
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
        // A folder present only in the partial match uses that one.
        assert_eq!(result[1].project.as_ref().unwrap().id, 8);
    }

    #[test]
    fn an_override_picks_the_classic_era_file_on_a_forever_installation() {
        // An addon that publishes for Classic Era but not for Forever.
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
        // An unrecognized folder has no forced version.
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
            "forever",
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

    /// Pins the current expansion of the progressive Classic: when it changes, this test must be
    /// updated together with `CLASSIC_PROGRESSIVE`, and the change shows in the diff.
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
        assert_eq!(version_type_for("forever"), Some(FOREVER));
        assert_eq!(version_type_for("_forever_"), Some(FOREVER));
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
        // Without a computed mirror the two new fields are there, both `null`.
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
        use crate::sources::curseforge::{BytesResponse, HttpResponse, TransportError};
        use crate::store::InstalledRecord;
        use crate::test_support::TempDir;

        const API: &str = "https://proxy.example.com";

        /// Fake transport that answers by URL and records the requests.
        #[derive(Default)]
        struct Routes {
            fingerprints: String,
            mods: String,
            files: HashMap<String, String>,
            /// The response of `POST /v1/mods/files`; without it, 404.
            file_list: Option<String>,
            zips: HashMap<String, Vec<u8>>,
            /// Shared with the test, which cannot read the transport back from the client.
            log: Arc<Log>,
        }

        #[derive(Default)]
        struct Log {
            gets: Mutex<Vec<String>>,
            posts: Mutex<Vec<String>>,
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
            fn get(&self, url: &str) -> Result<HttpResponse, TransportError> {
                self.log.gets.lock().unwrap().push(url.to_owned());
                reply(self.files.get(url))
            }

            fn post_json(&self, url: &str, _body: String) -> Result<HttpResponse, TransportError> {
                self.log.posts.lock().unwrap().push(url.to_owned());
                if url == format!("{API}/v1/fingerprints/1") {
                    reply(Some(&self.fingerprints))
                } else if url == format!("{API}/v1/mods") {
                    reply(Some(&self.mods))
                } else if url == format!("{API}/v1/mods/files") {
                    reply(self.file_list.as_ref())
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

        /// An addon with a `.toc` and a `.lua`: the content makes it recognizable.
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

        /// An installed project (file 10·id) with a newer file (11·id), if `newer`.
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

        /// `update_addons` without a record, for the tests that do not look at it.
        fn update_without_record<T: Transport>(
            installation: &Installation,
            client: &CurseForgeClient<T>,
            overrides: &VersionOverrides,
            mod_ids: &[u32],
        ) -> Result<Vec<UpdateOutcome>, String> {
            cf_update_addons(installation, client, overrides, mod_ids, &mut |_, _| Ok(()))
        }

        fn client(routes: Routes) -> CurseForgeClient<Routes> {
            CurseForgeClient::new(routes, Some("https://proxy.example.com".to_owned()))
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
            // No working folder left next to AddOns.
            assert_eq!(names(fx.addons.parent().unwrap()), ["AddOns"]);
        }

        /// A partially installed addon (a folder of the published file is missing) arrives only in
        /// `partialMatches`: it is recognized and updated like an exact one.
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
            // After the installation: a folder removed by hand, a modified one, and an
            // unrecognized addon that depends on a library of the project.
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
                // A name that leaves AddOns never becomes a path to remove.
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
                // Gamma (31) has no zip: the CDN answers 404.
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

            // Only the successful installation is recorded, with the zip's folders.
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

            // Beta (not distributable) was never requested or downloaded; Eps (no
            // link) was not downloaded.
            let gets = log.gets.lock().unwrap().clone();
            assert!(!gets.iter().any(|u| u.contains("/mods/2/")), "{gets:?}");
            let downloads = log.downloads.lock().unwrap().clone();
            assert_eq!(downloads, [download_url(31), download_url(11)]);

            // The error on Gamma touched nothing; Alpha is up to date.
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

        // ── Reinstall (issue #148) ──────────────────────────────────────────

        const REINSTALL: Mode = Mode {
            allow_beta: false,
            reinstall: true,
        };

        /// An up-to-date project with file `id` installed, its response and its zip.
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
            // A stray file, outside the fingerprint: the reinstall puts the folder back as it is
            // in the zip.
            fs::write(fx.addons.join("Alpha").join("Junk.txt"), "junk").unwrap();

            // Without reinstall: already up to date, nothing downloaded.
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
            // The folder is the zip's: the stray file is gone.
            assert_eq!(names(&fx.addons.join("Alpha")), ["Alpha.lua", "Alpha.toc"]);
            assert_eq!(names(&fx.addons), ["Alpha", "Other"]);
            // The progress is that of an update.
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

            // Without the option the single rule does not pick the Beta: nothing to reinstall.
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

        /// A Beta installed with the option on, then turned off, and an older Release: the
        /// rule picks the Release, but reinstalling it would go backwards.
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

        /// With the option off and a Beta newer than the installed one the reason names it.
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
            // A Retail file installed; the most recent is Classic Era only.
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

        // ── Fallback to the records (issue #216) ────────────────────────────

        /// A WoW Forever installation with `Ess`, which has only a `_Camelot` `.toc` and so
        /// no fingerprint, and `Plain`, with a fingerprint CurseForge does not know.
        fn forever_fixture() -> Fixture {
            let tmp = TempDir::new();
            let path = tmp.path().join("World of Warcraft").join("_forever_");
            let addons = path.join("Interface").join("AddOns");
            let ess = addons.join("Ess");
            fs::create_dir_all(&ess).unwrap();
            fs::write(
                ess.join("Ess_Camelot.toc"),
                "## Interface: 11508\n## Title: Ess\ncore.lua\n",
            )
            .unwrap();
            fs::write(ess.join("core.lua"), "ess v1").unwrap();
            write_addon(&addons, "Plain", "plain");
            let scanned = scan::scan_addons(&path, "forever").unwrap();
            let ess = scanned.iter().find(|a| a.folder == "Ess").unwrap();
            assert!(ess.fingerprint.is_none() && !ess.is_unreadable());
            let fingerprints = scanned
                .into_iter()
                .filter_map(|a| Some((a.folder, a.fingerprint?)))
                .collect();
            Fixture {
                _tmp: tmp,
                installation: Installation {
                    id: "test".to_owned(),
                    path,
                    flavor: "forever".to_owned(),
                    name: "WoW Forever".to_owned(),
                },
                addons,
                fingerprints,
            }
        }

        fn forever_file(mod_id: u32, id: u32, date: &str, modules: &[(&str, u32)]) -> Value {
            let mut file = file_json(mod_id, id, date, modules);
            file["sortableGameVersions"] =
                json!([{"gameVersionName": "1.15.8", "gameVersionTypeId": FOREVER}]);
            file
        }

        /// Project 77 with installed file 770 and, if `newer`, the newer 771.
        fn recorded_routes(newer: bool) -> Routes {
            let installed = forever_file(77, 770, "2026-09-01T00:00:00Z", &[("Ess", 5)]);
            let latest = if newer {
                forever_file(77, 771, "2026-10-01T00:00:00Z", &[("Ess", 6)])
            } else {
                installed.clone()
            };
            Routes {
                fingerprints: json!({"data": {}}).to_string(),
                mods: json!({"data": [{
                    "id": 77, "name": "Ess", "allowModDistribution": true, "latestFiles": [latest]
                }]})
                .to_string(),
                file_list: Some(json!({ "data": [installed] }).to_string()),
                ..Routes::default()
            }
        }

        fn ess_record(file: &str) -> SourceRecords {
            SourceRecords::from([(
                SourceKind::Curseforge,
                ProjectRecords::from([(
                    "77".to_owned(),
                    SourceRecord {
                        file: file.to_owned(),
                        folders: vec!["Ess".to_owned()],
                    },
                )]),
            )])
        }

        fn check_with_records<T: Transport>(
            fx: &Fixture,
            client: &CurseForgeClient<T>,
            records: &SourceRecords,
        ) -> Vec<AddonUpdate> {
            let overrides = no_overrides();
            let source = CurseForgeSource {
                client,
                overrides: &overrides,
                allow_beta: false,
                github: None,
            };
            check_updates(&fx.installation, &[&source], records).unwrap()
        }

        fn status_of<'a>(updates: &'a [AddonUpdate], folder: &str) -> &'a AddonUpdate {
            updates.iter().find(|u| u.folder == folder).unwrap()
        }

        #[test]
        fn a_folder_without_fingerprint_is_recognised_from_its_record() {
            let fx = forever_fixture();
            for newer in [true, false] {
                let client = client(recorded_routes(newer));
                let updates = check_with_records(&fx, &client, &ess_record("770"));

                let ess = status_of(&updates, "Ess");
                assert_eq!(ess.source, Some(SourceKind::Curseforge));
                let project = ess.project.as_ref().unwrap();
                assert_eq!((project.id, project.name.as_str()), (77, "Ess"));
                assert!(project.allow_mod_distribution);
                assert_eq!(ess.installed_file.as_ref().map(|f| f.id), Some(770));
                assert_eq!(ess.project_folders, ["Ess"]);
                if newer {
                    assert_eq!(
                        ess.status,
                        UpdateStatus::UpdateAvailable {
                            latest: FileSummary {
                                id: 771,
                                name: "File 771".to_owned(),
                                date: "2026-10-01T00:00:00Z".to_owned(),
                            }
                        }
                    );
                } else {
                    assert_eq!(ess.status, UpdateStatus::UpToDate);
                }
                // The record does not concern `Plain`: it stays unrecognized.
                assert_eq!(
                    status_of(&updates, "Plain").status,
                    UpdateStatus::NotMatched
                );
            }
        }

        #[test]
        fn without_a_usable_record_the_folder_is_not_matched() {
            let fx = forever_fixture();
            let gone = Routes {
                file_list: Some(json!({ "data": [] }).to_string()),
                ..recorded_routes(true)
            };
            let other_project = Routes {
                mods: json!({"data": [{"id": 78, "name": "Other"}]}).to_string(),
                ..recorded_routes(true)
            };
            let cases = [
                ("no record", recorded_routes(true), SourceRecords::new()),
                ("file gone", gone, ess_record("770")),
                ("project missing", other_project, ess_record("770")),
                ("bad file id", recorded_routes(true), ess_record("latest")),
            ];
            for (case, routes, records) in cases {
                let client = client(routes);
                let updates = check_with_records(&fx, &client, &records);
                let ess = status_of(&updates, "Ess");
                assert_eq!(ess.status, UpdateStatus::NotMatched, "{case}");
                assert_eq!(ess.source, None, "{case}");
                assert!(ess.project.is_none(), "{case}");
            }
        }

        #[test]
        fn a_failed_read_of_the_recorded_files_keeps_the_fingerprint_results() {
            let fx = forever_fixture();
            let plain = fx.fingerprints["Plain"];
            let routes = Routes {
                fingerprints: json!({"data": {"exactMatches": [{
                    "id": 1,
                    "file": forever_file(1, 10, "2026-09-01T00:00:00Z", &[("Plain", plain)]),
                    "latestFiles": [forever_file(1, 11, "2026-10-01T00:00:00Z", &[])],
                }]}})
                .to_string(),
                mods: json!({"data": [
                    project(1, "Plain", true),
                    {"id": 77, "name": "Ess", "allowModDistribution": true},
                ]})
                .to_string(),
                // `POST /v1/mods/files` answers 404.
                file_list: None,
                ..Routes::default()
            };
            let log = Arc::clone(&routes.log);
            let client = client(routes);
            let updates = check_with_records(&fx, &client, &ess_record("770"));

            assert!(
                log.posts
                    .lock()
                    .unwrap()
                    .contains(&format!("{API}/v1/mods/files"))
            );
            // The check does not fail and the fingerprint applies as before the fallback.
            let plain = status_of(&updates, "Plain");
            assert_eq!(plain.project.as_ref().map(|p| p.id), Some(1));
            assert!(matches!(plain.status, UpdateStatus::UpdateAvailable { .. }));
            let ess = status_of(&updates, "Ess");
            assert_eq!(ess.status, UpdateStatus::NotMatched);
            assert_eq!(ess.source, None);
            let json = serde_json::to_string(&updates).unwrap();
            assert!(!json.contains("Unreadable") && !json.contains("https://proxy.example.com"));
        }

        #[test]
        fn a_folder_named_by_the_records_of_two_projects_is_not_matched() {
            let fx = forever_fixture();
            let file = |mod_id: u32, id: u32| {
                forever_file(mod_id, id, "2026-09-01T00:00:00Z", &[("Ess", 5)])
            };
            let routes = Routes {
                fingerprints: json!({"data": {}}).to_string(),
                mods: json!({"data": [
                    {"id": 77, "name": "Ess", "allowModDistribution": true},
                    {"id": 78, "name": "Other Ess", "allowModDistribution": true},
                ]})
                .to_string(),
                file_list: Some(json!({ "data": [file(77, 770), file(78, 780)] }).to_string()),
                ..Routes::default()
            };
            let client = client(routes);
            let record = |file: &str| SourceRecord {
                file: file.to_owned(),
                folders: vec!["Ess".to_owned()],
            };
            let records = SourceRecords::from([(
                SourceKind::Curseforge,
                ProjectRecords::from([
                    ("77".to_owned(), record("770")),
                    ("78".to_owned(), record("780")),
                ]),
            )]);
            let updates = check_with_records(&fx, &client, &records);
            let ess = status_of(&updates, "Ess");
            assert_eq!(ess.status, UpdateStatus::NotMatched);
            assert!(ess.project.is_none());
        }

        #[test]
        fn a_folder_matched_by_fingerprint_ignores_the_records() {
            let fx = forever_fixture();
            let plain = fx.fingerprints["Plain"];
            let routes = Routes {
                fingerprints: json!({"data": {"exactMatches": [{
                    "id": 1,
                    "file": forever_file(1, 10, "2026-09-01T00:00:00Z", &[("Plain", plain)]),
                    "latestFiles": [forever_file(1, 10, "2026-09-01T00:00:00Z", &[])],
                }]}})
                .to_string(),
                mods: json!({"data": [project(1, "Plain", true)]}).to_string(),
                ..Routes::default()
            };
            let log = Arc::clone(&routes.log);
            let client = client(routes);
            // A record of another project names `Plain`: the fingerprint decides.
            let records = SourceRecords::from([(
                SourceKind::Curseforge,
                ProjectRecords::from([(
                    "77".to_owned(),
                    SourceRecord {
                        file: "770".to_owned(),
                        folders: vec!["Plain".to_owned()],
                    },
                )]),
            )]);
            let updates = check_with_records(&fx, &client, &records);
            let plain = status_of(&updates, "Plain");
            assert_eq!(plain.project.as_ref().map(|p| p.id), Some(1));
            assert_eq!(plain.status, UpdateStatus::UpToDate);
            // No folder to recover: the records' files cannot be read.
            assert!(
                !log.posts
                    .lock()
                    .unwrap()
                    .contains(&format!("{API}/v1/mods/files"))
            );
        }

        #[test]
        fn a_project_recognised_from_its_record_is_updated_and_recorded_again() {
            let fx = forever_fixture();
            let mut routes = recorded_routes(true);
            routes.files.insert(
                format!("{API}/v1/mods/77/files/771"),
                json!({ "data": forever_file(77, 771, "2026-10-01T00:00:00Z", &[("Ess", 6)]) })
                    .to_string(),
            );
            routes.zips.insert(
                download_url(771),
                zip_of(&[
                    ("Ess/Ess_Camelot.toc", "## Title: Ess\ncore.lua\n"),
                    ("Ess/core.lua", "ess v2"),
                ]),
            );
            let client = client(routes);
            let overrides = no_overrides();
            let source = CurseForgeSource {
                client: &client,
                overrides: &overrides,
                allow_beta: false,
                github: None,
            };
            let mut written = Vec::new();
            let outcomes = update_addons(
                &fx.installation,
                &[&source],
                &ess_record("770"),
                &[ProjectKey {
                    source: SourceKind::Curseforge,
                    key: "77".to_owned(),
                }],
                &mut |kind, key, record| {
                    written.push((kind, key.to_owned(), record));
                    Ok(())
                },
                &crate::progress::ignore,
            )
            .unwrap();

            assert_eq!(outcomes.len(), 1);
            assert_eq!(outcomes[0].status, OutcomeStatus::Updated, "{outcomes:?}");
            assert_eq!(outcomes[0].file.as_ref().map(|f| f.id), Some(771));
            assert_eq!(
                fs::read_to_string(fx.addons.join("Ess").join("core.lua")).unwrap(),
                "ess v2"
            );
            assert_eq!(
                written,
                [(
                    SourceKind::Curseforge,
                    "77".to_owned(),
                    SourceRecord {
                        file: "771".to_owned(),
                        folders: vec!["Ess".to_owned()],
                    }
                )]
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

        /// Really updates the addons of a real installation: **modifies `Interface/AddOns`**,
        /// so it is worth making a backup first. The user runs it, by hand, through the proxy,
        /// with the path of the `_<flavor>_` folder and
        /// the projects to update separated by commas:
        ///
        /// `WOW_DON_LIVE_INSTALL=<_flavor_-folder> WOW_DON_LIVE_MOD_IDS=61284 cargo test --manifest-path src-tauri/Cargo.toml live_update_addons -- --ignored --nocapture`
        ///
        /// Without `WOW_DON_LIVE_MOD_IDS` it prints only the projects with an update.
        #[test]
        #[ignore = "uses the network, the wow-don-proxy and modifies a real installation"]
        fn live_update_addons() {
            use crate::sources::curseforge::ReqwestTransport;

            let path = std::env::var("WOW_DON_LIVE_INSTALL").expect("WOW_DON_LIVE_INSTALL");
            let installation = crate::installations::validate(Path::new(&path)).unwrap();
            let config = crate::config::Config::load();
            let client = CurseForgeClient::new(
                ReqwestTransport::new().unwrap(),
                Some(config.curseforge_url.clone()),
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
            // The other sources have no CurseForge id.
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

        /// A fake source: claims the folders in `claims` with a project of its own, or
        /// fails; remembers the folders that were offered to it.
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
            // A claimed folder is not offered to the next source.
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
            // The folders were offered only once: one scan, one check.
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
            // The record names the folder with a different case from the one on disk.
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

        /// A source that marks every offered folder as unreadable, as CurseForge does with a
        /// folder without a fingerprint.
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
            // Nobody recognizes `Other`: the first source's error stays.
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

        /// CurseForge without an address, which the app never is since D44 (#123).
        fn keyless() -> CurseForgeClient<Routes> {
            CurseForgeClient::new(Routes::default(), None)
        }

        fn missing_key() -> String {
            CurseForgeError::NotConfigured.to_string()
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
            // The folder no free source recognizes says why.
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
            // D33: the record says where the app installed the folder from, and it wins even
            // when that source cannot check. Giving it to Tukui would have a project installed
            // from CurseForge updated from another source.
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
            // The repeated project is updated once; the absent source is skipped.
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

        // ── GitHub mirror of blocked projects (D34, issue #163) ─────────────

        mod mirror {
            use super::*;
            use crate::sources::github::GithubSource;
            use crate::sources::http::{ApiResponse, PublicTransport, RedirectResponse};

            const REPO: &str = "owner/Alpha";
            const SOURCE_URL: &str = "https://github.com/owner/Alpha";

            /// Fake GitHub: the releases by API URL, the zips by URL, and the requests made.
            /// No `release.json`: the site gives only the tag, the assets come from the API.
            #[derive(Default)]
            struct Hub {
                /// `releases/latest` on the site → `Location`, when the API is not enough to tell.
                redirects: HashMap<String, String>,
                /// API URL → (status, body, remaining requests).
                api: HashMap<String, (u16, String, u64)>,
                zips: HashMap<String, Vec<u8>>,
                calls: Arc<Mutex<Vec<String>>>,
            }

            impl PublicTransport for Hub {
                fn get(&self, url: &str) -> Result<HttpResponse, TransportError> {
                    self.calls.lock().unwrap().push(url.to_owned());
                    reply(None)
                }

                /// The tag of the release the API lists for the same repository; without it, 404.
                fn get_redirect(&self, url: &str) -> Result<RedirectResponse, TransportError> {
                    self.calls.lock().unwrap().push(url.to_owned());
                    let repo = url.trim_end_matches("/releases/latest");
                    let api = repo.replace("https://github.com/", "https://api.github.com/repos/");
                    let location = self.redirects.get(url).cloned().or_else(|| {
                        let (status, body, _) = self.api.get(&format!("{api}/releases/latest"))?;
                        let release: Value =
                            serde_json::from_str(body).ok().filter(|_| *status == 200)?;
                        Some(format!(
                            "{repo}/releases/tag/{}",
                            release["tag_name"].as_str()?
                        ))
                    });
                    Ok(RedirectResponse {
                        status: if location.is_some() { 302 } else { 404 },
                        location,
                    })
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

            /// The reads of a release without `release.json`: the tag from the site, the missing
            /// `release.json`, the assets from the API.
            fn release_reads(tag: &str) -> [String; 3] {
                [
                    format!("https://github.com/{REPO}/releases/latest"),
                    format!("https://github.com/{REPO}/releases/download/{tag}/release.json"),
                    latest_url(),
                ]
            }

            fn asset_url(tag: &str) -> String {
                format!("https://github.com/{REPO}/releases/download/{tag}/Alpha-{tag}.zip")
            }

            /// A GitHub with latest release `tag` and its only zip `zip`.
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

            /// A blocked project that declares the GitHub repository.
            fn blocked(mod_id: u32, name: &str) -> Value {
                json!({"id": mod_id, "name": name, "allowModDistribution": false,
                       "links": {"sourceUrl": SOURCE_URL}})
            }

            /// The CurseForge file `id` without a download link, as for blocked projects.
            fn blocked_file(mod_id: u32, id: u32, date: &str, modules: &[(&str, u32)]) -> Value {
                let mut file = file_json(mod_id, id, date, modules);
                file["downloadUrl"] = Value::Null;
                file
            }

            /// The CurseForge responses of an installed project (file 10) with update 11.
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
                // The record stays CurseForge, with the matching file id.
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
                // Nothing from CurseForge's CDN; from GitHub one read of the release (reused
                // between check and update) and the zip.
                assert!(cf_log.downloads.lock().unwrap().is_empty());
                let calls = calls.lock().unwrap().clone();
                let mut expected = release_reads("11").to_vec();
                expected.push(asset_url("11"));
                assert_eq!(calls, expected);
            }

            #[test]
            fn mirrors_are_resolved_after_the_github_source_checks() {
                // A registered GitHub project and a blocked CurseForge project: the first one's
                // release is read before the mirror's, so the limit of 60 requests an hour
                // is not used up by the mirrors (review finding on PR #173).
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
                let mut expected = vec![
                    "https://github.com/owner/Gamma/releases/latest".to_owned(),
                    "https://github.com/owner/Gamma/releases/download/2/release.json".to_owned(),
                    gamma.to_owned(),
                ];
                expected.extend(release_reads("11"));
                assert_eq!(*calls.lock().unwrap(), expected);
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

                // Up to date: the check asks GitHub for nothing.
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
                let mut expected = release_reads("10").to_vec();
                expected.push(asset_url("10"));
                assert_eq!(*calls.lock().unwrap(), expected);
            }

            #[test]
            fn the_rate_limit_is_a_reason_not_an_error() {
                let fx = fixture(&[("Alpha", "alpha v1")]);
                let client = client(routes(&fx, blocked(1, "Alpha")));
                // The tag matches; without `release.json` the zip is picked from the API, which
                // answers with the limit.
                let hub = Hub {
                    redirects: HashMap::from([(
                        format!("https://github.com/{REPO}/releases/latest"),
                        format!("https://github.com/{REPO}/releases/tag/11"),
                    )]),
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
