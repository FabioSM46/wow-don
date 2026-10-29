//! Tray icon and the "Close to tray" option (issue #158, D35).
//!
//! At startup [`create`] builds the icon with the "Show WoW-don" / "Quit" menu. If creation
//! fails (on Linux, for example, without the AppIndicator library) the error is written to stderr
//! and [`TrayStatus`] stays off; on Linux the icon counts as available only if something on the
//! session bus shows it. Without a visible icon the X quits the app even with `closeToTray`
//! on, so the user is never left with a hidden window and no icon to reopen it.
//!
//! [`on_window_event`] intercepts the closing of the main window; [`on_run_event`]
//! reopens the window on macOS from a click on the Dock icon.
//!
//! After every automatic round (D41, issue #202) [`show_round`] puts a non-clickable
//! "N updates available" item above "Show WoW-don" and repeats it in the tooltip; text and tooltip
//! come from the pure function [`round_summary`]. Without an icon nothing happens.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use tauri::menu::{Menu, MenuBuilder, MenuEvent, MenuItemBuilder};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, RunEvent, Runtime, Window, WindowEvent};

use crate::auto_update::RoundReport;
use crate::store::AppSettingsStore;

/// The label of the main window (the one in `tauri.conf.json`, without `label`).
const MAIN_WINDOW: &str = "main";
/// Specific to the app, not `"main"`: on Linux tray-icon writes the image to
/// `tray-icon-<id>-<n>.png` in the runtime folder and deletes it on exit, so another app with
/// the same ID would overwrite or remove ours (issue #252).
const TRAY_ID: &str = "wow-don";
const MENU_SHOW: &str = "show";
const MENU_QUIT: &str = "quit";
const MENU_UPDATES: &str = "updates";
const TOOLTIP: &str = "WoW-don";

/// Whether the tray icon was created. State managed by Tauri, read by `get_settings`
/// (`trayAvailable`) and by the close handling.
#[derive(Debug, Default)]
pub struct TrayStatus(AtomicBool);

impl TrayStatus {
    /// Whether the icon exists and something shows it right now: only then can the window hide.
    pub fn available(&self) -> bool {
        self.created() && host_shows_tray_icons()
    }

    fn created(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }

    fn set_created(&self, created: bool) {
        self.0.store(created, Ordering::Relaxed);
    }
}

/// Creates the tray icon and records in [`TrayStatus`] whether it exists. It never fails: without
/// an icon the app works as before, with the X quitting.
pub fn create<R: Runtime>(app: &AppHandle<R>) {
    let available = match build(app) {
        Ok(()) => true,
        Err(error) => {
            // Registered on stderr: the app has no log file, and the UI explains the missing
            // icon through `trayAvailable`.
            eprintln!(
                "WoW-don: the tray icon is not available ({error}); closing the window will quit the app."
            );
            false
        }
    };
    app.state::<TrayStatus>().set_created(available);
}

fn build<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    if !platform_supports_tray() {
        return Err("the AppIndicator library (libayatana-appindicator3) is not installed".into());
    }
    let icon = app
        .default_window_icon()
        .cloned()
        .ok_or_else(|| "the app has no icon".to_owned())?;
    let menu = menu(app, None).map_err(|error| error.to_string())?;
    TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon)
        .tooltip(TOOLTIP)
        .menu(&menu)
        // Left click shows the window where the platform reports it (Windows, macOS); the
        // menu stays on the right click. On Linux AppIndicator always opens the menu.
        .show_menu_on_left_click(false)
        .on_menu_event(on_menu_event)
        .on_tray_icon_event(on_tray_icon_event)
        .build(app)
        .map_err(|error| error.to_string())?;
    Ok(())
}

/// The tray menu: "Show WoW-don" and "Quit", with the non-clickable `updates` item above them
/// when there is one.
fn menu<R: Runtime>(app: &AppHandle<R>, updates: Option<&str>) -> tauri::Result<Menu<R>> {
    let mut builder = MenuBuilder::new(app);
    if let Some(text) = updates {
        let item = MenuItemBuilder::with_id(MENU_UPDATES, text)
            .enabled(false)
            .build(app)?;
        builder = builder.item(&item).separator();
    }
    builder
        .text(MENU_SHOW, "Show WoW-don")
        .separator()
        .text(MENU_QUIT, "Quit")
        .build()
}

/// Text of the menu item and tooltip after an automatic round.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoundSummary {
    /// The non-clickable item above "Show WoW-don"; `None` without updates.
    pub menu_item: Option<String>,
    pub tooltip: String,
}

/// How many updates are left to do after the round, summed over all the participating
/// installations: in `notify` the available ones; in `install` those the round tried to
/// install and did not install (skipped by the source or failed). In `notify` `updated` is empty,
/// so the same subtraction works for both modes. Projects are counted, not
/// keys: an update installed from the GitHub mirror (issue #162) can come back with a source
/// other than the one of the check. Those excluded by the user do not count: they are not in
/// `available`.
pub fn pending_updates(round: &RoundReport) -> usize {
    round
        .installations
        .iter()
        .map(|report| report.available.len().saturating_sub(report.updated.len()))
        .sum()
}

/// Text of the item and tooltip for the outcome of a round (or for no round).
pub fn round_summary(round: Option<&RoundReport>) -> RoundSummary {
    let pending = round.map_or(0, pending_updates);
    let menu_item = match pending {
        0 => None,
        1 => Some("1 update available".to_owned()),
        n => Some(format!("{n} updates available")),
    };
    let tooltip = match &menu_item {
        Some(text) => format!("{TOOLTIP} \u{2014} {text}"),
        None => TOOLTIP.to_owned(),
    };
    RoundSummary { menu_item, tooltip }
}

/// Updates the tray menu and tooltip with the outcome of a round. Without an icon (creation failed
/// or never happened) it does nothing; errors are ignored, as in [`show_main_window`]: the menu
/// stays as it was and the outcome can still be read in the app.
pub fn show_round<R: Runtime>(app: &AppHandle<R>, round: Option<&RoundReport>) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return;
    };
    let summary = round_summary(round);
    if let Ok(menu) = menu(app, summary.menu_item.as_deref()) {
        let _ = tray.set_menu(Some(menu));
    }
    let _ = tray.set_tooltip(Some(summary.tooltip));
}

fn on_menu_event<R: Runtime>(app: &AppHandle<R>, event: MenuEvent) {
    match event.id().as_ref() {
        MENU_SHOW => show_main_window(app),
        // `exit` emits `RunEvent::Exit`, where tauri-plugin-window-state writes the window
        // state (D28) also when the window is hidden.
        MENU_QUIT => app.exit(0),
        _ => {}
    }
}

fn on_tray_icon_event<R: Runtime>(tray: &TrayIcon<R>, event: TrayIconEvent) {
    if let TrayIconEvent::Click {
        button: MouseButton::Left,
        button_state: MouseButtonState::Up,
        ..
    } = event
    {
        show_main_window(tray.app_handle());
    }
}

/// Shows, unminimizes and focuses the main window. Errors are ignored: there is nobody
/// to report them to and the window stays as it was.
pub fn show_main_window<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

/// Window event handler (`Builder::on_window_event`): with `closeToTray` on and
/// the icon present, closing the main window hides it.
pub fn on_window_event<R: Runtime>(window: &Window<R>, event: &WindowEvent) {
    let WindowEvent::CloseRequested { api, .. } = event else {
        return;
    };
    if window.label() != MAIN_WINDOW {
        return;
    }
    let app = window.app_handle();
    let tray_available = app
        .try_state::<TrayStatus>()
        .is_some_and(|status| status.available());
    let close_to_tray = close_to_tray_setting(app);
    // A window that does not hide is closed: never a window that cannot be closed.
    if hide_on_close(close_to_tray, tray_available) && window.hide().is_ok() {
        api.prevent_close();
    }
}

/// The saved `closeToTray` option, or `None` if it cannot be read (store missing, poisoned lock,
/// corrupted `settings.json`). Used by the close handling and by the hidden start (issue #159).
pub fn close_to_tray_setting<R: Runtime>(app: &AppHandle<R>) -> Option<bool> {
    let settings = app.try_state::<Mutex<AppSettingsStore>>()?;
    let store = settings.lock().ok()?;
    store.get().ok().map(|settings| settings.close_to_tray)
}

/// Whether closing the main window must be intercepted. `close_to_tray` is `None` when
/// the setting cannot be read (poisoned lock, corrupted `settings.json`): then it closes,
/// as without the option. Without a tray icon it always closes.
pub fn hide_on_close(close_to_tray: Option<bool>, tray_available: bool) -> bool {
    tray_available && close_to_tray == Some(true)
}

/// App event handler (`App::run`): on macOS a click on the Dock icon reopens the
/// hidden window. On the other platforms there is no equivalent event: it is reopened from the
/// tray.
pub fn on_run_event<R: Runtime>(app: &AppHandle<R>, event: &RunEvent) {
    #[cfg(target_os = "macos")]
    if let RunEvent::Reopen { .. } = event {
        show_main_window(app);
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (app, event);
}

/// On Linux the icon uses libayatana-appindicator (or libappindicator), loaded at runtime by
/// tray-icon: without the library creation panics, and with `panic = "abort"` it would quit
/// the app at startup. The same dynamic loader is asked first whether the library loads.
#[cfg(target_os = "linux")]
fn platform_supports_tray() -> bool {
    linux::appindicator_loads()
}

/// On Windows and macOS the icon uses the system APIs: nothing is missing.
#[cfg(not(target_os = "linux"))]
fn platform_supports_tray() -> bool {
    true
}

/// Whether something is showing the icon right now. On Linux AppIndicator publishes it as a
/// StatusNotifierItem: without an `org.kde.StatusNotifierWatcher` with a host registered on the
/// session bus (GNOME without the AppIndicator extension) the icon exists but nobody draws it, and
/// hiding the window would make it unreachable. It is asked on every use and not at startup: with
/// autostart the app can start before the shell registers the watcher.
#[cfg(target_os = "linux")]
fn host_shows_tray_icons() -> bool {
    linux::status_notifier_host_present()
}

/// On Windows and macOS the notification area and the menu bar are always there.
#[cfg(not(target_os = "linux"))]
fn host_shows_tray_icons() -> bool {
    true
}

/// The calls to the system libraries the process already loads: `dlopen` (glibc, also used
/// by tray-icon for AppIndicator) and GIO/GLib (linked by GTK). No extra crate.
#[cfg(target_os = "linux")]
mod linux {
    use std::ffi::{CStr, c_char, c_int, c_void};
    use std::ptr;

    /// The same names, in the same order, that `libappindicator-sys` tries.
    const APPINDICATOR_LIBRARIES: [&CStr; 2] =
        [c"libayatana-appindicator3.so.1", c"libappindicator3.so.1"];
    /// `RTLD_LAZY | RTLD_LOCAL` (= 1, because `RTLD_LOCAL` is 0), like
    /// `libloading::Library::new`.
    const DLOPEN_FLAGS: c_int = 1;
    const G_BUS_TYPE_SESSION: c_int = 2;
    const G_DBUS_CALL_FLAGS_NONE: c_int = 0;
    const DBUS_TIMEOUT_MS: c_int = 200;

    unsafe extern "C" {
        fn dlopen(filename: *const c_char, flags: c_int) -> *mut c_void;
        fn g_bus_get_sync(
            bus_type: c_int,
            cancellable: *mut c_void,
            error: *mut *mut c_void,
        ) -> *mut c_void;
        #[allow(clippy::too_many_arguments)]
        fn g_dbus_connection_call_sync(
            connection: *mut c_void,
            bus_name: *const c_char,
            object_path: *const c_char,
            interface_name: *const c_char,
            method_name: *const c_char,
            parameters: *mut c_void,
            reply_type: *const c_void,
            flags: c_int,
            timeout_msec: c_int,
            cancellable: *mut c_void,
            error: *mut *mut c_void,
        ) -> *mut c_void;
        fn g_variant_new_string(string: *const c_char) -> *mut c_void;
        fn g_variant_new_tuple(children: *const *mut c_void, n_children: usize) -> *mut c_void;
        fn g_variant_get_child_value(value: *mut c_void, index: usize) -> *mut c_void;
        fn g_variant_get_variant(value: *mut c_void) -> *mut c_void;
        fn g_variant_is_of_type(value: *mut c_void, variant_type: *const c_void) -> c_int;
        fn g_variant_get_boolean(value: *mut c_void) -> c_int;
        fn g_variant_unref(value: *mut c_void);
        fn g_object_unref(object: *mut c_void);
    }

    /// Whether one of the AppIndicator libraries actually loads: a file that is present but has a
    /// missing dependency or is for another architecture answers no, as it would answer
    /// tray-icon. The loaded library stays in memory: tray-icon reopens it right after.
    pub fn appindicator_loads() -> bool {
        APPINDICATOR_LIBRARIES.iter().any(|name| {
            // SAFETY: `name` is a NUL-terminated string; a null handle means "not loadable".
            !unsafe { dlopen(name.as_ptr(), DLOPEN_FLAGS) }.is_null()
        })
    }

    /// `Properties.Get("org.kde.StatusNotifierWatcher", "IsStatusNotifierHostRegistered")` on the
    /// session bus: a watcher with no registered host owns the name but shows nothing, and
    /// without a watcher the call fails, so one call answers both questions.
    /// Every error (no bus, timeout, unexpected reply) counts as "no": the X quits.
    /// There is no shortcut on `DBUS_SESSION_BUS_ADDRESS`: if it is missing, GIO tries
    /// `$XDG_RUNTIME_DIR/bus` and only then X11 autolaunch, which fails at once without `DISPLAY`.
    /// The missing variable does not mean "no bus": a check there would give false negatives
    /// (autostart and launchers that remove it).
    pub fn status_notifier_host_present() -> bool {
        // SAFETY: plain GIO calls with valid NUL-terminated strings. `g_variant_new_tuple`
        // sinks the floating strings, `g_dbus_connection_call_sync` sinks the floating tuple,
        // and every returned reference is released exactly once. The reply type `(v)` is
        // checked by GIO before the reply is returned; the boxed value is checked to be a
        // boolean before it is read.
        unsafe {
            let bus = g_bus_get_sync(G_BUS_TYPE_SESSION, ptr::null_mut(), ptr::null_mut());
            if bus.is_null() {
                return false;
            }
            let arguments = [
                g_variant_new_string(c"org.kde.StatusNotifierWatcher".as_ptr()),
                g_variant_new_string(c"IsStatusNotifierHostRegistered".as_ptr()),
            ];
            let parameters = g_variant_new_tuple(arguments.as_ptr(), arguments.len());
            let reply = g_dbus_connection_call_sync(
                bus,
                c"org.kde.StatusNotifierWatcher".as_ptr(),
                c"/StatusNotifierWatcher".as_ptr(),
                c"org.freedesktop.DBus.Properties".as_ptr(),
                c"Get".as_ptr(),
                parameters,
                c"(v)".as_ptr().cast(),
                G_DBUS_CALL_FLAGS_NONE,
                DBUS_TIMEOUT_MS,
                ptr::null_mut(),
                ptr::null_mut(),
            );
            g_object_unref(bus);
            if reply.is_null() {
                return false;
            }
            let boxed = g_variant_get_child_value(reply, 0);
            let value = g_variant_get_variant(boxed);
            let present = g_variant_is_of_type(value, c"b".as_ptr().cast()) != 0
                && g_variant_get_boolean(value) != 0;
            g_variant_unref(value);
            g_variant_unref(boxed);
            g_variant_unref(reply);
            present
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_hides_only_with_the_option_on_and_a_tray_icon() {
        assert!(hide_on_close(Some(true), true));
        assert!(!hide_on_close(Some(false), true));
        // Unreadable setting: close as without the option.
        assert!(!hide_on_close(None, true));
        // No tray icon: always close, or the window could not be reopened.
        assert!(!hide_on_close(Some(true), false));
        assert!(!hide_on_close(Some(false), false));
        assert!(!hide_on_close(None, false));
    }

    #[test]
    fn a_tray_icon_that_was_not_created_is_never_available() {
        let status = TrayStatus::default();
        assert!(!status.created());
        assert!(!status.available());
        status.set_created(true);
        assert!(status.created());
    }

    fn report(available: usize, updated: usize) -> crate::auto_update::InstallationReport {
        use crate::auto_update::{AvailableUpdate, InstallationReport};
        use crate::sources::SourceKind;
        use crate::updates::{FileSummary, UpdateOutcome};
        let file = || FileSummary {
            id: 2,
            name: "v2".to_owned(),
            date: "2026-10-27T10:00:00Z".to_owned(),
        };
        InstallationReport {
            installation_id: "retail".to_owned(),
            installation_name: "Retail".to_owned(),
            checked_at: "2026-10-27T10:00:00Z".to_owned(),
            available: (0..available)
                .map(|i| AvailableUpdate {
                    source: SourceKind::Curseforge,
                    key: i.to_string(),
                    name: format!("Addon {i}"),
                    latest: file(),
                })
                .collect(),
            updated: (0..updated)
                .map(|i| {
                    UpdateOutcome::updated(
                        SourceKind::Curseforge,
                        &i.to_string(),
                        &format!("Addon {i}"),
                        file(),
                        None,
                    )
                })
                .collect(),
            excluded: Vec::new(),
            skipped: Vec::new(),
            errors: Vec::new(),
            check_error: None,
        }
    }

    fn round(
        mode: crate::store::AutoUpdateMode,
        installations: Vec<crate::auto_update::InstallationReport>,
    ) -> RoundReport {
        RoundReport {
            mode,
            started_at: "2026-10-27T10:00:00Z".to_owned(),
            finished_at: "2026-10-27T10:01:00Z".to_owned(),
            retrying: false,
            installations,
        }
    }

    fn summary(menu_item: Option<&str>, tooltip: &str) -> RoundSummary {
        RoundSummary {
            menu_item: menu_item.map(str::to_owned),
            tooltip: tooltip.to_owned(),
        }
    }

    #[test]
    fn without_updates_there_is_no_menu_item_and_the_plain_tooltip() {
        use crate::store::AutoUpdateMode::{Install, Notify};
        let plain = summary(None, "WoW-don");
        assert_eq!(round_summary(None), plain);
        assert_eq!(round_summary(Some(&round(Notify, Vec::new()))), plain);
        assert_eq!(
            round_summary(Some(&round(Notify, vec![report(0, 0)]))),
            plain
        );
        // Everything installed: nothing left.
        assert_eq!(
            round_summary(Some(&round(Install, vec![report(3, 3)]))),
            plain
        );
    }

    #[test]
    fn one_and_many_updates_are_counted() {
        use crate::store::AutoUpdateMode::Notify;
        assert_eq!(
            round_summary(Some(&round(Notify, vec![report(1, 0)]))),
            summary(
                Some("1 update available"),
                "WoW-don \u{2014} 1 update available"
            )
        );
        assert_eq!(
            round_summary(Some(&round(Notify, vec![report(7, 0)]))),
            summary(
                Some("7 updates available"),
                "WoW-don \u{2014} 7 updates available"
            )
        );
    }

    #[test]
    fn the_count_is_the_sum_over_every_installation() {
        use crate::store::AutoUpdateMode::Notify;
        let installations = vec![report(2, 0), report(0, 0), report(3, 0)];
        let round = round(Notify, installations);
        assert_eq!(pending_updates(&round), 5);
        assert_eq!(
            round_summary(Some(&round)).menu_item.as_deref(),
            Some("5 updates available")
        );
    }

    #[test]
    fn in_install_mode_only_the_updates_left_are_counted() {
        use crate::store::AutoUpdateMode::Install;
        // Retail: 4 tried, 3 installed (one skipped or failed); Classic: 2 tried, none installed.
        let round = round(Install, vec![report(4, 3), report(2, 0)]);
        assert_eq!(pending_updates(&round), 3);
        assert_eq!(
            round_summary(Some(&round)),
            summary(
                Some("3 updates available"),
                "WoW-don \u{2014} 3 updates available"
            )
        );
    }

    /// Calls the real loader and the real session bus: it must answer without crashing
    /// whatever the machine has (CI runners have neither a session bus nor a tray).
    #[cfg(target_os = "linux")]
    #[test]
    fn the_linux_probes_answer_without_crashing() {
        let _ = linux::appindicator_loads();
        let _ = linux::status_notifier_host_present();
    }
}
