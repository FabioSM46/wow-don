mod addon_import;
mod addon_list;
mod addon_remove;
mod addons;
mod auto_update;
mod autostart;
mod backup;
mod catalog;
mod commands;
mod config;
mod discovery;
mod install;
mod install_link;
mod install_local;
mod install_new;
mod installations;
mod parallel;
mod progress;
mod sources;
mod store;
#[cfg(test)]
mod test_support;
mod tray;
mod updates;

use std::sync::Mutex;

use config::Config;
use store::{
    ADDON_SETTINGS_FILE, APP_SETTINGS_FILE, AddonSettingsStore, AppSettingsStore,
    INSTALLATIONS_FILE, INSTALLED_ADDONS_FILE, InstallationStore, InstalledAddonsStore,
};
use tauri::Manager;
use tauri_plugin_window_state::StateFlags;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        // Start with the system (D36): only the backend uses it, through `get_autostart` and
        // `set_autostart`; no `autostart:*` permission reaches the frontend.
        .plugin(autostart::plugin())
        // Remember only size, position and maximized state of the main window (D28): the
        // default flags would also restore visibility, decorations and fullscreen, and
        // without the filter every future window would be tracked too.
        .plugin(
            tauri_plugin_window_state::Builder::default()
                .with_state_flags(StateFlags::SIZE | StateFlags::POSITION | StateFlags::MAXIMIZED)
                .with_filter(|label| label == "main")
                .build(),
        )
        .manage(Config::load())
        .manage(commands::InstallLock::default())
        .manage(addons::cache::FingerprintCache::default())
        .manage(tray::TrayStatus::default())
        .manage(auto_update::AutoUpdate::default())
        .setup(|app| {
            let config_dir = app.path().app_config_dir()?;
            app.manage(Mutex::new(InstallationStore::load(
                config_dir.join(INSTALLATIONS_FILE),
            )));
            app.manage(Mutex::new(AddonSettingsStore::load(
                config_dir.join(ADDON_SETTINGS_FILE),
            )));
            app.manage(Mutex::new(InstalledAddonsStore::load(
                config_dir.join(INSTALLED_ADDONS_FILE),
            )));
            app.manage(Mutex::new(AppSettingsStore::load(
                config_dir.join(APP_SETTINGS_FILE),
            )));
            tray::create(app.handle());
            // The main window is declared invisible in `tauri.conf.json` (no flash, D36), and
            // the window-state plugin above never restores visibility (no `VISIBLE` flag):
            // shown here unless an autostart launch stays in the tray.
            autostart::show_main_window_unless_hidden_start(app.handle());
            // Il giro automatico degli aggiornamenti (D41, issue #200) parte anche con
            // `--autostart`: il primo giro aspetta `STARTUP_DELAY`, gli stati sono già registrati.
            auto_update::start(app.handle().clone());
            Ok(())
        })
        .on_window_event(tray::on_window_event)
        .invoke_handler(tauri::generate_handler![
            commands::app_status,
            commands::list_installations,
            commands::add_installation,
            commands::remove_installation,
            commands::discover_installations,
            commands::refresh_addons,
            commands::export_addons,
            commands::preview_import,
            commands::import_addons,
            commands::list_addons,
            commands::list_version_types,
            commands::set_version_override,
            commands::clear_version_override,
            commands::check_updates,
            commands::update_addons,
            commands::install_addon,
            commands::github_mirror,
            commands::install_zip,
            commands::resolve_link,
            commands::install_from_link,
            commands::remove_addons,
            commands::search_catalog,
            commands::list_categories,
            commands::addon_detail,
            commands::create_backup,
            commands::list_backups,
            commands::get_settings,
            commands::set_theme,
            commands::set_beta_when_no_release,
            commands::set_close_to_tray,
            commands::set_auto_update,
            commands::get_auto_update,
            commands::set_auto_update_excluded,
            commands::set_installation_auto_update,
            commands::get_auto_update_status,
            commands::get_autostart,
            commands::set_autostart,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| tray::on_run_event(app, &event));
}
