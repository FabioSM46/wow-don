//! Stato locale salvato su disco: l'elenco delle installazioni (`installations.json`), le
//! impostazioni degli addon (`addon-settings.json`: la versione del gioco forzata, D6, e le
//! esclusioni dall'aggiornamento automatico, D41),
//! i progetti installati dall'app con le loro cartelle (`installed-addons.json`, issue #62) e le
//! impostazioni dell'app (`settings.json`, per ora il tema, issue #92).
//!
//! I file stanno nella cartella di configurazione dell'app risolta da Tauri (D22). Si scrive in modo atomico: file temporaneo nella stessa cartella, poi `rename`.
//! Un file mancante vuol dire nessuna installazione; un file illeggibile blocca le
//! modifiche invece di essere sovrascritto, così non si perde quello che c'era.
//!
//! Su disco si salvano solo i percorsi: id, flavor e nome si ricalcolano a ogni lettura,
//! così seguono sempre il codice attuale. Prima di ogni modifica il file si rilegge, per non
//! cancellare quello che un'altra istanza dell'app ha scritto nel frattempo. Due modifiche
//! davvero simultanee da due istanze possono ancora perdersene una: non c'è un lock sul file,
//! e l'app non impedisce di aprire due istanze.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::installations::{self, Installation};
use crate::sources::ProjectKey;
use crate::updates::{self, VersionOverrides};

pub const INSTALLATIONS_FILE: &str = "installations.json";
const FORMAT_VERSION: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
struct StoredInstallations {
    version: u32,
    paths: Vec<PathBuf>,
}

#[derive(Debug)]
pub struct InstallationStore {
    file: PathBuf,
    installations: Vec<Installation>,
    /// Impostato quando il file esiste ma non si può leggere: ogni comando lo riporta.
    load_error: Option<String>,
}

impl InstallationStore {
    pub fn load(file: PathBuf) -> Self {
        match read(&file) {
            Ok(installations) => Self {
                file,
                installations,
                load_error: None,
            },
            Err(error) => Self {
                file,
                installations: Vec::new(),
                load_error: Some(error),
            },
        }
    }

    pub fn list(&self) -> Result<Vec<Installation>, String> {
        self.check_loaded()?;
        Ok(self.installations.clone())
    }

    pub fn add(&mut self, path: &Path) -> Result<Installation, String> {
        self.reload()?;
        let installation = installations::validate(path)?;
        if self
            .installations
            .iter()
            .any(|existing| existing.id == installation.id || existing.path == installation.path)
        {
            return Err("This installation has already been added.".to_owned());
        }
        let mut next = self.installations.clone();
        next.push(installation.clone());
        self.save(next)?;
        Ok(installation)
    }

    pub fn remove(&mut self, id: &str) -> Result<(), String> {
        self.reload()?;
        if !self.installations.iter().any(|existing| existing.id == id) {
            return Err("Installation not found.".to_owned());
        }
        let next = self
            .installations
            .iter()
            .filter(|existing| existing.id != id)
            .cloned()
            .collect();
        self.save(next)
    }

    /// Rilegge il file prima di una modifica. Se non si legge, la modifica non avviene.
    fn reload(&mut self) -> Result<(), String> {
        self.check_loaded()?;
        self.installations = read(&self.file)?;
        Ok(())
    }

    fn check_loaded(&self) -> Result<(), String> {
        match &self.load_error {
            Some(error) => Err(error.clone()),
            None => Ok(()),
        }
    }

    /// Scrive prima su disco e aggiorna la memoria solo se la scrittura è riuscita.
    fn save(&mut self, installations: Vec<Installation>) -> Result<(), String> {
        let stored = StoredInstallations {
            version: FORMAT_VERSION,
            paths: installations.iter().map(|i| i.path.clone()).collect(),
        };
        write_atomically(&self.file, &stored)
            .map_err(|error| format!("Could not save the installations ({}).", error.kind()))?;
        self.installations = installations;
        Ok(())
    }
}

// ── Impostazioni degli addon ────────────────────────────────────────────────

pub const ADDON_SETTINGS_FILE: &str = "addon-settings.json";
const ADDON_SETTINGS_VERSION: u32 = 1;

/// Le impostazioni di un progetto CurseForge in un'installazione.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AddonSetting {
    /// La versione del gioco forzata (D6).
    game_version_type_id: u32,
}

/// Id dell'installazione → id del progetto CurseForge → impostazioni.
type SettingsByInstallation = BTreeMap<String, BTreeMap<u32, AddonSetting>>;

/// L'aggiornamento automatico di un'installazione (D41, issue #199). Senza voce l'installazione
/// partecipa e nessun progetto è escluso.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallationAutoUpdate {
    /// L'installazione è esclusa per intero.
    #[serde(default)]
    pub disabled: bool,
    /// I progetti esclusi, di qualunque fonte.
    #[serde(default)]
    pub excluded: BTreeSet<ProjectKey>,
}

impl InstallationAutoUpdate {
    fn is_default(&self) -> bool {
        !self.disabled && self.excluded.is_empty()
    }
}

/// Id dell'installazione → aggiornamento automatico.
type AutoUpdateByInstallation = BTreeMap<String, InstallationAutoUpdate>;

/// Tutto `addon-settings.json` in memoria.
#[derive(Debug, Clone, Default)]
struct AddonSettings {
    overrides: SettingsByInstallation,
    auto_update: AutoUpdateByInstallation,
}

/// Il campo `autoUpdate` è un'aggiunta della issue #199: un file di prima non lo ha e vale
/// "nessuna esclusione", senza cambiare versione (come i campi aggiunti a `settings.json`).
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredAddonSettings {
    version: u32,
    installations: SettingsByInstallation,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    auto_update: AutoUpdateByInstallation,
}

/// Le impostazioni per addon, con le stesse regole di [`InstallationStore`]: rilettura prima
/// di ogni modifica, scrittura atomica, file illeggibile che blocca le modifiche. I messaggi
/// sono in inglese, come l'interfaccia (issue #45).
#[derive(Debug)]
pub struct AddonSettingsStore {
    file: PathBuf,
    settings: AddonSettings,
    load_error: Option<String>,
}

impl AddonSettingsStore {
    pub fn load(file: PathBuf) -> Self {
        match read_addon_settings(&file) {
            Ok(settings) => Self {
                file,
                settings,
                load_error: None,
            },
            Err(error) => Self {
                file,
                settings: AddonSettings::default(),
                load_error: Some(error),
            },
        }
    }

    /// Le versioni forzate di un'installazione. Un tipo che l'app non conosce più (per esempio
    /// la Classic progressiva dopo un cambio di espansione) non si usa, e il salvataggio
    /// successivo lo toglie dal file.
    pub fn overrides_for(&self, installation_id: &str) -> Result<VersionOverrides, String> {
        self.check_loaded()?;
        Ok(self
            .settings
            .overrides
            .get(installation_id)
            .into_iter()
            .flatten()
            .filter(|(_, setting)| updates::is_known_version_type(setting.game_version_type_id))
            .map(|(mod_id, setting)| (*mod_id, setting.game_version_type_id))
            .collect::<HashMap<_, _>>())
    }

    /// Forza la versione del gioco `version_type` per il progetto `mod_id`.
    pub fn set_override(
        &mut self,
        installation_id: &str,
        mod_id: u32,
        version_type: u32,
    ) -> Result<(), String> {
        if !updates::is_known_version_type(version_type) {
            return Err("Unknown game version.".to_owned());
        }
        self.reload()?;
        let mut next = self.settings.clone();
        next.overrides
            .entry(installation_id.to_owned())
            .or_default()
            .insert(
                mod_id,
                AddonSetting {
                    game_version_type_id: version_type,
                },
            );
        self.save(next)
    }

    /// Toglie la versione forzata; senza una versione forzata non fa niente.
    pub fn clear_override(&mut self, installation_id: &str, mod_id: u32) -> Result<(), String> {
        self.reload()?;
        let mut next = self.settings.clone();
        let Some(addons) = next.overrides.get_mut(installation_id) else {
            return Ok(());
        };
        if addons.remove(&mod_id).is_none() {
            return Ok(());
        }
        self.save(next)
    }

    /// L'aggiornamento automatico di un'installazione: se partecipa e quali progetti sono
    /// esclusi.
    pub fn auto_update_for(&self, installation_id: &str) -> Result<InstallationAutoUpdate, String> {
        self.check_loaded()?;
        Ok(self
            .settings
            .auto_update
            .get(installation_id)
            .cloned()
            .unwrap_or_default())
    }

    /// Esclude il progetto dall'aggiornamento automatico, o lo riammette.
    pub fn set_auto_update_excluded(
        &mut self,
        installation_id: &str,
        project: ProjectKey,
        excluded: bool,
    ) -> Result<InstallationAutoUpdate, String> {
        self.change_auto_update(installation_id, |entry| {
            if excluded {
                entry.excluded.insert(project);
            } else {
                entry.excluded.remove(&project);
            }
        })
    }

    /// Fa partecipare l'installazione all'aggiornamento automatico, o la esclude per intero.
    /// Le esclusioni dei singoli progetti restano.
    pub fn set_installation_auto_update(
        &mut self,
        installation_id: &str,
        enabled: bool,
    ) -> Result<InstallationAutoUpdate, String> {
        self.change_auto_update(installation_id, |entry| entry.disabled = !enabled)
    }

    fn change_auto_update(
        &mut self,
        installation_id: &str,
        change: impl FnOnce(&mut InstallationAutoUpdate),
    ) -> Result<InstallationAutoUpdate, String> {
        self.reload()?;
        let mut next = self.settings.clone();
        let entry = next
            .auto_update
            .entry(installation_id.to_owned())
            .or_default();
        change(entry);
        let changed = entry.clone();
        if next.auto_update == self.settings.auto_update {
            return Ok(changed);
        }
        self.save(next)?;
        Ok(changed)
    }

    /// Toglie tutte le impostazioni di un'installazione rimossa: versioni forzate ed esclusioni.
    pub fn remove_installation(&mut self, installation_id: &str) -> Result<(), String> {
        self.reload()?;
        if !self.settings.overrides.contains_key(installation_id)
            && !self.settings.auto_update.contains_key(installation_id)
        {
            return Ok(());
        }
        let mut next = self.settings.clone();
        next.overrides.remove(installation_id);
        next.auto_update.remove(installation_id);
        self.save(next)
    }

    fn reload(&mut self) -> Result<(), String> {
        self.check_loaded()?;
        self.settings = read_addon_settings(&self.file)?;
        Ok(())
    }

    fn check_loaded(&self) -> Result<(), String> {
        match &self.load_error {
            Some(error) => Err(error.clone()),
            None => Ok(()),
        }
    }

    /// Scrive le impostazioni senza i tipi che l'app non conosce più e senza le voci vuote.
    fn save(&mut self, mut settings: AddonSettings) -> Result<(), String> {
        for addons in settings.overrides.values_mut() {
            addons
                .retain(|_, setting| updates::is_known_version_type(setting.game_version_type_id));
        }
        settings.overrides.retain(|_, addons| !addons.is_empty());
        settings.auto_update.retain(|_, entry| !entry.is_default());
        let stored = StoredAddonSettings {
            version: ADDON_SETTINGS_VERSION,
            installations: settings.overrides,
            auto_update: settings.auto_update,
        };
        write_atomically(&self.file, &stored)
            .map_err(|error| format!("Could not save the addon settings ({}).", error.kind()))?;
        self.settings = AddonSettings {
            overrides: stored.installations,
            auto_update: stored.auto_update,
        };
        Ok(())
    }
}

fn read_addon_settings(file: &Path) -> Result<AddonSettings, String> {
    let text = match read_text(file) {
        Ok(Some(text)) => text,
        Ok(None) => return Ok(AddonSettings::default()),
        Err(error) => {
            return Err(format!(
                "Could not read {ADDON_SETTINGS_FILE} ({}).",
                error.kind()
            ));
        }
    };
    let stored: StoredAddonSettings = serde_json::from_str(&text).map_err(|_| {
        format!("{ADDON_SETTINGS_FILE} is damaged: fix it or move it away and restart the app.")
    })?;
    if stored.version != ADDON_SETTINGS_VERSION {
        return Err(format!(
            "{ADDON_SETTINGS_FILE} has an unsupported version ({}): update the app or move the file away, then restart.",
            stored.version
        ));
    }
    Ok(AddonSettings {
        overrides: stored.installations,
        auto_update: stored.auto_update,
    })
}

// ── Addon installati dall'app ───────────────────────────────────────────────

pub const INSTALLED_ADDONS_FILE: &str = "installed-addons.json";
/// Versione 2: record per fonte e id testuale (issue #104). La versione 1 aveva solo progetti
/// CurseForge con id numerico; si legge ancora e si riscrive come 2 alla prima modifica.
/// Una nuova variante di [`SourceKind`] richiede una versione nuova.
const INSTALLED_ADDONS_VERSION: u32 = 2;
const INSTALLED_ADDONS_VERSION_V1: u32 = 1;

/// La fonte da cui l'app ha installato un progetto. Su disco è la chiave in minuscolo
/// (`curseforge`, `github`, `wowinterface`, `tukui`).
///
/// **Aggiungere una variante cambia il formato del file**: va alzato anche
/// [`INSTALLED_ADDONS_VERSION`]. Una versione dell'app più vecchia legge allora una versione
/// sconosciuta e chiede di aggiornare l'app lasciando il file com'è; con la stessa versione
/// leggerebbe una fonte sconosciuta e direbbe che il file è danneggiato, invitando a spostarlo
/// e a perdere tutti i record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceKind {
    Curseforge,
    // Le fonti libere scriveranno i loro record con le issue #106, #107 e #108; per ora queste
    // varianti si leggono solo da file.
    Github,
    Wowinterface,
    Tukui,
}

impl SourceKind {
    /// Il nome della fonte mostrato nei messaggi (in inglese, come tutta l'interfaccia).
    pub fn label(self) -> &'static str {
        match self {
            Self::Curseforge => "CurseForge",
            Self::Github => "GitHub",
            Self::Wowinterface => "WoWInterface",
            Self::Tukui => "Tukui",
        }
    }
}

/// Quello che l'app ha installato per un progetto di una fonte. `file` identifica il file
/// installato nel modo della fonte: l'id del file per CurseForge, il tag per GitHub, la
/// versione per WoWInterface e Tukui.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceRecord {
    pub file: String,
    /// I nomi delle cartelle di primo livello dello zip, come li ha restituiti l'estrazione.
    pub folders: Vec<String>,
}

/// Id del progetto nella fonte (`61284`, `owner/repo`, `-2`) → record.
pub type ProjectRecords = BTreeMap<String, SourceRecord>;

/// Fonte → record dei suoi progetti.
pub type SourceRecords = BTreeMap<SourceKind, ProjectRecords>;

/// Id dell'installazione → record per fonte.
type RecordsByInstallation = BTreeMap<String, SourceRecords>;

/// Quello che l'app ha installato per un progetto CurseForge: il file e le cartelle del suo
/// zip. Serve a ritrovare le cartelle di un progetto senza indovinarle dai `.toc` (issue #62).
/// È la vista CurseForge dei record v2, quella che usano oggi controllo e installazione.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledRecord {
    pub file_id: u32,
    /// I nomi delle cartelle di primo livello dello zip, come li ha restituiti l'estrazione.
    pub folders: Vec<String>,
}

/// Id del progetto CurseForge → il suo record.
pub type InstalledRecords = BTreeMap<u32, InstalledRecord>;

#[derive(Debug, Serialize, Deserialize)]
struct StoredInstalledAddons {
    version: u32,
    installations: RecordsByInstallation,
}

/// Il formato della versione 1, letto solo per la migrazione.
#[derive(Debug, Deserialize)]
struct StoredInstalledAddonsV1 {
    installations: BTreeMap<String, InstalledRecords>,
}

#[derive(Debug, Deserialize)]
struct StoredVersion {
    version: u32,
}

/// I progetti installati dall'app, con le stesse regole di [`InstallationStore`]: rilettura
/// prima di ogni modifica, scrittura atomica, file illeggibile che blocca le modifiche.
#[derive(Debug)]
pub struct InstalledAddonsStore {
    file: PathBuf,
    records: RecordsByInstallation,
    load_error: Option<String>,
}

impl InstalledAddonsStore {
    pub fn load(file: PathBuf) -> Self {
        match read_installed_addons(&file) {
            Ok(records) => Self {
                file,
                records,
                load_error: None,
            },
            Err(error) => Self {
                file,
                records: RecordsByInstallation::new(),
                load_error: Some(error),
            },
        }
    }

    /// I record CurseForge di un'installazione; vuoto se l'app non vi ha ancora installato
    /// niente. I record delle altre fonti restano nel file e non compaiono qui.
    pub fn records_for(&self, installation_id: &str) -> Result<InstalledRecords, String> {
        self.check_loaded()?;
        let Some(projects) = self
            .records
            .get(installation_id)
            .and_then(|sources| sources.get(&SourceKind::Curseforge))
        else {
            return Ok(InstalledRecords::new());
        };
        projects
            .iter()
            .map(|(id, record)| curseforge_record(id, record))
            .collect()
    }

    /// Registra (o sostituisce) quello che l'app ha appena installato per il progetto
    /// CurseForge `mod_id`.
    pub fn record(
        &mut self,
        installation_id: &str,
        mod_id: u32,
        record: InstalledRecord,
    ) -> Result<(), String> {
        self.record_source(
            installation_id,
            SourceKind::Curseforge,
            &mod_id.to_string(),
            SourceRecord {
                file: record.file_id.to_string(),
                folders: record.folders,
            },
        )
    }

    /// Tutti i record di un'installazione, per fonte; vuoto se l'app non vi ha ancora installato
    /// niente.
    pub fn source_records_for(&self, installation_id: &str) -> Result<SourceRecords, String> {
        self.check_loaded()?;
        Ok(self
            .records
            .get(installation_id)
            .cloned()
            .unwrap_or_default())
    }

    /// Registra (o sostituisce) il record di un progetto di qualsiasi fonte.
    pub fn record_source(
        &mut self,
        installation_id: &str,
        source: SourceKind,
        project_id: &str,
        record: SourceRecord,
    ) -> Result<(), String> {
        self.reload()?;
        let mut next = self.records.clone();
        next.entry(installation_id.to_owned())
            .or_default()
            .entry(source)
            .or_default()
            .insert(project_id.to_owned(), record);
        self.save(next)
    }

    /// Toglie il record di un solo progetto, se c'è (l'Undo del giro automatico per un
    /// progetto che prima non aveva record, issue #201). Senza record da togliere non scrive.
    pub fn forget_project(
        &mut self,
        installation_id: &str,
        source: SourceKind,
        project_id: &str,
    ) -> Result<(), String> {
        self.reload()?;
        let mut next = self.records.clone();
        let Some(sources) = next.get_mut(installation_id) else {
            return Ok(());
        };
        let Some(projects) = sources.get_mut(&source) else {
            return Ok(());
        };
        if projects.remove(project_id).is_none() {
            return Ok(());
        }
        if projects.is_empty() {
            sources.remove(&source);
        }
        if sources.is_empty() {
            next.remove(installation_id);
        }
        self.save(next)
    }

    /// Toglie i record (di qualsiasi fonte) dell'installazione che nominano almeno una delle
    /// cartelle `folders`, senza distinguere maiuscole e minuscole: servono quando uno zip
    /// installato a mano scrive quelle cartelle (D32), così il record non descrive più un file
    /// che non c'è. Si toglie il record intero, anche se solo una delle sue cartelle è cambiata:
    /// un record dice "questo file è installato in queste cartelle", e dopo una sovrascrittura
    /// parziale non è più vero per nessuna. Le cartelle rimaste si riconoscono di nuovo come
    /// quelle installate a mano (per CurseForge dal fingerprint, che non usa i record).
    /// Restituisce quanti record ha tolto; senza record da togliere non scrive.
    pub fn forget_folders(
        &mut self,
        installation_id: &str,
        folders: &[String],
    ) -> Result<usize, String> {
        self.reload()?;
        let replaced: HashSet<String> = folders.iter().map(|f| f.to_lowercase()).collect();
        let mut next = self.records.clone();
        let mut removed = 0;
        if let Some(sources) = next.get_mut(installation_id) {
            for projects in sources.values_mut() {
                let before = projects.len();
                projects.retain(|_, record| {
                    !record
                        .folders
                        .iter()
                        .any(|folder| replaced.contains(&folder.to_lowercase()))
                });
                removed += before - projects.len();
            }
            sources.retain(|_, projects| !projects.is_empty());
            if sources.is_empty() {
                next.remove(installation_id);
            }
        }
        if removed > 0 {
            self.save(next)?;
        }
        Ok(removed)
    }

    /// Toglie tutti i record di un'installazione rimossa.
    pub fn remove_installation(&mut self, installation_id: &str) -> Result<(), String> {
        self.reload()?;
        if !self.records.contains_key(installation_id) {
            return Ok(());
        }
        let mut next = self.records.clone();
        next.remove(installation_id);
        self.save(next)
    }

    fn reload(&mut self) -> Result<(), String> {
        self.check_loaded()?;
        self.records = read_installed_addons(&self.file)?;
        Ok(())
    }

    fn check_loaded(&self) -> Result<(), String> {
        match &self.load_error {
            Some(error) => Err(error.clone()),
            None => Ok(()),
        }
    }

    fn save(&mut self, records: RecordsByInstallation) -> Result<(), String> {
        let stored = StoredInstalledAddons {
            version: INSTALLED_ADDONS_VERSION,
            installations: records,
        };
        write_atomically(&self.file, &stored).map_err(|error| {
            format!(
                "Could not save the installed addons list ({}).",
                error.kind()
            )
        })?;
        self.records = stored.installations;
        Ok(())
    }
}

/// Un record v2 di CurseForge nella forma usata da controllo e installazione. Id del progetto
/// e del file devono essere numerici: [`read_installed_addons`] lo verifica già alla lettura.
fn curseforge_record(id: &str, record: &SourceRecord) -> Result<(u32, InstalledRecord), String> {
    match (id.parse(), record.file.parse()) {
        (Ok(mod_id), Ok(file_id)) => Ok((
            mod_id,
            InstalledRecord {
                file_id,
                folders: record.folders.clone(),
            },
        )),
        _ => Err(damaged_installed_addons()),
    }
}

fn damaged_installed_addons() -> String {
    format!("{INSTALLED_ADDONS_FILE} is damaged: fix it or move it away and restart the app.")
}

fn read_installed_addons(file: &Path) -> Result<RecordsByInstallation, String> {
    let text = match read_text(file) {
        Ok(Some(text)) => text,
        Ok(None) => return Ok(RecordsByInstallation::new()),
        Err(error) => {
            return Err(format!(
                "Could not read {INSTALLED_ADDONS_FILE} ({}).",
                error.kind()
            ));
        }
    };
    let version: StoredVersion =
        serde_json::from_str(&text).map_err(|_| damaged_installed_addons())?;
    let records = match version.version {
        INSTALLED_ADDONS_VERSION => {
            let stored: StoredInstalledAddons =
                serde_json::from_str(&text).map_err(|_| damaged_installed_addons())?;
            stored.installations
        }
        INSTALLED_ADDONS_VERSION_V1 => {
            let stored: StoredInstalledAddonsV1 =
                serde_json::from_str(&text).map_err(|_| damaged_installed_addons())?;
            migrate_v1(stored)
        }
        other => {
            return Err(format!(
                "{INSTALLED_ADDONS_FILE} has an unsupported version ({other}): update the app or move the file away, then restart.",
            ));
        }
    };
    // Un record CurseForge con id non numerici renderebbe illeggibile la vista di
    // `records_for`: meglio bloccare subito, come per un file danneggiato.
    for sources in records.values() {
        if let Some(projects) = sources.get(&SourceKind::Curseforge) {
            for (id, record) in projects {
                curseforge_record(id, record)?;
            }
        }
    }
    Ok(records)
}

/// Versione 1 → 2: ogni progetto era CurseForge, con id e file numerici.
fn migrate_v1(stored: StoredInstalledAddonsV1) -> RecordsByInstallation {
    stored
        .installations
        .into_iter()
        .map(|(installation_id, projects)| {
            let curseforge = projects
                .into_iter()
                .map(|(mod_id, record)| {
                    (
                        mod_id.to_string(),
                        SourceRecord {
                            file: record.file_id.to_string(),
                            folders: record.folders,
                        },
                    )
                })
                .collect();
            (
                installation_id,
                SourceRecords::from([(SourceKind::Curseforge, curseforge)]),
            )
        })
        .collect()
}

// ── Impostazioni dell'app ───────────────────────────────────────────────────

pub const APP_SETTINGS_FILE: &str = "settings.json";
const APP_SETTINGS_VERSION: u32 = 1;

/// Il tema dell'interfaccia: `system` segue il sistema operativo.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
}

impl Theme {
    /// Il tema dal suo nome; un valore sconosciuto è un errore con un messaggio in inglese.
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "system" => Ok(Self::System),
            "light" => Ok(Self::Light),
            "dark" => Ok(Self::Dark),
            _ => Err("Unknown theme: use system, light or dark.".to_owned()),
        }
    }
}

/// Cosa fa l'aggiornamento automatico degli addon (D41, issue #199): niente, solo il
/// controllo, oppure il controllo e l'installazione.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AutoUpdateMode {
    Off,
    #[default]
    Notify,
    Install,
}

impl AutoUpdateMode {
    /// La modalità dal suo nome; un valore sconosciuto è un errore con un messaggio in inglese.
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "off" => Ok(Self::Off),
            "notify" => Ok(Self::Notify),
            "install" => Ok(Self::Install),
            _ => Err("Unknown automatic update mode: use off, notify or install.".to_owned()),
        }
    }
}

/// Gli intervalli ammessi tra due giri dell'aggiornamento automatico, in ore.
pub const AUTO_UPDATE_INTERVALS_HOURS: [u32; 5] = [1, 3, 6, 12, 24];
pub const DEFAULT_AUTO_UPDATE_INTERVAL_HOURS: u32 = 6;

/// Controlla un intervallo scelto dall'utente.
pub fn check_auto_update_interval(hours: u32) -> Result<u32, String> {
    if AUTO_UPDATE_INTERVALS_HOURS.contains(&hours) {
        Ok(hours)
    } else {
        Err("Unsupported automatic update interval: use 1, 3, 6, 12 or 24 hours.".to_owned())
    }
}

fn default_auto_update_interval() -> u32 {
    DEFAULT_AUTO_UPDATE_INTERVAL_HOURS
}

/// Le impostazioni dell'app, come le riceve il frontend (`get_settings`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    pub theme: Theme,
    /// Installare e aggiornare alla Beta quando la versione del gioco non ha una Release (issue
    /// #141). Un `settings.json` di prima non ha il campo: vale falso, senza cambiare versione.
    #[serde(default)]
    pub beta_when_no_release: bool,
    /// Chiudere la finestra principale la nasconde nella tray invece di chiudere l'app (issue
    /// #158, D35). Un `settings.json` di prima non ha il campo: vale falso, la X chiude.
    #[serde(default)]
    pub close_to_tray: bool,
    /// L'aggiornamento automatico (D41, issue #199). Un `settings.json` di prima non ha il
    /// campo: vale `notify`, che controlla senza toccare il disco.
    #[serde(default)]
    pub auto_update: AutoUpdateMode,
    /// Le ore tra due giri, uno di [`AUTO_UPDATE_INTERVALS_HOURS`]. Un file senza il campo vale
    /// 6; un valore non ammesso scritto a mano si legge come 6, senza bloccare le impostazioni.
    #[serde(default = "default_auto_update_interval")]
    pub auto_update_interval_hours: u32,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            theme: Theme::default(),
            beta_when_no_release: false,
            close_to_tray: false,
            auto_update: AutoUpdateMode::default(),
            auto_update_interval_hours: DEFAULT_AUTO_UPDATE_INTERVAL_HOURS,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredAppSettings {
    version: u32,
    #[serde(flatten)]
    settings: AppSettings,
}

/// Le impostazioni dell'app, con le stesse regole di [`InstallationStore`]: rilettura prima di
/// ogni modifica, scrittura atomica, file illeggibile che blocca le modifiche. Senza file valgono
/// i valori di default (tema `system`).
#[derive(Debug)]
pub struct AppSettingsStore {
    file: PathBuf,
    settings: AppSettings,
    load_error: Option<String>,
}

impl AppSettingsStore {
    pub fn load(file: PathBuf) -> Self {
        match read_app_settings(&file) {
            Ok(settings) => Self {
                file,
                settings,
                load_error: None,
            },
            Err(error) => Self {
                file,
                settings: AppSettings::default(),
                load_error: Some(error),
            },
        }
    }

    pub fn get(&self) -> Result<AppSettings, String> {
        self.check_loaded()?;
        Ok(self.settings)
    }

    /// Salva il tema e restituisce le impostazioni aggiornate.
    pub fn set_theme(&mut self, theme: Theme) -> Result<AppSettings, String> {
        self.reload()?;
        // Si cambia solo il tema: le altre impostazioni restano quelle appena rilette.
        let mut next = self.settings;
        next.theme = theme;
        self.save(next)?;
        Ok(next)
    }

    /// Salva l'opzione della Beta e restituisce le impostazioni aggiornate.
    pub fn set_beta_when_no_release(&mut self, enabled: bool) -> Result<AppSettings, String> {
        self.reload()?;
        let mut next = self.settings;
        next.beta_when_no_release = enabled;
        self.save(next)?;
        Ok(next)
    }

    /// Salva modalità e intervallo dell'aggiornamento automatico e restituisce le impostazioni
    /// aggiornate. Un intervallo non ammesso non tocca né la memoria né il file.
    pub fn set_auto_update(
        &mut self,
        mode: AutoUpdateMode,
        interval_hours: u32,
    ) -> Result<AppSettings, String> {
        let interval_hours = check_auto_update_interval(interval_hours)?;
        self.reload()?;
        let mut next = self.settings;
        next.auto_update = mode;
        next.auto_update_interval_hours = interval_hours;
        self.save(next)?;
        Ok(next)
    }

    /// Salva l'opzione "Close to tray" e restituisce le impostazioni aggiornate.
    pub fn set_close_to_tray(&mut self, enabled: bool) -> Result<AppSettings, String> {
        self.reload()?;
        let mut next = self.settings;
        next.close_to_tray = enabled;
        self.save(next)?;
        Ok(next)
    }

    fn reload(&mut self) -> Result<(), String> {
        self.check_loaded()?;
        self.settings = read_app_settings(&self.file)?;
        Ok(())
    }

    fn check_loaded(&self) -> Result<(), String> {
        match &self.load_error {
            Some(error) => Err(error.clone()),
            None => Ok(()),
        }
    }

    fn save(&mut self, settings: AppSettings) -> Result<(), String> {
        let stored = StoredAppSettings {
            version: APP_SETTINGS_VERSION,
            settings,
        };
        write_atomically(&self.file, &stored)
            .map_err(|error| format!("Could not save the settings ({}).", error.kind()))?;
        self.settings = settings;
        Ok(())
    }
}

fn read_app_settings(file: &Path) -> Result<AppSettings, String> {
    let text = match read_text(file) {
        Ok(Some(text)) => text,
        Ok(None) => return Ok(AppSettings::default()),
        Err(error) => {
            return Err(format!(
                "Could not read {APP_SETTINGS_FILE} ({}).",
                error.kind()
            ));
        }
    };
    // Prima la versione, poi il resto: un file di una versione futura si riconosce come tale
    // anche se i suoi campi non si leggono più.
    let value: serde_json::Value = serde_json::from_str(&text).map_err(|_| damaged_settings())?;
    let version = value.get("version").and_then(serde_json::Value::as_u64);
    if version != Some(u64::from(APP_SETTINGS_VERSION)) {
        return match version {
            Some(version) => Err(format!(
                "{APP_SETTINGS_FILE} has an unsupported version ({version}): update the app or move the file away, then restart."
            )),
            None => Err(damaged_settings()),
        };
    }
    let stored: StoredAppSettings =
        serde_json::from_value(value).map_err(|_| damaged_settings())?;
    let mut settings = stored.settings;
    if check_auto_update_interval(settings.auto_update_interval_hours).is_err() {
        settings.auto_update_interval_hours = DEFAULT_AUTO_UPDATE_INTERVAL_HOURS;
    }
    Ok(settings)
}

fn damaged_settings() -> String {
    format!("{APP_SETTINGS_FILE} is damaged: fix it or move it away and restart the app.")
}

fn read(file: &Path) -> Result<Vec<Installation>, String> {
    let text = match read_text(file) {
        Ok(Some(text)) => text,
        Ok(None) => return Ok(Vec::new()),
        Err(error) => {
            return Err(format!(
                "Could not read {INSTALLATIONS_FILE} ({}).",
                error.kind()
            ));
        }
    };
    let stored: StoredInstallations = serde_json::from_str(&text).map_err(|_| {
        format!("{INSTALLATIONS_FILE} is damaged: fix it or move it away and restart the app.")
    })?;
    if stored.version != FORMAT_VERSION {
        return Err(format!(
            "{INSTALLATIONS_FILE} has an unsupported version ({}).",
            stored.version
        ));
    }
    stored
        .paths
        .into_iter()
        .map(|path| {
            Installation::from_canonical(path).ok_or_else(|| {
                format!("{INSTALLATIONS_FILE} contains a path that is not a _<flavor>_ folder.")
            })
        })
        .collect()
}

/// Il contenuto del file; `None` se non esiste.
pub(crate) fn read_text(file: &Path) -> std::io::Result<Option<String>> {
    match fs::read_to_string(file) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

pub(crate) fn write_atomically<S: Serialize>(file: &Path, stored: &S) -> std::io::Result<()> {
    let dir = file.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(dir)?;
    let json = serde_json::to_vec_pretty(stored).map_err(std::io::Error::other)?;
    let tmp = file.with_extension("json.tmp");
    {
        let mut out = fs::File::create(&tmp)?;
        out.write_all(&json)?;
        out.sync_all()?;
    }
    fs::rename(&tmp, file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;
    use crate::updates::{CLASSIC_ERA, RETAIL};

    fn game_dir(root: &Path, flavor_dir: &str) -> PathBuf {
        let dir = root.join("World of Warcraft").join(flavor_dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_missing_file_means_no_installations() {
        let tmp = TempDir::new();
        let store = InstallationStore::load(tmp.path().join("config/installations.json"));
        assert_eq!(store.list().unwrap(), Vec::new());
    }

    #[test]
    fn add_and_remove_survive_a_reload() {
        let tmp = TempDir::new();
        let file = tmp.path().join("config").join(INSTALLATIONS_FILE);
        let retail = game_dir(tmp.path(), "_retail_");
        let beta = game_dir(tmp.path(), "_classic_beta_");

        let mut store = InstallationStore::load(file.clone());
        let added = store.add(&retail).unwrap();
        store.add(&beta).unwrap();

        let reloaded = InstallationStore::load(file.clone());
        let flavors: Vec<_> = reloaded
            .list()
            .unwrap()
            .into_iter()
            .map(|i| i.flavor)
            .collect();
        assert_eq!(flavors, ["retail", "classic_beta"]);

        let mut reloaded = reloaded;
        reloaded.remove(&added.id).unwrap();
        let after = InstallationStore::load(file.clone()).list().unwrap();
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].flavor, "classic_beta");
        assert!(!file.with_extension("json.tmp").exists());
    }

    #[test]
    fn only_paths_are_stored_and_derived_fields_are_recomputed() {
        let tmp = TempDir::new();
        let file = tmp.path().join(INSTALLATIONS_FILE);
        let retail = game_dir(tmp.path(), "_retail_");
        let mut store = InstallationStore::load(file.clone());
        let added = store.add(&retail).unwrap();

        let saved: serde_json::Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
        assert_eq!(saved["version"], 1);
        assert_eq!(saved["paths"].as_array().unwrap().len(), 1);
        assert!(saved.get("installations").is_none());

        let reloaded = InstallationStore::load(file).list().unwrap();
        assert_eq!(reloaded, [added]);
    }

    #[test]
    fn a_change_made_by_another_instance_is_not_lost() {
        let tmp = TempDir::new();
        let file = tmp.path().join(INSTALLATIONS_FILE);
        let retail = game_dir(tmp.path(), "_retail_");
        let era = game_dir(tmp.path(), "_classic_era_");

        let mut first = InstallationStore::load(file.clone());
        let mut second = InstallationStore::load(file.clone());
        first.add(&retail).unwrap();
        second.add(&era).unwrap();

        let flavors: Vec<_> = InstallationStore::load(file)
            .list()
            .unwrap()
            .into_iter()
            .map(|i| i.flavor)
            .collect();
        assert_eq!(flavors, ["retail", "classic_era"]);
    }

    #[test]
    fn a_stored_path_that_is_not_a_flavor_folder_is_reported() {
        let tmp = TempDir::new();
        let file = tmp.path().join(INSTALLATIONS_FILE);
        fs::write(&file, br#"{"version": 1, "paths": ["games/Interface"]}"#).unwrap();
        let store = InstallationStore::load(file);
        assert!(store.list().unwrap_err().contains("_<flavor>_"));
    }

    #[test]
    fn duplicates_are_rejected() {
        let tmp = TempDir::new();
        let retail = game_dir(tmp.path(), "_retail_");
        let mut store = InstallationStore::load(tmp.path().join(INSTALLATIONS_FILE));
        store.add(&retail).unwrap();

        let same = tmp
            .path()
            .join("World of Warcraft/../World of Warcraft/_retail_");
        assert!(store.add(&same).unwrap_err().contains("already"));
        assert_eq!(store.list().unwrap().len(), 1);
    }

    #[test]
    fn removing_an_unknown_id_is_an_error() {
        let tmp = TempDir::new();
        let mut store = InstallationStore::load(tmp.path().join(INSTALLATIONS_FILE));
        assert!(store.remove("missing").is_err());
    }

    #[test]
    fn invalid_folders_are_not_saved() {
        let tmp = TempDir::new();
        let file = tmp.path().join(INSTALLATIONS_FILE);
        let not_flavor = game_dir(tmp.path(), "Interface");
        let mut store = InstallationStore::load(file.clone());
        assert!(store.add(&not_flavor).is_err());
        assert!(!file.exists());
    }

    #[test]
    fn a_corrupt_file_is_reported_and_left_untouched() {
        let tmp = TempDir::new();
        let file = tmp.path().join(INSTALLATIONS_FILE);
        fs::write(&file, b"{ not json").unwrap();
        let retail = game_dir(tmp.path(), "_retail_");

        let mut store = InstallationStore::load(file.clone());
        assert!(store.list().unwrap_err().contains("damaged"));
        assert!(store.add(&retail).is_err());
        assert!(store.remove("any").is_err());
        assert_eq!(fs::read(&file).unwrap(), b"{ not json");
    }

    // ── AddonSettingsStore ──────────────────────────────────────────────────

    #[test]
    fn missing_addon_settings_mean_no_overrides() {
        let tmp = TempDir::new();
        let store = AddonSettingsStore::load(tmp.path().join("config").join(ADDON_SETTINGS_FILE));
        assert!(store.overrides_for("a").unwrap().is_empty());
    }

    #[test]
    fn overrides_survive_a_reload_and_are_per_installation() {
        let tmp = TempDir::new();
        let file = tmp.path().join("config").join(ADDON_SETTINGS_FILE);
        let mut store = AddonSettingsStore::load(file.clone());
        store.set_override("a", 7, CLASSIC_ERA).unwrap();
        store.set_override("a", 8, RETAIL).unwrap();
        store.set_override("b", 7, RETAIL).unwrap();
        store.set_override("a", 8, CLASSIC_ERA).unwrap();

        let reloaded = AddonSettingsStore::load(file.clone());
        assert_eq!(
            reloaded.overrides_for("a").unwrap(),
            VersionOverrides::from([(7, CLASSIC_ERA), (8, CLASSIC_ERA)])
        );
        assert_eq!(
            reloaded.overrides_for("b").unwrap(),
            VersionOverrides::from([(7, RETAIL)])
        );
        assert!(!file.with_extension("json.tmp").exists());

        let saved: serde_json::Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
        assert_eq!(saved["version"], 1);
        assert_eq!(
            saved["installations"]["a"]["7"]["gameVersionTypeId"],
            CLASSIC_ERA
        );
    }

    #[test]
    fn clearing_an_override() {
        let tmp = TempDir::new();
        let file = tmp.path().join(ADDON_SETTINGS_FILE);
        let mut store = AddonSettingsStore::load(file.clone());
        store.set_override("a", 7, CLASSIC_ERA).unwrap();
        store.clear_override("a", 7).unwrap();
        // Togliere una versione forzata che non c'è non è un errore.
        store.clear_override("a", 7).unwrap();
        store.clear_override("missing", 1).unwrap();

        let saved: serde_json::Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
        assert_eq!(saved["installations"], serde_json::json!({}));
        assert!(
            AddonSettingsStore::load(file)
                .overrides_for("a")
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn an_unknown_game_version_is_rejected() {
        let tmp = TempDir::new();
        let file = tmp.path().join(ADDON_SETTINGS_FILE);
        let mut store = AddonSettingsStore::load(file.clone());
        let error = store.set_override("a", 7, 81212).unwrap_err();
        assert_eq!(error, "Unknown game version.");
        assert!(!file.exists());
    }

    #[test]
    fn a_stored_type_the_app_no_longer_knows_is_not_used() {
        let tmp = TempDir::new();
        let file = tmp.path().join(ADDON_SETTINGS_FILE);
        fs::write(
            &file,
            br#"{"version": 1, "installations": {"a": {"7": {"gameVersionTypeId": 1}, "8": {"gameVersionTypeId": 517}}}}"#,
        )
        .unwrap();
        let mut store = AddonSettingsStore::load(file.clone());
        assert_eq!(
            store.overrides_for("a").unwrap(),
            VersionOverrides::from([(8, RETAIL)])
        );

        // La lettura non tocca il file: il tipo sconosciuto resta finché non si salva.
        let before: serde_json::Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
        assert_eq!(
            before["installations"]["a"]["7"],
            serde_json::json!({"gameVersionTypeId": 1})
        );

        // Il salvataggio successivo lo toglie dal file.
        store.set_override("b", 7, RETAIL).unwrap();
        let saved: serde_json::Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
        assert_eq!(
            saved["installations"],
            serde_json::json!({"a": {"8": {"gameVersionTypeId": RETAIL}}, "b": {"7": {"gameVersionTypeId": RETAIL}}})
        );
    }

    #[test]
    fn removing_an_installation_removes_its_settings() {
        let tmp = TempDir::new();
        let file = tmp.path().join(ADDON_SETTINGS_FILE);
        let mut store = AddonSettingsStore::load(file.clone());
        store.set_override("a", 7, CLASSIC_ERA).unwrap();
        store.set_override("b", 7, RETAIL).unwrap();
        store.remove_installation("a").unwrap();
        store.remove_installation("never-had-settings").unwrap();

        let reloaded = AddonSettingsStore::load(file);
        assert!(reloaded.overrides_for("a").unwrap().is_empty());
        assert_eq!(reloaded.overrides_for("b").unwrap().len(), 1);
    }

    #[test]
    fn a_change_to_the_settings_by_another_instance_is_not_lost() {
        let tmp = TempDir::new();
        let file = tmp.path().join(ADDON_SETTINGS_FILE);
        let mut first = AddonSettingsStore::load(file.clone());
        let mut second = AddonSettingsStore::load(file.clone());
        first.set_override("a", 7, CLASSIC_ERA).unwrap();
        second.set_override("a", 8, RETAIL).unwrap();
        assert_eq!(
            AddonSettingsStore::load(file)
                .overrides_for("a")
                .unwrap()
                .len(),
            2
        );
    }

    fn project(source: SourceKind, key: &str) -> ProjectKey {
        ProjectKey {
            source,
            key: key.to_owned(),
        }
    }

    #[test]
    fn without_settings_every_installation_takes_part_in_automatic_updates() {
        let tmp = TempDir::new();
        let store = AddonSettingsStore::load(tmp.path().join(ADDON_SETTINGS_FILE));
        assert_eq!(
            store.auto_update_for("a").unwrap(),
            InstallationAutoUpdate::default()
        );
    }

    #[test]
    fn excluded_projects_of_any_source_survive_a_reload_and_keep_the_overrides() {
        let tmp = TempDir::new();
        let file = tmp.path().join(ADDON_SETTINGS_FILE);
        let mut store = AddonSettingsStore::load(file.clone());
        store.set_override("a", 7, CLASSIC_ERA).unwrap();
        store
            .set_auto_update_excluded("a", project(SourceKind::Curseforge, "7"), true)
            .unwrap();
        store
            .set_auto_update_excluded("a", project(SourceKind::Github, "owner/repo"), true)
            .unwrap();
        // Escludere due volte lo stesso progetto non lo duplica.
        let entry = store
            .set_auto_update_excluded("a", project(SourceKind::Tukui, "-2"), true)
            .unwrap();
        store
            .set_auto_update_excluded("a", project(SourceKind::Tukui, "-2"), true)
            .unwrap();
        assert_eq!(entry.excluded.len(), 3);
        assert!(!entry.disabled);

        let reloaded = AddonSettingsStore::load(file.clone());
        let entry = reloaded.auto_update_for("a").unwrap();
        assert_eq!(
            entry.excluded,
            BTreeSet::from([
                project(SourceKind::Curseforge, "7"),
                project(SourceKind::Github, "owner/repo"),
                project(SourceKind::Tukui, "-2"),
            ])
        );
        assert_eq!(
            reloaded.overrides_for("a").unwrap(),
            VersionOverrides::from([(7, CLASSIC_ERA)])
        );
        assert_eq!(
            reloaded.auto_update_for("b").unwrap(),
            InstallationAutoUpdate::default()
        );

        let saved: serde_json::Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
        assert_eq!(saved["version"], 1);
        assert_eq!(
            saved["autoUpdate"]["a"]["excluded"][0],
            serde_json::json!({"source": "curseforge", "key": "7"})
        );

        let mut store = reloaded;
        store
            .set_auto_update_excluded("a", project(SourceKind::Github, "owner/repo"), false)
            .unwrap();
        assert_eq!(store.auto_update_for("a").unwrap().excluded.len(), 2);
    }

    #[test]
    fn an_installation_can_leave_automatic_updates_and_keep_its_exclusions() {
        let tmp = TempDir::new();
        let file = tmp.path().join(ADDON_SETTINGS_FILE);
        let mut store = AddonSettingsStore::load(file.clone());
        store
            .set_auto_update_excluded("a", project(SourceKind::Wowinterface, "5"), true)
            .unwrap();
        let entry = store.set_installation_auto_update("a", false).unwrap();
        assert!(entry.disabled);
        assert_eq!(entry.excluded.len(), 1);
        assert!(
            AddonSettingsStore::load(file.clone())
                .auto_update_for("a")
                .unwrap()
                .disabled
        );

        store.set_installation_auto_update("a", true).unwrap();
        store
            .set_auto_update_excluded("a", project(SourceKind::Wowinterface, "5"), false)
            .unwrap();
        // Una voce tornata al default non resta nel file.
        let saved: serde_json::Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
        assert!(saved.get("autoUpdate").is_none());
    }

    #[test]
    fn clearing_the_last_override_keeps_the_automatic_update_settings() {
        let tmp = TempDir::new();
        let file = tmp.path().join(ADDON_SETTINGS_FILE);
        let mut store = AddonSettingsStore::load(file.clone());
        store.set_override("a", 7, RETAIL).unwrap();
        store.set_installation_auto_update("a", false).unwrap();
        store
            .set_auto_update_excluded("a", project(SourceKind::Curseforge, "9"), true)
            .unwrap();
        store.clear_override("a", 7).unwrap();

        let reloaded = AddonSettingsStore::load(file.clone());
        let entry = reloaded.auto_update_for("a").unwrap();
        assert!(entry.disabled);
        assert_eq!(entry.excluded.len(), 1);
        assert!(reloaded.overrides_for("a").unwrap().is_empty());
        let saved: serde_json::Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
        assert_eq!(saved["installations"], serde_json::json!({}));
    }

    #[test]
    fn a_file_without_automatic_update_settings_reads_as_no_exclusions() {
        let tmp = TempDir::new();
        let file = tmp.path().join(ADDON_SETTINGS_FILE);
        fs::write(
            &file,
            r#"{"version": 1, "installations": {"a": {"7": {"gameVersionTypeId": 517}}}}"#,
        )
        .unwrap();
        let store = AddonSettingsStore::load(file);
        assert_eq!(
            store.auto_update_for("a").unwrap(),
            InstallationAutoUpdate::default()
        );
        assert_eq!(
            store.overrides_for("a").unwrap(),
            VersionOverrides::from([(7, RETAIL)])
        );
    }

    #[test]
    fn removing_an_installation_removes_its_exclusions() {
        let tmp = TempDir::new();
        let file = tmp.path().join(ADDON_SETTINGS_FILE);
        let mut store = AddonSettingsStore::load(file.clone());
        store.set_installation_auto_update("a", false).unwrap();
        store
            .set_auto_update_excluded("b", project(SourceKind::Curseforge, "9"), true)
            .unwrap();
        store.remove_installation("a").unwrap();
        let reloaded = AddonSettingsStore::load(file);
        assert_eq!(
            reloaded.auto_update_for("a").unwrap(),
            InstallationAutoUpdate::default()
        );
        assert_eq!(reloaded.auto_update_for("b").unwrap().excluded.len(), 1);
    }

    #[test]
    fn corrupt_addon_settings_are_reported_and_left_untouched() {
        let tmp = TempDir::new();
        let file = tmp.path().join(ADDON_SETTINGS_FILE);
        fs::write(&file, b"{ not json").unwrap();
        let mut store = AddonSettingsStore::load(file.clone());
        assert!(store.overrides_for("a").unwrap_err().contains("damaged"));
        assert!(store.set_override("a", 7, RETAIL).is_err());
        assert!(store.clear_override("a", 7).is_err());
        assert!(store.remove_installation("a").is_err());
        assert!(store.auto_update_for("a").is_err());
        assert!(store.set_installation_auto_update("a", false).is_err());
        assert!(
            store
                .set_auto_update_excluded("a", project(SourceKind::Curseforge, "7"), true)
                .is_err()
        );
        assert_eq!(fs::read(&file).unwrap(), b"{ not json");

        fs::write(&file, br#"{"version": 2, "installations": {}}"#).unwrap();
        let store = AddonSettingsStore::load(file);
        assert!(
            store
                .overrides_for("a")
                .unwrap_err()
                .contains("unsupported")
        );
    }

    // ── InstalledAddonsStore ────────────────────────────────────────────────

    fn record(file_id: u32, folders: &[&str]) -> InstalledRecord {
        InstalledRecord {
            file_id,
            folders: folders.iter().map(|f| (*f).to_owned()).collect(),
        }
    }

    #[test]
    fn missing_installed_addons_mean_no_records() {
        let tmp = TempDir::new();
        let store =
            InstalledAddonsStore::load(tmp.path().join("config").join(INSTALLED_ADDONS_FILE));
        assert!(store.records_for("a").unwrap().is_empty());
    }

    #[test]
    fn records_survive_a_reload_and_are_per_installation() {
        let tmp = TempDir::new();
        let file = tmp.path().join("config").join(INSTALLED_ADDONS_FILE);
        let mut store = InstalledAddonsStore::load(file.clone());
        store
            .record("a", 7, record(70, &["Details", "Details_Compare2"]))
            .unwrap();
        store.record("b", 7, record(71, &["Details"])).unwrap();
        // Un nuovo record dello stesso progetto sostituisce il precedente.
        store.record("a", 7, record(72, &["Details"])).unwrap();

        let reloaded = InstalledAddonsStore::load(file.clone());
        assert_eq!(
            reloaded.records_for("a").unwrap(),
            InstalledRecords::from([(7, record(72, &["Details"]))])
        );
        assert_eq!(
            reloaded.records_for("b").unwrap(),
            InstalledRecords::from([(7, record(71, &["Details"]))])
        );
        assert!(!file.with_extension("json.tmp").exists());

        let saved: serde_json::Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
        assert_eq!(saved["version"], 2);
        assert_eq!(
            saved["installations"]["a"]["curseforge"]["7"],
            serde_json::json!({"file": "72", "folders": ["Details"]})
        );
    }

    #[test]
    fn removing_an_installation_removes_its_records() {
        let tmp = TempDir::new();
        let file = tmp.path().join(INSTALLED_ADDONS_FILE);
        let mut store = InstalledAddonsStore::load(file.clone());
        store.record("a", 7, record(70, &["Alpha"])).unwrap();
        store.record("b", 8, record(80, &["Beta"])).unwrap();
        store.remove_installation("a").unwrap();
        store.remove_installation("never-had-records").unwrap();

        let reloaded = InstalledAddonsStore::load(file);
        assert!(reloaded.records_for("a").unwrap().is_empty());
        assert_eq!(reloaded.records_for("b").unwrap().len(), 1);
    }

    #[test]
    fn a_record_by_another_instance_is_not_lost() {
        let tmp = TempDir::new();
        let file = tmp.path().join(INSTALLED_ADDONS_FILE);
        let mut first = InstalledAddonsStore::load(file.clone());
        let mut second = InstalledAddonsStore::load(file.clone());
        first.record("a", 7, record(70, &["Alpha"])).unwrap();
        second.record("a", 8, record(80, &["Beta"])).unwrap();
        assert_eq!(
            InstalledAddonsStore::load(file)
                .records_for("a")
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn corrupt_installed_addons_are_reported_and_left_untouched() {
        let tmp = TempDir::new();
        let file = tmp.path().join(INSTALLED_ADDONS_FILE);
        fs::write(&file, b"{ not json").unwrap();
        let mut store = InstalledAddonsStore::load(file.clone());
        assert!(store.records_for("a").unwrap_err().contains("damaged"));
        assert!(store.record("a", 7, record(70, &["Alpha"])).is_err());
        assert!(store.remove_installation("a").is_err());
        assert_eq!(fs::read(&file).unwrap(), b"{ not json");

        fs::write(&file, br#"{"version": 3, "installations": {}}"#).unwrap();
        let store = InstalledAddonsStore::load(file);
        assert!(store.records_for("a").unwrap_err().contains("unsupported"));
    }

    #[test]
    fn a_version_1_file_is_migrated_on_the_first_change() {
        let tmp = TempDir::new();
        let file = tmp.path().join(INSTALLED_ADDONS_FILE);
        let v1 = br#"{"version": 1, "installations": {"a": {"61284": {"fileId": 5000, "folders": ["Details", "Details_Compare2"]}}, "b": {}}}"#;
        fs::write(&file, v1).unwrap();

        let mut store = InstalledAddonsStore::load(file.clone());
        assert_eq!(
            store.records_for("a").unwrap(),
            InstalledRecords::from([(61284, record(5000, &["Details", "Details_Compare2"]))])
        );
        // Leggere non riscrive il file.
        assert_eq!(fs::read(&file).unwrap(), v1);

        store.record("b", 7, record(70, &["Alpha"])).unwrap();
        let saved: serde_json::Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
        assert_eq!(
            saved,
            serde_json::json!({
                "version": 2,
                "installations": {
                    "a": {"curseforge": {"61284": {"file": "5000", "folders": ["Details", "Details_Compare2"]}}},
                    "b": {"curseforge": {"7": {"file": "70", "folders": ["Alpha"]}}},
                }
            })
        );
        assert_eq!(
            InstalledAddonsStore::load(file)
                .records_for("a")
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn records_of_other_sources_are_kept_but_not_listed() {
        let tmp = TempDir::new();
        let file = tmp.path().join(INSTALLED_ADDONS_FILE);
        let mut store = InstalledAddonsStore::load(file.clone());
        store.record("a", 7, record(70, &["Alpha"])).unwrap();
        for (source, id, version) in [
            (SourceKind::Github, "owner/repo", "v1.2"),
            (SourceKind::Wowinterface, "5108", "v5.0.15"),
            (SourceKind::Tukui, "-2", "16.00"),
        ] {
            store
                .record_source(
                    "a",
                    source,
                    id,
                    SourceRecord {
                        file: version.to_owned(),
                        folders: vec!["Other".to_owned()],
                    },
                )
                .unwrap();
        }
        // Un record CurseForge scritto dopo non cancella quelli delle altre fonti.
        store.record("a", 8, record(80, &["Beta"])).unwrap();

        let reloaded = InstalledAddonsStore::load(file.clone());
        assert_eq!(
            reloaded.records_for("a").unwrap(),
            InstalledRecords::from([(7, record(70, &["Alpha"])), (8, record(80, &["Beta"]))])
        );
        let saved: serde_json::Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
        let a = &saved["installations"]["a"];
        assert_eq!(a["github"]["owner/repo"]["file"], "v1.2");
        assert_eq!(a["wowinterface"]["5108"]["file"], "v5.0.15");
        assert_eq!(a["tukui"]["-2"]["file"], "16.00");
    }

    #[test]
    fn forgetting_a_project_drops_only_its_record() {
        let tmp = TempDir::new();
        let file = tmp.path().join(INSTALLED_ADDONS_FILE);
        let mut store = InstalledAddonsStore::load(file.clone());
        store.record("a", 7, record(70, &["Alpha"])).unwrap();
        store.record("a", 8, record(80, &["Beta"])).unwrap();
        store.record("b", 7, record(70, &["Alpha"])).unwrap();
        store
            .forget_project("a", SourceKind::Curseforge, "7")
            .unwrap();
        // Un progetto senza record non scrive e non fallisce.
        store
            .forget_project("a", SourceKind::Github, "x/y")
            .unwrap();
        store
            .forget_project("c", SourceKind::Curseforge, "7")
            .unwrap();
        let reloaded = InstalledAddonsStore::load(file.clone());
        assert_eq!(
            reloaded.records_for("a").unwrap(),
            InstalledRecords::from([(8, record(80, &["Beta"]))])
        );
        assert_eq!(reloaded.records_for("b").unwrap().len(), 1);
        store
            .forget_project("a", SourceKind::Curseforge, "8")
            .unwrap();
        let saved: serde_json::Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
        assert!(saved["installations"].get("a").is_none());
    }

    #[test]
    fn forgetting_folders_drops_only_the_records_that_name_them() {
        let tmp = TempDir::new();
        let file = tmp.path().join(INSTALLED_ADDONS_FILE);
        let mut store = InstalledAddonsStore::load(file.clone());
        store
            .record("a", 7, record(70, &["Alpha", "Alpha_Options"]))
            .unwrap();
        store.record("a", 8, record(80, &["Beta"])).unwrap();
        store
            .record_source(
                "a",
                SourceKind::Tukui,
                "-2",
                SourceRecord {
                    file: "16.00".to_owned(),
                    folders: vec!["ElvUI".to_owned()],
                },
            )
            .unwrap();
        store.record("b", 7, record(70, &["Alpha"])).unwrap();

        // Le maiuscole non contano; l'altra installazione resta com'è.
        let removed = store
            .forget_folders("a", &["alpha_options".to_owned(), "ELVUI".to_owned()])
            .unwrap();
        assert_eq!(removed, 2);
        let reloaded = InstalledAddonsStore::load(file.clone());
        assert_eq!(
            reloaded.records_for("a").unwrap(),
            InstalledRecords::from([(8, record(80, &["Beta"]))])
        );
        assert_eq!(
            reloaded.records_for("b").unwrap(),
            InstalledRecords::from([(7, record(70, &["Alpha"]))])
        );
        let saved: serde_json::Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
        assert!(saved["installations"]["a"].get("tukui").is_none());

        // Nessun record da togliere: il file non si riscrive.
        let before = fs::read(&file).unwrap();
        assert_eq!(store.forget_folders("a", &["Other".to_owned()]).unwrap(), 0);
        assert_eq!(
            store
                .forget_folders("missing", &["Beta".to_owned()])
                .unwrap(),
            0
        );
        assert_eq!(fs::read(&file).unwrap(), before);
    }

    #[test]
    fn forgetting_folders_in_a_damaged_file_fails_without_writing() {
        let tmp = TempDir::new();
        let file = tmp.path().join(INSTALLED_ADDONS_FILE);
        fs::write(&file, b"{ not json").unwrap();
        let mut store = InstalledAddonsStore::load(file.clone());
        assert!(
            store
                .forget_folders("a", &["Alpha".to_owned()])
                .unwrap_err()
                .contains("damaged")
        );
        assert_eq!(fs::read(&file).unwrap(), b"{ not json");
    }

    #[test]
    fn unreadable_version_2_records_block_changes() {
        let tmp = TempDir::new();
        let file = tmp.path().join(INSTALLED_ADDONS_FILE);
        for body in [
            // Fonte sconosciuta.
            r#"{"version": 2, "installations": {"a": {"wago": {"x": {"file": "1", "folders": []}}}}}"#,
            // Id CurseForge non numerico.
            r#"{"version": 2, "installations": {"a": {"curseforge": {"abc": {"file": "1", "folders": []}}}}}"#,
            // File CurseForge non numerico.
            r#"{"version": 2, "installations": {"a": {"curseforge": {"7": {"file": "v1", "folders": []}}}}}"#,
            // Versione illeggibile.
            r#"{"installations": {}}"#,
        ] {
            fs::write(&file, body).unwrap();
            let mut store = InstalledAddonsStore::load(file.clone());
            assert!(
                store.records_for("a").unwrap_err().contains("damaged"),
                "{body}"
            );
            assert!(store.record("a", 7, record(70, &["Alpha"])).is_err());
            assert_eq!(fs::read_to_string(&file).unwrap(), body);
        }
    }

    // ── AppSettingsStore ────────────────────────────────────────────────────

    #[test]
    fn missing_app_settings_mean_the_system_theme() {
        let tmp = TempDir::new();
        let file = tmp.path().join("config").join(APP_SETTINGS_FILE);
        let store = AppSettingsStore::load(file.clone());
        assert_eq!(store.get().unwrap().theme, Theme::System);
        // Leggere non crea il file.
        assert!(!file.exists());
    }

    #[test]
    fn the_theme_survives_a_reload() {
        let tmp = TempDir::new();
        let file = tmp.path().join("config").join(APP_SETTINGS_FILE);
        let mut store = AppSettingsStore::load(file.clone());
        assert_eq!(store.set_theme(Theme::Dark).unwrap().theme, Theme::Dark);
        assert_eq!(store.get().unwrap().theme, Theme::Dark);
        assert_eq!(
            AppSettingsStore::load(file.clone()).get().unwrap().theme,
            Theme::Dark
        );
        assert!(!file.with_extension("json.tmp").exists());

        let saved: serde_json::Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
        assert_eq!(
            saved,
            serde_json::json!({
                "version": 1,
                "theme": "dark",
                "betaWhenNoRelease": false,
                "closeToTray": false,
                "autoUpdate": "notify",
                "autoUpdateIntervalHours": 6
            })
        );

        store.set_theme(Theme::System).unwrap();
        assert_eq!(
            AppSettingsStore::load(file).get().unwrap().theme,
            Theme::System
        );
    }

    #[test]
    fn the_beta_option_defaults_to_off_and_keeps_the_theme() {
        let tmp = TempDir::new();
        let file = tmp.path().join(APP_SETTINGS_FILE);
        // Un file scritto prima della issue #141 non ha il campo.
        fs::write(&file, r#"{"version": 1, "theme": "dark"}"#).unwrap();
        let mut store = AppSettingsStore::load(file.clone());
        assert!(!store.get().unwrap().beta_when_no_release);
        let saved = store.set_beta_when_no_release(true).unwrap();
        assert!(saved.beta_when_no_release);
        assert_eq!(saved.theme, Theme::Dark);
        let reloaded = AppSettingsStore::load(file).get().unwrap();
        assert!(reloaded.beta_when_no_release);
        assert_eq!(reloaded.theme, Theme::Dark);
    }

    #[test]
    fn close_to_tray_defaults_to_off_and_keeps_the_other_settings() {
        let tmp = TempDir::new();
        let file = tmp.path().join(APP_SETTINGS_FILE);
        assert!(
            !AppSettingsStore::load(file.clone())
                .get()
                .unwrap()
                .close_to_tray
        );

        // A file written before the option existed has no field: it reads as off.
        fs::write(
            &file,
            r#"{"version": 1, "theme": "dark", "betaWhenNoRelease": true}"#,
        )
        .unwrap();
        let mut store = AppSettingsStore::load(file.clone());
        assert!(!store.get().unwrap().close_to_tray);

        let saved = store.set_close_to_tray(true).unwrap();
        assert!(saved.close_to_tray);
        assert_eq!(saved.theme, Theme::Dark);
        assert!(saved.beta_when_no_release);
        let reloaded = AppSettingsStore::load(file.clone()).get().unwrap();
        assert!(reloaded.close_to_tray);
        assert_eq!(reloaded.theme, Theme::Dark);
        assert!(reloaded.beta_when_no_release);

        assert!(!store.set_close_to_tray(false).unwrap().close_to_tray);
        assert!(!AppSettingsStore::load(file).get().unwrap().close_to_tray);
    }

    #[test]
    fn a_damaged_settings_file_blocks_the_close_to_tray_option() {
        let tmp = TempDir::new();
        let file = tmp.path().join(APP_SETTINGS_FILE);
        fs::write(&file, b"not json").unwrap();
        let mut store = AppSettingsStore::load(file.clone());
        assert!(store.get().is_err());
        assert!(store.set_close_to_tray(true).is_err());
        assert_eq!(fs::read(&file).unwrap(), b"not json");
    }

    #[test]
    fn automatic_updates_default_to_notify_every_six_hours() {
        let tmp = TempDir::new();
        let file = tmp.path().join(APP_SETTINGS_FILE);
        let missing = AppSettingsStore::load(file.clone()).get().unwrap();
        assert_eq!(missing.auto_update, AutoUpdateMode::Notify);
        assert_eq!(missing.auto_update_interval_hours, 6);

        // Un file scritto prima della issue #199 non ha i campi.
        fs::write(
            &file,
            r#"{"version": 1, "theme": "dark", "closeToTray": true}"#,
        )
        .unwrap();
        let mut store = AppSettingsStore::load(file.clone());
        let old = store.get().unwrap();
        assert_eq!(old.auto_update, AutoUpdateMode::Notify);
        assert_eq!(old.auto_update_interval_hours, 6);

        let saved = store.set_auto_update(AutoUpdateMode::Off, 24).unwrap();
        assert_eq!(saved.auto_update, AutoUpdateMode::Off);
        assert_eq!(saved.auto_update_interval_hours, 24);
        assert_eq!(saved.theme, Theme::Dark);
        assert!(saved.close_to_tray);
        let reloaded = AppSettingsStore::load(file).get().unwrap();
        assert_eq!(reloaded, saved);
    }

    #[test]
    fn an_unsupported_interval_is_rejected_and_one_written_by_hand_reads_as_six_hours() {
        let tmp = TempDir::new();
        let file = tmp.path().join(APP_SETTINGS_FILE);
        let mut store = AppSettingsStore::load(file.clone());
        assert!(store.set_auto_update(AutoUpdateMode::Install, 5).is_err());
        assert!(!file.exists());

        fs::write(
            &file,
            r#"{"version": 1, "theme": "system", "autoUpdate": "install", "autoUpdateIntervalHours": 5}"#,
        )
        .unwrap();
        let settings = AppSettingsStore::load(file).get().unwrap();
        assert_eq!(settings.auto_update, AutoUpdateMode::Install);
        assert_eq!(settings.auto_update_interval_hours, 6);
    }

    #[test]
    fn an_unknown_mode_in_the_file_blocks_the_settings() {
        let tmp = TempDir::new();
        let file = tmp.path().join(APP_SETTINGS_FILE);
        fs::write(
            &file,
            r#"{"version": 1, "theme": "system", "autoUpdate": "always"}"#,
        )
        .unwrap();
        let mut store = AppSettingsStore::load(file.clone());
        assert!(store.get().unwrap_err().contains("damaged"));
        assert!(store.set_auto_update(AutoUpdateMode::Off, 6).is_err());
    }

    #[test]
    fn auto_update_mode_names_are_parsed_and_unknown_ones_rejected() {
        assert_eq!(AutoUpdateMode::parse("off").unwrap(), AutoUpdateMode::Off);
        assert_eq!(
            AutoUpdateMode::parse("notify").unwrap(),
            AutoUpdateMode::Notify
        );
        assert_eq!(
            AutoUpdateMode::parse("install").unwrap(),
            AutoUpdateMode::Install
        );
        for unknown in ["", "Notify", "always"] {
            assert!(AutoUpdateMode::parse(unknown).is_err());
        }
    }

    #[test]
    fn theme_names_are_parsed_and_unknown_ones_rejected() {
        assert_eq!(Theme::parse("system").unwrap(), Theme::System);
        assert_eq!(Theme::parse("light").unwrap(), Theme::Light);
        assert_eq!(Theme::parse("dark").unwrap(), Theme::Dark);
        for unknown in ["", "Dark", "sepia", "auto"] {
            assert_eq!(
                Theme::parse(unknown).unwrap_err(),
                "Unknown theme: use system, light or dark."
            );
        }
    }

    #[test]
    fn a_theme_change_by_another_instance_is_seen_before_saving() {
        let tmp = TempDir::new();
        let file = tmp.path().join(APP_SETTINGS_FILE);
        let mut first = AppSettingsStore::load(file.clone());
        let mut second = AppSettingsStore::load(file.clone());
        first.set_theme(Theme::Dark).unwrap();
        assert_eq!(second.set_theme(Theme::Light).unwrap().theme, Theme::Light);
        assert_eq!(
            AppSettingsStore::load(file).get().unwrap().theme,
            Theme::Light
        );
    }

    #[test]
    fn corrupt_app_settings_are_reported_and_left_untouched() {
        let tmp = TempDir::new();
        let file = tmp.path().join(APP_SETTINGS_FILE);
        for damaged in [
            &b"{ not json"[..],
            br#"{"theme": "dark"}"#,
            br#"{"version": 1, "theme": "sepia"}"#,
        ] {
            fs::write(&file, damaged).unwrap();
            let mut store = AppSettingsStore::load(file.clone());
            assert!(store.get().unwrap_err().contains("damaged"));
            assert!(store.set_theme(Theme::Dark).is_err());
            assert_eq!(fs::read(&file).unwrap(), damaged);
        }

        fs::write(&file, br#"{"version": 2, "theme": "sepia"}"#).unwrap();
        let store = AppSettingsStore::load(file);
        assert!(store.get().unwrap_err().contains("unsupported version (2)"));
    }

    #[test]
    fn an_unknown_format_version_is_reported() {
        let tmp = TempDir::new();
        let file = tmp.path().join(INSTALLATIONS_FILE);
        fs::write(&file, br#"{"version": 99, "paths": []}"#).unwrap();
        let store = InstallationStore::load(file);
        assert!(store.list().unwrap_err().contains("unsupported version"));
    }
}
