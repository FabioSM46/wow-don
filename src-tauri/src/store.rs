//! Stato locale salvato su disco: l'elenco delle installazioni (`installations.json`) e le
//! impostazioni degli addon (`addon-settings.json`, per ora la versione del gioco forzata, D6).
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

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::installations::{self, Installation};
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
            return Err("Questa installazione è già stata aggiunta.".to_owned());
        }
        let mut next = self.installations.clone();
        next.push(installation.clone());
        self.save(next)?;
        Ok(installation)
    }

    pub fn remove(&mut self, id: &str) -> Result<(), String> {
        self.reload()?;
        if !self.installations.iter().any(|existing| existing.id == id) {
            return Err("Installazione non trovata.".to_owned());
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
            .map_err(|error| format!("Impossibile salvare le installazioni ({}).", error.kind()))?;
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

#[derive(Debug, Serialize, Deserialize)]
struct StoredAddonSettings {
    version: u32,
    installations: SettingsByInstallation,
}

/// Le impostazioni per addon, con le stesse regole di [`InstallationStore`]: rilettura prima
/// di ogni modifica, scrittura atomica, file illeggibile che blocca le modifiche. I messaggi
/// sono in inglese, come l'interfaccia (issue #45).
#[derive(Debug)]
pub struct AddonSettingsStore {
    file: PathBuf,
    settings: SettingsByInstallation,
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
                settings: SettingsByInstallation::new(),
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
        next.entry(installation_id.to_owned()).or_default().insert(
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
        let Some(addons) = next.get_mut(installation_id) else {
            return Ok(());
        };
        if addons.remove(&mod_id).is_none() {
            return Ok(());
        }
        if addons.is_empty() {
            next.remove(installation_id);
        }
        self.save(next)
    }

    /// Toglie tutte le impostazioni di un'installazione rimossa.
    pub fn remove_installation(&mut self, installation_id: &str) -> Result<(), String> {
        self.reload()?;
        if !self.settings.contains_key(installation_id) {
            return Ok(());
        }
        let mut next = self.settings.clone();
        next.remove(installation_id);
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

    /// Scrive le impostazioni senza i tipi che l'app non conosce più.
    fn save(&mut self, mut settings: SettingsByInstallation) -> Result<(), String> {
        for addons in settings.values_mut() {
            addons
                .retain(|_, setting| updates::is_known_version_type(setting.game_version_type_id));
        }
        settings.retain(|_, addons| !addons.is_empty());
        let stored = StoredAddonSettings {
            version: ADDON_SETTINGS_VERSION,
            installations: settings,
        };
        write_atomically(&self.file, &stored)
            .map_err(|error| format!("Could not save the addon settings ({}).", error.kind()))?;
        self.settings = stored.installations;
        Ok(())
    }
}

fn read_addon_settings(file: &Path) -> Result<SettingsByInstallation, String> {
    let text = match read_text(file) {
        Ok(Some(text)) => text,
        Ok(None) => return Ok(SettingsByInstallation::new()),
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
    Ok(stored.installations)
}

fn read(file: &Path) -> Result<Vec<Installation>, String> {
    let text = match read_text(file) {
        Ok(Some(text)) => text,
        Ok(None) => return Ok(Vec::new()),
        Err(error) => {
            return Err(format!(
                "Impossibile leggere {INSTALLATIONS_FILE} ({}).",
                error.kind()
            ));
        }
    };
    let stored: StoredInstallations = serde_json::from_str(&text).map_err(|_| {
        format!("{INSTALLATIONS_FILE} è danneggiato: correggilo o spostalo e riavvia l'app.")
    })?;
    if stored.version != FORMAT_VERSION {
        return Err(format!(
            "{INSTALLATIONS_FILE} ha una versione non supportata ({}).",
            stored.version
        ));
    }
    stored
        .paths
        .into_iter()
        .map(|path| {
            Installation::from_canonical(path).ok_or_else(|| {
                format!(
                    "{INSTALLATIONS_FILE} contiene un percorso che non è una cartella _<flavor>_."
                )
            })
        })
        .collect()
}

/// Il contenuto del file; `None` se non esiste.
fn read_text(file: &Path) -> std::io::Result<Option<String>> {
    match fs::read_to_string(file) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn write_atomically<S: Serialize>(file: &Path, stored: &S) -> std::io::Result<()> {
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
        assert!(store.add(&same).unwrap_err().contains("già"));
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
        assert!(store.list().unwrap_err().contains("danneggiato"));
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

    #[test]
    fn an_unknown_format_version_is_reported() {
        let tmp = TempDir::new();
        let file = tmp.path().join(INSTALLATIONS_FILE);
        fs::write(&file, br#"{"version": 99, "paths": []}"#).unwrap();
        let store = InstallationStore::load(file);
        assert!(store.list().unwrap_err().contains("versione"));
    }
}
