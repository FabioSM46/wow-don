//! Start automatically with the system (issue #159, D36).
//!
//! The official `tauri-plugin-autostart` plugin writes the operating system entry: on Linux a
//! `.desktop` file in `~/.config/autostart`, on Windows a `Run` value in the registry, on
//! macOS a LaunchAgent in `~/Library/LaunchAgents`. The entry launches the app with
//! [`AUTOSTART_ARG`]. The state is always read from the operating system: no copy in
//! `settings.json`, so an entry removed by hand (or disabled in the Windows Task Manager) does
//! not show as active.
//!
//! At startup the main window is invisible (`"visible": false` in `tauri.conf.json`, with no
//! flash and without touching the D28 state) and [`show_main_window_unless_hidden_start`] shows
//! it, except when [`start_hidden`] says to start in the tray.

use std::ffi::OsStr;

use tauri::plugin::TauriPlugin;
use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_autostart::{AutoLaunchManager, MacosLauncher, ManagerExt};

use crate::tray::{self, TrayStatus};

/// The argument the operating system entry launches the app with.
pub const AUTOSTART_ARG: &str = "--autostart";

/// The plugin, registered for the backend only: no `autostart:*` permission goes to the
/// frontend, which goes through `get_autostart` and `set_autostart`.
pub fn plugin<R: Runtime>() -> TauriPlugin<R> {
    tauri_plugin_autostart::init(MacosLauncher::LaunchAgent, Some(vec![AUTOSTART_ARG]))
}

/// The plugin's three operations on the operating system entry. Kept apart from the plugin to
/// test [`read`] and [`write`] without writing the real entry.
trait Launcher {
    fn enable(&self) -> Result<(), String>;
    fn disable(&self) -> Result<(), String>;
    fn is_enabled(&self) -> Result<bool, String>;
}

impl Launcher for AutoLaunchManager {
    fn enable(&self) -> Result<(), String> {
        AutoLaunchManager::enable(self).map_err(|error| error.to_string())
    }

    fn disable(&self) -> Result<(), String> {
        AutoLaunchManager::disable(self).map_err(|error| error.to_string())
    }

    fn is_enabled(&self) -> Result<bool, String> {
        AutoLaunchManager::is_enabled(self).map_err(|error| error.to_string())
    }
}

/// Whether the app starts with the system, read from the operating system.
pub fn is_enabled<R: Runtime>(app: &AppHandle<R>) -> Result<bool, String> {
    read(&*app.autolaunch())
}

/// Turns starting with the system on or off and returns the state read back from the
/// operating system, not the requested one.
pub fn set_enabled<R: Runtime>(app: &AppHandle<R>, enabled: bool) -> Result<bool, String> {
    write(&*app.autolaunch(), enabled)
}

fn read(launcher: &impl Launcher) -> Result<bool, String> {
    launcher
        .is_enabled()
        .map_err(|error| format!("Could not read whether WoW-don starts with the system: {error}"))
}

fn write(launcher: &impl Launcher, enabled: bool) -> Result<bool, String> {
    let result = if enabled {
        launcher.enable()
    } else {
        launcher.disable()
    };
    result.map_err(|error| {
        let action = if enabled { "turn on" } else { "turn off" };
        format!("Could not {action} starting with the system: {error}")
    })?;
    read(launcher)
}

/// Whether the app should start with the window hidden and only the icon in the tray: only if
/// the autostart entry launched it (`args` without the program name), `closeToTray` was read and
/// is on, and someone is showing the icon right now. `tray_available` is asked last: on Linux
/// it queries the session bus, and a manual launch must not wait for it.
///
/// It is asked once, without retrying: if the shell has not yet registered who shows the icon,
/// the window opens. An extra window is closed with the X; a hidden window with no icon would
/// be unreachable, and retrying for a few seconds would leave the app invisible and unreachable
/// exactly in those seconds, with no guarantee that the shell arrives.
pub fn start_hidden<I, S>(
    args: I,
    close_to_tray: Option<bool>,
    tray_available: impl FnOnce() -> bool,
) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let autostarted = args
        .into_iter()
        .any(|arg| arg.as_ref() == OsStr::new(AUTOSTART_ARG));
    autostarted && close_to_tray == Some(true) && tray_available()
}

/// Shows the main window, invisible at startup, except when [`start_hidden`] says to stay in
/// the tray. It must be called in `setup` after [`tray::create`].
pub fn show_main_window_unless_hidden_start<R: Runtime>(app: &AppHandle<R>) {
    let hidden = start_hidden(
        std::env::args_os().skip(1),
        tray::close_to_tray_setting(app),
        || {
            app.try_state::<TrayStatus>()
                .is_some_and(|status| status.available())
        },
    );
    if !hidden {
        tray::show_main_window(app);
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    #[test]
    fn only_an_autostart_launch_with_the_option_on_and_a_tray_icon_starts_hidden() {
        let autostart = [AUTOSTART_ARG];
        let manual: [&str; 0] = [];
        assert!(start_hidden(autostart, Some(true), || true));
        // Launched by hand: always the window.
        assert!(!start_hidden(manual, Some(true), || true));
        assert!(!start_hidden(["--other"], Some(true), || true));
        // Option off or unreadable: the window.
        assert!(!start_hidden(autostart, Some(false), || true));
        assert!(!start_hidden(autostart, None, || true));
        // No visible tray icon: the window, or the app could not be reached.
        assert!(!start_hidden(autostart, Some(true), || false));
    }

    #[test]
    fn the_argument_is_found_among_others_and_only_as_a_whole_word() {
        assert!(start_hidden(["--x", AUTOSTART_ARG], Some(true), || true));
        assert!(!start_hidden(["--autostart=1"], Some(true), || true));
        assert!(!start_hidden(["--AUTOSTART"], Some(true), || true));
    }

    #[test]
    fn the_tray_is_asked_only_when_it_decides() {
        let asked = RefCell::new(0);
        let probe = || {
            *asked.borrow_mut() += 1;
            true
        };
        let manual: [&str; 0] = [];
        assert!(!start_hidden(manual, Some(true), probe));
        assert!(!start_hidden([AUTOSTART_ARG], Some(false), probe));
        assert_eq!(*asked.borrow(), 0);
        assert!(start_hidden([AUTOSTART_ARG], Some(true), probe));
        assert_eq!(*asked.borrow(), 1);
    }

    /// A launcher in memory: the tests never write the real operating system entry.
    #[derive(Default)]
    struct FakeLauncher {
        enabled: RefCell<bool>,
        fail_write: Option<&'static str>,
        fail_read: Option<&'static str>,
        /// The OS refuses silently: the write succeeds but the entry stays as it was.
        ignore_write: bool,
    }

    impl Launcher for FakeLauncher {
        fn enable(&self) -> Result<(), String> {
            self.set(true)
        }

        fn disable(&self) -> Result<(), String> {
            self.set(false)
        }

        fn is_enabled(&self) -> Result<bool, String> {
            match self.fail_read {
                Some(error) => Err(error.to_owned()),
                None => Ok(*self.enabled.borrow()),
            }
        }
    }

    impl FakeLauncher {
        fn set(&self, enabled: bool) -> Result<(), String> {
            if let Some(error) = self.fail_write {
                return Err(error.to_owned());
            }
            if !self.ignore_write {
                *self.enabled.borrow_mut() = enabled;
            }
            Ok(())
        }
    }

    #[test]
    fn writing_returns_the_state_read_back_from_the_system() {
        let launcher = FakeLauncher::default();
        assert_eq!(read(&launcher), Ok(false));
        assert_eq!(write(&launcher, true), Ok(true));
        assert_eq!(read(&launcher), Ok(true));
        assert_eq!(write(&launcher, false), Ok(false));

        let stubborn = FakeLauncher {
            ignore_write: true,
            ..FakeLauncher::default()
        };
        assert_eq!(write(&stubborn, true), Ok(false));
    }

    #[test]
    fn errors_come_back_in_english_with_the_cause() {
        let failing = FakeLauncher {
            fail_write: Some("access denied"),
            ..FakeLauncher::default()
        };
        assert_eq!(
            write(&failing, true),
            Err("Could not turn on starting with the system: access denied".to_owned())
        );
        assert_eq!(
            write(&failing, false),
            Err("Could not turn off starting with the system: access denied".to_owned())
        );

        let unreadable = FakeLauncher {
            fail_read: Some("no home directory"),
            ..FakeLauncher::default()
        };
        assert_eq!(
            read(&unreadable),
            Err(
                "Could not read whether WoW-don starts with the system: no home directory"
                    .to_owned()
            )
        );
        assert!(write(&unreadable, true).is_err());
    }
}
