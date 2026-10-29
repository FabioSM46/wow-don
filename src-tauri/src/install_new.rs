//! Installation of a new addon from the catalog (`install_addon`, issue #70), with its
//! required dependencies (issue #71).
//!
//! It follows the flow of `updates::update_addons` and reuses its `install_file`: download, safe
//! extraction into staging, replacement with restore. In addition, before touching the disk:
//! - a project already present (same rule as the search's `installed` flag,
//!   [`catalog::installed_projects`]) is skipped without any network call;
//! - a zip folder that already exists and belongs to another project (in the records of
//!   `installed-addons.json` or, without records, by the `## X-Curse-Project-ID` of its `.toc`)
//!   makes the installation fail; an existing folder with neither record nor project id
//!   (installed by hand) is replaced.
//!
//! As in WowUp, after a successful installation the file's dependencies with
//! `relationType = 3` (RequiredDependency) are installed, and recursively theirs, with the same
//! rules.
//!
//! A project that CurseForge does not let you download (D13) is installed from its GitHub mirror
//! (D34, issue #164) when the release is the same as the chosen CurseForge file: the GitHub zip is
//! downloaded with the same rules and the same folder check, and the record stays
//! **CurseForge**, with the id of the matching file. [`github_mirror`] gives the same outcome on
//! request, without installing: the search does not query GitHub.
//!
//! Blocking: it must be called off the main thread.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::Path;
use std::sync::{Mutex, MutexGuard, PoisonError};

use serde::Serialize;

use crate::catalog;
use crate::installations::Installation;
use crate::parallel;
use crate::progress::{Phase, Progress, ProgressSink, Reporter};
use crate::sources::SourceKind;
use crate::sources::curseforge::{
    CurseForgeClient, File, Mod, RELATION_REQUIRED_DEPENDENCY, RELEASE_TYPE_BETA, Transport,
    pick_file,
};
use crate::sources::github::Asset;
use crate::sources::github_mirror::{self, MirrorSource, Outcome, Reason};
use crate::store::{InstalledRecord, InstalledRecords};
use crate::updates::{self, FileSummary, VersionOverrides};

/// How many levels of dependencies are followed: the chosen project is level 0, its
/// dependencies level 1. Deeper dependencies are not installed and the outcome says so.
pub const MAX_DEPENDENCY_DEPTH: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum InstallStatus {
    Installed,
    Skipped,
    Error,
}

/// The outcome of installing a project, with the same shape as `UpdateOutcome` plus
/// `dependencyOf` for the dependencies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallOutcome {
    pub mod_id: u32,
    /// The project's name; empty if it was not read from CurseForge (for example when the
    /// project is already installed and there is no need to ask).
    pub name: String,
    pub status: InstallStatus,
    /// Why it was skipped, what went wrong or a warning on a successful installation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// The installed file, only for `installed`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<FileSummary>,
    /// For a dependency, the id of the project that requires it; absent for the chosen project.
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

/// Installs the project `mod_id` into the installation: the file `file_id` if given (as long as it
/// belongs to that project), otherwise the latest Release file for the game version forced for the
/// project (`overrides`) or for the installation's. `records` are the installation's records in
/// `installed-addons.json`; `record` saves the one just installed.
///
/// If the project installs, its required dependencies are installed too, one level
/// at a time up to [`MAX_DEPENDENCY_DEPTH`] levels: each with the latest Release for the
/// same game version as the chosen project and with the same rules, each project at
/// most once (cycles and shared dependencies). The dependencies of a project that is already
/// installed or not installed are not followed. For each level the projects are read with a
/// single `get_mods`, which gives the name also to those already installed or too deep. After each
/// installation the local records and the set of installed projects include the one just
/// done, so a dependency just installed counts as installed and its folders as
/// registered.
///
/// A project blocked by CurseForge (a dependency too) is installed from the GitHub mirror with
/// `github` (D34); without `github`, or without a usable mirror, it stays skipped with the reason.
///
/// Returns the outcome of the chosen project followed by one for each visited dependency, never
/// `Err`: an error on a dependency does not touch the chosen project or the other dependencies.
#[allow(clippy::too_many_arguments)]
pub fn install_addon<T: Transport>(
    installation: &Installation,
    client: &CurseForgeClient<T>,
    github: Option<&(dyn MirrorSource + Sync)>,
    overrides: &VersionOverrides,
    records: &InstalledRecords,
    mod_id: u32,
    file_id: Option<u32>,
    allow_beta: bool,
    record: &mut dyn FnMut(u32, InstalledRecord) -> Result<(), String>,
    progress: &ProgressSink<'_>,
) -> Vec<InstallOutcome> {
    let live = Live {
        client,
        github,
        allow_beta,
    };
    install_addon_with(
        installation,
        &live,
        overrides,
        records,
        mod_id,
        file_id,
        record,
        progress,
    )
}

/// [`install_addon`] with projects, files and zips taken from `fetcher`: the real requests
/// ([`Live`]) or those already made by the first phase of an import ([`Cached`], issue #180).
/// The disk is touched only here, one installation at a time.
#[allow(clippy::too_many_arguments)]
pub(crate) fn install_addon_with(
    installation: &Installation,
    fetcher: &dyn Fetcher,
    overrides: &VersionOverrides,
    records: &InstalledRecords,
    mod_id: u32,
    file_id: Option<u32>,
    record: &mut dyn FnMut(u32, InstalledRecord) -> Result<(), String>,
    progress: &ProgressSink<'_>,
) -> Vec<InstallOutcome> {
    // An installation adds projects and removes none: another project's folder is not
    // replaced, so it is enough to read the disk once and add the installed ones.
    let installed = catalog::installed_projects(installation, records);
    if installed.contains(&mod_id) {
        return vec![InstallOutcome::skipped(mod_id, "", "Already installed.")];
    }
    // The game version of the chosen project also applies to its dependencies.
    let version_type = version_type_of(installation, overrides, mod_id);
    let mut installer = Installer {
        installation,
        fetcher,
        version_type,
        records: records.clone(),
        installed,
        record,
        progress,
    };

    let key = mod_id.to_string();
    if fetcher.announces() {
        Reporter::new(progress, Some(SourceKind::Curseforge), &key, "").phase(Phase::Checking);
    }
    let project = match fetcher.mods(&[mod_id]) {
        Ok(mods) => mods.into_iter().find(|m| m.id == mod_id),
        Err(error) => return vec![InstallOutcome::error(mod_id, "", error)],
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
    // (dependency, project that requires it) of the current level.
    let mut level = Vec::new();
    enqueue(&mut level, &mut visited, &dependencies, mod_id);
    let mut depth = 1;
    while !level.is_empty() {
        let ids: Vec<u32> = level.iter().map(|(dependency, _)| *dependency).collect();
        let projects: Result<HashMap<u32, Mod>, String> = fetcher
            .mods(&ids)
            .map(|mods| mods.into_iter().map(|m| (m.id, m)).collect());
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

/// Adds to the level the dependencies not visited yet, each only once.
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

/// The game version for which the file of `mod_id` is chosen: the one forced for the project
/// or the installation's. It also applies to the project's dependencies.
pub(crate) fn version_type_of(
    installation: &Installation,
    overrides: &VersionOverrides,
    mod_id: u32,
) -> Option<u32> {
    overrides
        .get(&mod_id)
        .copied()
        .or_else(|| updates::version_type_for(&installation.flavor))
}

/// The file chosen for a project with its zip already downloaded: the network part of
/// an installation, which does not touch the disk (issue #180).
/// Neither `Clone` nor a `Debug` with the bytes: the zip can weigh tens of MB.
pub(crate) struct Fetched {
    file: File,
    zip: Vec<u8>,
    /// For the GitHub mirror zip, the outcome's message.
    note: Option<String>,
}

impl std::fmt::Debug for Fetched {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Fetched")
            .field("file", &self.file.id)
            .field("zip_bytes", &self.zip.len())
            .field("note", &self.note)
            .finish()
    }
}

/// Where [`install_addon_with`] takes projects, files and zips from.
pub(crate) trait Fetcher {
    /// The projects `ids` as `get_mods`: those CurseForge knows, or the error of the
    /// request.
    fn mods(&self, ids: &[u32]) -> Result<Vec<Mod>, String>;

    /// The file to install for `project` (the file `file_id`, or the latest for `version_type`)
    /// in the game of `installation` and its zip, or the outcome that says why it is not installed.
    /// The installation is that of the installer: a single source per choice and installation.
    fn fetch(
        &self,
        installation: &Installation,
        project: &Mod,
        version_type: Option<u32>,
        file_id: Option<u32>,
        reporter: &Reporter<'_>,
    ) -> Result<Fetched, InstallOutcome>;

    /// Whether reading the chosen project is announced with `checking`: not when the first phase
    /// of an import has already done it.
    fn announces(&self) -> bool {
        true
    }
}

/// The real requests, made when needed: installation from Search and from links.
pub(crate) struct Live<'a, T: Transport> {
    pub client: &'a CurseForgeClient<T>,
    /// The GitHub mirror of the blocked projects (D34); `None` skips them.
    pub github: Option<&'a (dyn MirrorSource + Sync)>,
    /// A Beta when the version has no Release (issue #141), for the dependencies too.
    pub allow_beta: bool,
}

impl<T: Transport> Live<'_, T> {
    /// [`Fetcher::fetch`], with the file already read in bulk if there is one (`known`).
    fn fetch_known(
        &self,
        installation: &Installation,
        project: &Mod,
        version_type: Option<u32>,
        file_id: Option<u32>,
        known: Option<File>,
        reporter: &Reporter<'_>,
    ) -> Result<Fetched, InstallOutcome> {
        reporter.phase(Phase::Checking);
        let (file, download) = find_file(
            installation,
            self.client,
            self.github,
            project,
            version_type,
            self.allow_beta,
            file_id,
            known,
        )?;
        let downloaded = match download {
            Download::Curseforge => {
                updates::download_file(self.client, &file, reporter).map(|zip| (zip, None))
            }
            Download::Mirror {
                github,
                repo,
                tag,
                asset,
            } => github_mirror::download(github, &asset, reporter)
                .map(|zip| (zip, Some(github_mirror::installed_message(&repo, &tag)))),
        };
        match downloaded {
            Ok((zip, note)) => Ok(Fetched { file, zip, note }),
            Err(message) => Err(InstallOutcome::error(project.id, &project.name, message)),
        }
    }
}

impl<T: Transport> Fetcher for Live<'_, T> {
    fn mods(&self, ids: &[u32]) -> Result<Vec<Mod>, String> {
        self.client.get_mods(ids).map_err(|error| error.to_string())
    }

    fn fetch(
        &self,
        installation: &Installation,
        project: &Mod,
        version_type: Option<u32>,
        file_id: Option<u32>,
        reporter: &Reporter<'_>,
    ) -> Result<Fetched, InstallOutcome> {
        self.fetch_known(installation, project, version_type, file_id, None, reporter)
    }
}

/// The file with a project's zip, or the outcome that says why there is none.
type FetchResult = Result<Fetched, InstallOutcome>;

/// How many entries are requested in a single `get_mods` or `get_files`.
const IDS_PER_REQUEST: usize = 100;

/// The first phase of an import (issue #180): projects, files and zips read in advance by
/// [`prefetch`], which [`Cached`] returns to the installation.
#[derive(Default)]
pub(crate) struct Prefetched {
    /// Each project read with `get_mods`: `None` if CurseForge does not know it. A project
    /// whose request failed is missing, so the second phase requests it.
    mods: Mutex<HashMap<u32, Option<Mod>>>,
    /// By (project, game version), the file with the zip or the outcome that says why not. A
    /// zip leaves when its installation takes it; an outcome stays for whoever still asks for it.
    files: Mutex<HashMap<(u32, Option<u32>), FetchResult>>,
}

fn lock<V>(mutex: &Mutex<V>) -> MutexGuard<'_, V> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Resolves and downloads in parallel, without touching the disk, the projects `projects` (id and
/// game version) and their required dependencies, as [`install_addon_with`] would install them:
/// up to [`MAX_DEPENDENCY_DEPTH`] levels, with the game version of the project that
/// requires them. One level at a time: one `get_mods` and one `get_files` for the whole level, then
/// at most [`parallel::DOWNLOADS_AT_ONCE`] downloads at once. Each project is downloaded only
/// once even if several entries require it; those in `installed` are not downloaded. The
/// dependencies already installed and those one level beyond the limit are also read, without
/// downloading them: the second phase asks for their name with the same level. Reports `checking`,
/// the download and `downloaded` for each downloaded project.
pub(crate) fn prefetch<T: Transport>(
    installation: &Installation,
    live: &Live<'_, T>,
    installed: &HashSet<u32>,
    projects: &[(u32, Option<u32>)],
    prefetched: &Prefetched,
    progress: &(dyn Fn(Progress) + Sync),
) {
    let mut visited = HashSet::new();
    let mut level: Vec<(u32, Option<u32>)> = projects
        .iter()
        .filter(|(id, _)| !installed.contains(id))
        .filter(|project| visited.insert(**project))
        .copied()
        .collect();
    let mut depth = 0;
    while !level.is_empty() {
        let missing: Vec<u32> = {
            let mods = lock(&prefetched.mods);
            let mut ids: Vec<u32> = level
                .iter()
                .map(|(id, _)| *id)
                .filter(|id| !mods.contains_key(id))
                .collect();
            ids.sort_unstable();
            ids.dedup();
            ids
        };
        for chunk in missing.chunks(IDS_PER_REQUEST) {
            // A chunk that fails is not remembered: the second phase requests it.
            if let Ok(found) = live.mods(chunk) {
                let mut by_id: HashMap<u32, Mod> = found.into_iter().map(|m| (m.id, m)).collect();
                let mut mods = lock(&prefetched.mods);
                for id in chunk {
                    mods.insert(*id, by_id.remove(id));
                }
            }
        }
        // Beyond the limit only the names are needed.
        if depth > MAX_DEPENDENCY_DEPTH {
            break;
        }
        // The projects found and not downloaded yet, with the id of the file that would be chosen.
        let todo: Vec<(Mod, Option<u32>, Option<u32>)> = {
            let mods = lock(&prefetched.mods);
            let files = lock(&prefetched.files);
            level
                .iter()
                .filter(|(id, _)| !installed.contains(id))
                .filter(|project| !files.contains_key(project))
                .filter_map(|(id, version_type)| {
                    let project = mods.get(id)?.as_ref()?;
                    let file_id = version_type
                        .and_then(|version| latest_file(project, version, live.allow_beta));
                    Some((project.clone(), *version_type, file_id))
                })
                .collect()
        };
        let file_ids: Vec<u32> = todo.iter().filter_map(|(_, _, file)| *file).collect();
        // A file missing here (or a failed request) is read on its own during the download.
        let known: HashMap<u32, File> = file_ids
            .chunks(IDS_PER_REQUEST)
            .filter_map(|chunk| live.client.get_files(chunk).ok())
            .flatten()
            .map(|file| (file.id, file))
            .collect();
        let results = parallel::map(&todo, |(project, version_type, file_id)| {
            let key = project.id.to_string();
            let reporter =
                Reporter::new(progress, Some(SourceKind::Curseforge), &key, &project.name);
            let known = file_id.and_then(|id| known.get(&id).cloned());
            let fetched =
                live.fetch_known(installation, project, *version_type, None, known, &reporter);
            if fetched.is_ok() {
                reporter.phase(Phase::Downloaded);
            }
            fetched
        });
        let mut next = Vec::new();
        let mut files = lock(&prefetched.files);
        for ((project, version_type, _), fetched) in todo.into_iter().zip(results) {
            if let Ok(fetched) = &fetched {
                for dependency in required_dependencies(&fetched.file) {
                    let item = (dependency, version_type);
                    if visited.insert(item) {
                        next.push(item);
                    }
                }
            }
            files.insert((project.id, version_type), fetched);
        }
        level = next;
        depth += 1;
    }
}

/// [`Prefetched`] in front of the real requests: what the first phase did not read (a file
/// chosen by hand, a project beyond the depth limit) is asked of `live`.
pub(crate) struct Cached<'a, T: Transport> {
    pub live: &'a Live<'a, T>,
    pub prefetched: &'a Prefetched,
}

impl<T: Transport> Fetcher for Cached<'_, T> {
    fn mods(&self, ids: &[u32]) -> Result<Vec<Mod>, String> {
        {
            let mods = lock(&self.prefetched.mods);
            if ids.iter().all(|id| mods.contains_key(id)) {
                return Ok(ids.iter().filter_map(|id| mods[id].clone()).collect());
            }
        }
        self.live.mods(ids)
    }

    fn fetch(
        &self,
        installation: &Installation,
        project: &Mod,
        version_type: Option<u32>,
        file_id: Option<u32>,
        reporter: &Reporter<'_>,
    ) -> Result<Fetched, InstallOutcome> {
        if file_id.is_none() {
            let key = (project.id, version_type);
            let mut files = lock(&self.prefetched.files);
            match files.remove(&key) {
                Some(Ok(fetched)) => return Ok(fetched),
                Some(Err(outcome)) => {
                    files.insert(key, Err(outcome.clone()));
                    return Err(outcome);
                }
                None => {}
            }
        }
        self.live
            .fetch(installation, project, version_type, file_id, reporter)
    }

    fn announces(&self) -> bool {
        false
    }
}

/// The state shared by the installations of one call to [`install_addon_with`].
struct Installer<'a> {
    installation: &'a Installation,
    fetcher: &'a dyn Fetcher,
    /// The game version of the chosen project, for projects without `file_id`.
    version_type: Option<u32>,
    /// The starting records plus those of the installations already done.
    records: InstalledRecords,
    /// The installed projects, updated after each successful installation.
    installed: HashSet<u32>,
    record: &'a mut dyn FnMut(u32, InstalledRecord) -> Result<(), String>,
    /// Where to report the progress of each installed project (issue #127).
    progress: &'a ProgressSink<'a>,
}

impl Installer<'_> {
    /// Installs a project read from CurseForge and not installed yet. Returns the outcome
    /// and, if it was installed, the file's required dependencies.
    fn install(&mut self, project: &Mod, file_id: Option<u32>) -> (InstallOutcome, Vec<u32>) {
        let key = project.id.to_string();
        let reporter = Reporter::new(
            self.progress,
            Some(SourceKind::Curseforge),
            &key,
            &project.name,
        );
        let fetched = match self.fetcher.fetch(
            self.installation,
            project,
            self.version_type,
            file_id,
            &reporter,
        ) {
            Ok(fetched) => fetched,
            Err(outcome) => return (outcome, Vec::new()),
        };
        let mut result = install_fetched(
            self.installation,
            &mut self.records,
            &fetched,
            &project.name,
            self.record,
            &reporter,
        );
        if result.0.status == InstallStatus::Installed {
            self.installed.insert(project.id);
            // A Beta chosen by the app, not by the user in the detail: the outcome says so.
            if file_id.is_none() && fetched.file.release_type == RELEASE_TYPE_BETA {
                result.0.message.get_or_insert_with(|| {
                    "Installed the beta: there is no release for this game version.".to_owned()
                });
            }
        }
        result
    }
}

/// Where the chosen file is downloaded from.
enum Download<'a> {
    /// The `downloadUrl` of the CurseForge file.
    Curseforge,
    /// The zip of the GitHub release that matches the CurseForge file (D34).
    Mirror {
        github: &'a (dyn MirrorSource + Sync),
        repo: String,
        tag: String,
        asset: Asset,
    },
}

/// The message for a blocked project without a GitHub mirror to ask.
const BLOCKED: &str = "The author does not allow downloads from third-party apps: install it from the CurseForge website.";

/// The file to install for the project, already checked: a file of the project, and downloadable
/// from CurseForge (distribution allowed and a `downloadUrl`) or from the GitHub mirror for a
/// blocked project. Otherwise the outcome that explains why not. `known` is the file already
/// read, if any.
#[allow(clippy::too_many_arguments)]
fn find_file<'a, T: Transport>(
    installation: &Installation,
    client: &CurseForgeClient<T>,
    github: Option<&'a (dyn MirrorSource + Sync)>,
    project: &Mod,
    version_type: Option<u32>,
    allow_beta: bool,
    file_id: Option<u32>,
    known: Option<File>,
) -> Result<(File, Download<'a>), InstallOutcome> {
    let mod_id = project.id;
    let name = project.name.as_str();
    // D13: unknown counts as `false`.
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
        known,
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

/// The outcome of a blocked project without a usable mirror: skipped with the reason, or
/// an error when GitHub did not answer (as in the updates).
fn unavailable(mod_id: u32, name: &str, reason: &Reason) -> InstallOutcome {
    let message = github_mirror::unavailable_message(reason);
    match reason {
        Reason::Failed(_) => InstallOutcome::error(mod_id, name, message),
        _ => InstallOutcome::skipped(mod_id, name, &message),
    }
}

/// Why there is no file to install: a `skipped` outcome or an error.
enum FileError {
    Skipped(String),
    /// The installation's game version is not supported: an error for the installation,
    /// a reason for the mirror check.
    Unsupported(String),
    Error(String),
}

/// The CurseForge file `file_id` of the project if given, otherwise the latest for the game
/// version (with the Beta rule), read from CurseForge and checked as a file of the project.
/// It does not check whether the file can be downloaded. `known` is the file already read, if any:
/// with the right id it avoids the request.
fn chosen_file<T: Transport>(
    installation: &Installation,
    client: &CurseForgeClient<T>,
    project: &Mod,
    version_type: Option<u32>,
    allow_beta: bool,
    file_id: Option<u32>,
    known: Option<File>,
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
    let file = match known.filter(|file| file.id == file_id) {
        Some(file) => file,
        None => client
            .get_file(mod_id, file_id)
            .map_err(|error| FileError::Error(error.to_string()))?,
    };
    if file.mod_id != mod_id {
        return Err(FileError::Error(
            "The requested file does not belong to this addon.".to_owned(),
        ));
    }
    Ok(file)
}

/// The outcome of the GitHub mirror of a CurseForge project (`github_mirror` command, issue #164).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MirrorAvailability {
    /// The project is installed from the GitHub zip.
    pub available: bool,
    /// `owner/repo`, only with `available`.
    pub repo: Option<String>,
    /// The release tag, only with `available`.
    pub tag: Option<String>,
    /// Why the mirror is not used (D27, in English); `null` for a project that CurseForge
    /// lets you download, and when the mirror is there.
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

/// The GitHub mirror of the project `mod_id` for the installation, with the same file and the same
/// rule as [`install_addon`] with the same `file_id` (the given file, or the latest for the
/// game version): it is called on request (detail or Install), never during the search, to
/// stay within the limit of 60 requests per hour. A project that CurseForge lets you download makes
/// no request to GitHub. `Err` only when CurseForge does not answer, does not know the
/// project or gives a file of another project; an unsupported game version, a missing
/// file and GitHub's problems are a `reason`.
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
        None,
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

/// The file to install for the version among those CurseForge lists as the project's latest,
/// with the rule of [`pick_file`] (a Beta only with `allow_beta`). The index has no
/// date: the id decides, which CurseForge assigns in increasing order (like the search's latest
/// file).
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

/// Installs the already downloaded zip of `fetched` (from CurseForge or, for a blocked project,
/// from its GitHub mirror), rejecting other projects' folders, then records it both with
/// `record` and in the local `records` (even if `record` fails: the folders are on disk and
/// belong to the project). On a successful installation it also returns the file's required
/// dependencies.
fn install_fetched(
    installation: &Installation,
    records: &mut InstalledRecords,
    fetched: &Fetched,
    name: &str,
    record: &mut dyn FnMut(u32, InstalledRecord) -> Result<(), String>,
    reporter: &Reporter<'_>,
) -> (InstallOutcome, Vec<u32>) {
    let file = &fetched.file;
    let mod_id = file.mod_id;
    let addons_dir = installation.path.join("Interface").join("AddOns");
    let flavor = installation.flavor.as_str();
    let check =
        |folders: &[String]| check_foreign_folders(&addons_dir, flavor, records, mod_id, folders);
    let folders = match updates::install_bytes(&fetched.zip, &addons_dir, &[], &check, reporter) {
        Ok(folders) => folders,
        Err(message) => return (InstallOutcome::error(mod_id, name, message), Vec::new()),
    };
    let installed = InstalledRecord {
        file_id: file.id,
        folders,
    };
    // The folders are already in place: if the registry is not saved the installation stays
    // successful, and the message says so (as in `update_addons`).
    let failed = record(mod_id, installed.clone())
        .err()
        .map(|error| format!("Installed, but the app could not remember its folders: {error}"));
    // The record stays CurseForge even from the mirror: the outcome says the zip comes from GitHub.
    let message = match (fetched.note.clone(), failed) {
        (Some(note), Some(failed)) => Some(format!("{note} {failed}")),
        (note, failed) => note.or(failed),
    };
    records.insert(mod_id, installed);
    (
        InstallOutcome {
            mod_id,
            name: name.to_owned(),
            status: InstallStatus::Installed,
            message,
            file: Some(FileSummary::from(file)),
            dependency_of: None,
        },
        required_dependencies(file),
    )
}

/// The file's required dependencies (`relationType = 3`).
fn required_dependencies(file: &File) -> Vec<u32> {
    file.dependencies
        .iter()
        .filter(|dependency| dependency.relation_type == RELATION_REQUIRED_DEPENDENCY)
        .map(|dependency| dependency.mod_id)
        .collect()
}

/// Rejects the zip if one of its folders already exists in `AddOns` and belongs to another
/// project: a record says so, or, for a folder without a record, the
/// `## X-Curse-Project-ID` of its `.toc` for the flavor (the same rule as
/// [`catalog::installed_projects`]). A folder with neither record nor id is replaced. Names
/// are compared ignoring case, as the game does. An `AddOns` folder
/// that does not exist counts as empty; an unreadable one makes the installation fail, because it
/// would be impossible to know what is being replaced.
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
        // The `.toc` counts only for the folders no record knows, as in the search.
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

/// The folders in `AddOns`: lowercase name → name on disk.
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

    /// Fake transport that answers by URL and records the requests.
    #[derive(Default)]
    struct Routes {
        /// The projects of `POST /v1/mods`: the response contains those requested in the body.
        projects: Vec<Value>,
        files: HashMap<String, String>,
        zips: HashMap<String, Vec<u8>>,
        /// The addresses for which `get` fails as if there were no network.
        broken: Vec<String>,
        /// How long each download takes: to see the downloads in parallel.
        delay: std::time::Duration,
        /// Shared with the test, which cannot reread the transport from the client.
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
            if url == format!("{API}/v1/mods/files") {
                // The files requested among those of `files`, as `get_file` would give them.
                let ids: Value = serde_json::from_str::<Value>(&body).unwrap()["fileIds"].clone();
                let data: Vec<Value> = ids
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter_map(|id| {
                        let suffix = format!("/files/{id}");
                        let body = self.files.iter().find(|(url, _)| url.ends_with(&suffix))?.1;
                        Some(serde_json::from_str::<Value>(body).unwrap()["data"].clone())
                    })
                    .collect();
                return reply(Some(&json!({ "data": data }).to_string()));
            }
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
            std::thread::sleep(self.delay);
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

    /// A project with a Release file for Retail (`id·10`) and one for Classic Era (`id·10+1`),
    /// plus a more recent Retail beta that must not be chosen.
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

    /// The routes of an installable project: file `file_id` with the zip `zip`.
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

    /// The routes of several installable projects, with their dependencies `(id, relationType)`.
    /// Each project has the Retail and Classic Era files of [`project`], and the zip of both
    /// contains the folder `P<id>`.
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

    /// `(modId, status, dependencyOf)` of each outcome, in order.
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

    /// Installs a project without dependencies, with no forced versions, collecting the records
    /// written.
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

    /// Installs with no forced versions, with the dependencies, collecting the records written.
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
        // Nothing remains next to AddOns.
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

    /// A project that has only a Beta for WoW Forever (like RareScanner, issue #141).
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
        // No record, but the `.toc` says `SharedLib` belongs to project 2.
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
        // Project 2 recorded `Alpha`, but the folder is no longer there.
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

        // The given file does not exist: 404 from the API.
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

        // The dependencies of a dependency that is already installed are not followed.
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
        // 1 → 2 → 1 is a cycle; 3 is required by 1 and by 2.
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
        // The name comes from the level's `get_mods`, even without installing.
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
        // The chosen project, then a level [2, 3] and a level [4, 5].
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

        // The forced version of the chosen project applies to the dependency, not its own.
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
        // Each project, dependency included, reports its phases with its own id.
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
        // 2 and 3 carry the same folder: after 2, `Lib` is registered as its own.
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

    /// [`summary`] of an entry's outcomes.
    type Summary = Vec<(u32, InstallStatus, Option<u32>)>;

    /// The dependencies of `graph` for [`two_phase_imports_match_the_sequential_install`]: 1 and 4
    /// both require 3, which requires 6; 7 does not exist.
    fn import_graph() -> Routes {
        graph(&[
            (1, &[(2, REQUIRED), (3, REQUIRED)]),
            (2, &[]),
            (3, &[(6, REQUIRED)]),
            (4, &[(3, REQUIRED)]),
            (6, &[]),
        ])
    }

    /// Installs `entries` one after another with `fetcher`, like the import: each entry sees the
    /// records of the previous ones.
    fn install_entries(fx: &Fixture, fetcher: &dyn Fetcher, entries: &[u32]) -> Vec<Summary> {
        install_entries_with(
            fx,
            fetcher,
            entries,
            InstalledRecords::new(),
            &VersionOverrides::new(),
        )
    }

    /// [`install_entries`] starting from `records` and with the forced versions `overrides`.
    fn install_entries_with(
        fx: &Fixture,
        fetcher: &dyn Fetcher,
        entries: &[u32],
        mut records: InstalledRecords,
        overrides: &VersionOverrides,
    ) -> Vec<Summary> {
        entries
            .iter()
            .map(|mod_id| {
                let mut written = Vec::new();
                let outcomes = install_addon_with(
                    &fx.installation,
                    fetcher,
                    overrides,
                    &records,
                    *mod_id,
                    None,
                    &mut |id, r| {
                        written.push((id, r));
                        Ok(())
                    },
                    &progress::ignore,
                );
                records.extend(written);
                summary(&outcomes)
            })
            .collect()
    }

    fn projects(fx: &Fixture, entries: &[u32]) -> Vec<(u32, Option<u32>)> {
        projects_with(fx, entries, &VersionOverrides::new())
    }

    fn projects_with(
        fx: &Fixture,
        entries: &[u32],
        overrides: &VersionOverrides,
    ) -> Vec<(u32, Option<u32>)> {
        entries
            .iter()
            .map(|id| (*id, version_type_of(&fx.installation, overrides, *id)))
            .collect()
    }

    /// The first and second phase of `entries` on `fx`, starting from `records` and with
    /// `overrides`: the outcomes of the second and the requests and downloads it made.
    fn two_phases(
        fx: &Fixture,
        routes: Routes,
        entries: &[u32],
        records: InstalledRecords,
        overrides: &VersionOverrides,
    ) -> (Vec<Summary>, usize, usize) {
        let log = Arc::clone(&routes.log);
        let client = client(routes);
        let live = Live {
            client: &client,
            github: None,
            allow_beta: false,
        };
        let prefetched = Prefetched::default();
        prefetch(
            &fx.installation,
            &live,
            &catalog::installed_projects(&fx.installation, &records),
            &projects_with(fx, entries, overrides),
            &prefetched,
            &|_| {},
        );
        let requests = log.requests.lock().unwrap().len();
        let downloads = log.downloads.lock().unwrap().len();
        let cached = Cached {
            live: &live,
            prefetched: &prefetched,
        };
        let outcomes = install_entries_with(fx, &cached, entries, records, overrides);
        (
            outcomes,
            log.requests.lock().unwrap().len() - requests,
            log.downloads.lock().unwrap().len() - downloads,
        )
    }

    #[test]
    fn installed_and_too_deep_dependencies_need_no_request_in_the_second_phase() {
        // 3 is already installed; the chain 10 → 11 → … → 17 goes beyond the depth limit.
        let chain: Vec<(u32, Vec<(u32, u8)>)> = (10..=17)
            .map(|id| {
                (
                    id,
                    if id < 17 {
                        vec![(id + 1, REQUIRED)]
                    } else {
                        vec![]
                    },
                )
            })
            .collect();
        let mut nodes: Vec<(u32, &[(u32, u8)])> = vec![
            (1, &[(2, REQUIRED), (3, REQUIRED)]),
            (2, &[]),
            (3, &[(6, REQUIRED)]),
            (4, &[(3, REQUIRED)]),
            (6, &[]),
        ];
        nodes.extend(chain.iter().map(|(id, deps)| (*id, deps.as_slice())));
        let entries = [1, 4, 10];
        let fx = fixture();
        write_folder(&fx.addons, "P3", "");
        let records = InstalledRecords::from([(3, record(30, &["P3"]))]);

        let sequential = fixture();
        write_folder(&sequential.addons, "P3", "");
        let client_one = client(graph(&nodes));
        let live = Live {
            client: &client_one,
            github: None,
            allow_beta: false,
        };
        let expected = install_entries_with(
            &sequential,
            &live,
            &entries,
            records.clone(),
            &VersionOverrides::new(),
        );

        let (actual, requests, downloads) = two_phases(
            &fx,
            graph(&nodes),
            &entries,
            records,
            &VersionOverrides::new(),
        );
        assert_eq!(actual, expected);
        assert!(actual[0].contains(&(3, Skipped, Some(1))));
        assert!(actual[2].iter().any(|o| o.0 == 16 && o.1 == Skipped));
        assert_eq!((requests, downloads), (0, 0));
    }

    #[test]
    fn a_forced_game_version_is_downloaded_once() {
        let fx = fixture();
        let overrides = VersionOverrides::from([(1, CLASSIC_ERA)]);
        let (actual, requests, downloads) = two_phases(
            &fx,
            import_graph(),
            &[1],
            InstalledRecords::new(),
            &overrides,
        );
        assert_eq!(actual[0][0], (1, Installed, None));
        assert_eq!((requests, downloads), (0, 0));
    }

    #[test]
    fn two_phase_imports_match_the_sequential_install() {
        let entries = [1, 7, 4, 3];
        let sequential = fixture();
        let client_one = client(import_graph());
        let live = Live {
            client: &client_one,
            github: None,
            allow_beta: false,
        };
        let expected = install_entries(&sequential, &live, &entries);

        let fx = fixture();
        let routes = import_graph();
        let log = Arc::clone(&routes.log);
        let client = client(routes);
        let live = Live {
            client: &client,
            github: None,
            allow_beta: false,
        };
        let prefetched = Prefetched::default();
        let events = Mutex::new(Vec::new());
        prefetch(
            &fx.installation,
            &live,
            &HashSet::new(),
            &projects(&fx, &entries),
            &prefetched,
            &|p: Progress| events.lock().unwrap().push((p.key, p.phase)),
        );
        // Each zip only once, even 3 which is needed by 1, by 4 and by the list; the files in bulk.
        let mut downloads = log.downloads.lock().unwrap().clone();
        downloads.sort();
        let mut unique = downloads.clone();
        unique.dedup();
        assert_eq!(downloads, unique);
        assert_eq!(downloads.len(), 5);
        let requests = log.requests.lock().unwrap().clone();
        assert!(
            requests.iter().all(|url| !url.contains("/files/")),
            "{requests:?}"
        );
        // The first phase does not touch the disk.
        assert!(names(&fx.addons).is_empty());
        assert!(events.lock().unwrap().iter().all(|(_, phase)| {
            matches!(
                phase,
                Phase::Checking | Phase::Downloading | Phase::Downloaded
            )
        }));

        let cached = Cached {
            live: &live,
            prefetched: &prefetched,
        };
        let before = log.requests.lock().unwrap().len();
        let actual = install_entries(&fx, &cached, &entries);
        assert_eq!(actual, expected);
        assert_eq!(names(&fx.addons), names(&sequential.addons));
        // The second phase asks nothing of the network.
        assert_eq!(log.requests.lock().unwrap().len(), before);
        assert_eq!(log.downloads.lock().unwrap().len(), 5);
        assert_eq!(
            actual[1],
            [(7, Failed, None)],
            "an unknown project does not stop the others"
        );
    }

    #[test]
    fn the_first_phase_downloads_in_parallel() {
        let fx = fixture();
        let entries: Vec<u32> = (1..=parallel::DOWNLOADS_AT_ONCE as u32).collect();
        let nodes: Vec<(u32, &[(u32, u8)])> = entries.iter().map(|id| (*id, &[][..])).collect();
        let delay = std::time::Duration::from_millis(200);
        let client = client(Routes {
            delay,
            ..graph(&nodes)
        });
        let live = Live {
            client: &client,
            github: None,
            allow_beta: false,
        };
        let prefetched = Prefetched::default();
        let start = std::time::Instant::now();
        prefetch(
            &fx.installation,
            &live,
            &HashSet::new(),
            &projects(&fx, &entries),
            &prefetched,
            &|_| {},
        );
        // Close to the slowest download, not to the sum.
        let elapsed = start.elapsed();
        assert!(elapsed < delay * 3, "{elapsed:?}");
        assert_eq!(lock(&prefetched.files).len(), entries.len());
    }

    #[test]
    fn installed_projects_are_not_prefetched() {
        let fx = fixture();
        let routes = import_graph();
        let log = Arc::clone(&routes.log);
        let client = client(routes);
        let live = Live {
            client: &client,
            github: None,
            allow_beta: false,
        };
        let prefetched = Prefetched::default();
        // 4 and 3 are already there: only 1 and its dependency 2 are downloaded.
        prefetch(
            &fx.installation,
            &live,
            &HashSet::from([3, 4]),
            &projects(&fx, &[1, 4]),
            &prefetched,
            &|_| {},
        );
        let mut downloads = log.downloads.lock().unwrap().clone();
        downloads.sort();
        assert_eq!(downloads, [download_url(10), download_url(20)]);
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

    // ── GitHub mirror of blocked projects (D34, issue #164) ───────────

    const REPO: &str = "owner/alpha";

    /// The fake GitHub mirror: the `resolve` outcome per project and the release zips, with the
    /// requests made.
    #[derive(Default)]
    struct FakeMirror {
        outcomes: HashMap<u32, Outcome>,
        zips: HashMap<String, Vec<u8>>,
        /// `(project, CurseForge file)` of each `resolve`.
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
        /// A `v1` release that matches for the project `mod_id`, with the zip `zip`.
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

    /// The routes of a blocked project: the file `file_id` exists but without a `downloadUrl`.
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

    /// Installs with the mirror, with the dependencies, collecting the records written.
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
        // The file of the outcome and of the record is the matching CurseForge one.
        assert_eq!(outcome.file.as_ref().unwrap().id, 10);
        assert_eq!(written, [(1, record(10, &["Alpha"]))]);
        assert_eq!(*mirror.resolved.lock().unwrap(), [(1, 10)]);
        assert_eq!(*mirror.downloads.lock().unwrap(), [asset_url(1)]);
        assert!(log.downloads.lock().unwrap().is_empty());
        assert_eq!(names(&fx.addons), ["Alpha"]);
        assert_eq!(names(fx.addons.parent().unwrap()), ["AddOns"]);
    }

    #[test]
    fn the_first_phase_of_an_import_downloads_the_mirror_zip() {
        // Issue #180: the GitHub mirror is resolved and downloaded in the first phase.
        let fx = fixture();
        let mirror = FakeMirror::default().with_release(1, &[("Alpha/Alpha.toc", "")]);
        let client = client(blocked_routes(1, 10));
        let live = Live {
            client: &client,
            github: Some(&mirror),
            allow_beta: false,
        };
        let prefetched = Prefetched::default();
        prefetch(
            &fx.installation,
            &live,
            &HashSet::new(),
            &projects(&fx, &[1]),
            &prefetched,
            &|_| {},
        );
        assert_eq!(*mirror.downloads.lock().unwrap(), [asset_url(1)]);
        assert!(names(&fx.addons).is_empty());
        let cached = Cached {
            live: &live,
            prefetched: &prefetched,
        };
        let outcomes = install_addon_with(
            &fx.installation,
            &cached,
            &VersionOverrides::new(),
            &InstalledRecords::new(),
            1,
            None,
            &mut |_, _| Ok(()),
            &progress::ignore,
        );
        assert_eq!(summary(&outcomes), [(1, Installed, None)]);
        assert_eq!(
            outcomes[0].message.as_deref(),
            Some(
                "Installed from GitHub (owner/alpha v1): CurseForge blocks third-party downloads."
            )
        );
        // The second phase does not ask GitHub again.
        assert_eq!(mirror.resolved.lock().unwrap().len(), 1);
        assert_eq!(mirror.downloads.lock().unwrap().len(), 1);
        assert_eq!(names(&fx.addons), ["Alpha"]);
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

        // GitHub not answering is an error, not a skip.
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
        // The given file (for example the Beta chosen in the detail), even without a Release
        // for the game version.
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
        // The check downloads nothing.
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

        // A project that CurseForge lets you download: no request to GitHub.
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
        // No file for the game version: the reason is the installation's.
        let mut routes = blocked_routes(1, 10);
        routes.projects[0]["latestFilesIndexes"] = json!([]);
        let result = check_mirror(routes, &mirror).unwrap();
        assert!(!result.available);
        assert_eq!(
            result.reason.as_deref(),
            Some("There is no release file for this game version.")
        );
        // A project CurseForge does not know, or CurseForge not answering, is an error.
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
