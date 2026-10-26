//! Comandi Tauri invocabili dal frontend.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, PoisonError};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::addon_import;
use crate::addon_list;
use crate::addon_remove;
use crate::addons::cache::FingerprintCache;
use crate::addons::scan::{self, ListedAddon};
use crate::backup::{self, BackupInfo};
use crate::catalog::{self, AddonDetail, CatalogCategory, CatalogPage, CatalogQuery, CatalogSort};
use crate::config::Config;
use crate::discovery::{self, DiscoveredInstallation};
use crate::install_link::{self, LinkPreview, LinkSources};
use crate::install_local::{self, LocalInstall};
use crate::install_new::{self, InstallOutcome};
use crate::installations::Installation;
use crate::progress::{self, Progress, ProgressEvent};
use crate::sources::curseforge::{CurseForgeClient, CurseForgeError, ReqwestTransport, SortOrder};
use crate::sources::github::{self, GithubSource};
use crate::sources::http::PublicReqwestTransport;
use crate::sources::tukui::{self, TukuiSource};
use crate::sources::wowinterface::{self, WowInterfaceSource};
use crate::sources::{ProjectKey, SourceKind};
use crate::store::{
    AddonSettingsStore, AppSettings, AppSettingsStore, InstallationStore, InstalledAddonsStore,
    InstalledRecords, SourceRecord, SourceRecords, Theme,
};
use crate::updates::{
    self, AddonUpdate, CurseForgeSource, UpdateOutcome, VersionOverrides, VersionType,
};

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

type AppSettingsState<'a> = State<'a, Mutex<AppSettingsStore>>;

fn locked_app_settings(
    settings: &Mutex<AppSettingsStore>,
) -> Result<MutexGuard<'_, AppSettingsStore>, String> {
    settings
        .lock()
        .map_err(|_| "Settings are not available: restart the app.".to_owned())
}

/// L'opzione "Beta quando non c'è una Release" (issue #141). Un `settings.json` illeggibile non
/// blocca ricerca, installazione e aggiornamenti: vale il default, cioè solo Release.
fn beta_when_no_release(settings: &Mutex<AppSettingsStore>) -> bool {
    locked_app_settings(settings)
        .and_then(|store| store.get())
        .is_ok_and(|settings| settings.beta_when_no_release)
}

/// Salva l'opzione "Beta quando non c'è una Release" e restituisce le impostazioni aggiornate.
#[tauri::command]
pub fn set_beta_when_no_release(
    enabled: bool,
    settings: AppSettingsState<'_>,
) -> Result<AppSettings, String> {
    locked_app_settings(&settings)?.set_beta_when_no_release(enabled)
}

/// Le impostazioni dell'app (tema e opzione della Beta); senza `settings.json` valgono i default.
#[tauri::command]
pub fn get_settings(settings: AppSettingsState<'_>) -> Result<AppSettings, String> {
    locked_app_settings(&settings)?.get()
}

/// Salva il tema (`system`, `light` o `dark`) e restituisce le impostazioni aggiornate.
#[tauri::command]
pub fn set_theme(theme: String, settings: AppSettingsState<'_>) -> Result<AppSettings, String> {
    save_theme(&settings, &theme)
}

/// Il corpo di [`set_theme`], separato dallo `State` di Tauri per i test. Il nome si controlla
/// prima del lock: un valore sconosciuto non tocca né la memoria né il file.
fn save_theme(settings: &Mutex<AppSettingsStore>, theme: &str) -> Result<AppSettings, String> {
    let theme = Theme::parse(theme)?;
    locked_app_settings(settings)?.set_theme(theme)
}

/// Serializza installazioni e aggiornamenti: `install_addon` e `update_addons` lo tengono per
/// tutto il lavoro bloccante, così il controllo delle cartelle di altri progetti e la
/// sostituzione sul disco non si intrecciano con un'altra installazione. Il lock non protegge
/// dati: un panic che lo avvelena non lascia niente di incoerente, quindi si riprende.
#[derive(Default)]
pub struct InstallLock(Mutex<()>);

impl InstallLock {
    fn hold(&self) -> MutexGuard<'_, ()> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
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

/// Cerca le installazioni di WoW nei percorsi standard della piattaforma e restituisce quelle
/// non ancora aggiunte. Suggerisce soltanto: non scrive `installations.json`. Non trovare
/// niente non è un errore. La lettura del disco (e del registro su Windows) gira fuori dal
/// thread principale.
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
/// finestra con molti addon; il registro si legge prima, qui. Un registro illeggibile non
/// nasconde gli addon: la lettura procede senza record, mentre le scritture restano bloccate e
/// il prossimo aggiornamento riporta l'errore nel suo esito.
///
/// I fingerprint vengono da [`FingerprintCache`]; con `force` (il pulsante "Reload addons") si
/// ricalcolano tutti, così una modifica fatta a mano fuori dall'app si vede sempre.
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
    // L'errore di Tauri dice se il compito è andato in panic o è stato annullato.
    .map_err(|error| format!("Reading the addons was interrupted ({error})."))?
}

/// La lista degli addon e i loro stati di aggiornamento da una sola scansione (issue #126):
/// "Reload addons" e le riletture dopo un aggiornamento o un'installazione la usano al posto di
/// `list_addons` seguito da `check_updates`. Un controllo fallito non toglie la lista, che arriva
/// con `checkError`. `force` ricalcola i fingerprint come in `list_addons`.
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
    // Come in `list_addons`: un registro illeggibile non nasconde gli addon.
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

/// Il corpo di [`refresh_addons`], fuori dal runtime asincrono: lo usa anche l'export.
fn refresh_blocking(
    installation: &Installation,
    cache: &FingerprintCache,
    key: Option<String>,
    overrides: &VersionOverrides,
    allow_beta: bool,
    records: &SourceRecords,
    listed_records: &InstalledRecords,
) -> Result<updates::Refresh, String> {
    // Il client bloccante di reqwest va creato e distrutto fuori dal runtime asincrono. Se le
    // connessioni non si preparano, la lista arriva lo stesso, con il motivo.
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

/// La stringa con gli addon dell'installazione (issue #133): i progetti riconosciuti dal
/// controllo e quelli registrati dall'app, con la versione del gioco forzata. Le cartelle che
/// nessun progetto copre (per esempio addon installati da zip) restano fuori, per nome. Senza
/// chiave CurseForge esce almeno quello che le fonti libere e i record conoscono.
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
        // L'opzione della Beta cambia solo gli stati, non quali progetti si riconoscono.
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

/// L'anteprima dell'import di una stringa (issue #134): per ogni progetto se è nuovo, già
/// installato o non scaricabile, le voci non valide e le cartelle che *Replace all* toglierebbe.
/// Non scrive niente.
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

/// Importa una stringa di addon (issue #134): installa i progetti che mancano e, con `replace`,
/// toglie da `Interface/AddOns` le cartelle che nessun progetto importato reclama, ma solo quelle
/// in `confirmedRemovals` (l'elenco che l'utente ha visto nell'anteprima). Niente backup, per
/// scelta dell'utente. Tiene [`InstallLock`] per tutto il lavoro.
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
    // Le versioni forzate si rileggono dopo averle scritte: qui serve solo che si leggano.
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
        // Lo stato prima e dopo le installazioni, con le versioni forzate di quel momento.
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
        let mut installer = CommandInstaller {
            app: &app,
            installation: &installation,
            client: &client,
            tukui: &tukui,
            github: &github,
            availability: &availability,
            allow_beta,
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
            // Le cartelle sono già tolte: un registro che non si aggiorna non annulla niente.
            let _ = locked_installed(&installed)
                .and_then(|mut store| store.forget_folders(&installation.id, &result.removed));
        }
        Ok(result)
    })
    .await
    .map_err(|error| format!("The import was interrupted ({error})."))?
}

type Availabilities = HashMap<(SourceKind, String), addon_import::Availability>;

/// I progetti CurseForge chiesti in una sola `get_mods` dall'anteprima di un import.
const MODS_PER_REQUEST: usize = 100;

/// Se i progetti della stringa si possono scaricare: per CurseForge da `allowModDistribution`
/// (una sola richiesta per tutti), per Tukui dal suo elenco. Se una fonte non risponde, i suoi
/// progetti valgono come scaricabili (l'installazione dirà il motivo) e il secondo valore
/// riporta l'errore.
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
            // A pezzi: una stringa può avere fino a 1.000 progetti, e un pezzo che fallisce non
            // toglie le risposte degli altri.
            for chunk in mod_ids.chunks(MODS_PER_REQUEST) {
                match client.get_mods(chunk) {
                    Ok(mods) => {
                        let by_id: HashMap<u32, _> = mods.into_iter().map(|m| (m.id, m)).collect();
                        for id in chunk {
                            let state = match by_id.get(id) {
                                Some(found) if found.allow_mod_distribution == Some(true) => {
                                    Availability::Available
                                }
                                Some(found) => not_downloadable(
                                    "The author does not allow downloads from third-party apps: install it from the CurseForge website.",
                                    found.links.website_url.clone(),
                                ),
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
        // Un progetto GitHub vale come installabile senza leggere la release, per non spendere
        // il limite di richieste; un id che non è `owner/repo` no.
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

fn availability_of(
    availability: &Availabilities,
    entry: &addon_list::ListEntry,
) -> addon_import::Availability {
    availability
        .get(&(entry.source, entry.key.clone()))
        .cloned()
        .unwrap_or(addon_import::Availability::Available)
}

/// Installa i progetti di un import con le fonti vere.
struct CommandInstaller<'a> {
    app: &'a AppHandle,
    installation: &'a Installation,
    client: &'a CurseForgeClient<ReqwestTransport>,
    tukui: &'a TukuiSource<PublicReqwestTransport>,
    /// Senza un controllo nell'anteprima: ogni release costa una delle 60 richieste l'ora.
    github: &'a GithubSource<PublicReqwestTransport>,
    availability: &'a Availabilities,
    /// L'impostazione "Use beta files when there is no release" (issue #141).
    allow_beta: bool,
}

impl addon_import::Installer for CommandInstaller<'_> {
    fn install(&mut self, entry: &addon_list::ListEntry) -> addon_import::EntryOutcome {
        use addon_import::{EntryOutcome, EntryStatus};
        if let addon_import::Availability::NotDownloadable { message, .. } =
            availability_of(self.availability, entry)
        {
            return EntryOutcome::new(entry, EntryStatus::Skipped, Some(message));
        }
        let installation = self.installation;
        let addons_dir = installation.path.join("Interface").join("AddOns");
        let installed = self.app.state::<Mutex<InstalledAddonsStore>>();
        let cache = self.app.state::<FingerprintCache>();
        let progress = emit_progress(self.app, &installation.id);
        match entry.source {
            SourceKind::Curseforge => {
                let Ok(mod_id) = entry.key.parse::<u32>() else {
                    return EntryOutcome::new(
                        entry,
                        EntryStatus::Error,
                        Some("Invalid CurseForge project id.".to_owned()),
                    );
                };
                let settings = self.app.state::<Mutex<AddonSettingsStore>>();
                let mut note = None;
                let overrides = match settings.lock() {
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
                let records = match locked_installed(&installed)
                    .and_then(|store| store.records_for(&installation.id))
                {
                    Ok(records) => records,
                    Err(error) => return EntryOutcome::new(entry, EntryStatus::Error, Some(error)),
                };
                let outcomes = install_new::install_addon(
                    installation,
                    self.client,
                    &overrides,
                    &records,
                    mod_id,
                    None,
                    self.allow_beta,
                    &mut |mod_id, record| {
                        cache.forget(&addons_dir, &record.folders);
                        locked_installed(&installed)?.record(&installation.id, mod_id, record)
                    },
                    &progress,
                );
                import_outcome_from_install(entry, &outcomes, note)
            }
            SourceKind::Tukui | SourceKind::Github => {
                let mut record = |source, key: &str, record: SourceRecord| {
                    cache.forget(&addons_dir, &record.folders);
                    locked_installed(&installed)?.record_source(
                        &installation.id,
                        source,
                        key,
                        record,
                    )
                };
                let outcome = if entry.source == SourceKind::Tukui {
                    self.tukui.install_new(
                        installation,
                        &entry.key,
                        &|_| Ok(()),
                        &mut record,
                        &progress,
                    )
                } else {
                    self.github.install_new(
                        installation,
                        &entry.key,
                        &|_| Ok(()),
                        &mut record,
                        &progress,
                    )
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

/// L'esito di un progetto CurseForge importato dagli esiti di `install_addon`: il primo è il
/// progetto, gli altri le sue dipendenze.
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

/// Riconosce gli addon con le fonti (D33) e indica quali hanno un aggiornamento. Scansione e rete
/// girano fuori dal thread principale.
#[tauri::command]
pub async fn check_updates(
    installation_id: String,
    app: AppHandle,
    store: Store<'_>,
    settings: Settings<'_>,
    installed: Installed<'_>,
    config: State<'_, Config>,
) -> Result<Vec<AddonUpdate>, String> {
    // Senza chiave CurseForge non controlla niente, ma le fonti libere sì (D31): le cartelle
    // che nessuna riconosce riportano il motivo.
    let key = config.curseforge_api_key.clone();
    let installation = find_installation(&store, &installation_id)?;
    let overrides = overrides_for(&settings, &installation.id)?;
    let allow_beta = beta_when_no_release(&app.state::<Mutex<AppSettingsStore>>());
    let records = records_for_matching(&installed, &installation.id);
    tauri::async_runtime::spawn_blocking(move || {
        // Il client bloccante di reqwest va creato e distrutto fuori dal runtime asincrono.
        let transport = ReqwestTransport::new().map_err(|e| e.to_string())?;
        let client = CurseForgeClient::new(transport, key);
        let tukui = tukui_source()?;
        let wowi = wowinterface_source()?;
        let github = github_source()?;
        // I progetti che CurseForge non lascia scaricare usano il mirror GitHub (D34).
        let curseforge = CurseForgeSource {
            client: &client,
            overrides: &overrides,
            allow_beta,
            github: Some(&github),
        };
        let cache = app.state::<FingerprintCache>();
        updates::check_updates_with(
            &installation,
            Some(&cache),
            &[&curseforge, &tukui, &wowi, &github],
            &records,
        )
    })
    .await
    .map_err(|error| format!("The update check was interrupted ({error})."))?
}

/// Aggiorna i progetti dell'installazione: `mod_ids` sono progetti CurseForge, `projects` quelli
/// di qualsiasi fonte (`{source, key}`). Con `reinstall` (default `false`, issue #148) riscarica
/// anche i progetti già aggiornati, con lo stesso ciclo e le stesse regole. Rifà il controllo, scarica e installa, e registra in
/// `installed-addons.json` quello che ha installato. Rete e disco girano fuori dal thread
/// principale, sotto [`InstallLock`]; il lock del registro si prende solo per scrivere ogni
/// record.
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
    // Senza chiave CurseForge non controlla niente, ma le fonti libere sì (D31): le cartelle
    // che nessuna riconosce riportano il motivo.
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
        // Il client bloccante di reqwest va creato e distrutto fuori dal runtime asincrono.
        let transport = ReqwestTransport::new().map_err(|e| e.to_string())?;
        let client = CurseForgeClient::new(transport, key);
        let tukui = tukui_source()?;
        let wowi = wowinterface_source()?;
        let github = github_source()?;
        // I progetti che CurseForge non lascia scaricare usano il mirror GitHub (D34).
        let curseforge = CurseForgeSource {
            client: &client,
            overrides: &overrides,
            allow_beta,
            github: Some(&github),
        };
        let installed = app.state::<Mutex<InstalledAddonsStore>>();
        let cache = app.state::<FingerprintCache>();
        let addons_dir = installation.path.join("Interface").join("AddOns");
        updates::update_addons_with(
            &installation,
            Some(&cache),
            &[&curseforge, &tukui, &wowi, &github],
            &records,
            &projects,
            reinstall,
            &mut |source, key, record| {
                // Le cartelle appena scritte si ricalcolano alla prossima scansione.
                cache.forget(&addons_dir, &record.folders);
                locked_installed(&installed)?.record_source(&installation.id, source, key, record)
            },
            &emit_progress(&app, &installation.id),
        )
    })
    .await
    .map_err(|error| format!("The update was interrupted ({error})."))?
}

/// Consegna gli avanzamenti di un'installazione alla finestra come evento
/// [`progress::EVENT`] (issue #127). Un evento che non parte non ferma l'installazione.
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

/// La fonte Tukui (D31), senza chiave. Va creata fuori dal runtime asincrono, come il client
/// di CurseForge.
fn tukui_source() -> Result<TukuiSource<PublicReqwestTransport>, String> {
    let transport = PublicReqwestTransport::new(&tukui::DOMAINS)
        .map_err(|_| "Could not prepare the connection to Tukui.".to_owned())?;
    Ok(TukuiSource::new(transport))
}

/// La fonte WoWInterface (D31, issue #107), senza chiave. Va creata fuori dal runtime
/// asincrono, come le altre.
fn wowinterface_source() -> Result<WowInterfaceSource<PublicReqwestTransport>, String> {
    let transport = PublicReqwestTransport::new(&wowinterface::DOMAINS)
        .map_err(|_| "Could not prepare the connection to WoWInterface.".to_owned())?;
    Ok(WowInterfaceSource::new(transport))
}

/// La fonte GitHub Releases (issue #108), senza token. Va creata fuori dal runtime asincrono.
fn github_source() -> Result<GithubSource<PublicReqwestTransport>, String> {
    let transport = PublicReqwestTransport::new(&github::DOMAINS)
        .map_err(|_| "Could not prepare the connection to GitHub.".to_owned())?;
    Ok(GithubSource::with_cache(
        transport,
        Arc::clone(&GITHUB_CACHE),
    ))
}

/// Le release GitHub lette e il limite di richieste, condivisi da tutti i comandi: ogni Reload
/// rifà il controllo, e le richieste senza token sono 60 l'ora.
static GITHUB_CACHE: LazyLock<Arc<github::Cache>> = LazyLock::new(Arc::default);

/// I record dell'installazione per l'ordine di abbinamento (D33). Un `installed-addons.json`
/// illeggibile non blocca il controllo: senza record le cartelle si offrono a tutte le fonti,
/// come prima dei record; le scritture lo segnalano comunque.
fn records_for_matching(installed: &Installed<'_>, installation_id: &str) -> SourceRecords {
    locked_installed(installed)
        .and_then(|store| store.source_records_for(installation_id))
        .unwrap_or_default()
}

/// Installa un addon nuovo dal catalogo: il file `file_id` se indicato, altrimenti l'ultimo
/// Release per la versione del gioco forzata per il progetto o dell'installazione, poi le sue
/// dipendenze obbligatorie. Restituisce l'esito del progetto seguito da quelli delle
/// dipendenze, ognuno con `dependencyOf`. Tutta la visita delle dipendenze resta sotto lo
/// stesso lock. Rete e disco
/// girano fuori dal thread principale, sotto [`InstallLock`]. I record si leggono lì, con il lock
/// già preso, così nessun'altra installazione li cambia prima della sostituzione: un registro
/// illeggibile blocca l'installazione, perché senza record non si sa quali cartelle
/// appartengono ad altri progetti.
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
        // Il client bloccante di reqwest va creato e distrutto fuori dal runtime asincrono.
        let transport = ReqwestTransport::new().map_err(|e| e.to_string())?;
        let client = CurseForgeClient::new(transport, Some(key));
        let installed = app.state::<Mutex<InstalledAddonsStore>>();
        let records = locked_installed(&installed)?.records_for(&installation.id)?;
        Ok(install_new::install_addon(
            &installation,
            &client,
            &overrides,
            &records,
            mod_id,
            file_id,
            allow_beta,
            &mut |mod_id, record| {
                // Le cartelle appena scritte si ricalcolano alla prossima scansione.
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

/// Installa nell'installazione lo zip che l'utente ha scaricato a mano e trascinato nella
/// finestra o scelto con il selettore (D32). Il frontend passa solo il percorso: il file si legge
/// qui. Toglie i record dei progetti di cui ha sostituito le cartelle. Tiene `InstallLock` come
/// gli altri comandi che scrivono in `AddOns`, sia per la sostituzione sia per i record; il disco
/// si tocca fuori dal thread principale.
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
        // Un record che nomina una cartella appena scritta descriverebbe un file che non c'è più
        // (D32, D33): si toglie. Valgono tutte le cartelle dello zip, non solo quelle sostituite:
        // un record può nominare una cartella cancellata a mano e ora ricreata dallo zip.
        // L'installazione è già fatta, quindi un errore qui diventa un avviso e non un fallimento.
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

/// Riconosce un link di GitHub, WoWInterface o Tukui e ne restituisce l'anteprima (issue #109):
/// fonte, id, nome, versione, se si può scaricare per l'installazione, pagina del progetto e se
/// l'app l'ha già installato. Un link non riconosciuto o malformato è un errore prima di
/// qualunque richiesta di rete; la rete gira fuori dal thread principale.
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
        // Le fonti bloccanti di reqwest vanno create e distrutte fuori dal runtime asincrono.
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

/// Installa il progetto `id` della fonte `source` (`github`, `wowinterface` o `tukui`) letto da
/// [`resolve_link`]. Un progetto già installato si salta senza rete; una cartella dello zip di un
/// altro progetto fa fallire l'installazione senza toccare il disco. Tiene [`InstallLock`] e
/// legge lì i record di tutte le fonti: un registro illeggibile blocca l'installazione, perché
/// senza record non si sa quali cartelle appartengono ad altri progetti. Registra il progetto in
/// `installed-addons.json` e restituisce un esito con la forma di `UpdateOutcome`.
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

/// Toglie dall'installazione le cartelle `folders` di `Interface/AddOns` (issue #147), con un
/// esito per cartella: un nome non valido o una cartella che non si toglie non ferma le altre.
/// Le cartelle tolte escono dai record e dalla cache dei fingerprint. Niente backup (scelta
/// dell'utente) e niente fuori da `AddOns`. Tiene `InstallLock` come gli altri comandi che
/// scrivono in `AddOns`; il disco si tocca fuori dal thread principale.
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

/// Una pagina della ricerca nel catalogo di CurseForge per l'installazione (50 risultati da
/// `index`). Senza `version_type` vale la versione del gioco dell'installazione. Il flag
/// `installed` si calcola dai record e dai `.toc`, senza rete; rete e disco girano fuori dal
/// thread principale.
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
    // Come in `list_addons`: un registro illeggibile non blocca la ricerca, vale solo il `.toc`.
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
        // Il client bloccante di reqwest va creato e distrutto fuori dal runtime asincrono.
        let transport = ReqwestTransport::new().map_err(|e| e.to_string())?;
        let client = CurseForgeClient::new(transport, Some(key));
        catalog::search_catalog(&installation, &client, &records, &request)
    })
    .await
    .map_err(|error| format!("The search was interrupted ({error})."))?
}

/// Le categorie degli addon di WoW su CurseForge, per nome, senza le classi.
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

/// Il dettaglio di un addon del catalogo: descrizione in testo semplice, file Release e Beta
/// recenti e dipendenze obbligatorie. Senza `version_type` vale la versione forzata del
/// progetto, altrimenti quella dell'installazione. Rete e disco girano fuori dal thread
/// principale.
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
    // Come nella ricerca: un registro illeggibile non blocca il dettaglio, vale solo il `.toc`.
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

    #[test]
    fn settings_serialize_as_the_frontend_expects() {
        let json = serde_json::to_value(AppSettings {
            theme: Theme::Dark,
            beta_when_no_release: true,
        })
        .unwrap();
        assert_eq!(
            json,
            serde_json::json!({"theme": "dark", "betaWhenNoRelease": true})
        );
    }
}
