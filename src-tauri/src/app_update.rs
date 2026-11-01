//! The check for a newer WoW-don release (D46, issue #259).
//!
//! Only tells the user that a release exists: it downloads and installs nothing. A background
//! thread reads the tag of the latest release from the `releases/latest` redirect of the app's
//! repository, as in D37: no API and no token. GitHub leaves drafts and pre-releases out of that
//! redirect. The check is independent of the automatic addon updates (D41) and runs only with
//! the `checkAppUpdates` option.
//!
//! A failed check is silent: the network is not the user's problem here. What an earlier check
//! found stays known, because a release does not stop existing when a read fails.

use std::sync::{Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::sources::github::{self, Target};
use crate::sources::http::{PublicReqwestTransport, PublicTransport};
use crate::store::AppSettingsStore;

/// Emitted with an [`AppUpdateStatus`] when what the frontend must show changes.
pub const EVENT: &str = "app-update";

const REPO: &str = "FabioSM46/wow-don";
const RELEASES_URL: &str = "https://github.com/FabioSM46/wow-don/releases";
const CURRENT: &str = env!("CARGO_PKG_VERSION");

/// Between two successful checks.
const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
/// After a failed check: with autostart the first one can run before the network is up.
const RETRY_INTERVAL: Duration = Duration::from_secs(60 * 60);

type Version = (u64, u64, u64);

/// `major.minor.patch`, with or without a leading `v`. Anything else (`0.2.0-rc1`, `0.2`) is
/// not a version: a tag that cannot be compared never produces a notice.
fn parse_version(text: &str) -> Option<Version> {
    let text = text.strip_prefix('v').unwrap_or(text);
    let number = |part: &str| {
        (!part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
            .then(|| part.parse().ok())
            .flatten()
    };
    let mut parts = text.split('.');
    let version = (
        number(parts.next()?)?,
        number(parts.next()?)?,
        number(parts.next()?)?,
    );
    parts.next().is_none().then_some(version)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Release {
    version: Version,
    url: String,
}

/// The latest release, or `None` for any failure. The tag page is built only from a tag that
/// came through `latest_target` and parsed as a version, hence digits and dots.
fn latest_release(transport: &dyn PublicTransport) -> Option<Release> {
    let url = format!("{RELEASES_URL}/latest");
    let response = transport.get_redirect(&url).ok()?;
    if !matches!(response.status, 301 | 302 | 303 | 307 | 308) {
        return None;
    }
    match github::latest_target(&url, response.location.as_deref(), REPO)? {
        Target::Tag(tag) => {
            let version = parse_version(&tag)?;
            let url = format!("{RELEASES_URL}/tag/{tag}");
            Some(Release { version, url })
        }
        _ => None,
    }
}

/// What the frontend receives: the response of `get_app_update` and the payload of [`EVENT`].
/// `latest` and `url` are `None` until a check succeeds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppUpdateStatus {
    current: String,
    latest: Option<String>,
    url: Option<String>,
    update_available: bool,
}

/// An update only when the release is strictly newer: a development build ahead of the release
/// reports nothing, and neither does a `current` that is not a version.
fn status_of(current: &str, release: Option<&Release>) -> AppUpdateStatus {
    let newer = |release: &Release| {
        parse_version(current).is_some_and(|installed| release.version > installed)
    };
    AppUpdateStatus {
        current: current.to_owned(),
        latest: release.map(
            |Release {
                 version: (a, b, c), ..
             }| format!("{a}.{b}.{c}"),
        ),
        url: release.map(|release| release.url.clone()),
        update_available: release.is_some_and(newer),
    }
}

#[derive(Default)]
struct Shared {
    release: Option<Release>,
    woken: bool,
}

/// The release known to the app, shared by the thread and the commands.
#[derive(Default)]
pub struct AppUpdate {
    shared: Mutex<Shared>,
    wake: Condvar,
}

impl AppUpdate {
    /// The current status (`get_app_update`): memory only, no network.
    pub fn status(&self) -> AppUpdateStatus {
        status_of(CURRENT, self.lock().release.as_ref())
    }

    /// Makes the thread re-read the option (`set_check_app_updates`): turning it on checks right
    /// away, turning it off forgets the release.
    pub fn wake(&self) {
        self.lock().woken = true;
        self.wake.notify_all();
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Shared> {
        // Only a known release: a panic does not leave it inconsistent.
        self.shared.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Replaces the known release; `true` if it changed.
    fn set_release(&self, release: Option<Release>) -> bool {
        let mut shared = self.lock();
        let changed = shared.release != release;
        shared.release = release;
        changed
    }

    /// Waits for `wait` (forever with `None`) or until [`wake`](Self::wake).
    fn sleep(&self, wait: Option<Duration>) {
        let due = wait.map(|wait| Instant::now() + wait);
        let mut shared = self.lock();
        while !shared.woken {
            shared = match due {
                None => self
                    .wake
                    .wait(shared)
                    .unwrap_or_else(PoisonError::into_inner),
                Some(due) => {
                    let now = Instant::now();
                    if now >= due {
                        break;
                    }
                    self.wake
                        .wait_timeout(shared, due - now)
                        .unwrap_or_else(PoisonError::into_inner)
                        .0
                }
            };
        }
        shared.woken = false;
    }
}

/// One pass of the thread.
#[derive(Debug, PartialEq, Eq)]
struct Pass {
    /// When the next one is due; `None` waits for a change of the option.
    next: Option<Duration>,
    /// The frontend must be told.
    changed: bool,
}

/// `enabled` is asked again after the read: the option can be turned off during the request,
/// and that wake-up would show a release the user has just dismissed for one more instant.
fn pass(state: &AppUpdate, transport: &dyn PublicTransport, enabled: &dyn Fn() -> bool) -> Pass {
    if !enabled() {
        return Pass {
            next: None,
            changed: state.set_release(None),
        };
    }
    match latest_release(transport) {
        Some(release) if enabled() => Pass {
            next: Some(CHECK_INTERVAL),
            changed: state.set_release(Some(release)),
        },
        Some(_) => Pass {
            next: None,
            changed: false,
        },
        None => Pass {
            next: Some(RETRY_INTERVAL),
            changed: false,
        },
    }
}

/// Whether the check may run. An unreadable `settings.json` counts as no: without knowing what
/// the user chose the app does not use the network.
fn enabled(app: &AppHandle) -> bool {
    app.state::<Mutex<AppSettingsStore>>()
        .lock()
        .ok()
        .and_then(|settings| settings.get().ok())
        .is_some_and(|settings| settings.check_app_updates)
}

fn run(app: &AppHandle) {
    let Ok(transport) = PublicReqwestTransport::new(&github::DOMAINS) else {
        eprintln!("The check for a new version is not available: no HTTP client.");
        return;
    };
    let state = app.state::<AppUpdate>();
    loop {
        let done = pass(&state, &transport, &|| enabled(app));
        if done.changed {
            // Nobody listening only means the frontend reads the status when it starts.
            let _ = app.emit(EVENT, state.status());
        }
        state.sleep(done.next);
    }
}

/// Starts the thread; must be called in `setup`, after registering the states.
pub fn start(app: AppHandle) {
    let spawned = std::thread::Builder::new()
        .name("app-update".to_owned())
        .spawn(move || run(&app));
    if spawned.is_err() {
        eprintln!("The check for a new version is not available: the thread did not start.");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::http::{BytesResponse, HttpResponse, RedirectResponse, TransportError};

    const LATEST: &str = "https://github.com/FabioSM46/wow-don/releases/latest";
    const TAG: &str = "https://github.com/FabioSM46/wow-don/releases/tag/";

    /// Answers every redirect the same way and counts the requests.
    struct FakeTransport {
        answer: Option<(u16, Option<String>)>,
        calls: Mutex<Vec<String>>,
    }

    impl FakeTransport {
        fn new(answer: Option<(u16, Option<String>)>) -> Self {
            Self {
                answer,
                calls: Mutex::new(Vec::new()),
            }
        }

        fn tagged(tag: &str) -> Self {
            Self::new(Some((302, Some(format!("{TAG}{tag}")))))
        }

        fn calls(&self) -> usize {
            self.calls.lock().unwrap().len()
        }
    }

    impl PublicTransport for FakeTransport {
        fn get(&self, _url: &str) -> Result<HttpResponse, TransportError> {
            unreachable!("the check reads only the redirect")
        }

        fn get_redirect(&self, url: &str) -> Result<RedirectResponse, TransportError> {
            self.calls.lock().unwrap().push(url.to_owned());
            let (status, location) = self.answer.clone().ok_or(TransportError::Other)?;
            Ok(RedirectResponse { status, location })
        }

        fn get_bytes(&self, _url: &str, _max_len: u64) -> Result<BytesResponse, TransportError> {
            unreachable!("the check downloads nothing")
        }
    }

    fn release(version: Version) -> Release {
        let (a, b, c) = version;
        Release {
            version,
            url: format!("{TAG}v{a}.{b}.{c}"),
        }
    }

    #[test]
    fn versions_parse_with_and_without_v() {
        assert_eq!(parse_version("v0.1.2"), Some((0, 1, 2)));
        assert_eq!(parse_version("0.10.0"), Some((0, 10, 0)));
    }

    #[test]
    fn anything_but_three_numbers_is_not_a_version() {
        for text in [
            "",
            "v",
            "0.2",
            "0.2.0.1",
            "0.2.0-rc1",
            "a.b.c",
            "0..1",
            "+1.2.3",
            "vv1.2.3",
            "99999999999999999999.0.0",
        ] {
            assert_eq!(parse_version(text), None, "{text}");
        }
    }

    #[test]
    fn versions_compare_as_numbers_not_as_text() {
        assert!(parse_version("0.10.0") > parse_version("0.9.0"));
    }

    #[test]
    fn only_a_strictly_newer_release_is_an_update() {
        let newer = release((0, 2, 0));
        assert!(status_of("0.1.1", Some(&newer)).update_available);
        assert!(!status_of("0.2.0", Some(&newer)).update_available);
        assert!(!status_of("0.3.0", Some(&newer)).update_available);
    }

    #[test]
    fn a_current_version_that_is_not_one_reports_nothing() {
        assert!(!status_of("dev", Some(&release((9, 0, 0)))).update_available);
    }

    #[test]
    fn the_status_has_no_release_until_a_check_succeeds() {
        assert_eq!(
            status_of("0.1.1", None),
            AppUpdateStatus {
                current: "0.1.1".to_owned(),
                latest: None,
                url: None,
                update_available: false,
            }
        );
    }

    #[test]
    fn the_status_serializes_in_camel_case() {
        let status = status_of("0.1.1", Some(&release((0, 2, 0))));
        assert_eq!(
            serde_json::to_value(status).unwrap(),
            serde_json::json!({
                "current": "0.1.1",
                "latest": "0.2.0",
                "url": format!("{TAG}v0.2.0"),
                "updateAvailable": true,
            })
        );
    }

    #[test]
    fn the_latest_release_comes_from_the_redirect() {
        let transport = FakeTransport::tagged("v0.2.0");
        assert_eq!(latest_release(&transport), Some(release((0, 2, 0))));
        assert_eq!(*transport.calls.lock().unwrap(), [LATEST]);
    }

    #[test]
    fn a_failed_read_gives_no_release() {
        let other_repo = "https://github.com/Other/repo/releases/tag/v9.0.0";
        let other_host = "https://example.com/FabioSM46/wow-don/releases/tag/v9.0.0";
        for answer in [
            None,
            Some((404, None)),
            Some((200, None)),
            Some((302, None)),
            Some((302, Some(other_repo.to_owned()))),
            Some((302, Some(other_host.to_owned()))),
            Some((302, Some(format!("{TAG}nightly")))),
            Some((302, Some(format!("{TAG}v0.2.0-rc1")))),
        ] {
            assert_eq!(
                latest_release(&FakeTransport::new(answer.clone())),
                None,
                "{answer:?}"
            );
        }
    }

    #[test]
    fn a_pass_with_the_option_off_sends_no_request_and_forgets_the_release() {
        let state = AppUpdate::default();
        state.set_release(Some(release((0, 2, 0))));
        let transport = FakeTransport::tagged("v0.3.0");
        let done = pass(&state, &transport, &|| false);
        assert_eq!(transport.calls(), 0);
        assert_eq!(
            done,
            Pass {
                next: None,
                changed: true
            }
        );
        assert_eq!(state.lock().release, None);
    }

    #[test]
    fn a_pass_stores_the_release_and_reports_only_a_change() {
        let state = AppUpdate::default();
        let transport = FakeTransport::tagged("v0.2.0");
        let first = pass(&state, &transport, &|| true);
        assert_eq!(
            first,
            Pass {
                next: Some(CHECK_INTERVAL),
                changed: true
            }
        );
        let second = pass(&state, &transport, &|| true);
        assert_eq!(
            second,
            Pass {
                next: Some(CHECK_INTERVAL),
                changed: false
            }
        );
    }

    #[test]
    fn a_failed_pass_keeps_what_is_known_and_retries_sooner() {
        let state = AppUpdate::default();
        state.set_release(Some(release((0, 2, 0))));
        let done = pass(&state, &FakeTransport::new(None), &|| true);
        assert_eq!(
            done,
            Pass {
                next: Some(RETRY_INTERVAL),
                changed: false
            }
        );
        assert_eq!(state.lock().release, Some(release((0, 2, 0))));
    }

    #[test]
    fn turning_the_option_off_during_the_read_stores_nothing() {
        let state = AppUpdate::default();
        let asked = Mutex::new(0);
        let enabled = || {
            *asked.lock().unwrap() += 1;
            *asked.lock().unwrap() == 1
        };
        let done = pass(&state, &FakeTransport::tagged("v0.2.0"), &enabled);
        assert_eq!(
            done,
            Pass {
                next: None,
                changed: false
            }
        );
        assert_eq!(state.lock().release, None);
    }

    #[test]
    fn a_wake_ends_the_wait_once() {
        let state = AppUpdate::default();
        state.wake();
        state.sleep(None);
        // Consumed: a short wait now runs to its end instead of returning at once.
        let started = Instant::now();
        state.sleep(Some(Duration::from_millis(20)));
        assert!(started.elapsed() >= Duration::from_millis(20));
    }
}
