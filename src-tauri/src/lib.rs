mod addons;
mod commands;
mod config;

use config::Config;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(Config::load())
        .invoke_handler(tauri::generate_handler![commands::app_status])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
