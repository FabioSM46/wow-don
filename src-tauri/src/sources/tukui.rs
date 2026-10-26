//! Tukui come fonte (D31): l'API pubblica `https://api.tukui.org/v1/addons`, senza chiave, con
//! ElvUI (id `-2`) e Tukui (id `-1`).
//!
//! Risposta verificata il 2026-10-25: un array di addon con `id`, `name`, `url` (lo zip, con un
//! token che cambia a ogni richiesta e che non va nei log), `version` (`16.00`), `patch` (le
//! versioni del gioco, `12.1.0`, `1.15.9`…), `web_url`, `last_update` e `directories` (le cartelle
//! dello zip). Il `.toc` di ElvUI scrive la versione con la `v` (`## Version: v16.00`).
//!
//! Un addon è installato quando tutte le sue `directories` sono in `AddOns` (senza distinguere
//! maiuscole e minuscole) o quando l'app l'ha registrato. La versione installata è quella del
//! record, altrimenti `## Version` del `.toc` della prima cartella; c'è un aggiornamento quando
//! è diversa da `version`, senza contare una `v` iniziale.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

use serde::Deserialize;

use super::curseforge::{MAX_DOWNLOAD_BYTES, TransportError};
use super::http::{DownloadProgress, PublicTransport, is_allowed_url};
use super::{FolderCheck, LinkInfo, RecordWriter, Source, SourceKind, UpdateTarget};
use crate::addons::scan::InstalledAddon;
use crate::addons::toc;
use crate::install;
use crate::installations::Installation;
use crate::progress::{Phase, ProgressSink, Reporter};
use crate::store::{ProjectRecords, SourceRecord};
use crate::updates::{self, AddonUpdate, FileSummary, Project, UpdateOutcome, UpdateStatus};

pub const ADDONS_URL: &str = "https://api.tukui.org/v1/addons";
/// I domini da cui Tukui serve l'API e gli zip, con i loro sottodomini.
pub const DOMAINS: [&str; 1] = ["tukui.org"];

const TUKUI: SourceKind = SourceKind::Tukui;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct TukuiAddon {
    pub id: i64,
    pub name: String,
    pub url: String,
    pub version: String,
    #[serde(default)]
    pub patch: Vec<String>,
    #[serde(default)]
    pub web_url: Option<String>,
    #[serde(default)]
    pub last_update: Option<String>,
    /// Le cartelle dello zip; un addon senza cartelle non si abbina mai.
    #[serde(default)]
    pub directories: Vec<String>,
}

/// La fonte Tukui. L'elenco si legge una volta per controllo e si riusa per l'aggiornamento che
/// lo segue (`update_addons` rifà il controllo e poi aggiorna).
pub struct TukuiSource<T: PublicTransport> {
    transport: T,
    addons: Mutex<Option<Vec<TukuiAddon>>>,
}

impl<T: PublicTransport> TukuiSource<T> {
    pub fn new(transport: T) -> Self {
        Self {
            transport,
            addons: Mutex::new(None),
        }
    }

    fn fetch(&self) -> Result<Vec<TukuiAddon>, String> {
        let response = self
            .transport
            .get(ADDONS_URL)
            .map_err(|error| match error {
                TransportError::TooLarge => {
                    "Unrecognized response from Tukui: it is too large.".to_owned()
                }
                other => network_error(other),
            })?;
        if response.status != 200 {
            return Err(format!(
                "Tukui responded with an error (HTTP {}).",
                response.status
            ));
        }
        // Una voce che non si legge (un addon nuovo con un campo diverso) si salta: non deve
        // togliere tutta la fonte. Una risposta che non è un elenco resta un errore.
        let entries: Vec<serde_json::Value> = serde_json::from_str(&response.body)
            .map_err(|_| "Unrecognized response from Tukui.".to_owned())?;
        let addons: Vec<TukuiAddon> = entries
            .into_iter()
            .filter_map(|entry| serde_json::from_value(entry).ok())
            .collect();
        *self.addons.lock().unwrap_or_else(|e| e.into_inner()) = Some(addons.clone());
        Ok(addons)
    }

    fn cached(&self) -> Result<Vec<TukuiAddon>, String> {
        let cached = self
            .addons
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        match cached {
            Some(addons) => Ok(addons),
            None => self.fetch(),
        }
    }

    fn download(
        &self,
        addon: &TukuiAddon,
        progress: &mut DownloadProgress<'_>,
    ) -> Result<Vec<u8>, String> {
        let allowed =
            reqwest::Url::parse(&addon.url).is_ok_and(|url| is_allowed_url(&url, &DOMAINS));
        if !allowed {
            return Err("Tukui gave an unrecognized download address: download cancelled.".into());
        }
        let response = self
            .transport
            .get_bytes_with_progress(&addon.url, MAX_DOWNLOAD_BYTES, progress)
            .map_err(|error| match error {
                TransportError::TooLarge => format!(
                    "The file to download exceeds the {} MiB limit: download cancelled.",
                    MAX_DOWNLOAD_BYTES / (1024 * 1024)
                ),
                other => network_error(other),
            })?;
        if response.status != 200 {
            return Err(format!(
                "Tukui responded with an error (HTTP {}).",
                response.status
            ));
        }
        Ok(response.body)
    }
}

impl<T: PublicTransport> TukuiSource<T> {
    /// Gli addon che Tukui elenca: l'anteprima di un import li usa per sapere se un progetto si
    /// può installare.
    pub fn listed(&self) -> Result<Vec<TukuiAddon>, String> {
        self.cached()
    }

    /// Installa un progetto Tukui che non c'è ancora (l'import, issue #134). Vale se `patch`
    /// elenca il gioco dell'installazione oppure se un `.toc` dello zip lo dichiara, come per
    /// il controllo (`patch` non elenca WoW Forever). `check` può rifiutare le cartelle dello zip
    /// prima che tocchino il disco. L'esito usa la forma di un aggiornamento: `updated` vuol dire
    /// installato.
    pub fn install_new(
        &self,
        installation: &Installation,
        key: &str,
        check: &FolderCheck<'_>,
        record: &mut RecordWriter<'_>,
        progress: &ProgressSink<'_>,
    ) -> UpdateOutcome {
        let addon = match self.cached() {
            Ok(addons) => addons.into_iter().find(|a| a.id.to_string() == key),
            Err(error) => return UpdateOutcome::error(TUKUI, key, "", error),
        };
        let Some(addon) = addon else {
            return UpdateOutcome::skipped(TUKUI, key, "", "Tukui does not list this addon.");
        };
        let name = addon.name.as_str();
        let reporter = Reporter::new(progress, Some(TUKUI), key, name);
        reporter.download(0, None);
        let bytes = match self.download(&addon, &mut |received, total| {
            reporter.download(received, total)
        }) {
            Ok(bytes) => bytes,
            Err(error) => return UpdateOutcome::error(TUKUI, key, name, error),
        };
        reporter.downloaded(bytes.len() as u64);
        reporter.phase(Phase::Extracting);
        let addons_dir = installation.path.join("Interface").join("AddOns");
        let installed = (|| {
            let staging = install::WorkDir::staging(&addons_dir)?;
            let folders = install::extract(&bytes, staging.path())?;
            let listed = addon
                .patch
                .iter()
                .filter_map(|patch| patch_interface(patch))
                .any(|interface| fits(&installation.flavor, interface));
            if !listed && !declares_game(staging.path(), &folders, &installation.flavor) {
                return Ok(None);
            }
            check(&folders)?;
            reporter.phase(Phase::Installing);
            install::replace(&addons_dir, staging.path(), &[])?;
            Ok::<_, String>(Some(folders))
        })();
        let folders = match installed {
            Ok(Some(folders)) => folders,
            Ok(None) => {
                return UpdateOutcome::skipped(
                    TUKUI,
                    key,
                    name,
                    "Tukui does not offer this addon for this game version.",
                );
            }
            Err(error) => return UpdateOutcome::error(TUKUI, key, name, error),
        };
        let message = record(
            TUKUI,
            key,
            SourceRecord {
                file: addon.version.clone(),
                folders,
            },
        )
        .err()
        .map(|error| format!("Installed, but the app could not remember its folders: {error}"));
        UpdateOutcome::updated(
            TUKUI,
            key,
            name,
            summary(&addon.version, addon.last_update.as_deref()),
            message,
        )
    }
}

impl<T: PublicTransport> TukuiSource<T> {
    /// L'anteprima di un link (issue #109): nome, versione e pagina dall'elenco di Tukui. Si
    /// può scaricare se Tukui lo elenca; se `patch` non nomina il gioco dell'installazione lo
    /// dice il messaggio, perché l'installazione guarda anche i `.toc` dello zip (`patch` non
    /// elenca WoW Forever). `Err` se l'elenco non si legge o non contiene il progetto.
    pub fn preview(&self, installation: &Installation, key: &str) -> Result<LinkInfo, String> {
        let addon = self
            .cached()?
            .into_iter()
            .find(|a| a.id.to_string() == key)
            .ok_or_else(|| "Tukui does not list this addon.".to_owned())?;
        let listed = addon
            .patch
            .iter()
            .filter_map(|patch| patch_interface(patch))
            .any(|interface| fits(&installation.flavor, interface));
        let message = (!listed).then(|| {
            "Tukui does not list this game version: the addon's files are checked when installing."
                .to_owned()
        });
        let url = addon
            .web_url
            .clone()
            .filter(|url| reqwest::Url::parse(url).is_ok_and(|url| is_allowed_url(&url, &DOMAINS)))
            .unwrap_or_else(|| "https://tukui.org".to_owned());
        Ok(LinkInfo {
            name: addon.name,
            version: addon.version,
            downloadable: true,
            message,
            url,
        })
    }
}

/// Se un `.toc` delle cartelle estratte in `staging` dichiara un `## Interface` del gioco
/// `flavor`.
fn declares_game(staging: &Path, folders: &[String], flavor: &str) -> bool {
    folders.iter().any(|folder| {
        let dir = staging.join(folder);
        let names: Vec<String> = std::fs::read_dir(&dir)
            .map(|entries| {
                entries
                    .flatten()
                    .filter_map(|entry| entry.file_name().into_string().ok())
                    .collect()
            })
            .unwrap_or_default();
        toc::select_toc(folder, &names, flavor)
            .and_then(|toc_file| std::fs::read(dir.join(toc_file)).ok())
            .is_some_and(|bytes| {
                toc::parse_toc(&String::from_utf8_lossy(&bytes))
                    .interface
                    .iter()
                    .any(|&interface| fits(flavor, interface))
            })
    })
}

fn network_error(error: TransportError) -> String {
    match error {
        TransportError::Timeout => "Tukui did not respond in time.".to_owned(),
        _ => "Could not reach Tukui: check the connection.".to_owned(),
    }
}

/// La versione senza spazi e senza una `v` iniziale: `v16.00` e `16.00` sono la stessa.
fn normalized(version: &str) -> &str {
    let version = version.trim();
    version
        .strip_prefix(['v', 'V'])
        .filter(|rest| rest.starts_with(|c: char| c.is_ascii_digit()))
        .unwrap_or(version)
}

/// `12.1.0` → `120100`, come i numeri di `## Interface`.
fn patch_interface(patch: &str) -> Option<u32> {
    let mut parts = patch.trim().split('.').map(|part| part.parse::<u32>().ok());
    let major = parts.next()??;
    let minor = parts.next().unwrap_or(Some(0))?;
    let fix = parts.next().unwrap_or(Some(0))?;
    // Il testo viene dalla rete: un numero enorme non deve traboccare.
    major
        .checked_mul(10_000)?
        .checked_add(minor.checked_mul(100)?)?
        .checked_add(fix)
}

/// Se un numero di `## Interface` appartiene al gioco della cartella `flavor`. La tabella:
///
/// | Cartella | Gioco | Interface |
/// | --- | --- | --- |
/// | `_retail_` | Retail | da 100000 (10.0) |
/// | `_classic_era_` | Classic Era | 11000–11599 (1.10–1.15) |
/// | `_classic_beta_` | WoW Forever | 16000–16999 (1.16) |
/// | `_anniversary_` | Anniversary | 20000–29999 (2.x) |
/// | `_classic_` | Classic progressiva | 30000–99999 (3.x–9.x, oggi Mists 5.x) |
///
/// Un'altra cartella non ha un gioco noto: niente vale.
fn fits(flavor: &str, interface: u32) -> bool {
    match flavor.trim_matches('_').to_ascii_lowercase().as_str() {
        "retail" => interface >= 100_000,
        "classic_era" => (11_000..11_600).contains(&interface),
        "classic_beta" => (16_000..17_000).contains(&interface),
        "anniversary" => (20_000..30_000).contains(&interface),
        "classic" => (30_000..100_000).contains(&interface),
        _ => false,
    }
}

/// L'addon vale per l'installazione se `patch` contiene una versione del suo gioco, oppure se
/// il `.toc` installato la dichiara già: `patch` non elenca WoW Forever, che lo zip di ElvUI
/// supporta (`## Interface: …, 16001, …`, verificato il 2026-10-25).
fn compatible(addon: &TukuiAddon, flavor: &str, installed: &[&InstalledAddon]) -> bool {
    addon
        .patch
        .iter()
        .filter_map(|patch| patch_interface(patch))
        .chain(
            installed
                .iter()
                .flat_map(|a| a.toc.interface.iter().copied()),
        )
        .any(|interface| fits(flavor, interface))
}

impl<T: PublicTransport> Source for TukuiSource<T> {
    fn kind(&self) -> SourceKind {
        TUKUI
    }

    fn check(
        &self,
        installation: &Installation,
        offered: &[InstalledAddon],
        records: &ProjectRecords,
    ) -> Result<Vec<AddonUpdate>, String> {
        let addons = self.fetch()?;
        let by_folder: HashMap<String, &InstalledAddon> = offered
            .iter()
            .map(|a| (a.folder.to_lowercase(), a))
            .collect();
        let mut updates = Vec::new();
        for addon in &addons {
            let key = addon.id.to_string();
            let record = records.get(&key);
            let present: Vec<&InstalledAddon> = addon
                .directories
                .iter()
                .filter_map(|dir| by_folder.get(&dir.to_lowercase()).copied())
                .collect();
            let all_present =
                !addon.directories.is_empty() && present.len() == addon.directories.len();
            if !all_present && (record.is_none() || present.is_empty()) {
                continue;
            }
            let installed_version = record.map(|r| r.file.clone()).or_else(|| {
                present
                    .iter()
                    .find(|a| a.folder.eq_ignore_ascii_case(&addon.directories[0]))
                    .or(present.first())
                    .and_then(|a| a.toc.version.clone())
            });
            let status = if !compatible(addon, &installation.flavor, &present) {
                UpdateStatus::Error {
                    message: format!("Tukui does not list {} for this game version.", addon.name),
                }
            } else if installed_version
                .as_deref()
                .is_some_and(|v| normalized(v) == normalized(&addon.version))
            {
                UpdateStatus::UpToDate
            } else {
                UpdateStatus::UpdateAvailable {
                    latest: summary(&addon.version, addon.last_update.as_deref()),
                }
            };
            for folder in &present {
                updates.push(AddonUpdate {
                    folder: folder.folder.clone(),
                    source: Some(TUKUI),
                    project: Some(Project {
                        id: 0,
                        source: TUKUI,
                        key: key.clone(),
                        name: addon.name.clone(),
                        url: addon.web_url.clone(),
                        allow_mod_distribution: true,
                    }),
                    installed_file: installed_version.as_deref().map(|v| summary(v, None)),
                    status: status.clone(),
                    version_override: None,
                    project_folders: addon.directories.clone(),
                    installed_modules: Vec::new(),
                    reinstall_file: None,
                    mirror: Default::default(),
                });
            }
        }
        Ok(updates)
    }

    fn update(&self, target: &UpdateTarget<'_>, record: &mut RecordWriter<'_>) -> UpdateOutcome {
        let key = target.key;
        let folders: Vec<&AddonUpdate> = target
            .updates
            .iter()
            .filter(|u| {
                u.project
                    .as_ref()
                    .is_some_and(|p| p.source == TUKUI && p.key == key)
            })
            .collect();
        let Some(project) = folders.first().and_then(|u| u.project.as_ref()) else {
            return target.not_found(TUKUI);
        };
        let name = project.name.as_str();
        // Con `reinstall` (issue #148) un addon aggiornato si riscarica comunque: Tukui elenca una
        // sola versione, quella installata.
        let wanted = |status: &UpdateStatus| match status {
            UpdateStatus::UpdateAvailable { .. } => true,
            UpdateStatus::UpToDate => target.reinstall,
            _ => false,
        };
        if !folders.iter().any(|u| wanted(&u.status)) {
            // Un addon non elencato per questo gioco riporta il motivo, non "aggiornato".
            let reason = folders.iter().find_map(|u| match &u.status {
                UpdateStatus::Error { message } => Some(message.as_str()),
                _ => None,
            });
            return UpdateOutcome::skipped(
                TUKUI,
                key,
                name,
                reason.unwrap_or("Already up to date."),
            );
        }
        let addon = match self.cached() {
            Ok(addons) => addons.into_iter().find(|a| a.id.to_string() == key),
            Err(error) => return UpdateOutcome::error(TUKUI, key, name, error),
        };
        let Some(addon) = addon else {
            return UpdateOutcome::skipped(TUKUI, key, name, "Tukui no longer lists this addon.");
        };
        let reporter = Reporter::new(target.progress, Some(TUKUI), key, name);
        reporter.download(0, None);
        let bytes = match self.download(&addon, &mut |received, total| {
            reporter.download(received, total)
        }) {
            Ok(bytes) => bytes,
            Err(error) => return UpdateOutcome::error(TUKUI, key, name, error),
        };
        // Le cartelle del progetto sul disco: quelle che il nuovo zip non ha più si tolgono, come
        // per CurseForge, invece di restare come addon sconosciuti.
        let old_folders: Vec<String> = folders.iter().map(|u| u.folder.clone()).collect();
        reporter.downloaded(bytes.len() as u64);
        let installed = match updates::install_bytes(
            &bytes,
            target.addons_dir,
            &old_folders,
            &|_| Ok(()),
            &reporter,
        ) {
            Ok(folders) => folders,
            Err(error) => return UpdateOutcome::error(TUKUI, key, name, error),
        };
        // Le cartelle sono già sostituite: se il registro non si salva l'aggiornamento resta
        // riuscito, e il messaggio lo dice.
        let message = record(
            TUKUI,
            key,
            SourceRecord {
                file: addon.version.clone(),
                folders: installed,
            },
        )
        .err()
        .map(|error| format!("Installed, but the app could not remember its folders: {error}"));
        UpdateOutcome::updated(
            TUKUI,
            key,
            name,
            summary(&addon.version, addon.last_update.as_deref()),
            message,
        )
    }
}

/// Un file Tukui: nessun id numerico, la versione come nome (vedi `FileSummary`).
fn summary(version: &str, date: Option<&str>) -> FileSummary {
    FileSummary {
        id: 0,
        name: version.to_owned(),
        date: date.unwrap_or_default().to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::addons::toc::TocInfo;
    use crate::progress;
    use crate::sources::curseforge::{BytesResponse, HttpResponse};
    use crate::store::SourceRecords;
    use crate::test_support::TempDir;
    use std::fs;
    use std::io::{Cursor, Write};
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;

    const DOWNLOAD: &str = "https://api.tukui.org/v1/download/elvui/token";

    /// La risposta reale del 2026-10-25, ridotta ai campi usati.
    fn body(elvui_version: &str) -> String {
        serde_json::json!([
            {
                "id": -2, "slug": "elvui", "name": "ElvUI", "url": DOWNLOAD,
                "version": elvui_version, "patch": ["12.1.0", "5.5.4", "1.15.9", "2.5.6"],
                "last_update": "2026-10-13", "web_url": "https://tukui.org/elvui",
                "directories": ["ElvUI", "ElvUI_Libraries", "ElvUI_Options"]
            },
            {
                "id": -1, "slug": "tukui", "name": "Tukui",
                "url": "https://api.tukui.org/v1/download/tukui/token",
                "version": "20.463", "patch": ["11.1.7", "5.5.0", "1.15.7"],
                "last_update": "2025-12-05", "web_url": "https://tukui.org/tukui",
                "directories": ["Tukui"]
            }
        ])
        .to_string()
    }

    #[derive(Default)]
    struct Fake {
        json: Option<Result<HttpResponse, TransportError>>,
        zip: Option<Vec<u8>>,
        calls: Arc<Mutex<Vec<String>>>,
    }

    impl PublicTransport for Fake {
        fn get(&self, url: &str) -> Result<HttpResponse, TransportError> {
            self.calls.lock().unwrap().push(url.to_owned());
            match &self.json {
                Some(Ok(response)) => Ok(HttpResponse {
                    status: response.status,
                    body: response.body.clone(),
                }),
                Some(Err(error)) => Err(*error),
                None => Err(TransportError::Connect),
            }
        }

        fn get_bytes(&self, url: &str, _max_len: u64) -> Result<BytesResponse, TransportError> {
            self.calls.lock().unwrap().push(url.to_owned());
            match &self.zip {
                Some(body) => Ok(BytesResponse {
                    status: 200,
                    body: body.clone(),
                }),
                None => Ok(BytesResponse {
                    status: 404,
                    body: Vec::new(),
                }),
            }
        }
    }

    fn source(version: &str) -> TukuiSource<Fake> {
        TukuiSource::new(Fake {
            json: Some(Ok(HttpResponse {
                status: 200,
                body: body(version),
            })),
            ..Fake::default()
        })
    }

    fn installation(flavor: &str) -> Installation {
        Installation {
            id: "test".to_owned(),
            path: PathBuf::from("unused"),
            flavor: flavor.to_owned(),
            name: "Test".to_owned(),
        }
    }

    fn addon(folder: &str, version: Option<&str>, interface: &[u32]) -> InstalledAddon {
        InstalledAddon {
            folder: folder.to_owned(),
            toc_file: Some(format!("{folder}.toc")),
            toc: TocInfo {
                version: version.map(str::to_owned),
                interface: interface.to_vec(),
                ..TocInfo::default()
            },
            modified_ms: None,
            fingerprint: Some(1),
            error: None,
        }
    }

    fn elvui(version: &str) -> Vec<InstalledAddon> {
        vec![
            addon("ElvUI", Some(version), &[120100]),
            addon("elvui_libraries", None, &[120100]),
            addon("ElvUI_Options", None, &[120100]),
            addon("Other", Some("1"), &[120100]),
        ]
    }

    fn statuses(updates: &[AddonUpdate]) -> Vec<(&str, &str, &UpdateStatus)> {
        updates
            .iter()
            .map(|u| {
                (
                    u.folder.as_str(),
                    u.project.as_ref().unwrap().key.as_str(),
                    &u.status,
                )
            })
            .collect()
    }

    #[test]
    fn matches_every_directory_and_ignores_a_leading_v() {
        let updates = source("16.00")
            .check(
                &installation("_retail_"),
                &elvui("v16.00"),
                &ProjectRecords::new(),
            )
            .unwrap();
        assert_eq!(
            statuses(&updates),
            [
                ("ElvUI", "-2", &UpdateStatus::UpToDate),
                ("elvui_libraries", "-2", &UpdateStatus::UpToDate),
                ("ElvUI_Options", "-2", &UpdateStatus::UpToDate),
            ]
        );
        let project = updates[0].project.as_ref().unwrap();
        assert_eq!(project.source, SourceKind::Tukui);
        assert_eq!(project.id, 0);
        assert_eq!(project.url.as_deref(), Some("https://tukui.org/elvui"));
        assert_eq!(
            updates[0].project_folders,
            ["ElvUI", "ElvUI_Libraries", "ElvUI_Options"]
        );
    }

    #[test]
    fn a_different_version_is_an_update() {
        let updates = source("16.01")
            .check(
                &installation("_retail_"),
                &elvui("v16.00"),
                &ProjectRecords::new(),
            )
            .unwrap();
        assert_eq!(
            updates[0].status,
            UpdateStatus::UpdateAvailable {
                latest: summary("16.01", Some("2026-10-13"))
            }
        );
        assert_eq!(updates[0].installed_file, Some(summary("v16.00", None)));
    }

    #[test]
    fn a_missing_directory_is_not_a_match() {
        let mut addons = elvui("v16.00");
        addons.retain(|a| a.folder != "ElvUI_Options");
        let updates = source("16.00")
            .check(&installation("_retail_"), &addons, &ProjectRecords::new())
            .unwrap();
        assert!(updates.is_empty());
    }

    #[test]
    fn the_record_gives_the_installed_version() {
        let records = ProjectRecords::from([(
            "-2".to_owned(),
            SourceRecord {
                file: "16.01".to_owned(),
                folders: vec!["ElvUI".to_owned()],
            },
        )]);
        // Senza `ElvUI_Options` sul disco il record basta ad abbinare le cartelle presenti.
        let mut addons = elvui("v15.00");
        addons.retain(|a| a.folder != "ElvUI_Options");
        let updates = source("16.01")
            .check(&installation("_retail_"), &addons, &records)
            .unwrap();
        assert_eq!(updates.len(), 2);
        assert_eq!(updates[0].status, UpdateStatus::UpToDate);
    }

    #[test]
    fn compatibility_comes_from_patch_or_the_installed_toc() {
        let tukui = vec![addon("Tukui", Some("20.463"), &[16001])];
        // `patch` di Tukui non elenca Anniversary (2.x): nessun aggiornamento proposto.
        let no_patch = vec![addon("Tukui", Some("20.000"), &[])];
        let updates = source("16.00")
            .check(
                &installation("_anniversary_"),
                &no_patch,
                &ProjectRecords::new(),
            )
            .unwrap();
        assert!(matches!(updates[0].status, UpdateStatus::Error { .. }));
        // WoW Forever non è in `patch`, ma il `.toc` installato lo dichiara.
        let updates = source("16.00")
            .check(
                &installation("_classic_beta_"),
                &tukui,
                &ProjectRecords::new(),
            )
            .unwrap();
        assert_eq!(updates[0].status, UpdateStatus::UpToDate);
        // Classic Era è in `patch` (1.15.7).
        let updates = source("16.00")
            .check(
                &installation("_classic_era_"),
                &no_patch,
                &ProjectRecords::new(),
            )
            .unwrap();
        assert!(matches!(
            updates[0].status,
            UpdateStatus::UpdateAvailable { .. }
        ));
    }

    #[test]
    fn interface_families() {
        assert_eq!(patch_interface("12.1.0"), Some(120_100));
        assert_eq!(patch_interface("1.15.9"), Some(11_509));
        assert_eq!(patch_interface("5.5"), Some(50_500));
        assert_eq!(patch_interface("x"), None);
        assert_eq!(patch_interface("500000.0.0"), None);
        assert_eq!(patch_interface("1.99999999999.0"), None);
        assert!(fits("_retail_", 120_100));
        assert!(fits("_classic_era_", 11_509));
        assert!(!fits("_classic_era_", 16_001));
        assert!(fits("_classic_beta_", 16_001));
        assert!(fits("_anniversary_", 20_506));
        assert!(fits("_classic_", 50_504));
        assert!(!fits("_ptr_", 120_100));
    }

    #[test]
    fn errors_belong_to_tukui() {
        let failing = |json| {
            TukuiSource::new(Fake {
                json: Some(json),
                ..Fake::default()
            })
        };
        let check = |source: TukuiSource<Fake>| {
            source.check(
                &installation("_retail_"),
                &elvui("v16.00"),
                &ProjectRecords::new(),
            )
        };
        assert_eq!(
            check(failing(Err(TransportError::Timeout))),
            Err("Tukui did not respond in time.".to_owned())
        );
        assert_eq!(
            check(failing(Ok(HttpResponse {
                status: 503,
                body: String::new()
            }))),
            Err("Tukui responded with an error (HTTP 503).".to_owned())
        );
        assert_eq!(
            check(failing(Ok(HttpResponse {
                status: 200,
                body: "{".to_owned()
            }))),
            Err("Unrecognized response from Tukui.".to_owned())
        );
        assert_eq!(
            check(failing(Err(TransportError::TooLarge))),
            Err("Unrecognized response from Tukui: it is too large.".to_owned())
        );
    }

    // ── Aggiornamento ──

    fn zip_of(files: &[(&str, &str)]) -> Vec<u8> {
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        for (name, text) in files {
            writer
                .start_file(*name, SimpleFileOptions::default())
                .unwrap();
            writer.write_all(text.as_bytes()).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    fn write_addon(addons: &Path, folder: &str, toc: &str) {
        fs::create_dir_all(addons.join(folder)).unwrap();
        fs::write(addons.join(folder).join(format!("{folder}.toc")), toc).unwrap();
    }

    #[test]
    fn updates_download_and_record_the_new_version() {
        let tmp = TempDir::new();
        let path = tmp.path().join("World of Warcraft").join("_retail_");
        let addons = path.join("Interface").join("AddOns");
        for folder in ["ElvUI", "ElvUI_Libraries", "ElvUI_Options"] {
            write_addon(
                &addons,
                folder,
                "## Interface: 120100\n## Version: v16.00\n",
            );
        }
        let installation = Installation {
            id: "test".to_owned(),
            path,
            flavor: "retail".to_owned(),
            name: "Retail".to_owned(),
        };
        let calls = Arc::new(Mutex::new(Vec::new()));
        let tukui = TukuiSource::new(Fake {
            json: Some(Ok(HttpResponse {
                status: 200,
                body: body("16.01"),
            })),
            zip: Some(zip_of(&[
                ("ElvUI/ElvUI.toc", "## Version: v16.01\n"),
                ("ElvUI_Libraries/ElvUI_Libraries.toc", ""),
                ("ElvUI_Options/ElvUI_Options.toc", ""),
            ])),
            calls: Arc::clone(&calls),
        });
        let mut written = Vec::new();
        let collected = progress::tests::Collected::default();
        let outcomes = updates::update_addons(
            &installation,
            &[&tukui],
            &SourceRecords::new(),
            &[super::super::ProjectKey {
                source: TUKUI,
                key: "-2".to_owned(),
            }],
            &mut |source, key, record| {
                written.push((source, key.to_owned(), record));
                Ok(())
            },
            &collected.sink(),
        )
        .unwrap();
        use progress::Phase::*;
        let key = || "-2".to_owned();
        assert_eq!(
            collected.phases(),
            [
                (key(), Checking),
                (key(), Downloading),
                (key(), Extracting),
                (key(), Installing)
            ]
        );
        let (received, total) = collected.last_download("-2").unwrap();
        assert!(received.is_some() && received == total);
        assert!(collected.0.borrow()[1..].iter().all(|p| p.name == "ElvUI"));
        assert_eq!(outcomes.len(), 1);
        assert_eq!(
            outcomes[0].status,
            updates::OutcomeStatus::Updated,
            "{outcomes:?}"
        );
        assert_eq!(outcomes[0].file, Some(summary("16.01", Some("2026-10-13"))));
        assert_eq!(
            fs::read_to_string(addons.join("ElvUI").join("ElvUI.toc")).unwrap(),
            "## Version: v16.01\n"
        );
        assert_eq!(
            written,
            [(
                TUKUI,
                "-2".to_owned(),
                SourceRecord {
                    file: "16.01".to_owned(),
                    folders: vec![
                        "ElvUI".to_owned(),
                        "ElvUI_Libraries".to_owned(),
                        "ElvUI_Options".to_owned()
                    ],
                }
            )]
        );
        // L'elenco si legge una volta: l'aggiornamento riusa quello del controllo.
        assert_eq!(*calls.lock().unwrap(), [ADDONS_URL, DOWNLOAD]);
    }

    /// Issue #148: con `reinstall` un ElvUI già aggiornato si riscarica comunque.
    #[test]
    fn reinstall_downloads_the_installed_version_again() {
        let tmp = TempDir::new();
        let path = tmp.path().join("World of Warcraft").join("_retail_");
        let addons = path.join("Interface").join("AddOns");
        for folder in ["ElvUI", "ElvUI_Libraries", "ElvUI_Options"] {
            write_addon(
                &addons,
                folder,
                "## Interface: 120100\n## Version: v16.01\n",
            );
        }
        // Un file estraneo che la reinstallazione toglie.
        fs::write(addons.join("ElvUI").join("Junk.lua"), "junk").unwrap();
        let installation = Installation {
            id: "test".to_owned(),
            path,
            flavor: "retail".to_owned(),
            name: "Retail".to_owned(),
        };
        let calls = Arc::new(Mutex::new(Vec::new()));
        let tukui = TukuiSource::new(Fake {
            json: Some(Ok(HttpResponse {
                status: 200,
                body: body("16.01"),
            })),
            zip: Some(zip_of(&[
                ("ElvUI/ElvUI.toc", "## Version: v16.01\n"),
                ("ElvUI_Libraries/ElvUI_Libraries.toc", ""),
                ("ElvUI_Options/ElvUI_Options.toc", ""),
            ])),
            calls: Arc::clone(&calls),
        });
        let projects = [super::super::ProjectKey {
            source: TUKUI,
            key: "-2".to_owned(),
        }];
        let run = |reinstall: bool, written: &mut Vec<SourceRecord>| {
            updates::update_addons_with(
                &installation,
                None,
                &[&tukui],
                &SourceRecords::new(),
                &projects,
                reinstall,
                &mut |_, _, record| {
                    written.push(record);
                    Ok(())
                },
                &progress::ignore,
            )
            .unwrap()
            .remove(0)
        };
        let downloads = || {
            calls
                .lock()
                .unwrap()
                .iter()
                .filter(|c| *c == DOWNLOAD)
                .count()
        };

        let mut written = Vec::new();
        let plain = run(false, &mut written);
        assert_eq!(plain.status, updates::OutcomeStatus::Skipped);
        assert_eq!(plain.message.as_deref(), Some("Already up to date."));
        assert_eq!(downloads(), 0);

        let again = run(true, &mut written);
        assert_eq!(again.status, updates::OutcomeStatus::Updated, "{again:?}");
        assert_eq!(again.file, Some(summary("16.01", Some("2026-10-13"))));
        assert_eq!(downloads(), 1);
        assert_eq!(written.len(), 1);
        assert_eq!(written[0].file, "16.01");
        assert!(!addons.join("ElvUI").join("Junk.lua").exists());
    }

    #[test]
    fn a_download_outside_tukui_is_refused() {
        let addon = TukuiAddon {
            id: -2,
            name: "ElvUI".to_owned(),
            url: "https://example.com/elvui.zip".to_owned(),
            version: "1".to_owned(),
            patch: Vec::new(),
            web_url: None,
            last_update: None,
            directories: vec!["ElvUI".to_owned()],
        };
        let error = source("1").download(&addon, &mut |_, _| {}).unwrap_err();
        assert!(error.contains("unrecognized download address"), "{error}");
    }

    /// Legge l'API vera: `cargo test --locked live_tukui_addons -- --ignored`.
    #[test]
    #[ignore = "needs the network"]
    fn live_tukui_addons() {
        let transport = crate::sources::http::PublicReqwestTransport::new(&DOMAINS).unwrap();
        let addons = TukuiSource::new(transport).fetch().unwrap();
        let elvui = addons.iter().find(|a| a.id == -2).expect("ElvUI");
        assert_eq!(elvui.name, "ElvUI");
        assert!(elvui.directories.iter().any(|d| d == "ElvUI"));
        assert!(reqwest::Url::parse(&elvui.url).is_ok_and(|url| is_allowed_url(&url, &DOMAINS)));
    }

    #[test]
    fn a_malformed_entry_does_not_hide_the_others() {
        let mut entries: Vec<serde_json::Value> = serde_json::from_str(&body("16.00")).unwrap();
        // Una voce senza `name` e una senza `directories`.
        entries.push(serde_json::json!({"id": -3, "url": "x", "version": "1"}));
        entries.push(serde_json::json!({"id": -4, "name": "New", "url": "x", "version": "1"}));
        let source = TukuiSource::new(Fake {
            json: Some(Ok(HttpResponse {
                status: 200,
                body: serde_json::Value::Array(entries).to_string(),
            })),
            ..Fake::default()
        });
        let updates = source
            .check(
                &installation("_retail_"),
                &elvui("v16.00"),
                &ProjectRecords::new(),
            )
            .unwrap();
        assert_eq!(updates.len(), 3);
    }

    #[test]
    fn an_incompatible_addon_reports_why_it_is_not_updated() {
        let tmp = TempDir::new();
        let path = tmp.path().join("World of Warcraft").join("_anniversary_");
        let addons = path.join("Interface").join("AddOns");
        write_addon(&addons, "Tukui", "## Version: 20.000\n");
        let installation = Installation {
            id: "test".to_owned(),
            path,
            flavor: "anniversary".to_owned(),
            name: "Anniversary".to_owned(),
        };
        let outcomes = updates::update_addons(
            &installation,
            &[&source("16.00")],
            &SourceRecords::new(),
            &[super::super::ProjectKey {
                source: TUKUI,
                key: "-1".to_owned(),
            }],
            &mut |_, _, _| Ok(()),
            &progress::ignore,
        )
        .unwrap();
        assert_eq!(
            outcomes[0].message.as_deref(),
            Some("Tukui does not list Tukui for this game version.")
        );
    }

    #[test]
    fn a_folder_the_new_zip_drops_is_removed() {
        let tmp = TempDir::new();
        let path = tmp.path().join("World of Warcraft").join("_retail_");
        let addons = path.join("Interface").join("AddOns");
        for folder in ["ElvUI", "ElvUI_Libraries", "ElvUI_Options"] {
            write_addon(
                &addons,
                folder,
                "## Interface: 120100\n## Version: v16.00\n",
            );
        }
        write_addon(&addons, "Other", "## Interface: 120100\n");
        let installation = Installation {
            id: "test".to_owned(),
            path,
            flavor: "retail".to_owned(),
            name: "Retail".to_owned(),
        };
        let tukui = TukuiSource::new(Fake {
            json: Some(Ok(HttpResponse {
                status: 200,
                body: body("16.01"),
            })),
            zip: Some(zip_of(&[
                ("ElvUI/ElvUI.toc", "## Version: v16.01\n"),
                ("ElvUI_Libraries/ElvUI_Libraries.toc", ""),
            ])),
            ..Fake::default()
        });
        updates::update_addons(
            &installation,
            &[&tukui],
            &SourceRecords::new(),
            &[super::super::ProjectKey {
                source: TUKUI,
                key: "-2".to_owned(),
            }],
            &mut |_, _, _| Ok(()),
            &progress::ignore,
        )
        .unwrap();
        let mut names: Vec<String> = fs::read_dir(&addons)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        assert_eq!(names, ["ElvUI", "ElvUI_Libraries", "Other"]);
    }

    // ── Installazione di un progetto nuovo (import, issue #134) ──

    fn new_install(
        flavor: &str,
        toc: &str,
        key: &str,
    ) -> (TempDir, PathBuf, UpdateOutcome, Vec<String>) {
        let tmp = TempDir::new();
        let path = tmp.path().join("World of Warcraft").join(flavor);
        let addons = path.join("Interface").join("AddOns");
        fs::create_dir_all(&addons).unwrap();
        let tukui = TukuiSource::new(Fake {
            json: Some(Ok(HttpResponse {
                status: 200,
                body: body("16.00"),
            })),
            zip: Some(zip_of(&[
                ("ElvUI/ElvUI.toc", toc),
                ("ElvUI_Options/ElvUI_Options.toc", toc),
            ])),
            ..Fake::default()
        });
        let mut written = Vec::new();
        let outcome = tukui.install_new(
            &Installation {
                id: "test".to_owned(),
                path,
                flavor: flavor.trim_matches('_').to_owned(),
                name: "Test".to_owned(),
            },
            key,
            &|_| Ok(()),
            &mut |source, key, record| {
                assert_eq!(source, TUKUI);
                written.push(format!(
                    "{key}={}:{}",
                    record.file,
                    record.folders.join(",")
                ));
                Ok(())
            },
            &progress::ignore,
        );
        (tmp, addons, outcome, written)
    }

    #[test]
    fn a_new_project_listed_for_the_game_is_installed_and_recorded() {
        let (_tmp, addons, outcome, written) =
            new_install("_retail_", "## Interface: 120100\n", "-2");
        assert_eq!(
            outcome.status,
            updates::OutcomeStatus::Updated,
            "{outcome:?}"
        );
        assert_eq!(outcome.name, "ElvUI");
        assert!(addons.join("ElvUI").join("ElvUI.toc").is_file());
        assert_eq!(written, ["-2=16.00:ElvUI,ElvUI_Options"]);
    }

    #[test]
    fn a_new_project_counts_for_forever_when_its_toc_declares_it() {
        // `patch` non elenca WoW Forever: decide il `.toc` dello zip.
        let (_tmp, addons, outcome, _) =
            new_install("_classic_beta_", "## Interface: 11508, 16001\n", "-2");
        assert_eq!(
            outcome.status,
            updates::OutcomeStatus::Updated,
            "{outcome:?}"
        );
        assert!(addons.join("ElvUI_Options").is_dir());
    }

    #[test]
    fn a_new_project_for_another_game_is_skipped_and_writes_nothing() {
        let (_tmp, addons, outcome, written) =
            new_install("_classic_beta_", "## Interface: 120100\n", "-2");
        assert_eq!(outcome.status, updates::OutcomeStatus::Skipped);
        assert_eq!(
            outcome.message.as_deref(),
            Some("Tukui does not offer this addon for this game version.")
        );
        assert!(!addons.join("ElvUI").exists());
        assert!(written.is_empty());
        // Nemmeno la cartella di appoggio resta.
        assert_eq!(fs::read_dir(addons.parent().unwrap()).unwrap().count(), 1);
    }

    #[test]
    fn an_unlisted_project_is_skipped() {
        let (_tmp, _addons, outcome, _) = new_install("_retail_", "## Interface: 120100\n", "-9");
        assert_eq!(outcome.status, updates::OutcomeStatus::Skipped);
        assert_eq!(
            outcome.message.as_deref(),
            Some("Tukui does not list this addon.")
        );
    }
}
