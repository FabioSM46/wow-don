//! Comandi Tauri invocabili dal frontend.

use serde::Serialize;
use tauri::State;

use crate::config::Config;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppStatus {
    version: &'static str,
    /// Solo se la chiave c'è: la chiave non lascia mai il backend.
    curseforge_key_configured: bool,
}

#[tauri::command]
pub fn app_status(config: State<'_, Config>) -> AppStatus {
    AppStatus {
        version: env!("CARGO_PKG_VERSION"),
        curseforge_key_configured: config.curseforge_api_key.is_some(),
    }
}
