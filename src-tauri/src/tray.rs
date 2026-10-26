//! Icona nella tray e opzione "Close to tray" (issue #158, D35).
//!
//! All'avvio [`create`] costruisce l'icona con il menu "Show WoW-don" / "Quit". Se la creazione
//! fallisce (su Linux, per esempio, senza la libreria AppIndicator) l'errore si scrive su stderr
//! e [`TrayStatus`] resta spento; su Linux l'icona conta come disponibile solo se sul bus di
//! sessione c'è chi la mostra. Senza icona visibile la X chiude l'app anche con `closeToTray`
//! attiva, così l'utente non resta mai con una finestra nascosta e nessuna icona per riaprirla.
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
    /// Se l'icona c'è e qualcuno la mostra adesso: solo allora la finestra si può nascondere.
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
/// l'app all'avvio. Si chiede prima allo stesso loader dinamico se la libreria si carica.
#[cfg(target_os = "linux")]
fn platform_supports_tray() -> bool {
    linux::appindicator_loads()
}

/// Su Windows e macOS l'icona usa le API del sistema: non manca niente.
#[cfg(not(target_os = "linux"))]
fn platform_supports_tray() -> bool {
    true
}

/// Se in questo momento c'è qualcuno che mostra l'icona. Su Linux AppIndicator la pubblica come
/// StatusNotifierItem: senza un `org.kde.StatusNotifierWatcher` con un host registrato sul bus
/// di sessione (GNOME senza l'estensione AppIndicator) l'icona esiste ma nessuno la disegna, e nascondere la
/// finestra la renderebbe irraggiungibile. Si chiede a ogni uso e non all'avvio: con l'avvio
/// automatico l'app può partire prima che la shell registri il watcher.
#[cfg(target_os = "linux")]
fn host_shows_tray_icons() -> bool {
    linux::status_notifier_host_present()
}

/// Su Windows e macOS l'area di notifica e la barra dei menu ci sono sempre.
#[cfg(not(target_os = "linux"))]
fn host_shows_tray_icons() -> bool {
    true
}

/// Le chiamate alle librerie di sistema che il processo carica già: `dlopen` (glibc, usata
/// anche da tray-icon per AppIndicator) e GIO/GLib (collegate da GTK). Nessuna crate in più.
#[cfg(target_os = "linux")]
mod linux {
    use std::ffi::{CStr, c_char, c_int, c_void};
    use std::ptr;

    /// Gli stessi nomi, nello stesso ordine, che prova `libappindicator-sys`.
    const APPINDICATOR_LIBRARIES: [&CStr; 2] =
        [c"libayatana-appindicator3.so.1", c"libappindicator3.so.1"];
    /// `RTLD_LAZY | RTLD_LOCAL` (= 1, perché `RTLD_LOCAL` vale 0), come
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

    /// Se una delle librerie AppIndicator si carica davvero: un file presente ma con una
    /// dipendenza mancante o di un'altra architettura risponde no, come risponderebbe a
    /// tray-icon. La libreria caricata resta in memoria: tray-icon la riapre subito dopo.
    pub fn appindicator_loads() -> bool {
        APPINDICATOR_LIBRARIES.iter().any(|name| {
            // SAFETY: `name` is a NUL-terminated string; a null handle means "not loadable".
            !unsafe { dlopen(name.as_ptr(), DLOPEN_FLAGS) }.is_null()
        })
    }

    /// `Properties.Get("org.kde.StatusNotifierWatcher", "IsStatusNotifierHostRegistered")` sul
    /// bus di sessione: un watcher senza host registrato possiede il nome ma non mostra niente, e
    /// senza watcher la chiamata fallisce, quindi una chiamata risponde a entrambe le domande.
    /// Ogni errore (nessun bus, timeout, risposta inattesa) vale "no": la X chiude.
    /// Non si scorcia su `DBUS_SESSION_BUS_ADDRESS`: se manca, GIO prova `$XDG_RUNTIME_DIR/bus`
    /// e solo dopo l'autolaunch X11, che senza `DISPLAY` fallisce subito. La variabile assente
    /// non vuol dire "nessun bus": un controllo lì darebbe falsi negativi (avvio automatico e
    /// launcher che la tolgono).
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

    /// Calls the real loader and the real session bus: it must answer without crashing
    /// whatever the machine has (CI runners have neither a session bus nor a tray).
    #[cfg(target_os = "linux")]
    #[test]
    fn the_linux_probes_answer_without_crashing() {
        let _ = linux::appindicator_loads();
        let _ = linux::status_notifier_host_present();
    }
}
