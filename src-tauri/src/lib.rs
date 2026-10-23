mod addons;
mod backup;
mod commands;
mod config;
mod install;
mod installations;
mod sources;
mod store;
#[cfg(test)]
mod test_support;
mod updates;

use std::sync::Mutex;

use config::Config;
use store::{ADDON_SETTINGS_FILE, AddonSettingsStore, INSTALLATIONS_FILE, InstallationStore};
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(Config::load())
        .setup(|app| {
            let config_dir = app.path().app_config_dir()?;
            app.manage(Mutex::new(InstallationStore::load(
                config_dir.join(INSTALLATIONS_FILE),
            )));
            app.manage(Mutex::new(AddonSettingsStore::load(
                config_dir.join(ADDON_SETTINGS_FILE),
            )));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::app_status,
            commands::list_installations,
            commands::add_installation,
            commands::remove_installation,
            commands::list_addons,
            commands::list_version_types,
            commands::set_version_override,
            commands::clear_version_override,
            commands::check_updates,
            commands::update_addons,
            commands::create_backup,
            commands::list_backups,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
