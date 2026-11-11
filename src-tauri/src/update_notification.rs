//! System notification after an automatic update round (D47, issue #298).
//!
//! After every automatic round (D41) [`show_round`] may show one notification through
//! `tauri-plugin-notification`, used only from Rust: no `notification:*` permission reaches the
//! frontend. What to show is decided by the pure function [`decide`]; the side effect only calls
//! the plugin, and a failure to show (for example on Linux without a notification server) is
//! written to stderr and changes nothing else: the round's outcome and the tray stay as they are.
//!
//! Nothing is shown with the `updateNotifications` setting off (or unreadable), while the main
//! window is visible and focused (the user is already looking at the app), when the round has
//! nothing to report, or when every pending update was already notified in this session and the
//! round updated nothing. Manual updates never notify: only the automatic round calls this.
//!
//! A round the user requested (issue #304, the tray's "Check for updates") always reports its
//! outcome, under the same setting and window rules: the pending updates even if already
//! notified, "All addons are up to date" when nothing is pending, "Could not check for updates"
//! when an installation's check failed and "Some addons could not be checked" when only single
//! projects failed: a failed check is never reported as up to date.

use std::collections::HashSet;
use std::sync::{Mutex, MutexGuard, PoisonError};

use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_notification::NotificationExt;

use crate::auto_update::RoundReport;
use crate::sources::SourceKind;
use crate::store::{AppSettingsStore, AutoUpdateMode};
use crate::tray::{MAIN_WINDOW, pending_projects, pending_updates};
use crate::updates::FileSummary;

/// The title of every notification.
const TITLE: &str = "WoW-don";

/// A pending update already reported by a notification: the same project in the same
/// installation with the same file is not reported again. A newer file is a new update.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct NotifiedUpdate {
    installation_id: String,
    source: SourceKind,
    key: String,
    latest: FileSummary,
}

/// The text of a notification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub title: String,
    pub body: String,
}

/// The pending updates notified in this session. State managed by Tauri, in memory only: after a
/// restart the first round notifies again. The lock is never held across the plugin call.
#[derive(Debug, Default)]
pub struct Notified(Mutex<HashSet<NotifiedUpdate>>);

impl Notified {
    fn lock(&self) -> MutexGuard<'_, HashSet<NotifiedUpdate>> {
        // A set of already-shown updates: a panic leaves it usable, at worst one repetition.
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// What to notify after `round`, given the updates already notified, and the new set of notified
/// updates (the old one plus this round's pending updates).
///
/// - `notify`: "N addon updates available", N being the tray's count ([`pending_updates`]);
/// - `install`: "N addons updated" when the round updated something, plus "M could not be
///   updated" for the updates left pending (skipped or failed); only the second line when
///   nothing was updated.
///
/// `None` when the round has nothing to report (projects excluded by the user are not in
/// `available`, so they never count) or when it updated nothing and all its pending updates are
/// already in `notified`.
///
/// A `requested` round (issue #304) always has a notice: it ignores `notified` (the user asked),
/// and with nothing to report it says "All addons are up to date", "Could not check for updates"
/// when an installation's check failed, or "Some addons could not be checked" when only single
/// projects' checks failed. The returned set is computed in the same way.
pub fn decide(
    round: &RoundReport,
    notified: &HashSet<NotifiedUpdate>,
    requested: bool,
) -> (Option<Notice>, HashSet<NotifiedUpdate>) {
    let pending: HashSet<NotifiedUpdate> = pending_projects(round)
        .map(|(report, available)| NotifiedUpdate {
            installation_id: report.installation_id.clone(),
            source: available.source,
            key: available.key.clone(),
            latest: available.latest.clone(),
        })
        .collect();
    let updated: usize = match round.mode {
        AutoUpdateMode::Install => round
            .installations
            .iter()
            .map(|report| report.updated.len())
            .sum(),
        AutoUpdateMode::Notify | AutoUpdateMode::Off => 0,
    };
    let something_new = requested || updated > 0 || !pending.is_subset(notified);
    let mut next = notified.clone();
    next.extend(pending);
    if !something_new {
        return (None, next);
    }
    let left = pending_updates(round);
    let mut lines: Vec<String> = match round.mode {
        AutoUpdateMode::Notify => match left {
            0 => Vec::new(),
            1 => vec!["1 addon update available".to_owned()],
            n => vec![format!("{n} addon updates available")],
        },
        AutoUpdateMode::Install => {
            let mut lines = Vec::new();
            match updated {
                0 => {}
                1 => lines.push("1 addon updated".to_owned()),
                n => lines.push(format!("{n} addons updated")),
            }
            if left > 0 {
                lines.push(format!("{left} could not be updated"));
            }
            lines
        }
        AutoUpdateMode::Off => Vec::new(),
    };
    if requested && lines.is_empty() {
        let installation_failed = round
            .installations
            .iter()
            .any(|report| report.check_error.is_some());
        // A project whose check failed is in `errors` and not in `available`.
        let project_failed = round
            .installations
            .iter()
            .any(|report| !report.errors.is_empty());
        lines.push(if installation_failed {
            "Could not check for updates".to_owned()
        } else if project_failed {
            "Some addons could not be checked".to_owned()
        } else {
            "All addons are up to date".to_owned()
        });
    }
    let notice = (!lines.is_empty()).then(|| Notice {
        title: TITLE.to_owned(),
        body: lines.join("\n"),
    });
    (notice, next)
}

/// Shows the notification for an automatic round, if [`decide`] returns one; `requested` for a
/// round the user asked for. Called by the round right after the tray is refreshed, without
/// holding any lock of the round.
pub fn show_round<R: Runtime>(app: &AppHandle<R>, round: &RoundReport, requested: bool) {
    if !setting_on(app) || main_window_in_front(app) {
        return;
    }
    let Some(state) = app.try_state::<Notified>() else {
        return;
    };
    let notice = {
        let mut notified = state.lock();
        let (notice, next) = decide(round, &notified, requested);
        *notified = next;
        notice
    };
    let Some(notice) = notice else {
        return;
    };
    // The plugin dispatches the notification on a background task; an error here only means it
    // could not be prepared. Nothing else depends on it.
    if let Err(error) = app
        .notification()
        .builder()
        .title(notice.title)
        .body(notice.body)
        .show()
    {
        eprintln!("WoW-don: the update notification could not be shown ({error}).");
    }
}

/// The saved `updateNotifications` setting; unreadable counts as off.
fn setting_on<R: Runtime>(app: &AppHandle<R>) -> bool {
    app.try_state::<Mutex<AppSettingsStore>>()
        .and_then(|settings| settings.lock().ok()?.get().ok())
        .is_some_and(|settings| settings.update_notifications)
}

/// The main window is visible and has the focus: the round's outcome is already in view.
fn main_window_in_front<R: Runtime>(app: &AppHandle<R>) -> bool {
    app.get_webview_window(MAIN_WINDOW).is_some_and(|window| {
        window.is_visible().unwrap_or(false) && window.is_focused().unwrap_or(false)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auto_update::{AvailableUpdate, InstallationReport};
    use crate::sources::ProjectKey;
    use crate::updates::UpdateOutcome;
    use AutoUpdateMode::{Install, Notify};

    fn file(id: u32) -> FileSummary {
        FileSummary {
            id,
            name: format!("v{id}"),
            date: "2026-11-10T10:00:00Z".to_owned(),
        }
    }

    fn available(key: &str, file_id: u32) -> AvailableUpdate {
        AvailableUpdate {
            source: SourceKind::Curseforge,
            key: key.to_owned(),
            name: format!("Addon {key}"),
            latest: file(file_id),
        }
    }

    fn report(id: &str, available: Vec<AvailableUpdate>) -> InstallationReport {
        InstallationReport {
            installation_id: id.to_owned(),
            installation_name: id.to_owned(),
            checked_at: "2026-11-10T10:00:00Z".to_owned(),
            available,
            updated: Vec::new(),
            updated_checked: Vec::new(),
            excluded: Vec::new(),
            skipped: Vec::new(),
            errors: Vec::new(),
            check_error: None,
        }
    }

    /// Marks `keys` of `report` as updated by the round, with the installed file.
    fn with_updated(mut report: InstallationReport, keys: &[&str]) -> InstallationReport {
        for key in keys {
            let latest = report
                .available
                .iter()
                .find(|available| available.key == *key)
                .map(|available| available.latest.clone())
                .unwrap();
            report.updated.push(UpdateOutcome::updated(
                SourceKind::Curseforge,
                key,
                &format!("Addon {key}"),
                latest,
                None,
            ));
            report.updated_checked.push(ProjectKey {
                source: SourceKind::Curseforge,
                key: (*key).to_owned(),
            });
        }
        report
    }

    fn round(mode: AutoUpdateMode, installations: Vec<InstallationReport>) -> RoundReport {
        RoundReport {
            mode,
            started_at: "2026-11-10T10:00:00Z".to_owned(),
            finished_at: "2026-11-10T10:01:00Z".to_owned(),
            retrying: false,
            installations,
        }
    }

    fn body(round: &RoundReport, notified: &HashSet<NotifiedUpdate>) -> Option<String> {
        notice_body(round, notified, false)
    }

    fn requested_body(round: &RoundReport, notified: &HashSet<NotifiedUpdate>) -> Option<String> {
        notice_body(round, notified, true)
    }

    fn notice_body(
        round: &RoundReport,
        notified: &HashSet<NotifiedUpdate>,
        requested: bool,
    ) -> Option<String> {
        let (notice, _) = decide(round, notified, requested);
        notice.map(|notice| {
            assert_eq!(notice.title, "WoW-don");
            notice.body
        })
    }

    #[test]
    fn notify_counts_the_pending_updates_in_singular_and_plural() {
        let none = HashSet::new();
        let one = round(Notify, vec![report("retail", vec![available("1", 2)])]);
        assert_eq!(
            body(&one, &none).as_deref(),
            Some("1 addon update available")
        );
        let many = round(
            Notify,
            vec![
                report("retail", vec![available("1", 2), available("2", 2)]),
                report("classic", vec![available("1", 2)]),
            ],
        );
        assert_eq!(
            body(&many, &none).as_deref(),
            Some("3 addon updates available")
        );
    }

    #[test]
    fn install_reports_what_was_updated_and_what_was_left() {
        let none = HashSet::new();
        let three = vec![available("1", 2), available("2", 2), available("3", 2)];

        let updated_only = round(
            Install,
            vec![with_updated(
                report("retail", three.clone()),
                &["1", "2", "3"],
            )],
        );
        assert_eq!(
            body(&updated_only, &none).as_deref(),
            Some("3 addons updated")
        );

        let one_updated = round(
            Install,
            vec![with_updated(
                report("retail", vec![available("1", 2)]),
                &["1"],
            )],
        );
        assert_eq!(
            body(&one_updated, &none).as_deref(),
            Some("1 addon updated")
        );

        let failed_only = round(Install, vec![report("retail", three.clone())]);
        assert_eq!(
            body(&failed_only, &none).as_deref(),
            Some("3 could not be updated")
        );

        let both = round(
            Install,
            vec![with_updated(report("retail", three), &["1", "2"])],
        );
        assert_eq!(
            body(&both, &none).as_deref(),
            Some("2 addons updated\n1 could not be updated")
        );
    }

    #[test]
    fn a_round_with_nothing_to_report_shows_nothing() {
        let none = HashSet::new();
        assert_eq!(body(&round(Notify, Vec::new()), &none), None);
        assert_eq!(
            body(&round(Install, vec![report("retail", Vec::new())]), &none),
            None
        );
        // Only projects excluded by the user: they are not in `available`.
        let mut excluded = report("retail", Vec::new());
        excluded.excluded.push(UpdateOutcome::skipped(
            SourceKind::Curseforge,
            "1",
            "Addon 1",
            "Excluded from automatic updates.",
        ));
        assert_eq!(body(&round(Notify, vec![excluded.clone()]), &none), None);
        assert_eq!(body(&round(Install, vec![excluded]), &none), None);
    }

    #[test]
    fn already_notified_updates_are_not_repeated() {
        let first = round(
            Notify,
            vec![report("retail", vec![available("1", 2), available("2", 2)])],
        );
        let (notice, notified) = decide(&first, &HashSet::new(), false);
        assert!(notice.is_some());
        // The next round finds the same updates: nothing new.
        let (notice, again) = decide(&first, &notified, false);
        assert_eq!(notice, None);
        assert_eq!(again, notified);
        // A subset of them: nothing new either.
        let fewer = round(Notify, vec![report("retail", vec![available("1", 2)])]);
        assert_eq!(body(&fewer, &notified), None);
        // Install mode leaving the same ones pending and updating nothing: silent too.
        let install = round(
            Install,
            vec![report("retail", vec![available("1", 2), available("2", 2)])],
        );
        assert_eq!(body(&install, &notified), None);
    }

    #[test]
    fn something_new_reports_the_whole_count_again() {
        let first = round(Notify, vec![report("retail", vec![available("1", 2)])]);
        let (_, notified) = decide(&first, &HashSet::new(), false);

        // A newer file of an already-notified project is a new update.
        let newer = round(Notify, vec![report("retail", vec![available("1", 3)])]);
        assert_eq!(
            body(&newer, &notified).as_deref(),
            Some("1 addon update available")
        );
        // Another project: the count includes the one already notified.
        let more = round(
            Notify,
            vec![report("retail", vec![available("1", 2), available("2", 2)])],
        );
        assert_eq!(
            body(&more, &notified).as_deref(),
            Some("2 addon updates available")
        );
        // The same project in another installation is another update.
        let other = round(Notify, vec![report("classic", vec![available("1", 2)])]);
        assert!(body(&other, &notified).is_some());
        // An install round that updated something always reports, even with the pending ones
        // already notified.
        let install = round(
            Install,
            vec![with_updated(
                report("retail", vec![available("1", 2), available("9", 2)]),
                &["9"],
            )],
        );
        assert_eq!(
            body(&install, &notified).as_deref(),
            Some("1 addon updated\n1 could not be updated")
        );
    }

    #[test]
    fn the_notified_set_keeps_the_earlier_updates() {
        let first = round(Notify, vec![report("retail", vec![available("1", 2)])]);
        let (_, notified) = decide(&first, &HashSet::new(), false);
        let second = round(Notify, vec![report("retail", vec![available("2", 2)])]);
        let (notice, notified) = decide(&second, &notified, false);
        assert!(notice.is_some());
        assert_eq!(notified.len(), 2);
        // The first one coming back is not new.
        assert_eq!(body(&first, &notified), None);
    }

    #[test]
    fn a_requested_round_repeats_updates_already_notified() {
        let first = round(
            Notify,
            vec![report("retail", vec![available("1", 2), available("2", 2)])],
        );
        let (_, notified) = decide(&first, &HashSet::new(), false);
        assert_eq!(body(&first, &notified), None);
        assert_eq!(
            requested_body(&first, &notified).as_deref(),
            Some("2 addon updates available")
        );
        // The set still records them, and a later scheduled round stays silent.
        let (_, after) = decide(&first, &notified, true);
        assert_eq!(after, notified);
        assert_eq!(body(&first, &after), None);
        // An update first reported by a requested round is not repeated by a scheduled one.
        let fresh = round(Notify, vec![report("retail", vec![available("3", 2)])]);
        let (_, after) = decide(&fresh, &HashSet::new(), true);
        assert_eq!(body(&fresh, &after), None);
    }

    #[test]
    fn a_requested_round_with_nothing_pending_says_all_is_up_to_date() {
        let none = HashSet::new();
        assert_eq!(
            requested_body(&round(Notify, Vec::new()), &none).as_deref(),
            Some("All addons are up to date")
        );
        assert_eq!(
            requested_body(&round(Notify, vec![report("retail", Vec::new())]), &none).as_deref(),
            Some("All addons are up to date")
        );
        // Install mode (the tray's "Update all", issue #305) uses the same text.
        let all_updated = round(
            Install,
            vec![with_updated(
                report("retail", vec![available("1", 2)]),
                &["1"],
            )],
        );
        assert_eq!(
            requested_body(&all_updated, &none).as_deref(),
            Some("1 addon updated")
        );
        assert_eq!(
            requested_body(&round(Install, vec![report("retail", Vec::new())]), &none).as_deref(),
            Some("All addons are up to date")
        );
    }

    #[test]
    fn a_requested_round_whose_check_failed_does_not_claim_up_to_date() {
        let mut failed = report("retail", Vec::new());
        failed.check_error = Some("The source did not answer.".to_owned());
        assert_eq!(
            requested_body(&round(Notify, vec![failed.clone()]), &HashSet::new()).as_deref(),
            Some("Could not check for updates")
        );
        // A scheduled round with a failed check stays silent, as before.
        assert_eq!(
            body(&round(Notify, vec![failed.clone()]), &HashSet::new()),
            None
        );
        // Updates found elsewhere are still reported.
        let found = report("classic", vec![available("1", 2)]);
        assert_eq!(
            requested_body(&round(Notify, vec![failed, found]), &HashSet::new()).as_deref(),
            Some("1 addon update available")
        );
    }

    #[test]
    fn a_requested_round_with_a_failed_project_does_not_claim_up_to_date() {
        let mut failed = report("retail", Vec::new());
        failed.errors.push(crate::auto_update::AutoUpdateError {
            source: Some(SourceKind::Curseforge),
            key: Some("1".to_owned()),
            name: "Addon 1".to_owned(),
            message: "The source did not answer.".to_owned(),
        });
        let clean = report("classic", Vec::new());
        assert_eq!(
            requested_body(&round(Notify, vec![failed.clone(), clean]), &HashSet::new()).as_deref(),
            Some("Some addons could not be checked")
        );
        // A failed installation check takes precedence.
        let mut whole = report("ptr", Vec::new());
        whole.check_error = Some("The source did not answer.".to_owned());
        assert_eq!(
            requested_body(&round(Notify, vec![failed.clone(), whole]), &HashSet::new()).as_deref(),
            Some("Could not check for updates")
        );
        // A scheduled round stays silent, as before.
        assert_eq!(body(&round(Notify, vec![failed]), &HashSet::new()), None);
    }
}
