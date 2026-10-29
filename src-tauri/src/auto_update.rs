//! The automatic update round (D41, issue #200): a backend thread that checks (`notify` mode)
//! or checks and installs (`install`) the updates of every participating installation, shortly
//! after startup and then every `autoUpdateIntervalHours`.
//!
//! The round uses the same sources, pinned versions and Beta option as the manual commands
//! (`commands::LiveSources`) and the same `check_updates_with` and `update_addons_with`. It takes
//! [`InstallLock`] without waiting: if a command holds it, the round skips and retries after
//! [`BUSY_RETRY`]. A check that failed because of the network retries after [`BACKOFF`], never
//! beyond the interval. Scheduling ([`Schedule`]), project filtering ([`plan`]) and failure
//! classification ([`needs_retry`]) are pure functions.
//!
//! The outcome of the last round stays in memory in [`AutoUpdate`], is read with
//! `get_auto_update_status` and is sent as the [`EVENT`] event at the end of every round: both
//! carry an [`AutoUpdateStatus`]. No outcome contains the CurseForge key, URLs or paths.
//!
//! In `install` the round keeps the previous version of the folders it replaces
//! ([`NextGeneration`], issue #201): the last round of an installation is undone with
//! `undo_last_auto_update`, and the status says which installations have an undoable round.

use std::cell::RefCell;
use std::collections::HashSet;
use std::sync::{Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::addons::cache::FingerprintCache;
use crate::addons::scan::InstalledAddon;
use crate::backup::iso_utc;
use crate::commands::{self, InstallLock, LiveSources};
use crate::installations::Installation;
use crate::previous::{NextGeneration, PreviousManifest, UndoableRound};
use crate::sources::{ProjectKey, RecordWriter, Source, SourceKind, UpdateTarget};
use crate::store::{
    AUTO_UPDATE_INTERVALS_HOURS, AddonSettingsStore, AppSettingsStore, AutoUpdateMode,
    InstallationAutoUpdate, InstallationStore, InstalledAddonsStore, ProjectRecords, SourceRecords,
};
use crate::updates::{self, AddonUpdate, FileSummary, OutcomeStatus, UpdateOutcome, UpdateStatus};

/// Name of the Tauri event emitted at the end of every round, with an [`AutoUpdateStatus`].
pub const EVENT: &str = "auto-update";

/// How long the first round waits after startup: the window and the tray start before the
/// network.
pub const STARTUP_DELAY: Duration = Duration::from_secs(30);

/// How long a round waits when skipped because a user command held [`InstallLock`].
pub const BUSY_RETRY: Duration = Duration::from_secs(5 * 60);

/// The waits after the first, second and third (and every later) check failed because of the
/// network, each capped by the chosen interval.
pub const BACKOFF: [Duration; 3] = [
    Duration::from_secs(5 * 60),
    Duration::from_secs(15 * 60),
    Duration::from_secs(60 * 60),
];

/// The bounds of the interval between two rounds, in hours: those of
/// [`AUTO_UPDATE_INTERVALS_HOURS`].
const MIN_INTERVAL_HOURS: u32 = AUTO_UPDATE_INTERVALS_HOURS[0];
const MAX_INTERVAL_HOURS: u32 = AUTO_UPDATE_INTERVALS_HOURS[AUTO_UPDATE_INTERVALS_HOURS.len() - 1];

/// The message for a project with an update the user has excluded (issue #199).
const EXCLUDED: &str = "Excluded from automatic updates.";

// ── Outcome ─────────────────────────────────────────────────────────────────

/// The status of the automatic round: the response of `get_auto_update_status` and the payload
/// of the [`EVENT`] event. In JSON it is camelCase.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutoUpdateStatus {
    /// A round is running right now.
    pub running: bool,
    /// When the next round is due, ISO 8601 UTC (`2026-10-27T10:00:00Z`); `null` with the
    /// `off` mode, during a round and before the thread has scheduled the first one.
    pub next_round_at: Option<String>,
    /// The last finished round; `null` before the first. A round skipped because a command held
    /// the lock does not replace it.
    pub last_round: Option<RoundReport>,
    /// The installations with an undoable automatic round (`undo_last_auto_update`, issue
    /// #201), in the order of `installations.json`: one entry `{ installationId, roundAt, addons }`
    /// per installation, where `addons` is how many projects Undo would put back. An installation
    /// without an entry has nothing to undo. It is read from the `auto-update-previous.json`
    /// manifest, so it holds even after a restart, before the first round.
    pub undoable: Vec<UndoableRound>,
}

/// A finished round.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoundReport {
    /// The mode of the round: `notify` or `install` (with `off` there are no rounds).
    pub mode: AutoUpdateMode,
    pub started_at: String,
    pub finished_at: String,
    /// At least one check failed because of the network: the next round is brought forward
    /// (backoff).
    pub retrying: bool,
    /// One entry per participating installation, in the order of `installations.json`; those
    /// excluded entirely are not there.
    pub installations: Vec<InstallationReport>,
}

/// The outcome of a round on one installation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallationReport {
    pub installation_id: String,
    pub installation_name: String,
    /// When the check was made, ISO 8601 UTC.
    pub checked_at: String,
    /// The participating projects with an update (not excluded), one per project. In
    /// `notify` it is the list to report; in `install` what the round tried to install.
    pub available: Vec<AvailableUpdate>,
    /// `install` only: the updated projects, with the installed file (`file`).
    pub updated: Vec<UpdateOutcome>,
    /// The projects with an update excluded by the user, with `message`
    /// `"Excluded from automatic updates."`: a choice, not a problem (issue #214).
    pub excluded: Vec<UpdateOutcome>,
    /// `install` only: the projects with an update that the source skips, with the reason
    /// (`message`), for example `allowModDistribution = false` without a GitHub mirror.
    pub skipped: Vec<UpdateOutcome>,
    /// The errors of single projects or folders, with today's message: a failed update (on
    /// Windows a file opened by the game blocks the `rename`) or a folder that no source could
    /// check. They are retried on the next round. In `install` there is also, with `source`,
    /// `key` and `name` empty, the error for the previous version that could not be kept for
    /// Undo (the updates are installed anyway).
    pub errors: Vec<AutoUpdateError>,
    /// The installation's check failed entirely (folder unreachable, no source answering,
    /// unreadable settings): the lists above stay empty, or hold what was known before the
    /// failure.
    pub check_error: Option<String>,
}

/// A project with an available update.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AvailableUpdate {
    pub source: SourceKind,
    /// The project's id in the source, as in `ProjectKey`.
    pub key: String,
    pub name: String,
    /// The file the update would install.
    pub latest: FileSummary,
}

/// An error of a project (`source` and `key`) or of a folder that no source recognised
/// (`source` and `key` null, `name` is the folder).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutoUpdateError {
    pub source: Option<SourceKind>,
    pub key: Option<String>,
    pub name: String,
    pub message: String,
}

impl From<UpdateOutcome> for AutoUpdateError {
    fn from(outcome: UpdateOutcome) -> Self {
        Self {
            source: Some(outcome.source),
            key: Some(outcome.key),
            name: outcome.name,
            message: outcome.message.unwrap_or_default(),
        }
    }
}

// ── Scheduling (pure) ───────────────────────────────────────────────────────

/// How a round ended, to decide the next one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoundResult {
    /// Done, even with errors in single projects: the next one after the interval.
    Done,
    /// Skipped because a command held [`InstallLock`]: retries after [`BUSY_RETRY`].
    Busy,
    /// A check failed because of the network: retries after [`BACKOFF`].
    Failed,
}

/// When to run the next round. Times are durations since the thread started (a base
/// `Instant`), so scheduling can be tested without a clock.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Schedule {
    /// The end of the last attempt, done or skipped.
    last_attempt: Option<Duration>,
    /// How the last attempt ended.
    busy: bool,
    /// The consecutive failed checks; zero after a successful round.
    failures: usize,
}

impl Schedule {
    /// The next round, `None` with the `off` mode (the thread waits to be woken up).
    /// The interval is re-read on every call: a change applies immediately, even backwards (a
    /// round already overdue starts immediately). It is clamped to the allowed intervals (1 to 24
    /// hours): the store already restores the default for an unknown value, but a zero here would
    /// spin the thread without pauses and a huge value would overflow the next round's `Instant`.
    pub fn next_due(&self, mode: AutoUpdateMode, interval_hours: u32) -> Option<Duration> {
        if mode == AutoUpdateMode::Off {
            return None;
        }
        let hours = interval_hours.clamp(MIN_INTERVAL_HOURS, MAX_INTERVAL_HOURS);
        let interval = Duration::from_secs(u64::from(hours) * 60 * 60);
        let Some(last) = self.last_attempt else {
            return Some(STARTUP_DELAY);
        };
        let wait = if self.busy {
            BUSY_RETRY
        } else if self.failures > 0 {
            BACKOFF[(self.failures - 1).min(BACKOFF.len() - 1)]
        } else {
            interval
        };
        Some(last + wait.min(interval))
    }

    /// Records the end of an attempt, `at` since the thread started.
    pub fn finished(&mut self, at: Duration, result: RoundResult) {
        self.last_attempt = Some(at);
        self.busy = result == RoundResult::Busy;
        match result {
            RoundResult::Done => self.failures = 0,
            RoundResult::Failed => self.failures += 1,
            // A skipped round says nothing about the network: the count stays.
            RoundResult::Busy => {}
        }
    }
}

/// Whether a round should be repeated soon: at least one source failed the check entirely, and
/// it is not CurseForge without a key (the missing key is not a network error and is not fixed
/// by retrying).
pub fn needs_retry(failed: &[SourceKind], curseforge_has_key: bool) -> bool {
    failed
        .iter()
        .any(|kind| *kind != SourceKind::Curseforge || curseforge_has_key)
}

/// What the check tells the round for one installation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Plan {
    /// The projects to install in `install`, in folder order and without repeats.
    pub projects: Vec<ProjectKey>,
    pub available: Vec<AvailableUpdate>,
    /// The projects with an update excluded by the user.
    pub excluded: Vec<UpdateOutcome>,
    /// The recognised projects whose check failed (for example the GitHub release of a
    /// recorded project that cannot be read) and the folders that no source could
    /// check.
    pub errors: Vec<AutoUpdateError>,
}

/// Filters the check: a project with `updateAvailable` (not `betaOnly`: the Beta counts only
/// with the option, and then the check already says `updateAvailable`) participates if it is
/// not among the exclusions of `auto`. Several folders of the same project give a single
/// entry. An installation excluded entirely has nothing to do (and the round does not even
/// check it).
pub fn plan(updates: &[AddonUpdate], auto: &InstallationAutoUpdate) -> Plan {
    let mut plan = Plan::default();
    if auto.disabled {
        return plan;
    }
    let mut seen = HashSet::new();
    for update in updates {
        let (project, status) = match (&update.project, &update.status) {
            (Some(project), status) => (project, status),
            (None, UpdateStatus::Error { message }) => {
                plan.errors.push(AutoUpdateError {
                    source: None,
                    key: None,
                    name: update.folder.clone(),
                    message: message.clone(),
                });
                continue;
            }
            (None, _) => continue,
        };
        let key = ProjectKey {
            source: project.source,
            key: project.key.clone(),
        };
        let latest = match status {
            UpdateStatus::UpdateAvailable { latest } => latest,
            UpdateStatus::Error { message } => {
                if seen.insert(key.clone()) {
                    plan.errors.push(AutoUpdateError {
                        source: Some(key.source),
                        key: Some(key.key),
                        name: project.name.clone(),
                        message: message.clone(),
                    });
                }
                continue;
            }
            _ => continue,
        };
        if !seen.insert(key.clone()) {
            continue;
        }
        if auto.excluded.contains(&key) {
            plan.excluded.push(UpdateOutcome::skipped(
                key.source,
                &key.key,
                &project.name,
                EXCLUDED,
            ));
            continue;
        }
        plan.available.push(AvailableUpdate {
            source: key.source,
            key: key.key.clone(),
            name: project.name.clone(),
            latest: latest.clone(),
        });
        plan.projects.push(key);
    }
    plan
}

// ── Round on one installation ───────────────────────────────────────────────

/// A source that remembers whether its check failed entirely; otherwise it passes everything
/// to the real source, without changing its result.
struct Watched<'a> {
    inner: &'a dyn Source,
    failed: &'a RefCell<Vec<SourceKind>>,
}

impl Source for Watched<'_> {
    fn kind(&self) -> SourceKind {
        self.inner.kind()
    }

    fn check(
        &self,
        installation: &Installation,
        offered: &[InstalledAddon],
        records: &ProjectRecords,
    ) -> Result<Vec<AddonUpdate>, String> {
        let result = self.inner.check(installation, offered, records);
        if result.is_err() {
            self.failed.borrow_mut().push(self.inner.kind());
        }
        result
    }

    fn update(&self, target: &UpdateTarget<'_>, record: &mut RecordWriter<'_>) -> UpdateOutcome {
        // Boundary between projects (issue #201): what a previous project kept without reaching
        // its record does not end up in this one's slot.
        crate::install::take_kept();
        self.inner.update(target, record)
    }

    fn finish(&self, installation: &Installation, updates: &mut [AddonUpdate]) {
        self.inner.finish(installation, updates);
    }
}

/// The round on a participating installation: it checks and, in `install`, updates only the
/// projects of [`plan`] without reinstalling, with `record` for the records. With `previous`
/// the update keeps the replaced folders in that generation (issue #201); the caller
/// confirms it with [`NextGeneration::commit`]. Returns the outcome and the sources whose check
/// failed entirely (for [`needs_retry`]). Blocking.
#[allow(clippy::too_many_arguments)]
pub fn run_installation(
    installation: &Installation,
    mode: AutoUpdateMode,
    auto: &InstallationAutoUpdate,
    cache: Option<&FingerprintCache>,
    sources: &[&dyn Source],
    records: &SourceRecords,
    record: &mut RecordWriter<'_>,
    previous: Option<&mut NextGeneration>,
    checked_at: String,
) -> (InstallationReport, Vec<SourceKind>) {
    let failed = RefCell::new(Vec::new());
    let watched: Vec<Watched<'_>> = sources
        .iter()
        .map(|inner| Watched {
            inner: *inner,
            failed: &failed,
        })
        .collect();
    let watched: Vec<&dyn Source> = watched.iter().map(|s| s as &dyn Source).collect();
    let mut report = InstallationReport {
        installation_id: installation.id.clone(),
        installation_name: installation.name.clone(),
        checked_at,
        available: Vec::new(),
        updated: Vec::new(),
        excluded: Vec::new(),
        skipped: Vec::new(),
        errors: Vec::new(),
        check_error: None,
    };
    match updates::check_updates_with(installation, cache, &watched, records) {
        Err(error) => report.check_error = Some(error),
        Ok(found) => {
            let plan = plan(&found, auto);
            report.available = plan.available;
            report.excluded = plan.excluded;
            report.errors = plan.errors;
            if mode == AutoUpdateMode::Install && !plan.projects.is_empty() {
                // `update_addons_with` redoes the check: a failure there touches nothing.
                let update = |record: &mut RecordWriter<'_>| {
                    updates::update_addons_with(
                        installation,
                        cache,
                        &watched,
                        records,
                        &plan.projects,
                        false,
                        record,
                        &|_| {},
                    )
                };
                let result = match previous {
                    Some(previous) => previous.run(records, record, update),
                    None => update(record),
                };
                match result {
                    Err(error) => report.check_error = Some(error),
                    Ok(outcomes) => {
                        for outcome in outcomes {
                            match outcome.status {
                                OutcomeStatus::Updated => report.updated.push(outcome),
                                OutcomeStatus::Skipped => report.skipped.push(outcome),
                                OutcomeStatus::Error => report.errors.push(outcome.into()),
                            }
                        }
                    }
                }
            }
        }
    }
    (report, failed.into_inner())
}

// ── Thread ──────────────────────────────────────────────────────────────────

/// The state shared between the round's thread and the commands (Tauri-managed state).
#[derive(Default)]
pub struct AutoUpdate {
    shared: Mutex<Shared>,
    wake: Condvar,
}

#[derive(Default)]
struct Shared {
    status: AutoUpdateStatus,
    /// The settings changed: the thread reschedules.
    woken: bool,
}

impl AutoUpdate {
    /// The current status (`get_auto_update_status`).
    pub fn status(&self) -> AutoUpdateStatus {
        self.lock().status.clone()
    }

    /// Makes the thread re-read mode and interval (`set_auto_update`): when the mode goes from
    /// `off` to another one the round restarts without waiting for a restart of the app.
    pub fn wake(&self) {
        self.lock().woken = true;
        self.wake.notify_all();
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Shared> {
        // The status is only an outcome to show: a panic does not leave it inconsistent.
        self.shared.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn update(&self, change: impl FnOnce(&mut AutoUpdateStatus)) -> AutoUpdateStatus {
        let mut shared = self.lock();
        change(&mut shared.status);
        shared.status.clone()
    }

    /// Waits until `due` (forever with `None`). `true` if the time has come, `false` if a
    /// command woke it up earlier.
    fn sleep_until(&self, due: Option<Instant>) -> bool {
        let mut shared = self.lock();
        loop {
            if shared.woken {
                shared.woken = false;
                return false;
            }
            shared = match due {
                None => self
                    .wake
                    .wait(shared)
                    .unwrap_or_else(PoisonError::into_inner),
                Some(due) => {
                    let now = Instant::now();
                    if now >= due {
                        return true;
                    }
                    self.wake
                        .wait_timeout(shared, due - now)
                        .unwrap_or_else(PoisonError::into_inner)
                        .0
                }
            };
        }
    }
}

/// The round's status with `undoable` read from the manifest, for the installations still
/// present: the response of `get_auto_update_status` and the payload of every [`EVENT`] event.
pub fn current_status(app: &AppHandle) -> AutoUpdateStatus {
    with_undoable(app, app.state::<AutoUpdate>().status())
}

fn with_undoable(app: &AppHandle, mut status: AutoUpdateStatus) -> AutoUpdateStatus {
    let ids: Vec<String> = match app.state::<Mutex<InstallationStore>>().lock() {
        Ok(store) => store
            .list()
            .unwrap_or_default()
            .into_iter()
            .map(|installation| installation.id)
            .collect(),
        Err(_) => Vec::new(),
    };
    status.undoable = crate::previous::undoable(&app.state::<PreviousManifest>(), &ids);
    status
}

/// Starts the round's thread; must be called in `setup`, after registering the states.
pub fn start(app: AppHandle) {
    let spawned = std::thread::Builder::new()
        .name("auto-update".to_owned())
        .spawn(move || run(&app));
    if spawned.is_err() {
        eprintln!("Automatic updates are not available: the background thread did not start.");
    }
}

/// The saved mode and interval. An unreadable `settings.json` counts as `off`: without knowing
/// what the user chose the round does not use the network and does not touch the disk.
fn read_settings(app: &AppHandle) -> (AutoUpdateMode, u32) {
    app.state::<Mutex<AppSettingsStore>>()
        .lock()
        .ok()
        .and_then(|store| store.get().ok())
        .map_or((AutoUpdateMode::Off, 0), |settings| {
            (settings.auto_update, settings.auto_update_interval_hours)
        })
}

fn run(app: &AppHandle) {
    let state = app.state::<AutoUpdate>();
    let base = Instant::now();
    let mut schedule = Schedule::default();
    loop {
        let (mode, interval_hours) = read_settings(app);
        let due = schedule.next_due(mode, interval_hours);
        state.update(|status| {
            status.next_round_at = due.map(|due| {
                let wait = due.saturating_sub(base.elapsed());
                iso_utc(SystemTime::now() + wait)
            });
        });
        if !state.sleep_until(due.map(|due| base + due)) {
            continue;
        }
        // The settings may have changed during the wait without waking the thread (for
        // example a file rewritten from outside): they are re-read.
        let (mode, _) = read_settings(app);
        if mode == AutoUpdateMode::Off {
            continue;
        }
        let result = round(app, &state, mode);
        schedule.finished(base.elapsed(), result);
    }
}

/// A round on all participating installations, under [`InstallLock`].
fn round(app: &AppHandle, state: &AutoUpdate, mode: AutoUpdateMode) -> RoundResult {
    let lock = app.state::<InstallLock>();
    let Some(held) = lock.try_hold() else {
        return RoundResult::Busy;
    };
    let started_at = iso_utc(SystemTime::now());
    state.update(|status| {
        status.running = true;
        status.next_round_at = None;
    });
    let (installations, retrying) = round_installations(app, mode);
    let status = state.update(|status| {
        status.running = false;
        status.last_round = Some(RoundReport {
            mode,
            started_at,
            finished_at: iso_utc(SystemTime::now()),
            retrying,
            installations,
        });
    });
    let last_round = status.last_round.clone();
    // An event that fails to send does not stop the round: the outcome stays readable with the
    // command.
    let _ = app.emit(EVENT, &with_undoable(app, status));
    drop(held);
    // The count in the tray (issue #202); without an icon it does nothing. Menu and tooltip are
    // changed on the main thread and this thread waits for it to happen: it is called without
    // holding any lock, so a command waiting for `InstallLock` cannot block the round.
    crate::tray::show_round(app, last_round.as_ref());
    if retrying {
        RoundResult::Failed
    } else {
        RoundResult::Done
    }
}

fn round_installations(app: &AppHandle, mode: AutoUpdateMode) -> (Vec<InstallationReport>, bool) {
    let installations = match app.state::<Mutex<InstallationStore>>().lock() {
        Ok(store) => store.list().unwrap_or_default(),
        Err(_) => Vec::new(),
    };
    if installations.is_empty() {
        return (Vec::new(), false);
    }
    let key = app
        .state::<crate::config::Config>()
        .curseforge_api_key
        .clone();
    let settings = app.state::<Mutex<AddonSettingsStore>>();
    let installed = app.state::<Mutex<InstalledAddonsStore>>();
    let cache = app.state::<FingerprintCache>();
    let allow_beta = commands::beta_when_no_release(&app.state::<Mutex<AppSettingsStore>>());
    let sources = match LiveSources::new(key) {
        Ok(sources) => sources,
        Err(error) => {
            // Without sources no installation is checked: it counts as a failed check.
            let reports = installations
                .iter()
                .map(|installation| failed_report(installation, error.clone()))
                .collect();
            return (reports, true);
        }
    };
    let mut reports = Vec::new();
    let mut retrying = false;
    for installation in &installations {
        let auto = settings
            .lock()
            .map_err(|_| "Addon settings are not available: restart the app.".to_owned())
            .and_then(|store| store.auto_update_for(&installation.id));
        let auto = match auto {
            Ok(auto) if auto.disabled => continue,
            Ok(auto) => auto,
            // Without the exclusions it is unknown what the user wants left as is: no round.
            Err(error) => {
                reports.push(failed_report(installation, error));
                continue;
            }
        };
        let overrides = match commands::overrides_for(&settings, &installation.id) {
            Ok(overrides) => overrides,
            Err(error) => {
                reports.push(failed_report(installation, error));
                continue;
            }
        };
        let records = commands::records_for_matching(&installed, &installation.id);
        let mut record = commands::record_writer(&installed, &cache, installation);
        let mut previous =
            (mode == AutoUpdateMode::Install).then(|| NextGeneration::new(installation));
        let (mut report, failed) = sources.with(&overrides, allow_beta, |sources| {
            run_installation(
                installation,
                mode,
                &auto,
                Some(&cache),
                sources,
                &records,
                &mut record,
                previous.as_mut(),
                iso_utc(SystemTime::now()),
            )
        });
        if let Some(previous) = previous {
            let manifest = app.state::<PreviousManifest>();
            if let Err(message) = previous.commit(
                &manifest,
                &installation.id,
                &report.checked_at,
                &report.updated,
            ) {
                report.errors.push(AutoUpdateError {
                    source: None,
                    key: None,
                    name: String::new(),
                    message,
                });
            }
        }
        retrying |= needs_retry(&failed, sources.has_curseforge_key());
        reports.push(report);
    }
    (reports, retrying)
}

fn failed_report(installation: &Installation, error: String) -> InstallationReport {
    InstallationReport {
        installation_id: installation.id.clone(),
        installation_name: installation.name.clone(),
        checked_at: iso_utc(SystemTime::now()),
        available: Vec::new(),
        updated: Vec::new(),
        excluded: Vec::new(),
        skipped: Vec::new(),
        errors: Vec::new(),
        check_error: Some(error),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use super::*;
    use crate::store::SourceRecord;
    use crate::test_support::TempDir;
    use crate::updates::Project;

    const MIN: Duration = Duration::from_secs(60);
    const HOUR: Duration = Duration::from_secs(60 * 60);

    fn finished(results: &[RoundResult]) -> Schedule {
        let mut schedule = Schedule::default();
        for (i, result) in results.iter().enumerate() {
            schedule.finished(HOUR * (i as u32 + 1), *result);
        }
        schedule
    }

    #[test]
    fn the_first_round_comes_shortly_after_start_and_then_every_interval() {
        let schedule = Schedule::default();
        assert_eq!(
            schedule.next_due(AutoUpdateMode::Notify, 6),
            Some(STARTUP_DELAY)
        );
        let schedule = finished(&[RoundResult::Done]);
        assert_eq!(
            schedule.next_due(AutoUpdateMode::Install, 6),
            Some(HOUR * 7)
        );
        // The interval is re-read on every round.
        assert_eq!(
            schedule.next_due(AutoUpdateMode::Install, 1),
            Some(HOUR * 2)
        );
    }

    #[test]
    fn an_interval_outside_the_allowed_ones_is_clamped() {
        let schedule = finished(&[RoundResult::Done]);
        assert_eq!(schedule.next_due(AutoUpdateMode::Notify, 0), Some(HOUR * 2));
        assert_eq!(
            schedule.next_due(AutoUpdateMode::Notify, u32::MAX),
            Some(HOUR * 25)
        );
        // The backoff also stays above zero with a zero interval.
        let schedule = finished(&[RoundResult::Failed]);
        assert_eq!(
            schedule.next_due(AutoUpdateMode::Notify, 0),
            Some(HOUR + MIN * 5)
        );
    }

    #[test]
    fn with_the_mode_off_nothing_is_planned() {
        assert_eq!(Schedule::default().next_due(AutoUpdateMode::Off, 6), None);
        let schedule = finished(&[RoundResult::Failed]);
        assert_eq!(schedule.next_due(AutoUpdateMode::Off, 6), None);
    }

    #[test]
    fn failed_checks_back_off_and_a_good_round_resets_it() {
        use RoundResult::{Done, Failed};
        let due = |results: &[RoundResult], hours| {
            let schedule = finished(results);
            schedule.next_due(AutoUpdateMode::Notify, hours).unwrap() - HOUR * results.len() as u32
        };
        assert_eq!(due(&[Failed], 6), MIN * 5);
        assert_eq!(due(&[Failed, Failed], 6), MIN * 15);
        assert_eq!(due(&[Failed, Failed, Failed], 6), MIN * 60);
        assert_eq!(due(&[Failed, Failed, Failed, Failed], 6), MIN * 60);
        assert_eq!(due(&[Failed, Failed, Done], 6), HOUR * 6);
        assert_eq!(due(&[Failed, Done, Failed], 6), MIN * 5);
        // Never beyond the interval.
        assert_eq!(due(&[Failed, Failed, Failed], 1), HOUR);
    }

    #[test]
    fn a_busy_lock_retries_in_five_minutes_and_keeps_the_backoff() {
        use RoundResult::{Busy, Failed};
        let schedule = finished(&[Busy]);
        assert_eq!(
            schedule.next_due(AutoUpdateMode::Install, 6),
            Some(HOUR + BUSY_RETRY)
        );
        // After the skipped round, a new failure is the second in a row.
        let schedule = finished(&[Failed, Busy, Failed]);
        assert_eq!(
            schedule.next_due(AutoUpdateMode::Install, 6),
            Some(HOUR * 3 + MIN * 15)
        );
    }

    #[test]
    fn a_missing_curseforge_key_is_not_a_network_failure() {
        assert!(!needs_retry(&[], true));
        assert!(!needs_retry(&[SourceKind::Curseforge], false));
        assert!(needs_retry(&[SourceKind::Curseforge], true));
        assert!(needs_retry(
            &[SourceKind::Curseforge, SourceKind::Tukui],
            false
        ));
    }

    fn summary(id: u32) -> FileSummary {
        FileSummary {
            id,
            name: format!("file {id}"),
            date: "2026-10-27T00:00:00Z".to_owned(),
        }
    }

    fn row(folder: &str, project: Option<(SourceKind, &str)>, status: UpdateStatus) -> AddonUpdate {
        AddonUpdate {
            folder: folder.to_owned(),
            source: project.map(|(source, _)| source),
            project: project.map(|(source, key)| Project {
                id: 0,
                source,
                key: key.to_owned(),
                name: format!("{key} name"),
                url: None,
                allow_mod_distribution: true,
            }),
            installed_file: None,
            status,
            version_override: None,
            project_folders: Vec::new(),
            installed_modules: Vec::new(),
            reinstall_file: None,
            mirror: Default::default(),
        }
    }

    fn available(id: u32) -> UpdateStatus {
        UpdateStatus::UpdateAvailable {
            latest: summary(id),
        }
    }

    fn key(source: SourceKind, key: &str) -> ProjectKey {
        ProjectKey {
            source,
            key: key.to_owned(),
        }
    }

    #[test]
    fn only_updates_that_are_not_excluded_are_planned_once_per_project() {
        const CF: SourceKind = SourceKind::Curseforge;
        const GH: SourceKind = SourceKind::Github;
        let rows = [
            row("DBM-Core", Some((CF, "3358")), available(2)),
            row("DBM-GUI", Some((CF, "3358")), available(2)),
            row("Details", Some((CF, "61284")), UpdateStatus::UpToDate),
            row(
                "Beta",
                Some((CF, "9")),
                UpdateStatus::BetaOnly { latest: summary(3) },
            ),
            row("BigWigs", Some((GH, "owner/repo")), available(4)),
            row("Plater", Some((CF, "100")), available(5)),
            row(
                "Lonely",
                None,
                UpdateStatus::Error {
                    message: "CurseForge check failed: offline".to_owned(),
                },
            ),
            row("Unknown", None, UpdateStatus::NotMatched),
            row(
                "Recorded",
                Some((GH, "other/repo")),
                UpdateStatus::Error {
                    message: "GitHub did not respond in time.".to_owned(),
                },
            ),
        ];
        let auto = InstallationAutoUpdate {
            disabled: false,
            excluded: [key(CF, "100")].into(),
        };
        let plan = plan(&rows, &auto);
        assert_eq!(plan.projects, [key(CF, "3358"), key(GH, "owner/repo")]);
        assert_eq!(
            plan.available
                .iter()
                .map(|a| (a.name.as_str(), a.latest.id))
                .collect::<Vec<_>>(),
            [("3358 name", 2), ("owner/repo name", 4)]
        );
        assert_eq!(plan.excluded.len(), 1);
        assert_eq!(plan.excluded[0].key, "100");
        assert_eq!(plan.excluded[0].status, OutcomeStatus::Skipped);
        assert_eq!(plan.excluded[0].message.as_deref(), Some(EXCLUDED));
        // The errors of folders without a source and those of recognised projects.
        assert_eq!(
            plan.errors,
            [
                AutoUpdateError {
                    source: None,
                    key: None,
                    name: "Lonely".to_owned(),
                    message: "CurseForge check failed: offline".to_owned(),
                },
                AutoUpdateError {
                    source: Some(GH),
                    key: Some("other/repo".to_owned()),
                    name: "other/repo name".to_owned(),
                    message: "GitHub did not respond in time.".to_owned(),
                },
            ]
        );

        // Without updates there is nothing to do.
        let nothing = plan_of(&[row("Details", Some((CF, "61284")), UpdateStatus::UpToDate)]);
        assert_eq!(nothing, Plan::default());

        // An installation excluded entirely plans nothing.
        let disabled = InstallationAutoUpdate {
            disabled: true,
            excluded: Default::default(),
        };
        assert_eq!(super::plan(&rows, &disabled), Plan::default());
    }

    fn plan_of(rows: &[AddonUpdate]) -> Plan {
        plan(rows, &InstallationAutoUpdate::default())
    }

    #[test]
    fn try_hold_does_not_wait_for_a_held_lock() {
        let lock = InstallLock::default();
        let held = lock.try_hold().expect("free lock");
        assert!(lock.try_hold().is_none());
        drop(held);
        assert!(lock.try_hold().is_some());
    }

    #[test]
    fn the_status_payload_is_camel_case() {
        let status = AutoUpdateStatus {
            running: false,
            next_round_at: Some("2026-10-27T10:00:00Z".to_owned()),
            last_round: Some(RoundReport {
                mode: AutoUpdateMode::Install,
                started_at: "a".to_owned(),
                finished_at: "b".to_owned(),
                retrying: true,
                installations: vec![InstallationReport {
                    installation_id: "id".to_owned(),
                    installation_name: "Retail".to_owned(),
                    checked_at: "c".to_owned(),
                    available: vec![AvailableUpdate {
                        source: SourceKind::Github,
                        key: "owner/repo".to_owned(),
                        name: "Addon".to_owned(),
                        latest: summary(1),
                    }],
                    updated: Vec::new(),
                    excluded: Vec::new(),
                    skipped: Vec::new(),
                    errors: Vec::new(),
                    check_error: None,
                }],
            }),
            undoable: vec![UndoableRound {
                installation_id: "id".to_owned(),
                round_at: "t".to_owned(),
                addons: 2,
            }],
        };
        let json = serde_json::to_value(&status).unwrap();
        assert_eq!(json["undoable"][0]["installationId"], "id");
        assert_eq!(json["undoable"][0]["addons"], 2);
        assert_eq!(json["nextRoundAt"], "2026-10-27T10:00:00Z");
        let round = &json["lastRound"];
        assert_eq!(round["mode"], "install");
        assert_eq!(round["retrying"], true);
        let installation = &round["installations"][0];
        assert_eq!(installation["installationId"], "id");
        assert_eq!(installation["checkError"], serde_json::Value::Null);
        assert_eq!(installation["available"][0]["source"], "github");
        assert_eq!(installation["available"][0]["latest"]["id"], 1);
        assert_eq!(installation["excluded"], serde_json::json!([]));
        assert_eq!(installation["skipped"], serde_json::json!([]));
    }

    /// A fake source: it claims the folders of `claims` with an update (project =
    /// folder), fails the check with `fails`, and on update gives an error for the
    /// projects of `broken` and skips those of `skips`. It remembers the projects it is asked to
    /// update.
    struct FakeSource {
        kind: SourceKind,
        claims: Vec<&'static str>,
        broken: Vec<&'static str>,
        skips: Vec<&'static str>,
        /// Projects that replace the folders and then fail without writing the record.
        unrecorded: Vec<&'static str>,
        fails: bool,
        asked: RefCell<Vec<String>>,
    }

    impl FakeSource {
        fn new(kind: SourceKind, claims: &[&'static str]) -> Self {
            Self {
                kind,
                claims: claims.to_vec(),
                broken: Vec::new(),
                skips: Vec::new(),
                unrecorded: Vec::new(),
                fails: false,
                asked: RefCell::new(Vec::new()),
            }
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
            if self.fails {
                return Err("Could not reach the source: check the connection.".to_owned());
            }
            Ok(offered
                .iter()
                .filter(|a| self.claims.contains(&a.folder.as_str()))
                .map(|a| row(&a.folder, Some((self.kind, &a.folder)), available(2)))
                .collect())
        }

        fn update(
            &self,
            target: &UpdateTarget<'_>,
            record: &mut RecordWriter<'_>,
        ) -> UpdateOutcome {
            self.asked.borrow_mut().push(target.key.to_owned());
            if self.broken.contains(&target.key) {
                // Today's message for a file opened by the game on Windows.
                return UpdateOutcome::error(
                    self.kind,
                    target.key,
                    target.key,
                    "Could not replace the addon folder: a file is in use.".to_owned(),
                );
            }
            if self.skips.contains(&target.key) {
                return UpdateOutcome::skipped(
                    self.kind,
                    target.key,
                    target.key,
                    "Not distributable.",
                );
            }
            // Like a real source: the project's folder is replaced with `replace`.
            let staging = crate::install::WorkDir::staging(target.addons_dir).unwrap();
            let dir = staging.path().join(target.key);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join(format!("{}.lua", target.key)), "new").unwrap();
            crate::install::replace(target.addons_dir, staging.path(), &[]).unwrap();
            if self.unrecorded.contains(&target.key) {
                return UpdateOutcome::error(
                    self.kind,
                    target.key,
                    target.key,
                    "Could not save the record.".to_owned(),
                );
            }
            record(
                self.kind,
                target.key,
                SourceRecord {
                    file: "2".to_owned(),
                    folders: vec![target.key.to_owned()],
                },
            )
            .unwrap();
            UpdateOutcome::updated(self.kind, target.key, target.key, summary(2), None)
        }
    }

    fn installation_with(tmp: &TempDir, folders: &[&str]) -> Installation {
        let path = tmp.path().join("World of Warcraft").join("_retail_");
        let addons = path.join("Interface").join("AddOns");
        for folder in folders {
            write_addon(&addons, folder);
        }
        Installation {
            id: "test".to_owned(),
            path,
            flavor: "retail".to_owned(),
            name: "Retail".to_owned(),
        }
    }

    fn write_addon(addons: &Path, folder: &str) {
        let dir = addons.join(folder);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join(format!("{folder}.toc")),
            format!("## Interface: 120000\n## Title: {folder}\n{folder}.lua\n"),
        )
        .unwrap();
        fs::write(dir.join(format!("{folder}.lua")), folder).unwrap();
    }

    fn run(
        installation: &Installation,
        mode: AutoUpdateMode,
        auto: &InstallationAutoUpdate,
        sources: &[&dyn Source],
    ) -> (InstallationReport, Vec<SourceKind>, Vec<String>) {
        let mut written = Vec::new();
        let (report, failed) = run_installation(
            installation,
            mode,
            auto,
            None,
            sources,
            &SourceRecords::new(),
            &mut |_, key, _| {
                written.push(key.to_owned());
                Ok(())
            },
            None,
            "2026-10-27T10:00:00Z".to_owned(),
        );
        (report, failed, written)
    }

    #[test]
    fn notify_only_checks() {
        let tmp = TempDir::new();
        let installation = installation_with(&tmp, &["Alpha", "Beta"]);
        let source = FakeSource::new(SourceKind::Tukui, &["Alpha", "Beta"]);
        let auto = InstallationAutoUpdate {
            disabled: false,
            excluded: [key(SourceKind::Tukui, "Beta")].into(),
        };
        let (report, failed, written) =
            run(&installation, AutoUpdateMode::Notify, &auto, &[&source]);
        assert_eq!(report.installation_name, "Retail");
        assert_eq!(report.checked_at, "2026-10-27T10:00:00Z");
        assert_eq!(report.available.len(), 1);
        assert_eq!(report.available[0].key, "Alpha");
        assert_eq!(report.excluded.len(), 1);
        assert_eq!(report.excluded[0].key, "Beta");
        assert_eq!(report.excluded[0].message.as_deref(), Some(EXCLUDED));
        assert!(report.skipped.is_empty());
        assert!(report.updated.is_empty());
        assert!(report.check_error.is_none());
        assert!(failed.is_empty());
        assert!(source.asked.borrow().is_empty());
        assert!(written.is_empty());
    }

    #[test]
    fn install_updates_the_planned_projects_and_a_failure_does_not_stop_the_others() {
        let tmp = TempDir::new();
        let installation = installation_with(&tmp, &["Alpha", "Beta", "Gamma", "Delta"]);
        let mut source = FakeSource::new(SourceKind::Tukui, &["Alpha", "Beta", "Gamma"]);
        source.broken = vec!["Alpha"];
        let auto = InstallationAutoUpdate {
            disabled: false,
            excluded: [key(SourceKind::Tukui, "Gamma")].into(),
        };
        let (report, failed, written) =
            run(&installation, AutoUpdateMode::Install, &auto, &[&source]);
        // Only the projects with an update and not excluded; nobody recognises "Delta".
        assert_eq!(*source.asked.borrow(), ["Alpha", "Beta"]);
        assert_eq!(written, ["Beta"]);
        assert_eq!(report.updated.len(), 1);
        assert_eq!(report.updated[0].key, "Beta");
        assert_eq!(report.updated[0].file.as_ref().map(|f| f.id), Some(2));
        assert_eq!(
            report.errors,
            [AutoUpdateError {
                source: Some(SourceKind::Tukui),
                key: Some("Alpha".to_owned()),
                name: "Alpha".to_owned(),
                message: "Could not replace the addon folder: a file is in use.".to_owned(),
            }]
        );
        assert_eq!(report.excluded.len(), 1);
        assert_eq!(report.excluded[0].key, "Gamma");
        assert!(report.skipped.is_empty());
        assert!(failed.is_empty());
    }

    #[test]
    fn an_excluded_project_goes_to_excluded_and_a_source_skip_to_skipped() {
        let tmp = TempDir::new();
        let installation = installation_with(&tmp, &["Alpha", "Beta", "Gamma"]);
        let mut source = FakeSource::new(SourceKind::Tukui, &["Alpha", "Beta", "Gamma"]);
        source.skips = vec!["Beta"];
        let auto = InstallationAutoUpdate {
            disabled: false,
            excluded: [key(SourceKind::Tukui, "Gamma")].into(),
        };
        let (report, failed, written) =
            run(&installation, AutoUpdateMode::Install, &auto, &[&source]);
        assert_eq!(*source.asked.borrow(), ["Alpha", "Beta"]);
        assert_eq!(written, ["Alpha"]);
        assert_eq!(report.available.len(), 2);
        let keys = |list: &[UpdateOutcome]| list.iter().map(|o| o.key.clone()).collect::<Vec<_>>();
        assert_eq!(keys(&report.updated), ["Alpha"]);
        assert_eq!(keys(&report.excluded), ["Gamma"]);
        assert_eq!(report.excluded[0].message.as_deref(), Some(EXCLUDED));
        assert_eq!(keys(&report.skipped), ["Beta"]);
        assert_eq!(
            report.skipped[0].message.as_deref(),
            Some("Not distributable.")
        );
        assert!(report.errors.is_empty());
        assert!(failed.is_empty());
    }

    #[test]
    fn only_a_round_with_a_generation_keeps_the_previous_version() {
        let tmp = TempDir::new();
        let installation = installation_with(&tmp, &["Alpha"]);
        let interface = installation.path.join("Interface");
        let addons = interface.join("AddOns");
        let source = FakeSource::new(SourceKind::Tukui, &["Alpha"]);
        let auto = InstallationAutoUpdate::default();

        // Without a generation (like a manual update) the old version is deleted.
        let (report, _, _) = run(&installation, AutoUpdateMode::Install, &auto, &[&source]);
        assert_eq!(report.updated.len(), 1);
        assert_eq!(
            fs::read_to_string(addons.join("Alpha/Alpha.lua")).unwrap(),
            "new"
        );
        assert_eq!(
            crate::install::folder_names(&interface).unwrap(),
            ["AddOns"]
        );

        write_addon(&addons, "Alpha");
        let manifest = PreviousManifest::new(tmp.path().join("config").join("previous.json"));
        let mut next = NextGeneration::new(&installation);
        let (report, _) = run_installation(
            &installation,
            AutoUpdateMode::Install,
            &auto,
            None,
            &[&source],
            &SourceRecords::new(),
            &mut |_, _, _| Ok(()),
            Some(&mut next),
            "2026-10-27T10:00:00Z".to_owned(),
        );
        next.commit(
            &manifest,
            &installation.id,
            &report.checked_at,
            &report.updated,
        )
        .unwrap();
        let generation = interface.join(crate::previous::GENERATION_DIR);
        assert_eq!(
            fs::read_to_string(generation.join("0").join("Alpha").join("Alpha.lua")).unwrap(),
            "Alpha"
        );
        let undoable = crate::previous::undoable(&manifest, std::slice::from_ref(&installation.id));
        assert_eq!(undoable.len(), 1);
        assert_eq!(undoable[0].addons, 1);
        assert_eq!(undoable[0].round_at, "2026-10-27T10:00:00Z");
        assert_eq!(manifest.read().unwrap()["test"].projects[0].name, "Alpha");
    }

    #[test]
    fn what_a_project_kept_without_its_record_does_not_go_to_the_next_one() {
        let tmp = TempDir::new();
        let installation = installation_with(&tmp, &["Alpha", "Beta"]);
        let mut source = FakeSource::new(SourceKind::Tukui, &["Alpha", "Beta"]);
        source.unrecorded = vec!["Alpha"];
        let manifest = PreviousManifest::new(tmp.path().join("config").join("previous.json"));
        let mut next = NextGeneration::new(&installation);
        let (report, _) = run_installation(
            &installation,
            AutoUpdateMode::Install,
            &InstallationAutoUpdate::default(),
            None,
            &[&source],
            &SourceRecords::new(),
            &mut |_, _, _| Ok(()),
            Some(&mut next),
            "t".to_owned(),
        );
        assert_eq!(report.updated.len(), 1);
        next.commit(&manifest, &installation.id, "t", &report.updated)
            .unwrap();
        let rounds = manifest.read().unwrap();
        let projects = &rounds["test"].projects;
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].key, "Beta");
        assert_eq!(projects[0].saved, ["Beta"]);
    }

    #[test]
    fn a_failed_source_is_reported_for_the_backoff() {
        let tmp = TempDir::new();
        let installation = installation_with(&tmp, &["Alpha"]);
        let mut offline = FakeSource::new(SourceKind::Curseforge, &[]);
        offline.fails = true;
        let tukui = FakeSource::new(SourceKind::Tukui, &["Alpha"]);
        let (report, failed, _) = run(
            &installation,
            AutoUpdateMode::Install,
            &InstallationAutoUpdate::default(),
            &[&offline, &tukui],
        );
        // A failing source does not stop the others; the failure is counted once per
        // check (here two: the round's and the one of `update_addons_with`).
        assert_eq!(report.updated.len(), 1);
        assert_eq!(failed, [SourceKind::Curseforge, SourceKind::Curseforge]);
        assert!(needs_retry(&failed, true));
        assert!(!needs_retry(&failed, false));

        // If they all fail, the check fails entirely and nothing is installed.
        let (report, failed, written) = run(
            &installation,
            AutoUpdateMode::Install,
            &InstallationAutoUpdate::default(),
            &[&offline],
        );
        assert_eq!(
            report.check_error.as_deref(),
            Some("Could not reach the source: check the connection.")
        );
        assert!(report.available.is_empty());
        assert!(written.is_empty());
        assert_eq!(failed, [SourceKind::Curseforge]);
    }
}
