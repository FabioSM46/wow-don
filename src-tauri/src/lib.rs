mod addons;
mod commands;
mod config;
mod installations;
mod store;
#[cfg(test)]
mod test_support;

use std::sync::Mutex;

use config::Config;
use store::{INSTALLATIONS_FILE, InstallationStore};
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(Config::load())
        .setup(|app| {
            let file = app.path().app_config_dir()?.join(INSTALLATIONS_FILE);
            app.manage(Mutex::new(InstallationStore::load(file)));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::app_status,
            commands::list_installations,
            commands::add_installation,
            commands::remove_installation,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
