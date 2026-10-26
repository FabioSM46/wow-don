//! Icona nella tray e opzione "Close to tray" (issue #158, D34).
//!
//! All'avvio [`create`] costruisce l'icona con il menu "Show WoW-don" / "Quit". Se la creazione
//! fallisce (su Linux, per esempio, senza la libreria AppIndicator) l'errore si scrive su stderr
//! e [`TrayStatus`] resta spento: la X chiude l'app anche con `closeToTray` attiva, così
//! l'utente non resta mai con una finestra nascosta e nessuna icona per riaprirla.
//!
//! [`on_window_event`] intercetta la chiusura della finestra principale; [`on_run_event`]
//! riapre la finestra su macOS dal clic sull'icona nel Dock.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use tauri::menu::{MenuBuilder, MenuEvent};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, RunEvent, Runtime, Window, WindowEvent};

use crate::store::AppSettingsStore;

/// L'etichetta della finestra principale (quella di `tauri.conf.json`, senza `label`).
const MAIN_WINDOW: &str = "main";
const TRAY_ID: &str = "main";
const MENU_SHOW: &str = "show";
const MENU_QUIT: &str = "quit";

/// Se l'icona nella tray è stata creata. Stato gestito da Tauri, letto da `get_settings`
/// (`trayAvailable`) e dalla gestione della chiusura.
#[derive(Debug, Default)]
pub struct TrayStatus(AtomicBool);

impl TrayStatus {
    pub fn available(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }

    fn set_available(&self, available: bool) {
        self.0.store(available, Ordering::Relaxed);
    }
}

/// Crea l'icona nella tray e registra in [`TrayStatus`] se c'è. Non fallisce mai: senza icona
/// l'app funziona come prima, con la X che chiude.
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
    app.state::<TrayStatus>().set_available(available);
}

fn build<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    if !platform_supports_tray() {
        return Err("the AppIndicator library (libayatana-appindicator3) is not installed".into());
    }
    let icon = app
        .default_window_icon()
        .cloned()
        .ok_or_else(|| "the app has no icon".to_owned())?;
    let menu = MenuBuilder::new(app)
        .text(MENU_SHOW, "Show WoW-don")
        .separator()
        .text(MENU_QUIT, "Quit")
        .build()
        .map_err(|error| error.to_string())?;
    TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon)
        .tooltip("WoW-don")
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

/// Mostra, de-minimizza e mette a fuoco la finestra principale. Gli errori si ignorano: non c'è
/// nessuno a cui riportarli e la finestra resta com'era.
pub fn show_main_window<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

/// Gestore degli eventi delle finestre (`Builder::on_window_event`): con `closeToTray` attiva e
/// l'icona presente, la chiusura della finestra principale la nasconde.
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
    let close_to_tray = app
        .try_state::<Mutex<AppSettingsStore>>()
        .and_then(|settings| {
            let store = settings.lock().ok()?;
            store.get().ok().map(|settings| settings.close_to_tray)
        });
    // A window that does not hide is closed: never a window that cannot be closed.
    if hide_on_close(close_to_tray, tray_available) && window.hide().is_ok() {
        api.prevent_close();
    }
}

/// Se la chiusura della finestra principale va intercettata. `close_to_tray` è `None` quando
/// l'impostazione non si legge (lock avvelenato, `settings.json` danneggiato): allora si chiude,
/// come senza l'opzione. Senza icona nella tray si chiude sempre.
pub fn hide_on_close(close_to_tray: Option<bool>, tray_available: bool) -> bool {
    tray_available && close_to_tray == Some(true)
}

/// Gestore degli eventi dell'app (`App::run`): su macOS il clic sull'icona nel Dock riapre la
/// finestra nascosta. Sulle altre piattaforme non c'è un evento equivalente: si riapre dalla
/// tray.
pub fn on_run_event<R: Runtime>(app: &AppHandle<R>, event: &RunEvent) {
    #[cfg(target_os = "macos")]
    if let RunEvent::Reopen { .. } = event {
        show_main_window(app);
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (app, event);
}

/// Su Linux l'icona usa libayatana-appindicator (o libappindicator), caricata a runtime da
/// tray-icon: senza la libreria la creazione va in panic, e con `panic = "abort"` chiuderebbe
/// l'app all'avvio. Si controlla prima che il file esista nelle cartelle del linker dinamico.
#[cfg(target_os = "linux")]
fn platform_supports_tray() -> bool {
    appindicator_in(&linux_library_dirs())
}

/// Su Windows e macOS l'icona usa le API del sistema: non manca niente.
#[cfg(not(target_os = "linux"))]
fn platform_supports_tray() -> bool {
    true
}

#[cfg(any(target_os = "linux", test))]
const APPINDICATOR_LIBRARIES: [&str; 2] =
    ["libayatana-appindicator3.so.1", "libappindicator3.so.1"];

/// Se una delle librerie AppIndicator sta in una delle cartelle.
#[cfg(any(target_os = "linux", test))]
fn appindicator_in(dirs: &[std::path::PathBuf]) -> bool {
    dirs.iter().any(|dir| {
        APPINDICATOR_LIBRARIES
            .iter()
            .any(|name| dir.join(name).exists())
    })
}

/// Le cartelle dove il linker dinamico cerca le librerie: `LD_LIBRARY_PATH`, le cartelle
/// standard e quelle multiarch di Debian e Ubuntu (`/usr/lib/x86_64-linux-gnu`). Una libreria
/// in una cartella non elencata spegne la tray: un falso negativo lascia la X che chiude, un
/// falso positivo chiuderebbe l'app.
#[cfg(target_os = "linux")]
fn linux_library_dirs() -> Vec<std::path::PathBuf> {
    use std::path::PathBuf;

    let mut dirs: Vec<PathBuf> = std::env::var_os("LD_LIBRARY_PATH")
        .map(|value| std::env::split_paths(&value).collect())
        .unwrap_or_default();
    for base in [
        "/usr/lib",
        "/usr/lib64",
        "/lib",
        "/lib64",
        "/usr/local/lib",
        "/run/current-system/sw/lib",
        "/app/lib",
    ] {
        let base = PathBuf::from(base);
        if let Ok(entries) = std::fs::read_dir(&base) {
            dirs.extend(
                entries
                    .flatten()
                    .filter(|entry| entry.file_name().to_string_lossy().contains("-linux-"))
                    .map(|entry| entry.path()),
            );
        }
        dirs.push(base);
    }
    dirs
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;

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
    fn the_tray_status_starts_unavailable() {
        let status = TrayStatus::default();
        assert!(!status.available());
        status.set_available(true);
        assert!(status.available());
    }

    #[test]
    fn appindicator_is_found_only_where_a_library_file_exists() {
        let tmp = TempDir::new();
        let empty = tmp.path().join("empty");
        let ayatana = tmp.path().join("ayatana");
        let legacy = tmp.path().join("legacy");
        for dir in [&empty, &ayatana, &legacy] {
            std::fs::create_dir_all(dir).unwrap();
        }
        std::fs::write(ayatana.join("libayatana-appindicator3.so.1"), b"").unwrap();
        std::fs::write(legacy.join("libappindicator3.so.1"), b"").unwrap();

        assert!(!appindicator_in(&[]));
        assert!(!appindicator_in(std::slice::from_ref(&empty)));
        assert!(!appindicator_in(&[tmp.path().join("missing")]));
        assert!(appindicator_in(&[empty.clone(), ayatana]));
        assert!(appindicator_in(&[empty, legacy]));
    }
}
