//! Comandi Tauri invocabili dal frontend.

use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use serde::Serialize;
use tauri::State;

use crate::addons::scan::{self, InstalledAddon};
use crate::config::Config;
use crate::installations::Installation;
use crate::store::InstallationStore;

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

type Store<'a> = State<'a, Mutex<InstallationStore>>;

fn locked<'a>(store: &'a Store<'_>) -> Result<MutexGuard<'a, InstallationStore>, String> {
    store
        .lock()
        .map_err(|_| "Stato delle installazioni non disponibile: riavvia l'app.".to_owned())
}

#[tauri::command]
pub fn list_installations(store: Store<'_>) -> Result<Vec<Installation>, String> {
    locked(&store)?.list()
}

#[tauri::command]
pub fn add_installation(path: String, store: Store<'_>) -> Result<Installation, String> {
    locked(&store)?.add(Path::new(&path))
}

#[tauri::command]
pub fn remove_installation(id: String, store: Store<'_>) -> Result<(), String> {
    locked(&store)?.remove(&id)
}

/// Gli addon di un'installazione. La scansione legge il disco, quindi gira fuori dal thread
/// principale per non bloccare la finestra con molti addon.
#[tauri::command]
pub async fn list_addons(
    installation_id: String,
    store: Store<'_>,
) -> Result<Vec<InstalledAddon>, String> {
    let installation = locked(&store)?
        .list()?
        .into_iter()
        .find(|installation| installation.id == installation_id)
        .ok_or_else(|| "Installazione non trovata.".to_owned())?;
    tauri::async_runtime::spawn_blocking(move || {
        scan::scan_addons(&installation.path, &installation.flavor)
    })
    .await
    // L'errore di Tauri dice se il compito è andato in panic o è stato annullato.
    .map_err(|error| format!("La lettura degli addon si è interrotta ({error})."))?
}
