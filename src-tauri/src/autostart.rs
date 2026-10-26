//! Avvio automatico con il sistema (issue #159, D36).
//!
//! Il plugin ufficiale `tauri-plugin-autostart` scrive la voce del sistema operativo: su Linux
//! un file `.desktop` in `~/.config/autostart`, su Windows un valore `Run` nel registro, su
//! macOS un LaunchAgent in `~/Library/LaunchAgents`. La voce lancia l'app con
//! [`AUTOSTART_ARG`]. Lo stato si legge sempre dal sistema operativo: nessuna copia in
//! `settings.json`, così una voce tolta a mano (o disattivata nel Task Manager di Windows) non
//! risulta attiva.
//!
//! All'avvio la finestra principale è invisibile (`"visible": false` in `tauri.conf.json`,
//! senza lampo e senza toccare lo stato di D28) e [`show_main_window_unless_hidden_start`] la
//! mostra, salvo quando [`start_hidden`] dice di partire nella tray.

use std::ffi::OsStr;

use tauri::plugin::TauriPlugin;
use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_autostart::{AutoLaunchManager, MacosLauncher, ManagerExt};

use crate::tray::{self, TrayStatus};

/// L'argomento con cui la voce del sistema operativo lancia l'app.
pub const AUTOSTART_ARG: &str = "--autostart";

/// Il plugin, registrato solo per il backend: nessun permesso `autostart:*` va al frontend, che
/// passa da `get_autostart` e `set_autostart`.
pub fn plugin<R: Runtime>() -> TauriPlugin<R> {
    tauri_plugin_autostart::init(MacosLauncher::LaunchAgent, Some(vec![AUTOSTART_ARG]))
}

/// Le tre operazioni del plugin sulla voce del sistema operativo. Separate dal plugin per
/// provare [`read`] e [`write`] senza scrivere la voce vera.
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

/// Se l'app parte con il sistema, letto dal sistema operativo.
pub fn is_enabled<R: Runtime>(app: &AppHandle<R>) -> Result<bool, String> {
    read(&*app.autolaunch())
}

/// Attiva o disattiva l'avvio con il sistema e restituisce lo stato riletto dal sistema
/// operativo, non quello richiesto.
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

/// Se l'app deve partire con la finestra nascosta e solo l'icona nella tray: solo se l'ha
/// lanciata la voce dell'avvio automatico (`args` senza il nome del programma), `closeToTray`
/// è letta e attiva e in questo momento qualcuno mostra l'icona. `tray_available` si chiede
/// per ultima: su Linux interroga il bus di sessione, e un avvio a mano non deve aspettarla.
///
/// Si chiede una volta sola, senza riprovare: se la shell non ha ancora registrato chi mostra
/// l'icona, la finestra si apre. Una finestra in più si chiude con la X; una finestra nascosta
/// senza icona sarebbe irraggiungibile, e riprovare per qualche secondo lascerebbe l'app
/// invisibile e irraggiungibile proprio in quei secondi, senza garanzia che la shell arrivi.
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

/// Mostra la finestra principale, invisibile all'avvio, salvo quando [`start_hidden`] dice di
/// restare nella tray. Va chiamata in `setup` dopo [`tray::create`].
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
