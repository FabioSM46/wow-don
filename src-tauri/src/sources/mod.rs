//! Addon sources behind a common interface (D12). CurseForge is the first implementation
//! (`updates::CurseForgeSource`); the free sources (D31) include Tukui ([`tukui`]),
//! WoWInterface ([`wowinterface`]) and GitHub Releases ([`github`]). [`github_mirror`] looks on
//! GitHub for the same file of a CurseForge project that cannot be downloaded (issue #162).
//!
//! The update check (`updates::check_updates`) offers the installed folders to the sources in
//! the order of D33: a folder registered in `installed-addons.json` goes only to the source of
//! its record, the others to all of them, in list order (CurseForge before the free sources);
//! a folder claimed by a source is not offered to the following ones.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::addons::scan::InstalledAddon;
use crate::installations::Installation;
use crate::progress::ProgressSink;
use crate::store::{ProjectRecords, SourceRecord};
use crate::updates::{AddonUpdate, UpdateOutcome};

pub mod curseforge;
pub mod github;
pub mod github_mirror;
pub mod http;
pub mod tukui;
pub mod wowinterface;

pub use crate::store::SourceKind;

/// A project of a source: the source and the project's id in that source, as a string
/// (`61284` for CurseForge, `owner/repo` for GitHub, `-2` for ElvUI on Tukui).
///
/// In JSON it is `{ "source": …, "key": … }`, the same two fields as `project` in an `AddonUpdate`
/// (`updates::Project`): the frontend builds a `ProjectKey` from `{ source: project.source,
/// key: project.key }` and compares the exclusions of `get_auto_update` on those two fields, never
/// on `project.id`, which is `0` outside CurseForge.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ProjectKey {
    pub source: SourceKind,
    pub key: String,
}

/// What a source needs to update a project: the check just redone on the whole
/// installation.
pub struct UpdateTarget<'a> {
    /// The project's id in the source.
    pub key: &'a str,
    pub addons_dir: &'a Path,
    /// The installation's `flavor`, for the sources that pick the file only when
    /// reinstalling (GitHub, issue #148).
    pub flavor: &'a str,
    /// The scan of `Interface/AddOns`.
    pub addons: &'a [InstalledAddon],
    /// The check of all sources, one entry per folder.
    pub updates: &'a [AddonUpdate],
    /// Where to report download, extraction and installation of the project (issue #127).
    pub progress: &'a ProgressSink<'a>,
    /// Reinstall even a project that is already up to date (issue #148): the source downloads the
    /// file it would pick for an update, even if it is the installed version.
    pub reinstall: bool,
}

impl UpdateTarget<'_> {
    /// The outcome for a project the check did not find among the installed folders. For a
    /// reinstall the reason is that no folder is linked to a source with that project.
    pub fn not_found(&self, source: SourceKind) -> UpdateOutcome {
        let message = if self.reinstall {
            "Not linked to a source: cannot reinstall."
        } else {
            "Not found among the installed addons."
        };
        UpdateOutcome::skipped(source, self.key, "", message)
    }
}

/// What a source says about a project before installing it from a link (issue #109).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkInfo {
    pub name: String,
    /// The version that would be installed, as the source writes it.
    pub version: String,
    /// Whether WoW-don can download it for the installation's game.
    pub downloadable: bool,
    /// Why it cannot be downloaded, or a warning about what would be installed.
    pub message: Option<String>,
    /// The project's page on the source's site.
    pub url: String,
}

/// Checks the top-level folders of an extracted zip before they replace the ones in
/// `AddOns`: an `Err` cancels the installation without touching the disk.
pub type FolderCheck<'a> = dyn Fn(&[String]) -> Result<(), String> + 'a;

/// Writes to `installed-addons.json` what a source has just installed.
pub type RecordWriter<'a> = dyn FnMut(SourceKind, &str, SourceRecord) -> Result<(), String> + 'a;

/// An addon source.
pub trait Source {
    fn kind(&self) -> SourceKind;

    /// Matches the `offered` folders to the source's projects and evaluates the updates. Only
    /// an entry with a project claims the folder. An entry with status `error` and no project
    /// does not claim it: the folder stays with the following sources and the error is shown only
    /// if none recognises it. The folders the source does not recognise can be omitted or
    /// returned as `notMatched`. `records` are the source's records
    /// for the installation. An `Err` applies to all offered folders.
    fn check(
        &self,
        installation: &Installation,
        offered: &[InstalledAddon],
        records: &ProjectRecords,
    ) -> Result<Vec<AddonUpdate>, String>;

    /// Updates project `target.key` to the latest version found by the check: downloads, replaces
    /// the folders and registers with `record`. With `target.reinstall` it does so even for
    /// a project that is already up to date. The source's distribution rules
    /// (for CurseForge `allowModDistribution`, D13) stay here.
    fn update(&self, target: &UpdateTarget<'_>, record: &mut RecordWriter<'_>) -> UpdateOutcome;

    /// After the check of all sources, with the final entries: the work that must not run
    /// before the following sources. CurseForge computes here the GitHub mirror of blocked
    /// projects (D34, issue #163), after the GitHub source's requests. By default nothing.
    fn finish(&self, installation: &Installation, updates: &mut [AddonUpdate]) {
        let _ = (installation, updates);
    }
}
