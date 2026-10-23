//! Comandi Tauri invocabili dal frontend.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use serde::Serialize;
use tauri::{AppHandle, Manager, State};

use crate::addons::scan::{self, ListedAddon};
use crate::backup::{self, BackupInfo};
use crate::config::Config;
use crate::installations::Installation;
use crate::sources::curseforge::{CurseForgeClient, CurseForgeError, ReqwestTransport};
use crate::store::{AddonSettingsStore, InstallationStore, InstalledAddonsStore};
use crate::updates::{self, AddonUpdate, UpdateOutcome, VersionOverrides, VersionType};

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
        .map_err(|_| "Installations are not available: restart the app.".to_owned())
}

type Settings<'a> = State<'a, Mutex<AddonSettingsStore>>;

fn locked_settings<'a>(
    settings: &'a Settings<'_>,
) -> Result<MutexGuard<'a, AddonSettingsStore>, String> {
    settings
        .lock()
        .map_err(|_| "Addon settings are not available: restart the app.".to_owned())
}

type Installed<'a> = State<'a, Mutex<InstalledAddonsStore>>;

fn locked_installed(
    installed: &Mutex<InstalledAddonsStore>,
) -> Result<MutexGuard<'_, InstalledAddonsStore>, String> {
    installed
        .lock()
        .map_err(|_| "The installed addons list is not available: restart the app.".to_owned())
}

fn overrides_for(
    settings: &Settings<'_>,
    installation_id: &str,
) -> Result<VersionOverrides, String> {
    locked_settings(settings)?.overrides_for(installation_id)
}

#[tauri::command]
pub fn list_installations(store: Store<'_>) -> Result<Vec<Installation>, String> {
    locked(&store)?.list()
}

#[tauri::command]
pub fn add_installation(path: String, store: Store<'_>) -> Result<Installation, String> {
    locked(&store)?.add(Path::new(&path))
}

/// Rimuove prima le impostazioni e i record degli addon, poi l'installazione: se qualcosa non
/// si può togliere l'installazione resta e si può riprovare, invece di lasciare dati orfani. I
/// lock non si tengono mai insieme.
#[tauri::command]
pub fn remove_installation(
    id: String,
    store: Store<'_>,
    settings: Settings<'_>,
    installed: Installed<'_>,
) -> Result<(), String> {
    locked_settings(&settings)?.remove_installation(&id)?;
    locked_installed(&installed)?.remove_installation(&id)?;
    locked(&store)?.remove(&id)
}

/// I tipi di versione del gioco che si possono forzare per un addon. Nessuna chiamata di rete.
#[tauri::command]
pub fn list_version_types() -> Vec<VersionType> {
    updates::version_types()
}

/// Forza la versione del gioco usata per cercare gli aggiornamenti del progetto `mod_id`.
#[tauri::command]
pub fn set_version_override(
    installation_id: String,
    mod_id: u32,
    version_type: u32,
    store: Store<'_>,
    settings: Settings<'_>,
) -> Result<(), String> {
    find_installation(&store, &installation_id)?;
    locked_settings(&settings)?.set_override(&installation_id, mod_id, version_type)
}

/// Torna alla versione del gioco dell'installazione per il progetto `mod_id`.
#[tauri::command]
pub fn clear_version_override(
    installation_id: String,
    mod_id: u32,
    settings: Settings<'_>,
) -> Result<(), String> {
    locked_settings(&settings)?.clear_override(&installation_id, mod_id)
}

/// Gli addon di un'installazione, con il progetto che l'app vi ha installato (issue #62). La
/// scansione legge il disco, quindi gira fuori dal thread principale per non bloccare la
/// finestra con molti addon; il registro si legge prima, qui.
#[tauri::command]
pub async fn list_addons(
    installation_id: String,
    store: Store<'_>,
    installed: Installed<'_>,
) -> Result<Vec<ListedAddon>, String> {
    let installation = locked(&store)?
        .list()?
        .into_iter()
        .find(|installation| installation.id == installation_id)
        .ok_or_else(|| "Installation not found.".to_owned())?;
    let records = locked_installed(&installed)?.records_for(&installation.id)?;
    tauri::async_runtime::spawn_blocking(move || {
        scan::scan_addons(&installation.path, &installation.flavor)
            .map(|addons| scan::with_installed_projects(addons, &records))
    })
    .await
    // L'errore di Tauri dice se il compito è andato in panic o è stato annullato.
    .map_err(|error| format!("Reading the addons was interrupted ({error})."))?
}

/// Riconosce gli addon su CurseForge e indica quali hanno un aggiornamento. Scansione e rete
/// girano fuori dal thread principale.
#[tauri::command]
pub async fn check_updates(
    installation_id: String,
    store: Store<'_>,
    settings: Settings<'_>,
    config: State<'_, Config>,
) -> Result<Vec<AddonUpdate>, String> {
    let key = config
        .curseforge_api_key
        .clone()
        .ok_or_else(|| CurseForgeError::MissingKey.to_string())?;
    let installation = locked(&store)?
        .list()?
        .into_iter()
        .find(|installation| installation.id == installation_id)
        .ok_or_else(|| "Installation not found.".to_owned())?;
    let overrides = overrides_for(&settings, &installation.id)?;
    tauri::async_runtime::spawn_blocking(move || {
        // Il client bloccante di reqwest va creato e distrutto fuori dal runtime asincrono.
        let transport = ReqwestTransport::new().map_err(|e| e.to_string())?;
        let client = CurseForgeClient::new(transport, Some(key));
        updates::check_updates(&installation, &client, &overrides)
    })
    .await
    .map_err(|error| format!("The update check was interrupted ({error})."))?
}

/// Aggiorna i progetti `mod_ids` dell'installazione: rifà il controllo, scarica e installa, e
/// registra in `installed-addons.json` quello che ha installato. Rete e disco girano fuori dal
/// thread principale; il lock del registro si prende solo per scrivere ogni record.
#[tauri::command]
pub async fn update_addons(
    installation_id: String,
    mod_ids: Vec<u32>,
    app: AppHandle,
    store: Store<'_>,
    settings: Settings<'_>,
    config: State<'_, Config>,
) -> Result<Vec<UpdateOutcome>, String> {
    let key = config
        .curseforge_api_key
        .clone()
        .ok_or_else(|| CurseForgeError::MissingKey.to_string())?;
    let installation = find_installation(&store, &installation_id)?;
    let overrides = overrides_for(&settings, &installation.id)?;
    tauri::async_runtime::spawn_blocking(move || {
        // Il client bloccante di reqwest va creato e distrutto fuori dal runtime asincrono.
        let transport = ReqwestTransport::new().map_err(|e| e.to_string())?;
        let client = CurseForgeClient::new(transport, Some(key));
        let installed = app.state::<Mutex<InstalledAddonsStore>>();
        updates::update_addons(
            &installation,
            &client,
            &overrides,
            &mod_ids,
            &mut |mod_id, record| {
                locked_installed(&installed)?.record(&installation.id, mod_id, record)
            },
        )
    })
    .await
    .map_err(|error| format!("The update was interrupted ({error})."))?
}

fn find_installation(store: &Store<'_>, installation_id: &str) -> Result<Installation, String> {
    locked(store)?
        .list()?
        .into_iter()
        .find(|installation| installation.id == installation_id)
        .ok_or_else(|| "Installation not found.".to_owned())
}

/// `app_data_dir()/backups/<id-installazione>/`: la cartella la sceglie Tauri.
fn backups_dir(app: &AppHandle, installation_id: &str) -> Result<PathBuf, String> {
    let data = app
        .path()
        .app_data_dir()
        .map_err(|_| "The app data folder is not available.".to_owned())?;
    Ok(data.join("backups").join(installation_id))
}

/// Zip di `WTF` e `Interface` dell'installazione. Può leggere centinaia di MB, quindi gira
/// fuori dal thread principale.
#[tauri::command]
pub async fn create_backup(
    installation_id: String,
    app: AppHandle,
    store: Store<'_>,
) -> Result<BackupInfo, String> {
    let installation = find_installation(&store, &installation_id)?;
    let dir = backups_dir(&app, &installation.id)?;
    tauri::async_runtime::spawn_blocking(move || backup::create_backup(&installation, &dir))
        .await
        .map_err(|error| format!("The backup was interrupted ({error})."))?
}

/// I backup di un'installazione, dal più recente.
#[tauri::command]
pub async fn list_backups(
    installation_id: String,
    app: AppHandle,
    store: Store<'_>,
) -> Result<Vec<BackupInfo>, String> {
    let installation = find_installation(&store, &installation_id)?;
    let dir = backups_dir(&app, &installation.id)?;
    tauri::async_runtime::spawn_blocking(move || backup::list_backups(&dir))
        .await
        .map_err(|error| format!("Reading the backups was interrupted ({error})."))?
}
