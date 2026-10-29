//! Tauri commands invocable from the frontend.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, PoisonError, TryLockError};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::addon_import;
use crate::addon_list;
use crate::addon_remove;
use crate::addons::cache::FingerprintCache;
use crate::addons::scan::{self, ListedAddon};
use crate::auto_update::{AutoUpdate, AutoUpdateStatus};
use crate::autostart;
use crate::backup::{self, BackupInfo};
use crate::catalog::{self, AddonDetail, CatalogCategory, CatalogPage, CatalogQuery, CatalogSort};
use crate::config::Config;
use crate::discovery::{self, DiscoveredInstallation};
use crate::install_link::{self, LinkPreview, LinkSources};
use crate::install_local::{self, LocalInstall};
use crate::install_new::{self, InstallOutcome, MirrorAvailability};
use crate::installations::Installation;
use crate::parallel;
use crate::previous::{self, PreviousManifest, UndoOutcome};
use crate::progress::{self, Progress, ProgressEvent};
use crate::sources::curseforge::{
    self, CurseForgeClient, CurseForgeError, ReqwestTransport, SortOrder,
};
use crate::sources::github::{self, GithubSource};
use crate::sources::github_mirror;
use crate::sources::http::PublicReqwestTransport;
use crate::sources::tukui::{self, TukuiSource};
use crate::sources::wowinterface::{self, WowInterfaceSource};
use crate::sources::{ProjectKey, Source, SourceKind};
use crate::store::{
    AddonSettingsStore, AppSettings, AppSettingsStore, AutoUpdateMode, InstallationAutoUpdate,
    InstallationStore, InstalledAddonsStore, InstalledRecords, SourceRecord, SourceRecords, Theme,
};
use crate::tray::TrayStatus;
use crate::updates::{
    self, AddonUpdate, CurseForgeSource, UpdateOutcome, VersionOverrides, VersionType,
};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppStatus {
    version: &'static str,
    /// Only if the key is there: the key never leaves the backend.
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

type AppSettingsState<'a> = State<'a, Mutex<AppSettingsStore>>;

fn locked_app_settings(
    settings: &Mutex<AppSettingsStore>,
) -> Result<MutexGuard<'_, AppSettingsStore>, String> {
    settings
        .lock()
        .map_err(|_| "Settings are not available: restart the app.".to_owned())
}

/// The "Beta when there is no Release" option (issue #141). An unreadable `settings.json` does not
/// block search, installation and updates: the default applies, that is Release only.
pub(crate) fn beta_when_no_release(settings: &Mutex<AppSettingsStore>) -> bool {
    locked_app_settings(settings)
        .and_then(|store| store.get())
        .is_ok_and(|settings| settings.beta_when_no_release)
}

/// The settings as the frontend receives them: those saved in `settings.json` plus
/// `trayAvailable`, which is not saved because it only says whether the tray icon is there right
/// now and something shows it (issue #158). Without an icon `closeToTray` stays saved but has no
/// effect: the X closes.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsView {
    #[serde(flatten)]
    settings: AppSettings,
    tray_available: bool,
}

fn settings_view(
    settings: Result<AppSettings, String>,
    tray: &TrayStatus,
) -> Result<SettingsView, String> {
    Ok(SettingsView {
        settings: settings?,
        tray_available: tray.available(),
    })
}

/// Saves the "Beta when there is no Release" option and returns the updated settings.
#[tauri::command]
pub fn set_beta_when_no_release(
    enabled: bool,
    settings: AppSettingsState<'_>,
    tray: State<'_, TrayStatus>,
) -> Result<SettingsView, String> {
    settings_view(
        locked_app_settings(&settings)?.set_beta_when_no_release(enabled),
        &tray,
    )
}

/// Saves the "Close to tray" option (issue #158) and returns the updated settings.
#[tauri::command]
pub fn set_close_to_tray(
    enabled: bool,
    settings: AppSettingsState<'_>,
    tray: State<'_, TrayStatus>,
) -> Result<SettingsView, String> {
    settings_view(
        locked_app_settings(&settings)?.set_close_to_tray(enabled),
        &tray,
    )
}

/// Saves the mode (`off`, `notify` or `install`) and the interval in hours of the automatic
/// update (D41, issue #199) and returns the updated settings.
#[tauri::command]
pub fn set_auto_update(
    mode: String,
    interval_hours: u32,
    settings: AppSettingsState<'_>,
    tray: State<'_, TrayStatus>,
    auto_update: State<'_, AutoUpdate>,
) -> Result<SettingsView, String> {
    let saved = save_auto_update(&settings, &mode, interval_hours);
    if saved.is_ok() {
        // The run reschedules with the new mode and interval.
        auto_update.wake();
    }
    settings_view(saved, &tray)
}

/// The outcome of the last automatic run and when the next one is due (issue #200), with the
/// installations that have an undoable run (`undoable`, issue #201): the same payload as the
/// `auto-update` event, see [`AutoUpdateStatus`]. Memory and the
/// `auto-update-previous.json` manifest, no network.
#[tauri::command]
pub fn get_auto_update_status(app: AppHandle) -> AutoUpdateStatus {
    crate::auto_update::current_status(&app)
}

/// Undoes the last automatic run of an installation (issue #201): for every project of the run it
/// puts back the previous folders and record, skipping those changed afterwards. Returns one
/// outcome per project ([`UndoOutcome`]) and emits the `auto-update` event with the new state.
/// Blocking, under [`InstallLock`] like the updates.
#[tauri::command]
pub async fn undo_last_auto_update(
    installation_id: String,
    app: AppHandle,
    store: Store<'_>,
) -> Result<Vec<UndoOutcome>, String> {
    let installation = find_installation(&store, &installation_id)?;
    tauri::async_runtime::spawn_blocking(move || {
        let lock = app.state::<InstallLock>();
        let _held = lock.hold();
        let result = previous::undo(
            &installation,
            &app.state::<PreviousManifest>(),
            &app.state::<Mutex<InstalledAddonsStore>>(),
            Some(&app.state::<FingerprintCache>()),
        );
        let _ = app.emit(
            crate::auto_update::EVENT,
            crate::auto_update::current_status(&app),
        );
        result
    })
    .await
    .map_err(|error| format!("The undo was interrupted ({error})."))?
}

/// The body of [`set_auto_update`], separated from Tauri's `State` for the tests. The values are
/// checked before the lock: an unknown one touches neither the memory nor the file.
fn save_auto_update(
    settings: &Mutex<AppSettingsStore>,
    mode: &str,
    interval_hours: u32,
) -> Result<AppSettings, String> {
    let mode = AutoUpdateMode::parse(mode)?;
    crate::store::check_auto_update_interval(interval_hours)?;
    locked_app_settings(settings)?.set_auto_update(mode, interval_hours)
}

/// Whether the installation takes part in the automatic update and which projects are excluded
/// from it (issue #199). Each excluded one is `{ source, key }`: the frontend matches it to a row
/// by comparing `project.source` and `project.key` of its `AddonUpdate` (see [`ProjectKey`]).
#[tauri::command]
pub fn get_auto_update(
    installation_id: String,
    store: Store<'_>,
    settings: Settings<'_>,
) -> Result<InstallationAutoUpdate, String> {
    find_installation(&store, &installation_id)?;
    locked_settings(&settings)?.auto_update_for(&installation_id)
}

/// Excludes a project from the automatic update (`excluded`) or readmits it.
#[tauri::command]
pub fn set_auto_update_excluded(
    installation_id: String,
    project: ProjectKey,
    excluded: bool,
    store: Store<'_>,
    settings: Settings<'_>,
) -> Result<InstallationAutoUpdate, String> {
    find_installation(&store, &installation_id)?;
    locked_settings(&settings)?.set_auto_update_excluded(&installation_id, project, excluded)
}

/// Makes an installation take part in the automatic update (`enabled`) or excludes it.
#[tauri::command]
pub fn set_installation_auto_update(
    installation_id: String,
    enabled: bool,
    store: Store<'_>,
    settings: Settings<'_>,
) -> Result<InstallationAutoUpdate, String> {
    find_installation(&store, &installation_id)?;
    locked_settings(&settings)?.set_installation_auto_update(&installation_id, enabled)
}

/// Whether the app starts with the system (issue #159). The state is read from the operating
/// system on every call: it is not in `settings.json`.
#[tauri::command]
pub fn get_autostart(app: AppHandle) -> Result<bool, String> {
    autostart::is_enabled(&app)
}

/// Turns starting with the system on or off and returns the state reread from the operating
/// system.
#[tauri::command]
pub fn set_autostart(app: AppHandle, enabled: bool) -> Result<bool, String> {
    autostart::set_enabled(&app, enabled)
}

/// The app's settings (theme, Beta option, "Close to tray", automatic update) and whether the
/// tray icon is there; without `settings.json` the defaults apply.
#[tauri::command]
pub fn get_settings(
    settings: AppSettingsState<'_>,
    tray: State<'_, TrayStatus>,
) -> Result<SettingsView, String> {
    settings_view(locked_app_settings(&settings)?.get(), &tray)
}

/// Saves the theme (`system`, `light` or `dark`) and returns the updated settings.
#[tauri::command]
pub fn set_theme(
    theme: String,
    settings: AppSettingsState<'_>,
    tray: State<'_, TrayStatus>,
) -> Result<SettingsView, String> {
    settings_view(save_theme(&settings, &theme), &tray)
}

/// The body of [`set_theme`], separated from Tauri's `State` for the tests. The name is checked
/// before the lock: an unknown value touches neither the memory nor the file.
fn save_theme(settings: &Mutex<AppSettingsStore>, theme: &str) -> Result<AppSettings, String> {
    let theme = Theme::parse(theme)?;
    locked_app_settings(settings)?.set_theme(theme)
}

/// Serialises installations and updates: `install_addon` and `update_addons` hold it for all the
/// blocking work, so the check of other projects' folders and the replacement on disk do not
/// interleave with another installation. The lock protects no data: a panic that poisons it
/// leaves nothing inconsistent, so it is recovered.
#[derive(Default)]
pub struct InstallLock(Mutex<()>);

impl InstallLock {
    fn hold(&self) -> MutexGuard<'_, ()> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Like [`hold`](Self::hold), but without waiting: `None` if an installation or an update
    /// already holds it. Used by the automatic run (issue #200), which skips and retries
    /// instead of queuing behind a user command.
    pub(crate) fn try_hold(&self) -> Option<MutexGuard<'_, ()>> {
        match self.0.try_lock() {
            Ok(guard) => Some(guard),
            Err(TryLockError::Poisoned(poisoned)) => Some(poisoned.into_inner()),
            Err(TryLockError::WouldBlock) => None,
        }
    }
}

pub(crate) fn overrides_for(
    settings: &Mutex<AddonSettingsStore>,
    installation_id: &str,
) -> Result<VersionOverrides, String> {
    settings
        .lock()
        .map_err(|_| "Addon settings are not available: restart the app.".to_owned())?
        .overrides_for(installation_id)
}

#[tauri::command]
pub fn list_installations(store: Store<'_>) -> Result<Vec<Installation>, String> {
    locked(&store)?.list()
}

#[tauri::command]
pub fn add_installation(path: String, store: Store<'_>) -> Result<Installation, String> {
    locked(&store)?.add(Path::new(&path))
}

/// Looks for WoW installations in the platform's standard paths and returns those not added
/// yet. It only suggests: it does not write `installations.json`. Finding nothing is not an
/// error. Reading the disk (and the registry on Windows) runs off the
/// main thread.
#[tauri::command]
pub async fn discover_installations(
    app: AppHandle,
    store: Store<'_>,
) -> Result<Vec<DiscoveredInstallation>, String> {
    let known = locked(&store)?.list()?;
    let home = app.path().home_dir().ok();
    tauri::async_runtime::spawn_blocking(move || {
        discovery::examine(&discovery::candidate_roots(home.as_deref()), &known)
    })
    .await
    .map_err(|error| format!("The search for installations was interrupted ({error})."))
}

/// Removes the settings and the addon records first, then the installation: if something cannot
/// be removed the installation stays and can be retried, instead of leaving orphaned data. The
/// locks are never held together.
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

/// The game version types that can be forced for an addon. No network call.
#[tauri::command]
pub fn list_version_types() -> Vec<VersionType> {
    updates::version_types()
}

/// Forces the game version used to look for updates of the project `mod_id`.
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

/// Goes back to the installation's game version for the project `mod_id`.
#[tauri::command]
pub fn clear_version_override(
    installation_id: String,
    mod_id: u32,
    settings: Settings<'_>,
) -> Result<(), String> {
    locked_settings(&settings)?.clear_override(&installation_id, mod_id)
}

/// The addons of an installation, with the project the app installed there (issue #62). The
/// scan reads the disk, so it runs off the main thread so as not to block the window with many
/// addons; the registry is read first, here. An unreadable registry does not hide the addons:
/// the read goes ahead without records, while writes stay blocked and the next update reports
/// the error in its outcome.
///
/// The fingerprints come from [`FingerprintCache`]; with `force` (the "Reload addons" button) they
/// are all recomputed, so a change made by hand outside the app always shows.
#[tauri::command]
pub async fn list_addons(
    installation_id: String,
    force: Option<bool>,
    app: AppHandle,
    store: Store<'_>,
    installed: Installed<'_>,
) -> Result<Vec<ListedAddon>, String> {
    let installation = locked(&store)?
        .list()?
        .into_iter()
        .find(|installation| installation.id == installation_id)
        .ok_or_else(|| "Installation not found.".to_owned())?;
    let records = locked_installed(&installed)?
        .records_for(&installation.id)
        .unwrap_or_default();
    tauri::async_runtime::spawn_blocking(move || {
        let cache = app.state::<FingerprintCache>();
        if force == Some(true) {
            cache.forget_all(&installation.path.join("Interface").join("AddOns"));
        }
        scan::scan_addons_with(&installation.path, &installation.flavor, Some(&cache))
            .map(|addons| scan::with_installed_projects(addons, &records))
    })
    .await
    // Tauri's error says whether the task panicked or was cancelled.
    .map_err(|error| format!("Reading the addons was interrupted ({error})."))?
}

/// The addon list and their update states from a single scan (issue #126): "Reload addons" and
/// the rereads after an update or an installation use it instead of `list_addons` followed by
/// `check_updates`. A failed check does not take away the list, which arrives with
/// `checkError`. `force` recomputes the fingerprints as in `list_addons`.
#[tauri::command]
pub async fn refresh_addons(
    installation_id: String,
    force: Option<bool>,
    app: AppHandle,
    store: Store<'_>,
    settings: Settings<'_>,
    installed: Installed<'_>,
    config: State<'_, Config>,
) -> Result<updates::Refresh, String> {
    let key = config.curseforge_api_key.clone();
    let installation = find_installation(&store, &installation_id)?;
    let overrides = overrides_for(&settings, &installation.id)?;
    let allow_beta = beta_when_no_release(&app.state::<Mutex<AppSettingsStore>>());
    let records = records_for_matching(&installed, &installation.id);
    // As in `list_addons`: an unreadable registry does not hide the addons.
    let listed_records = locked_installed(&installed)?
        .records_for(&installation.id)
        .unwrap_or_default();
    tauri::async_runtime::spawn_blocking(move || {
        let cache = app.state::<FingerprintCache>();
        if force == Some(true) {
            cache.forget_all(&installation.path.join("Interface").join("AddOns"));
        }
        refresh_blocking(
            &installation,
            &cache,
            key,
            &overrides,
            allow_beta,
            &records,
            &listed_records,
        )
    })
    .await
    .map_err(|error| format!("Reading the addons was interrupted ({error})."))?
}

/// The body of [`refresh_addons`], outside the async runtime: the export uses it too.
fn refresh_blocking(
    installation: &Installation,
    cache: &FingerprintCache,
    key: Option<String>,
    overrides: &VersionOverrides,
    allow_beta: bool,
    records: &SourceRecords,
    listed_records: &InstalledRecords,
) -> Result<updates::Refresh, String> {
    // reqwest's blocking client must be created and dropped outside the async runtime. If the
    // connections cannot be set up, the list arrives anyway, with the reason.
    let prepared = ReqwestTransport::new()
        .map_err(|e| e.to_string())
        .and_then(|transport| {
            Ok((
                transport,
                tukui_source()?,
                wowinterface_source()?,
                github_source()?,
            ))
        });
    let (transport, tukui, wowi, github) = match prepared {
        Ok(prepared) => prepared,
        Err(error) => {
            let addons =
                scan::scan_addons_with(&installation.path, &installation.flavor, Some(cache))?;
            return Ok(updates::Refresh {
                addons: scan::with_installed_projects(addons, listed_records),
                updates: Vec::new(),
                check_error: Some(error),
            });
        }
    };
    let client = CurseForgeClient::new(transport, key);
    let curseforge = CurseForgeSource {
        client: &client,
        overrides,
        allow_beta,
        github: Some(&github),
    };
    updates::refresh_addons(
        installation,
        Some(cache),
        &[&curseforge, &tukui, &wowi, &github],
        records,
        listed_records,
    )
}

/// The string with the installation's addons (issue #133): the projects recognised by the check
/// and those recorded by the app, with the forced game version. The folders that no project
/// covers (for example addons installed from a zip) are left out, by name. Without a CurseForge
/// key at least what the free sources and the records know comes out.
#[tauri::command]
pub async fn export_addons(
    installation_id: String,
    app: AppHandle,
    store: Store<'_>,
    settings: Settings<'_>,
    installed: Installed<'_>,
    config: State<'_, Config>,
) -> Result<addon_list::Export, String> {
    let key = config.curseforge_api_key.clone();
    let installation = find_installation(&store, &installation_id)?;
    let overrides = overrides_for(&settings, &installation.id)?;
    let records = records_for_matching(&installed, &installation.id);
    let listed_records = locked_installed(&installed)?
        .records_for(&installation.id)
        .unwrap_or_default();
    tauri::async_runtime::spawn_blocking(move || {
        let cache = app.state::<FingerprintCache>();
        // The Beta option changes only the states, not which projects are recognised.
        let allow_beta = beta_when_no_release(&app.state::<Mutex<AppSettingsStore>>());
        let refresh = refresh_blocking(
            &installation,
            &cache,
            key,
            &overrides,
            allow_beta,
            &records,
            &listed_records,
        )?;
        Ok(addon_list::export_list(
            &installation.flavor,
            &refresh,
            &records,
            &overrides,
        ))
    })
    .await
    .map_err(|error| format!("The export was interrupted ({error})."))?
}

/// The preview of a string import (issue #134): for each project whether it is new, already
/// installed or not downloadable, the invalid entries and the folders *Replace all* would remove.
/// It writes nothing.
#[tauri::command]
pub async fn preview_import(
    installation_id: String,
    text: String,
    app: AppHandle,
    store: Store<'_>,
    settings: Settings<'_>,
    installed: Installed<'_>,
    config: State<'_, Config>,
) -> Result<addon_import::Preview, String> {
    let decoded = addon_list::decode(&text)?;
    let key = config.curseforge_api_key.clone();
    let installation = find_installation(&store, &installation_id)?;
    let overrides = overrides_for(&settings, &installation.id)?;
    let records = records_for_matching(&installed, &installation.id);
    let listed_records = locked_installed(&installed)?
        .records_for(&installation.id)
        .unwrap_or_default();
    tauri::async_runtime::spawn_blocking(move || {
        let cache = app.state::<FingerprintCache>();
        let allow_beta = beta_when_no_release(&app.state::<Mutex<AppSettingsStore>>());
        let refresh = refresh_blocking(
            &installation,
            &cache,
            key.clone(),
            &overrides,
            allow_beta,
            &records,
            &listed_records,
        )?;
        let transport = ReqwestTransport::new().map_err(|e| e.to_string())?;
        let has_key = key.is_some();
        let client = CurseForgeClient::new(transport, key);
        let tukui = tukui_source()?;
        let (availability, lookup_error) =
            import_availability(&decoded.list.addons, &client, has_key, &tukui);
        Ok(addon_import::preview(
            &decoded,
            &refresh,
            &records,
            &|entry| availability_of(&availability, entry),
            lookup_error,
        ))
    })
    .await
    .map_err(|error| format!("The preview was interrupted ({error})."))?
}

/// Imports an addon string (issue #134): installs the missing projects and, with `replace`,
/// removes from `Interface/AddOns` the folders no imported project claims, but only those in
/// `confirmedRemovals` (the list the user saw in the preview). No backup, by the user's
/// choice. Holds [`InstallLock`] for all the work.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn import_addons(
    installation_id: String,
    text: String,
    mode: addon_import::ImportMode,
    confirmed_removals: Option<Vec<String>>,
    app: AppHandle,
    store: Store<'_>,
    settings: Settings<'_>,
    config: State<'_, Config>,
) -> Result<addon_import::ImportResult, String> {
    let decoded = addon_list::decode(&text)?;
    let key = config.curseforge_api_key.clone();
    let installation = find_installation(&store, &installation_id)?;
    // The forced versions are reread after writing them: here they only need to be readable.
    overrides_for(&settings, &installation.id)?;
    let confirmed = confirmed_removals.unwrap_or_default();
    tauri::async_runtime::spawn_blocking(move || {
        let lock = app.state::<InstallLock>();
        let _held = lock.hold();
        let cache = app.state::<FingerprintCache>();
        let installed = app.state::<Mutex<InstalledAddonsStore>>();
        let settings = app.state::<Mutex<AddonSettingsStore>>();
        let addons_dir = installation.path.join("Interface").join("AddOns");
        let allow_beta = beta_when_no_release(&app.state::<Mutex<AppSettingsStore>>());
        // The state before and after the installations, with the forced versions of that moment.
        let read_state = || -> Result<(updates::Refresh, SourceRecords), String> {
            let overrides = settings
                .lock()
                .map_err(|_| "Addon settings are not available: restart the app.".to_owned())?
                .overrides_for(&installation.id)?;
            let (records, listed) = {
                let store = locked_installed(&installed)?;
                (
                    store
                        .source_records_for(&installation.id)
                        .unwrap_or_default(),
                    store.records_for(&installation.id).unwrap_or_default(),
                )
            };
            let refresh = refresh_blocking(
                &installation,
                &cache,
                key.clone(),
                &overrides,
                allow_beta,
                &records,
                &listed,
            )?;
            Ok((refresh, records))
        };
        let (before, records) = read_state()?;
        let transport = ReqwestTransport::new().map_err(|e| e.to_string())?;
        let has_key = key.is_some();
        let client = CurseForgeClient::new(transport, key.clone());
        let tukui = tukui_source()?;
        let github = github_source()?;
        let (availability, lookup_error) =
            import_availability(&decoded.list.addons, &client, has_key, &tukui);
        let progress = emit_progress(&app, &installation.id);
        let mut installer = CommandInstaller {
            installation: &installation,
            client: &client,
            tukui: &tukui,
            github: &github,
            availability: &availability,
            allow_beta,
            installed: &installed,
            settings: &settings,
            cache: &cache,
            progress: &progress,
            prepared: Prepared::default(),
        };
        let mut result = addon_import::run_import(
            &addons_dir,
            &decoded,
            mode,
            &confirmed,
            &before,
            &records,
            &mut installer,
            &mut || read_state(),
        );
        result.lookup_error = lookup_error;
        if !result.removed.is_empty() {
            cache.forget(&addons_dir, &result.removed);
            // The folders are already removed: a registry that does not update undoes nothing.
            let _ = locked_installed(&installed)
                .and_then(|mut store| store.forget_folders(&installation.id, &result.removed));
        }
        Ok(result)
    })
    .await
    .map_err(|error| format!("The import was interrupted ({error})."))?
}

type Availabilities = HashMap<(SourceKind, String), addon_import::Availability>;

/// The CurseForge projects requested in a single `get_mods` by the preview of an import.
const MODS_PER_REQUEST: usize = 100;

/// Whether the projects of the string can be downloaded: for CurseForge from `allowModDistribution`
/// (a single request for all of them), for Tukui from its list. If a source does not answer, its
/// projects count as downloadable (the installation will give the reason) and the second value
/// reports the error.
fn import_availability(
    entries: &[addon_list::ListEntry],
    client: &CurseForgeClient<ReqwestTransport>,
    has_key: bool,
    tukui: &TukuiSource<PublicReqwestTransport>,
) -> (Availabilities, Option<String>) {
    use addon_import::Availability;
    let mut availability = Availabilities::new();
    let mut errors = Vec::new();
    let not_downloadable = |message: &str, url: Option<String>| Availability::NotDownloadable {
        message: message.to_owned(),
        url,
    };

    let mod_ids: Vec<u32> = entries
        .iter()
        .filter(|e| e.source == SourceKind::Curseforge)
        .filter_map(|e| e.key.parse().ok())
        .collect();
    if !mod_ids.is_empty() {
        if !has_key {
            for id in &mod_ids {
                availability.insert(
                    (SourceKind::Curseforge, id.to_string()),
                    not_downloadable(&CurseForgeError::MissingKey.to_string(), None),
                );
            }
        } else {
            // In chunks: a string can have up to 1,000 projects, and a chunk that fails does not
            // take away the answers of the others.
            for chunk in mod_ids.chunks(MODS_PER_REQUEST) {
                match client.get_mods(chunk) {
                    Ok(mods) => {
                        let by_id: HashMap<u32, _> = mods.into_iter().map(|m| (m.id, m)).collect();
                        for id in chunk {
                            let state = match by_id.get(id) {
                                Some(found) => curseforge_availability(found),
                                None => not_downloadable("Addon not found on CurseForge.", None),
                            };
                            availability.insert((SourceKind::Curseforge, id.to_string()), state);
                        }
                    }
                    Err(error) => {
                        let error = error.to_string();
                        if !errors.contains(&error) {
                            errors.push(error);
                        }
                    }
                }
            }
        }
    }

    if entries.iter().any(|e| e.source == SourceKind::Tukui) {
        match tukui.listed() {
            Ok(listed) => {
                for entry in entries.iter().filter(|e| e.source == SourceKind::Tukui) {
                    let found = listed.iter().find(|a| a.id.to_string() == entry.key);
                    let state = match found {
                        Some(_) => Availability::Available,
                        None => not_downloadable("Tukui does not list this addon.", None),
                    };
                    availability.insert((SourceKind::Tukui, entry.key.clone()), state);
                }
            }
            Err(error) => errors.push(error),
        }
    }

    for entry in entries
        .iter()
        .filter(|e| !matches!(e.source, SourceKind::Curseforge | SourceKind::Tukui))
    {
        // A GitHub project counts as installable without reading the release, so as not to spend
        // the request limit; an id that is not `owner/repo` does not.
        let message = match entry.source {
            SourceKind::Github if github::is_valid_repo(&entry.key) => continue,
            SourceKind::Github => "Invalid GitHub project: it must be owner/repo.".to_owned(),
            source => format!("WoW-don cannot install from {} yet.", source.label()),
        };
        availability.insert(
            (entry.source, entry.key.clone()),
            not_downloadable(&message, None),
        );
    }
    let error = (!errors.is_empty()).then(|| errors.join(" "));
    (availability, error)
}

/// Whether a CurseForge project in the string can be installed: yes if CurseForge allows the
/// download, and also if it blocks it but the project declares a GitHub repository (issue #179):
/// the installation tries the mirror (D34) and, if it cannot be used, gives the reason. No network.
fn curseforge_availability(found: &curseforge::Mod) -> addon_import::Availability {
    if found.allow_mod_distribution == Some(true)
        || github_mirror::github_repo(found.links.source_url.as_deref()).is_some()
    {
        return addon_import::Availability::Available;
    }
    addon_import::Availability::NotDownloadable {
        message: "The author does not allow downloads from third-party apps: install it from the CurseForge website.".to_owned(),
        url: found.links.website_url.clone(),
    }
}

fn availability_of(
    availability: &Availabilities,
    entry: &addon_list::ListEntry,
) -> addon_import::Availability {
    availability
        .get(&(entry.source, entry.key.clone()))
        .cloned()
        .unwrap_or(addon_import::Availability::Available)
}

/// Installs the projects of an import with the real sources. It does not know Tauri: the app
/// state arrives by reference, so the test bench (`import_bench`) uses the same path.
struct CommandInstaller<'a> {
    installation: &'a Installation,
    client: &'a CurseForgeClient<ReqwestTransport>,
    tukui: &'a TukuiSource<PublicReqwestTransport>,
    /// For GitHub projects and for the mirror of blocked CurseForge projects (issue #179).
    /// The preview does not read the releases.
    github: &'a GithubSource<PublicReqwestTransport>,
    availability: &'a Availabilities,
    /// The "Use beta files when there is no release" setting (issue #141).
    allow_beta: bool,
    installed: &'a Mutex<InstalledAddonsStore>,
    settings: &'a Mutex<AddonSettingsStore>,
    cache: &'a FingerprintCache,
    progress: &'a (dyn Fn(Progress) + Sync),
    /// What the first phase downloaded and the second has not installed yet (issue #180).
    prepared: Prepared,
}

/// The zips downloaded by the first phase of an import, waiting for their installation.
#[derive(Default)]
struct Prepared {
    curseforge: install_new::Prefetched,
    /// By Tukui key, the zip or the outcome that says why there is none.
    tukui: HashMap<String, Result<tukui::TukuiDownload, Box<updates::UpdateOutcome>>>,
    /// By GitHub repository, the release with the zip or the outcome that says why there is none.
    github: HashMap<String, Result<github::GithubDownload, Box<updates::UpdateOutcome>>>,
}

/// A free-source zip downloaded by the first phase.
enum FreeDownload {
    Tukui(Result<tukui::TukuiDownload, Box<updates::UpdateOutcome>>),
    Github(Result<github::GithubDownload, Box<updates::UpdateOutcome>>),
}

impl CommandInstaller<'_> {
    /// The real requests to CurseForge, for the first phase and for what it did not read.
    fn live(&self) -> install_new::Live<'_, ReqwestTransport> {
        install_new::Live {
            client: self.client,
            // A project blocked by CurseForge is installed from the GitHub mirror (D34): the
            // releases are read without the API (issue #179), so a long list does not use up the
            // limit of 60 requests per hour.
            github: Some(self.github),
            allow_beta: self.allow_beta,
        }
    }
}

impl addon_import::Installer for CommandInstaller<'_> {
    /// Resolves and downloads the group's entries, several at once
    /// ([`parallel::DOWNLOADS_AT_ONCE`]); the disk and the records are not touched.
    fn prepare(&mut self, entries: &[&addon_list::ListEntry]) {
        let installation = self.installation;
        let availability = self.availability;
        let available = |source: SourceKind| {
            entries.iter().filter(move |entry| {
                entry.source == source
                    && matches!(
                        availability_of(availability, entry),
                        addon_import::Availability::Available
                    )
            })
        };
        let overrides = self
            .settings
            .lock()
            .ok()
            .and_then(|store| store.overrides_for(&installation.id).ok())
            .unwrap_or_default();
        // The game version the installation will use, with the same function: `install` saves
        // the entry's one as the forced version before rereading them, so here it counts as
        // already saved.
        let projects: Vec<(u32, Option<u32>)> = available(SourceKind::Curseforge)
            .filter_map(|entry| {
                let mod_id = entry.key.parse::<u32>().ok()?;
                let mut overrides = overrides.clone();
                if let Some(version_type) = entry.version_type {
                    overrides.insert(mod_id, version_type);
                }
                Some((
                    mod_id,
                    install_new::version_type_of(installation, &overrides, mod_id),
                ))
            })
            .collect();
        // Without records the installation stops anyway: nothing to download.
        if !projects.is_empty()
            && let Ok(records) = locked_installed(self.installed)
                .and_then(|store| store.records_for(&installation.id))
        {
            let installed = catalog::installed_projects(installation, &records);
            install_new::prefetch(
                installation,
                &self.live(),
                &installed,
                &projects,
                &self.prepared.curseforge,
                self.progress,
            );
        }
        // Tukui and GitHub together, with the same limit.
        let free: Vec<(SourceKind, &str)> = available(SourceKind::Tukui)
            .chain(available(SourceKind::Github))
            .map(|entry| (entry.source, entry.key.as_str()))
            .collect();
        let progress = self.progress;
        let (tukui, github) = (self.tukui, self.github);
        let downloads = parallel::map(&free, |(source, key)| {
            let downloaded = |name: &str| {
                progress::Reporter::new(progress, Some(*source), key, name)
                    .phase(progress::Phase::Downloaded);
            };
            if *source == SourceKind::Tukui {
                let download = tukui.download_new(key, progress);
                if let Ok(download) = &download {
                    downloaded(download.name());
                }
                FreeDownload::Tukui(download)
            } else {
                let download = github.download_new(installation, key, progress);
                if download.is_ok() {
                    downloaded(github::project_name(key));
                }
                FreeDownload::Github(download)
            }
        });
        for ((_, key), download) in free.into_iter().zip(downloads) {
            match download {
                FreeDownload::Tukui(download) => {
                    self.prepared.tukui.insert(key.to_owned(), download);
                }
                FreeDownload::Github(download) => {
                    self.prepared.github.insert(key.to_owned(), download);
                }
            }
        }
    }

    fn install(&mut self, entry: &addon_list::ListEntry) -> addon_import::EntryOutcome {
        use addon_import::{EntryOutcome, EntryStatus};
        if let addon_import::Availability::NotDownloadable { message, .. } =
            availability_of(self.availability, entry)
        {
            return EntryOutcome::new(entry, EntryStatus::Skipped, Some(message));
        }
        let installation = self.installation;
        let addons_dir = installation.path.join("Interface").join("AddOns");
        let installed = self.installed;
        let cache = self.cache;
        let progress = self.progress;
        match entry.source {
            SourceKind::Curseforge => {
                let Ok(mod_id) = entry.key.parse::<u32>() else {
                    return EntryOutcome::new(
                        entry,
                        EntryStatus::Error,
                        Some("Invalid CurseForge project id.".to_owned()),
                    );
                };
                let mut note = None;
                let overrides = match self.settings.lock() {
                    Ok(mut store) => {
                        if let Some(version_type) = entry.version_type
                            && let Err(error) =
                                store.set_override(&installation.id, mod_id, version_type)
                        {
                            note = Some(format!("The game version could not be kept: {error}"));
                        }
                        store.overrides_for(&installation.id).unwrap_or_default()
                    }
                    Err(_) => VersionOverrides::new(),
                };
                let records = match locked_installed(installed)
                    .and_then(|store| store.records_for(&installation.id))
                {
                    Ok(records) => records,
                    Err(error) => return EntryOutcome::new(entry, EntryStatus::Error, Some(error)),
                };
                let live = self.live();
                let cached = install_new::Cached {
                    live: &live,
                    prefetched: &self.prepared.curseforge,
                };
                let outcomes = install_new::install_addon_with(
                    installation,
                    &cached,
                    &overrides,
                    &records,
                    mod_id,
                    None,
                    &mut |mod_id, record| {
                        cache.forget(&addons_dir, &record.folders);
                        locked_installed(installed)?.record(&installation.id, mod_id, record)
                    },
                    progress,
                );
                import_outcome_from_install(entry, &outcomes, note)
            }
            SourceKind::Tukui | SourceKind::Github => {
                let mut record = |source, key: &str, record: SourceRecord| {
                    cache.forget(&addons_dir, &record.folders);
                    locked_installed(installed)?.record_source(
                        &installation.id,
                        source,
                        key,
                        record,
                    )
                };
                let outcome = if entry.source == SourceKind::Tukui {
                    match self.prepared.tukui.remove(&entry.key) {
                        Some(Ok(download)) => tukui::install_downloaded(
                            installation,
                            &entry.key,
                            download,
                            &|_| Ok(()),
                            &mut record,
                            progress,
                        ),
                        Some(Err(outcome)) => *outcome,
                        None => self.tukui.install_new(
                            installation,
                            &entry.key,
                            &|_| Ok(()),
                            &mut record,
                            progress,
                        ),
                    }
                } else {
                    match self.prepared.github.remove(&entry.key) {
                        Some(Ok(download)) => github::install_downloaded(
                            installation,
                            &entry.key,
                            &download,
                            &|_| Ok(()),
                            &mut record,
                            progress,
                        ),
                        Some(Err(outcome)) => *outcome,
                        None => self.github.install_new(
                            installation,
                            &entry.key,
                            &|_| Ok(()),
                            &mut record,
                            progress,
                        ),
                    }
                };
                let status = match outcome.status {
                    updates::OutcomeStatus::Updated => EntryStatus::Installed,
                    updates::OutcomeStatus::Skipped => EntryStatus::Skipped,
                    updates::OutcomeStatus::Error => EntryStatus::Error,
                };
                EntryOutcome::new(entry, status, outcome.message)
            }
            _ => EntryOutcome::new(
                entry,
                EntryStatus::Skipped,
                Some(format!(
                    "WoW-don cannot install from {} yet.",
                    entry.source.label()
                )),
            ),
        }
    }
}

/// The outcome of an imported CurseForge project from the outcomes of `install_addon`: the first is
/// the project, the others its dependencies.
fn import_outcome_from_install(
    entry: &addon_list::ListEntry,
    outcomes: &[InstallOutcome],
    note: Option<String>,
) -> addon_import::EntryOutcome {
    use addon_import::{EntryOutcome, EntryStatus};
    let Some(first) = outcomes.first() else {
        return EntryOutcome::new(
            entry,
            EntryStatus::Error,
            Some("Nothing happened.".to_owned()),
        );
    };
    let status = match first.status {
        install_new::InstallStatus::Installed => EntryStatus::Installed,
        install_new::InstallStatus::Skipped => EntryStatus::Skipped,
        install_new::InstallStatus::Error => EntryStatus::Error,
    };
    let dependencies = outcomes[1..]
        .iter()
        .filter(|o| o.status == install_new::InstallStatus::Installed)
        .count();
    let mut parts: Vec<String> = first.message.iter().cloned().collect();
    if dependencies > 0 {
        parts.push(format!(
            "Also installed {dependencies} {}.",
            if dependencies == 1 {
                "dependency"
            } else {
                "dependencies"
            }
        ));
    }
    parts.extend(note);
    let mut outcome =
        EntryOutcome::new(entry, status, (!parts.is_empty()).then(|| parts.join(" ")));
    if !first.name.is_empty() {
        outcome.name = first.name.clone();
    }
    outcome
}

/// Recognises the addons with the sources (D33) and indicates which have an update. Scan and
/// network run off the main thread.
#[tauri::command]
pub async fn check_updates(
    installation_id: String,
    app: AppHandle,
    store: Store<'_>,
    settings: Settings<'_>,
    installed: Installed<'_>,
    config: State<'_, Config>,
) -> Result<Vec<AddonUpdate>, String> {
    // Without a CurseForge key it checks nothing, but the free sources do (D31): the folders
    // that none recognises report the reason.
    let key = config.curseforge_api_key.clone();
    let installation = find_installation(&store, &installation_id)?;
    let overrides = overrides_for(&settings, &installation.id)?;
    let allow_beta = beta_when_no_release(&app.state::<Mutex<AppSettingsStore>>());
    let records = records_for_matching(&installed, &installation.id);
    tauri::async_runtime::spawn_blocking(move || {
        let sources = LiveSources::new(key)?;
        let cache = app.state::<FingerprintCache>();
        sources.with(&overrides, allow_beta, |sources| {
            updates::check_updates_with(&installation, Some(&cache), sources, &records)
        })
    })
    .await
    .map_err(|error| format!("The update check was interrupted ({error})."))?
}

/// Updates the installation's projects: `mod_ids` are CurseForge projects, `projects` those
/// of any source (`{source, key}`). With `reinstall` (default `false`, issue #148) it also
/// redownloads the projects already up to date, with the same loop and the same rules. It redoes
/// the check, downloads and installs, and records in `installed-addons.json` what it installed.
/// Network and disk run off the main thread, under [`InstallLock`]; the registry lock is taken
/// only to write each record.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn update_addons(
    installation_id: String,
    mod_ids: Vec<u32>,
    projects: Option<Vec<ProjectKey>>,
    reinstall: Option<bool>,
    app: AppHandle,
    store: Store<'_>,
    settings: Settings<'_>,
    installed: Installed<'_>,
    config: State<'_, Config>,
) -> Result<Vec<UpdateOutcome>, String> {
    let reinstall = reinstall.unwrap_or(false);
    // Without a CurseForge key it checks nothing, but the free sources do (D31): the folders
    // that none recognises report the reason.
    let key = config.curseforge_api_key.clone();
    let installation = find_installation(&store, &installation_id)?;
    let overrides = overrides_for(&settings, &installation.id)?;
    let allow_beta = beta_when_no_release(&app.state::<Mutex<AppSettingsStore>>());
    let records = records_for_matching(&installed, &installation.id);
    let projects: Vec<ProjectKey> = mod_ids
        .iter()
        .map(|mod_id| ProjectKey {
            source: SourceKind::Curseforge,
            key: mod_id.to_string(),
        })
        .chain(projects.unwrap_or_default())
        .collect();
    tauri::async_runtime::spawn_blocking(move || {
        let lock = app.state::<InstallLock>();
        let _held = lock.hold();
        let sources = LiveSources::new(key)?;
        let installed = app.state::<Mutex<InstalledAddonsStore>>();
        let cache = app.state::<FingerprintCache>();
        let mut record = record_writer(&installed, &cache, &installation);
        sources.with(&overrides, allow_beta, |sources| {
            updates::update_addons_with(
                &installation,
                Some(&cache),
                sources,
                &records,
                &projects,
                reinstall,
                &mut record,
                &emit_progress(&app, &installation.id),
            )
        })
    })
    .await
    .map_err(|error| format!("The update was interrupted ({error})."))?
}

/// Delivers an installation's progress to the window as a [`progress::EVENT`] event
/// (issue #127). An event that fails to go out does not stop the installation.
fn emit_progress<'a>(app: &'a AppHandle, installation_id: &'a str) -> impl Fn(Progress) + 'a {
    move |progress| {
        let _ = app.emit(
            progress::EVENT,
            ProgressEvent {
                installation_id,
                progress,
            },
        );
    }
}

/// The sources for checks and updates, with the CurseForge key if there is one (D31: without it,
/// the free sources work anyway). `check_updates`, `update_addons` and the automatic run
/// (`auto_update`, issue #200) use them, so the three paths have the same sources in the same
/// order. reqwest's blocking client must be created and dropped outside the async runtime.
pub(crate) struct LiveSources {
    client: CurseForgeClient<ReqwestTransport>,
    tukui: TukuiSource<PublicReqwestTransport>,
    wowi: WowInterfaceSource<PublicReqwestTransport>,
    github: GithubSource<PublicReqwestTransport>,
}

impl LiveSources {
    pub(crate) fn new(key: Option<String>) -> Result<Self, String> {
        let transport = ReqwestTransport::new().map_err(|e| e.to_string())?;
        Ok(Self {
            client: CurseForgeClient::new(transport, key),
            tukui: tukui_source()?,
            wowi: wowinterface_source()?,
            github: github_source()?,
        })
    }

    /// Whether the CurseForge key is there: without it, the CurseForge check always fails, and not
    /// because of the network.
    pub(crate) fn has_curseforge_key(&self) -> bool {
        self.client.has_key()
    }

    /// Runs `run` with the sources in D33 order: CurseForge (with the installation's forced
    /// versions and the Beta option), Tukui, WoWInterface, GitHub.
    pub(crate) fn with<R>(
        &self,
        overrides: &VersionOverrides,
        allow_beta: bool,
        run: impl FnOnce(&[&dyn Source]) -> R,
    ) -> R {
        // The projects CurseForge does not let you download use the GitHub mirror (D34).
        let curseforge = CurseForgeSource {
            client: &self.client,
            overrides,
            allow_beta,
            github: Some(&self.github),
        };
        run(&[&curseforge, &self.tukui, &self.wowi, &self.github])
    }
}

/// Records in `installed-addons.json` what an update has just installed; the written folders
/// are recomputed at the next scan. The registry lock is taken only
/// to write each record.
pub(crate) fn record_writer<'a>(
    installed: &'a Mutex<InstalledAddonsStore>,
    cache: &'a FingerprintCache,
    installation: &'a Installation,
) -> impl FnMut(SourceKind, &str, SourceRecord) -> Result<(), String> + 'a {
    let addons_dir = installation.path.join("Interface").join("AddOns");
    move |source, key, record| {
        cache.forget(&addons_dir, &record.folders);
        locked_installed(installed)?.record_source(&installation.id, source, key, record)
    }
}

/// The Tukui source (D31), without a key. It must be created outside the async runtime, like the
/// CurseForge client.
fn tukui_source() -> Result<TukuiSource<PublicReqwestTransport>, String> {
    let transport = PublicReqwestTransport::new(&tukui::DOMAINS)
        .map_err(|_| "Could not prepare the connection to Tukui.".to_owned())?;
    Ok(TukuiSource::new(transport))
}

/// The WoWInterface source (D31, issue #107), without a key. It must be created outside the async
/// runtime, like the others.
fn wowinterface_source() -> Result<WowInterfaceSource<PublicReqwestTransport>, String> {
    let transport = PublicReqwestTransport::new(&wowinterface::DOMAINS)
        .map_err(|_| "Could not prepare the connection to WoWInterface.".to_owned())?;
    Ok(WowInterfaceSource::new(transport))
}

/// The GitHub Releases source (issue #108), without a token. It must be created outside the async
/// runtime.
fn github_source() -> Result<GithubSource<PublicReqwestTransport>, String> {
    let transport = PublicReqwestTransport::new(&github::DOMAINS)
        .map_err(|_| "Could not prepare the connection to GitHub.".to_owned())?;
    Ok(GithubSource::with_cache(
        transport,
        Arc::clone(&GITHUB_CACHE),
    ))
}

/// The GitHub releases read and the request limit, shared by all the commands: every Reload
/// redoes the check, and requests without a token are 60 per hour.
static GITHUB_CACHE: LazyLock<Arc<github::Cache>> = LazyLock::new(Arc::default);

/// The installation's records for the matching order (D33). An unreadable `installed-addons.json`
/// does not block the check: without records the folders are offered to all the sources, as
/// before the records; writes report it anyway.
pub(crate) fn records_for_matching(
    installed: &Mutex<InstalledAddonsStore>,
    installation_id: &str,
) -> SourceRecords {
    locked_installed(installed)
        .and_then(|store| store.source_records_for(installation_id))
        .unwrap_or_default()
}

/// Installs a new addon from the catalog: the file `file_id` if given, otherwise the latest
/// Release for the game version forced for the project or the installation's, then its required
/// dependencies. Returns the project's outcome followed by those of the
/// dependencies, each with `dependencyOf`. The whole dependency walk stays under the
/// same lock. Network and disk
/// run off the main thread, under [`InstallLock`]. The records are read there, with the lock
/// already taken, so no other installation changes them before the replacement: an unreadable
/// registry blocks the installation, because without records it is unknown which folders
/// belong to other projects.
#[tauri::command]
pub async fn install_addon(
    installation_id: String,
    mod_id: u32,
    file_id: Option<u32>,
    app: AppHandle,
    store: Store<'_>,
    settings: Settings<'_>,
    config: State<'_, Config>,
) -> Result<Vec<InstallOutcome>, String> {
    let key = config
        .curseforge_api_key
        .clone()
        .ok_or_else(|| CurseForgeError::MissingKey.to_string())?;
    let installation = find_installation(&store, &installation_id)?;
    let overrides = overrides_for(&settings, &installation.id)?;
    let allow_beta = beta_when_no_release(&app.state::<Mutex<AppSettingsStore>>());
    tauri::async_runtime::spawn_blocking(move || {
        let lock = app.state::<InstallLock>();
        let _held = lock.hold();
        // reqwest's blocking client must be created and dropped outside the async runtime.
        let transport = ReqwestTransport::new().map_err(|e| e.to_string())?;
        let client = CurseForgeClient::new(transport, Some(key));
        // The projects CurseForge does not let you download use the GitHub mirror (D34).
        let github = github_source()?;
        let installed = app.state::<Mutex<InstalledAddonsStore>>();
        let records = locked_installed(&installed)?.records_for(&installation.id)?;
        Ok(install_new::install_addon(
            &installation,
            &client,
            Some(&github),
            &overrides,
            &records,
            mod_id,
            file_id,
            allow_beta,
            &mut |mod_id, record| {
                // The folders just written are recomputed at the next scan.
                app.state::<FingerprintCache>().forget(
                    &installation.path.join("Interface").join("AddOns"),
                    &record.folders,
                );
                locked_installed(&installed)?.record(&installation.id, mod_id, record)
            },
            &emit_progress(&app, &installation.id),
        ))
    })
    .await
    .map_err(|error| format!("The installation was interrupted ({error})."))?
}

/// The GitHub mirror of the CurseForge project `mod_id` for the installation (D34, issue #164):
/// `{ available, repo, tag, reason }`, for the file `file_id` if given (like `install_addon`),
/// otherwise for the latest one of the game version. It is called on request: from the detail,
/// from Install and from Search, which calls it on its own for blocked projects with a
/// `githubRepo` in the [`CatalogAddon`](crate::catalog::CatalogAddon) (issue #222, #223); a
/// project without `githubRepo` has no possible mirror and there is no need to ask. A project
/// that CurseForge lets you download answers `available: false` with `reason: null` without
/// requests to GitHub. Network off the main thread.
#[tauri::command]
pub async fn github_mirror(
    installation_id: String,
    mod_id: u32,
    file_id: Option<u32>,
    app: AppHandle,
    store: Store<'_>,
    settings: Settings<'_>,
    config: State<'_, Config>,
) -> Result<MirrorAvailability, String> {
    let key = config
        .curseforge_api_key
        .clone()
        .ok_or_else(|| CurseForgeError::MissingKey.to_string())?;
    let installation = find_installation(&store, &installation_id)?;
    let overrides = overrides_for(&settings, &installation.id)?;
    let allow_beta = beta_when_no_release(&app.state::<Mutex<AppSettingsStore>>());
    tauri::async_runtime::spawn_blocking(move || {
        // reqwest's blocking client must be created and dropped outside the async runtime.
        let transport = ReqwestTransport::new().map_err(|e| e.to_string())?;
        let client = CurseForgeClient::new(transport, Some(key));
        let github = github_source()?;
        install_new::github_mirror(
            &installation,
            &client,
            &github,
            &overrides,
            mod_id,
            file_id,
            allow_beta,
        )
    })
    .await
    .map_err(|error| format!("The GitHub check was interrupted ({error})."))?
}

/// Installs into the installation the zip the user downloaded by hand and dragged into the
/// window or chose with the picker (D32). The frontend passes only the path: the file is read
/// here. Removes the records of the projects whose folders it replaced. Holds `InstallLock` like
/// the other commands that write to `AddOns`, both for the replacement and for the records; the
/// disk is touched off the main thread.
#[tauri::command]
pub async fn install_zip(
    installation_id: String,
    path: String,
    app: AppHandle,
    store: Store<'_>,
) -> Result<LocalInstall, String> {
    let installation = find_installation(&store, &installation_id)?;
    tauri::async_runtime::spawn_blocking(move || {
        let lock = app.state::<InstallLock>();
        let _held = lock.hold();
        let addons_dir = installation.path.join("Interface").join("AddOns");
        let mut result = install_local::install_zip(
            &addons_dir,
            Path::new(&path),
            &emit_progress(&app, &installation.id),
        )?;
        app.state::<FingerprintCache>()
            .forget(&addons_dir, &result.installed);
        // A record that names a folder just written would describe a file that no longer exists
        // (D32, D33): it is removed. All the zip's folders count, not only the replaced ones:
        // a record can name a folder deleted by hand and now recreated by the zip.
        // The installation is already done, so an error here becomes a warning and not a failure.
        {
            let installed = app.state::<Mutex<InstalledAddonsStore>>();
            if let Err(error) = locked_installed(&installed)
                .and_then(|mut store| store.forget_folders(&installation.id, &result.installed))
            {
                result.warning = Some(format!(
                    "Installed, but the app could not update its records: {error}"
                ));
            }
        }
        Ok(result)
    })
    .await
    .map_err(|error| format!("The installation was interrupted ({error})."))?
}

/// Recognises a GitHub, WoWInterface or Tukui link and returns its preview (issue #109):
/// source, id, name, version, whether it can be downloaded for the installation, project page
/// and whether the app has already installed it. An unrecognised or malformed link is an error
/// before any network request; the network runs off the main thread.
#[tauri::command]
pub async fn resolve_link(
    installation_id: String,
    url: String,
    store: Store<'_>,
    installed: Installed<'_>,
) -> Result<LinkPreview, String> {
    let target = install_link::parse_link(&url)?;
    let installation = find_installation(&store, &installation_id)?;
    let records = records_for_matching(&installed, &installation.id);
    tauri::async_runtime::spawn_blocking(move || {
        // reqwest's blocking sources must be created and dropped outside the async runtime.
        let (github, wowinterface, tukui) =
            (github_source()?, wowinterface_source()?, tukui_source()?);
        let sources = LinkSources {
            github: &github,
            wowinterface: &wowinterface,
            tukui: &tukui,
        };
        install_link::preview(&installation, &sources, &records, &target)
    })
    .await
    .map_err(|error| format!("Reading the link was interrupted ({error})."))?
}

/// Installs the project `id` of the source `source` (`github`, `wowinterface` or `tukui`) read by
/// [`resolve_link`]. A project already installed is skipped without network; a zip folder of
/// another project makes the installation fail without touching the disk. Holds [`InstallLock`]
/// and reads there the records of all the sources: an unreadable registry blocks the installation,
/// because without records it is unknown which folders belong to other projects. Records the
/// project in `installed-addons.json` and returns an outcome shaped like `UpdateOutcome`.
#[tauri::command]
pub async fn install_from_link(
    installation_id: String,
    source: SourceKind,
    id: String,
    app: AppHandle,
    store: Store<'_>,
) -> Result<UpdateOutcome, String> {
    let installation = find_installation(&store, &installation_id)?;
    tauri::async_runtime::spawn_blocking(move || {
        let lock = app.state::<InstallLock>();
        let _held = lock.hold();
        let installed = app.state::<Mutex<InstalledAddonsStore>>();
        let records = locked_installed(&installed)?.source_records_for(&installation.id)?;
        let (github, wowinterface, tukui) =
            (github_source()?, wowinterface_source()?, tukui_source()?);
        let sources = LinkSources {
            github: &github,
            wowinterface: &wowinterface,
            tukui: &tukui,
        };
        let cache = app.state::<FingerprintCache>();
        let addons_dir = installation.path.join("Interface").join("AddOns");
        Ok(install_link::install(
            &installation,
            &sources,
            &records,
            source,
            &id,
            &mut |source, key, record| {
                cache.forget(&addons_dir, &record.folders);
                locked_installed(&installed)?.record_source(&installation.id, source, key, record)
            },
            &emit_progress(&app, &installation.id),
        ))
    })
    .await
    .map_err(|error| format!("The installation was interrupted ({error})."))?
}

/// Removes the folders `folders` of `Interface/AddOns` from the installation (issue #147), with
/// one outcome per folder: an invalid name or a folder that cannot be removed does not stop the
/// others. The removed folders leave the records and the fingerprint cache. No backup (the
/// user's choice) and nothing outside `AddOns`. Holds `InstallLock` like the other commands that
/// write to `AddOns`; the disk is touched off the main thread.
#[tauri::command]
pub async fn remove_addons(
    installation_id: String,
    folders: Vec<String>,
    app: AppHandle,
    store: Store<'_>,
) -> Result<addon_remove::RemoveResult, String> {
    let installation = find_installation(&store, &installation_id)?;
    tauri::async_runtime::spawn_blocking(move || {
        let lock = app.state::<InstallLock>();
        let _held = lock.hold();
        let addons_dir = installation.path.join("Interface").join("AddOns");
        addon_remove::remove_addons(
            &addons_dir,
            &installation.id,
            &folders,
            &app.state::<Mutex<InstalledAddonsStore>>(),
            &app.state::<FingerprintCache>(),
        )
    })
    .await
    .map_err(|error| format!("The removal was interrupted ({error})."))
}

/// A page of the CurseForge catalog search for the installation (50 results from
/// `index`). Without `version_type` the installation's game version applies. The
/// `installed` flag is computed from the records and the `.toc` files, without network; network
/// and disk run off the main thread.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn search_catalog(
    installation_id: String,
    query: String,
    category_id: Option<u32>,
    version_type: Option<u32>,
    sort: CatalogSort,
    sort_order: SortOrder,
    index: u32,
    store: Store<'_>,
    installed: Installed<'_>,
    app_settings: AppSettingsState<'_>,
    config: State<'_, Config>,
) -> Result<CatalogPage, String> {
    let key = config
        .curseforge_api_key
        .clone()
        .ok_or_else(|| CurseForgeError::MissingKey.to_string())?;
    let installation = find_installation(&store, &installation_id)?;
    // As in `list_addons`: an unreadable registry does not block the search, only the `.toc`
    // counts.
    let records = locked_installed(&installed)?
        .records_for(&installation.id)
        .unwrap_or_default();
    let request = CatalogQuery {
        query,
        category_id,
        version_type,
        sort,
        sort_order,
        index,
        allow_beta: beta_when_no_release(&app_settings),
    };
    tauri::async_runtime::spawn_blocking(move || {
        // reqwest's blocking client must be created and dropped outside the async runtime.
        let transport = ReqwestTransport::new().map_err(|e| e.to_string())?;
        let client = CurseForgeClient::new(transport, Some(key));
        catalog::search_catalog(&installation, &client, &records, &request)
    })
    .await
    .map_err(|error| format!("The search was interrupted ({error})."))?
}

/// The categories of WoW addons on CurseForge, by name, without the classes.
#[tauri::command]
pub async fn list_categories(config: State<'_, Config>) -> Result<Vec<CatalogCategory>, String> {
    let key = config
        .curseforge_api_key
        .clone()
        .ok_or_else(|| CurseForgeError::MissingKey.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        let transport = ReqwestTransport::new().map_err(|e| e.to_string())?;
        let client = CurseForgeClient::new(transport, Some(key));
        catalog::list_categories(&client)
    })
    .await
    .map_err(|error| format!("Reading the categories was interrupted ({error})."))?
}

/// The detail of a catalog addon: description in plain text, recent Release and Beta files
/// and required dependencies. Without `version_type` the project's forced version applies,
/// otherwise the installation's. Network and disk run off the main
/// thread.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn addon_detail(
    installation_id: String,
    mod_id: u32,
    version_type: Option<u32>,
    store: Store<'_>,
    settings: Settings<'_>,
    installed: Installed<'_>,
    app_settings: AppSettingsState<'_>,
    config: State<'_, Config>,
) -> Result<AddonDetail, String> {
    let key = config
        .curseforge_api_key
        .clone()
        .ok_or_else(|| CurseForgeError::MissingKey.to_string())?;
    let installation = find_installation(&store, &installation_id)?;
    let overrides = overrides_for(&settings, &installation.id)?;
    let allow_beta = beta_when_no_release(&app_settings);
    // As in the search: an unreadable registry does not block the detail, only the `.toc` counts.
    let records = locked_installed(&installed)?
        .records_for(&installation.id)
        .unwrap_or_default();
    tauri::async_runtime::spawn_blocking(move || {
        let transport = ReqwestTransport::new().map_err(|e| e.to_string())?;
        let client = CurseForgeClient::new(transport, Some(key));
        catalog::addon_detail(
            &installation,
            &client,
            &records,
            &overrides,
            mod_id,
            version_type,
            allow_beta,
        )
    })
    .await
    .map_err(|error| format!("Reading the addon details was interrupted ({error})."))?
}

fn find_installation(store: &Store<'_>, installation_id: &str) -> Result<Installation, String> {
    locked(store)?
        .list()?
        .into_iter()
        .find(|installation| installation.id == installation_id)
        .ok_or_else(|| "Installation not found.".to_owned())
}

/// `app_data_dir()/backups/<installation-id>/`: Tauri chooses the folder.
fn backups_dir(app: &AppHandle, installation_id: &str) -> Result<PathBuf, String> {
    let data = app
        .path()
        .app_data_dir()
        .map_err(|_| "The app data folder is not available.".to_owned())?;
    Ok(data.join("backups").join(installation_id))
}

/// Zip of the installation's `WTF` and `Interface`. It can read hundreds of MB, so it runs
/// off the main thread.
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

/// The backups of an installation, most recent first.
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

/// The import test bench (issue #180): never in CI.
#[cfg(test)]
#[path = "import_bench.rs"]
mod import_bench;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::APP_SETTINGS_FILE;
    use crate::test_support::TempDir;

    #[test]
    fn set_theme_saves_a_known_theme_and_rejects_an_unknown_one() {
        let tmp = TempDir::new();
        let file = tmp.path().join(APP_SETTINGS_FILE);
        let settings = Mutex::new(AppSettingsStore::load(file.clone()));

        let error = save_theme(&settings, "sepia").unwrap_err();
        assert_eq!(error, "Unknown theme: use system, light or dark.");
        assert!(!file.exists());

        assert_eq!(save_theme(&settings, "light").unwrap().theme, Theme::Light);
        assert_eq!(
            AppSettingsStore::load(file).get().unwrap().theme,
            Theme::Light
        );
    }

    /// The frontend excludes an addon with the `source` and `key` fields of the `project` of an
    /// `AddonUpdate`: those two fields must be enough to build a `ProjectKey`.
    #[test]
    fn a_project_of_an_update_row_reads_as_a_project_key() {
        let project = serde_json::to_value(updates::Project {
            id: 0,
            source: SourceKind::Github,
            key: "owner/repo".to_owned(),
            name: "Addon".to_owned(),
            url: None,
            allow_mod_distribution: true,
        })
        .unwrap();
        let key: ProjectKey = serde_json::from_value(serde_json::json!({
            "source": project["source"],
            "key": project["key"],
        }))
        .unwrap();
        assert_eq!(
            key,
            ProjectKey {
                source: SourceKind::Github,
                key: "owner/repo".to_owned(),
            }
        );
        assert_eq!(
            serde_json::to_value(&key).unwrap(),
            serde_json::json!({"source": "github", "key": "owner/repo"})
        );
    }

    #[test]
    fn set_auto_update_rejects_unknown_values_before_touching_the_file() {
        let tmp = TempDir::new();
        let file = tmp.path().join(APP_SETTINGS_FILE);
        let settings = Mutex::new(AppSettingsStore::load(file.clone()));

        assert_eq!(
            save_auto_update(&settings, "always", 6).unwrap_err(),
            "Unknown automatic update mode: use off, notify or install."
        );
        for hours in [0, 2, 48] {
            assert_eq!(
                save_auto_update(&settings, "install", hours).unwrap_err(),
                "Unsupported automatic update interval: use 1, 3, 6, 12 or 24 hours."
            );
        }
        assert!(!file.exists());

        let saved = save_auto_update(&settings, "install", 1).unwrap();
        assert_eq!(saved.auto_update, AutoUpdateMode::Install);
        assert_eq!(saved.auto_update_interval_hours, 1);
        let reloaded = AppSettingsStore::load(file).get().unwrap();
        assert_eq!(reloaded.auto_update, AutoUpdateMode::Install);
        assert_eq!(reloaded.auto_update_interval_hours, 1);
    }

    /// Issue #179: a blocked project with a GitHub repository is imported from the mirror.
    #[test]
    fn a_blocked_project_with_a_github_repository_can_be_imported() {
        let project = |allow: Option<bool>, source_url: Option<&str>| -> curseforge::Mod {
            serde_json::from_value(serde_json::json!({
                "id": 13501, "name": "Bartender4", "slug": "bartender4",
                "allowModDistribution": allow,
                "links": {"websiteUrl": "https://www.curseforge.com/wow/addons/bartender4",
                          "sourceUrl": source_url},
            }))
            .unwrap()
        };
        let github = Some("https://github.com/Nevcairiel/Bartender4");
        for allowed in [
            project(Some(true), None),
            project(Some(false), github),
            project(None, github),
        ] {
            assert_eq!(
                curseforge_availability(&allowed),
                addon_import::Availability::Available
            );
        }
        for blocked in [
            project(Some(false), None),
            project(Some(false), Some("https://gitlab.com/someone/addon")),
        ] {
            assert_eq!(
                curseforge_availability(&blocked),
                addon_import::Availability::NotDownloadable {
                    message: "The author does not allow downloads from third-party apps: install it from the CurseForge website.".to_owned(),
                    url: Some("https://www.curseforge.com/wow/addons/bartender4".to_owned()),
                }
            );
        }
    }

    #[test]
    fn settings_serialize_as_the_frontend_expects() {
        let json = serde_json::to_value(AppSettings {
            theme: Theme::Dark,
            beta_when_no_release: true,
            close_to_tray: true,
            auto_update: AutoUpdateMode::Install,
            auto_update_interval_hours: 12,
        })
        .unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "theme": "dark",
                "betaWhenNoRelease": true,
                "closeToTray": true,
                "autoUpdate": "install",
                "autoUpdateIntervalHours": 12
            })
        );
    }

    #[test]
    fn the_settings_view_adds_whether_the_tray_icon_exists() {
        let tray = TrayStatus::default();
        let view = settings_view(Ok(AppSettings::default()), &tray).unwrap();
        assert_eq!(
            serde_json::to_value(view).unwrap(),
            serde_json::json!({
                "theme": "system",
                "betaWhenNoRelease": false,
                "closeToTray": false,
                "autoUpdate": "notify",
                "autoUpdateIntervalHours": 6,
                "trayAvailable": false
            })
        );
        let error = settings_view(Err("damaged".to_owned()), &tray).unwrap_err();
        assert_eq!(error, "damaged");
    }
}
