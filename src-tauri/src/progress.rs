//! Progress of installs and updates (issue #127): a project's phases and the downloaded bytes,
//! delivered to a callback. The modules that install do not know Tauri: the command connects the
//! callback to the window's `install-progress` event.

use std::cell::Cell;
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::sources::SourceKind;

/// Name of the Tauri event that carries a [`Progress`] to the frontend.
pub const EVENT: &str = "install-progress";

/// Minimum time between two download progress steps of the same project: at most about ten
/// events per second, plus the final one.
pub const MIN_INTERVAL: Duration = Duration::from_millis(100);

/// The phase of a project.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Phase {
    /// Reading the installation and the project's information, before the download.
    Checking,
    /// Downloading the zip: `received` and `total` are meaningful only here.
    Downloading,
    /// Zip downloaded, waiting for its turn: only the import, which downloads several entries at
    /// once and installs them one at a time in list order (issue #180).
    Downloaded,
    /// Extracting and checking the zip in a temporary folder.
    Extracting,
    /// Replacing the folders in `AddOns`.
    Installing,
}

/// A progress step. It holds only the project's identity and the counts: no URLs, keys,
/// headers or disk paths.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    /// The project's source; `None` for a zip chosen by the user, which has no source.
    pub source: Option<SourceKind>,
    /// The project's id in the source (empty for a zip chosen by the user).
    pub key: String,
    /// The project's name, empty until it is known.
    pub name: String,
    pub phase: Phase,
    /// Bytes received, only in [`Phase::Downloading`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub received: Option<u64>,
    /// Bytes expected, only in [`Phase::Downloading`] and only if the server declares them.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<u64>,
}

/// The payload of the [`EVENT`] event: the progress with the installation it belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgressEvent<'a> {
    pub installation_id: &'a str,
    #[serde(flatten)]
    pub progress: Progress,
}

/// Where the progress steps go.
pub type ProgressSink<'a> = dyn Fn(Progress) + 'a;

/// A callback that does nothing, for tests that do not look at progress.
#[cfg(test)]
pub fn ignore(_: Progress) {}

/// The progress of one project: calls `sink` with its identity and limits the rate of the
/// download progress steps.
pub struct Reporter<'a> {
    sink: &'a ProgressSink<'a>,
    source: Option<SourceKind>,
    key: &'a str,
    name: &'a str,
    throttle: Throttle,
    /// The last `total` received from the download: the one the server declared, if it did.
    declared_total: Cell<Option<u64>>,
}

impl<'a> Reporter<'a> {
    pub fn new(
        sink: &'a ProgressSink<'a>,
        source: Option<SourceKind>,
        key: &'a str,
        name: &'a str,
    ) -> Self {
        Self {
            sink,
            source,
            key,
            name,
            throttle: Throttle::new(MIN_INTERVAL),
            declared_total: Cell::new(None),
        }
    }

    /// A phase without counts.
    pub fn phase(&self, phase: Phase) {
        self.send(phase, None, None);
    }

    /// Bytes received so far, if enough time has passed since the last progress step.
    pub fn download(&self, received: u64, total: Option<u64>) {
        self.download_at(Instant::now(), received, total);
    }

    /// Like [`download`](Self::download), with the clock passed by the caller.
    pub fn download_at(&self, now: Instant, received: u64, total: Option<u64>) {
        self.declared_total.set(total);
        if self.throttle.ready(now) {
            self.send(Phase::Downloading, Some(received), total);
        }
    }

    /// Download finished: always delivered, with the `total` declared by the server if there was
    /// one. Without `Content-Length` it stays absent here too: the length was not known.
    pub fn downloaded(&self, len: u64) {
        self.send(Phase::Downloading, Some(len), self.declared_total.get());
    }

    fn send(&self, phase: Phase, received: Option<u64>, total: Option<u64>) {
        (self.sink)(Progress {
            source: self.source,
            key: self.key.to_owned(),
            name: self.name.to_owned(),
            phase,
            received,
            total,
        });
    }
}

/// Lets a progress step through on the first call and then only when `interval` has passed
/// since the last one it let through. The caller passes the clock, so the tests do not depend
/// on time.
pub struct Throttle {
    interval: Duration,
    last: Cell<Option<Instant>>,
}

impl Throttle {
    pub fn new(interval: Duration) -> Self {
        Self {
            interval,
            last: Cell::new(None),
        }
    }

    pub fn ready(&self, now: Instant) -> bool {
        let ready = self
            .last
            .get()
            .is_none_or(|last| now.saturating_duration_since(last) >= self.interval);
        if ready {
            self.last.set(Some(now));
        }
        ready
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::cell::RefCell;

    /// Collects the delivered progress steps, for the other modules' tests.
    #[derive(Default)]
    pub struct Collected(pub RefCell<Vec<Progress>>);

    impl Collected {
        pub fn sink(&self) -> impl Fn(Progress) + '_ {
            |progress| self.0.borrow_mut().push(progress)
        }

        /// (key, phase) of the progress steps in order, with equal consecutive steps merged: how
        /// many download steps get through depends on time.
        pub fn phases(&self) -> Vec<(String, Phase)> {
            let mut phases: Vec<(String, Phase)> = Vec::new();
            for p in self.0.borrow().iter() {
                let step = (p.key.clone(), p.phase);
                if phases.last() != Some(&step) {
                    phases.push(step);
                }
            }
            phases
        }

        /// The last download step of `key`: (received, expected).
        pub fn last_download(&self, key: &str) -> Option<(Option<u64>, Option<u64>)> {
            self.0
                .borrow()
                .iter()
                .rev()
                .find(|p| p.key == key && p.phase == Phase::Downloading)
                .map(|p| (p.received, p.total))
        }
    }

    #[test]
    fn the_throttle_lets_the_first_through_then_waits_the_interval() {
        let throttle = Throttle::new(Duration::from_millis(100));
        let start = Instant::now();
        assert!(throttle.ready(start));
        assert!(!throttle.ready(start + Duration::from_millis(50)));
        assert!(!throttle.ready(start + Duration::from_millis(99)));
        assert!(throttle.ready(start + Duration::from_millis(100)));
        assert!(!throttle.ready(start + Duration::from_millis(150)));
        assert!(throttle.ready(start + Duration::from_millis(250)));
    }

    #[test]
    fn the_last_download_step_is_always_delivered() {
        let collected = Collected::default();
        let sink = collected.sink();
        let reporter = Reporter::new(&sink, Some(SourceKind::Curseforge), "61284", "Bagnon");
        let start = Instant::now();
        for (ms, received) in [(0, 10), (40, 20), (80, 30)] {
            reporter.download_at(start + Duration::from_millis(ms), received, Some(100));
        }
        reporter.downloaded(100);
        let steps = collected.0.borrow();
        // The first gets through, the next ones within 100 ms do not, the final one always does.
        assert_eq!(steps.len(), 2);
        assert_eq!((steps[0].received, steps[0].total), (Some(10), Some(100)));
        assert_eq!((steps[1].received, steps[1].total), (Some(100), Some(100)));
    }

    #[test]
    fn without_a_declared_length_the_last_step_has_no_total() {
        let collected = Collected::default();
        let sink = collected.sink();
        let reporter = Reporter::new(&sink, Some(SourceKind::Tukui), "-2", "ElvUI");
        reporter.download_at(Instant::now(), 0, None);
        reporter.downloaded(4096);
        let last = collected.0.borrow().last().cloned().unwrap();
        assert_eq!((last.received, last.total), (Some(4096), None));
    }

    #[test]
    fn the_payload_has_only_identity_phase_and_counts() {
        let progress = Progress {
            source: Some(SourceKind::Tukui),
            key: "-2".to_owned(),
            name: "ElvUI".to_owned(),
            phase: Phase::Downloading,
            received: Some(5),
            total: None,
        };
        let event = ProgressEvent {
            installation_id: "abc",
            progress,
        };
        let json = serde_json::to_value(&event).unwrap();
        let mut keys: Vec<&str> = json
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "installationId",
                "key",
                "name",
                "phase",
                "received",
                "source"
            ]
        );
        assert_eq!(json["phase"], "downloading");
        assert_eq!(json["source"], "tukui");
    }
}
