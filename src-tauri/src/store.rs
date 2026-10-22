//! Stato locale salvato su disco: per ora l'elenco delle installazioni.
//!
//! Il file è `installations.json` nella cartella di configurazione dell'app risolta da
//! Tauri. Si scrive in modo atomico: file temporaneo nella stessa cartella, poi `rename`.
//! Un file mancante vuol dire nessuna installazione; un file illeggibile blocca le
//! modifiche invece di essere sovrascritto, così non si perde quello che c'era.
//!
//! Su disco si salvano solo i percorsi: id, flavor e nome si ricalcolano a ogni lettura,
//! così seguono sempre il codice attuale. Prima di ogni modifica il file si rilegge, per non
//! cancellare quello che un'altra istanza dell'app ha scritto nel frattempo. Due modifiche
//! davvero simultanee da due istanze possono ancora perdersene una: non c'è un lock sul file,
//! e l'app non impedisce di aprire due istanze.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::installations::{self, Installation};

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

fn read(file: &Path) -> Result<Vec<Installation>, String> {
    let text = match fs::read_to_string(file) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
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

fn write_atomically(file: &Path, stored: &StoredInstallations) -> std::io::Result<()> {
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

    #[test]
    fn an_unknown_format_version_is_reported() {
        let tmp = TempDir::new();
        let file = tmp.path().join(INSTALLATIONS_FILE);
        fs::write(&file, br#"{"version": 99, "paths": []}"#).unwrap();
        let store = InstallationStore::load(file);
        assert!(store.list().unwrap_err().contains("versione"));
    }
}
