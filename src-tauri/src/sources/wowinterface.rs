//! WoWInterface come fonte (D31): l'API pubblica MMOUI, senza chiave.
//!
//! `GET https://api.mmoui.com/v4/game/WOW/filedetails/{id}.json` risponde con un array di file;
//! verificato il 2026-10-26 per l'id 5108 (Clique): `id`, `version` (`v5.0.15-release`),
//! `lastUpdate` (millisecondi Unix), `fileName`, `downloadUri` (su `cdn.wowinterface.com`),
//! `title`, `author`, `checksum`. La stessa richiesta accetta più id separati da virgola
//! (`5108,23056.json`) e restituisce solo quelli che esistono; se non ne esiste nessuno risponde
//! `404` con `{"ERROR":"No AddOn found."}`. Il `filelist.json` completo (circa 2,7 MB) non si
//! scarica mai: il controllo chiede solo gli id installati.
//!
//! Una cartella appartiene a un progetto WoWInterface se un record della fonte la nomina,
//! oppure se il `.toc` del flavor dichiara `## X-WoWI-ID`; più cartelle con lo stesso id sono un
//! solo progetto. Le cartelle già reclamate da CurseForge non arrivano qui (D33). La versione
//! installata è quella del record, altrimenti `## Version` del `.toc`; c'è un aggiornamento
//! quando è diversa da `version`, senza contare una `v` iniziale.

use std::collections::{BTreeSet, HashMap};
use std::sync::Mutex;
use std::time::{Duration, UNIX_EPOCH};

use serde::Deserialize;

use super::curseforge::{MAX_DOWNLOAD_BYTES, TransportError};
use super::http::{DownloadProgress, PublicTransport, is_allowed_url};
use super::{RecordWriter, Source, SourceKind, UpdateTarget};
use crate::addons::scan::InstalledAddon;
use crate::backup;
use crate::installations::Installation;
use crate::progress::Reporter;
use crate::store::{ProjectRecords, SourceRecord};
use crate::updates::{self, AddonUpdate, FileSummary, Project, UpdateOutcome, UpdateStatus};

pub const DETAILS_URL: &str = "https://api.mmoui.com/v4/game/WOW/filedetails/";
/// I domini dell'API e degli zip, con i loro sottodomini.
pub const DOMAINS: [&str; 2] = ["mmoui.com", "wowinterface.com"];
/// Gli id chiesti in una sola richiesta: l'URL resta corto anche con molti addon.
const IDS_PER_REQUEST: usize = 50;

const WOWI: SourceKind = SourceKind::Wowinterface;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WowiFile {
    pub id: u64,
    pub title: String,
    pub version: String,
    pub download_uri: String,
    #[serde(default)]
    pub last_update: Option<i64>,
}

/// La pagina del progetto sul sito.
fn page_url(key: &str) -> String {
    format!("https://www.wowinterface.com/downloads/info{key}")
}

/// La fonte WoWInterface. Le risposte del controllo restano per l'aggiornamento che lo segue
/// (`update_addons` rifà il controllo e poi aggiorna).
pub struct WowInterfaceSource<T: PublicTransport> {
    transport: T,
    files: Mutex<HashMap<String, WowiFile>>,
}

impl<T: PublicTransport> WowInterfaceSource<T> {
    pub fn new(transport: T) -> Self {
        Self {
            transport,
            files: Mutex::new(HashMap::new()),
        }
    }

    /// Chiede i file degli id `ids`, a gruppi di [`IDS_PER_REQUEST`]. Gli id che WoWInterface
    /// non conosce mancano dal risultato.
    fn fetch(&self, ids: &[String]) -> Result<HashMap<String, WowiFile>, String> {
        let mut found = HashMap::new();
        for chunk in ids.chunks(IDS_PER_REQUEST) {
            let url = format!("{DETAILS_URL}{}.json", chunk.join(","));
            let response = self.transport.get(&url).map_err(|error| match error {
                TransportError::TooLarge => {
                    "Unrecognized response from WoWInterface: it is too large.".to_owned()
                }
                other => network_error(other),
            })?;
            if response.status == 404 && is_not_found(&response.body) {
                continue;
            }
            if response.status != 200 {
                return Err(format!(
                    "WoWInterface responded with an error (HTTP {}).",
                    response.status
                ));
            }
            // Una voce che non si legge si salta: non deve togliere gli altri progetti. Una
            // risposta che non è un elenco resta un errore.
            let entries: Vec<serde_json::Value> = serde_json::from_str(&response.body)
                .map_err(|_| "Unrecognized response from WoWInterface.".to_owned())?;
            for file in entries
                .into_iter()
                .filter_map(|entry| serde_json::from_value::<WowiFile>(entry).ok())
            {
                found.insert(file.id.to_string(), file);
            }
        }
        self.files
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .extend(found.clone());
        Ok(found)
    }

    /// Il file del progetto `key`: quello del controllo, altrimenti una richiesta nuova.
    fn file(&self, key: &str) -> Result<Option<WowiFile>, String> {
        let cached = self
            .files
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(key)
            .cloned();
        match cached {
            Some(file) => Ok(Some(file)),
            None => Ok(self.fetch(&[key.to_owned()])?.remove(key)),
        }
    }

    fn download(
        &self,
        file: &WowiFile,
        progress: &mut DownloadProgress<'_>,
    ) -> Result<Vec<u8>, String> {
        let allowed =
            reqwest::Url::parse(&file.download_uri).is_ok_and(|url| is_allowed_url(&url, &DOMAINS));
        if !allowed {
            return Err(
                "WoWInterface gave an unrecognized download address: download cancelled.".into(),
            );
        }
        let response = self
            .transport
            .get_bytes_with_progress(&file.download_uri, MAX_DOWNLOAD_BYTES, progress)
            .map_err(|error| match error {
                TransportError::TooLarge => format!(
                    "The file to download exceeds the {} MiB limit: download cancelled.",
                    MAX_DOWNLOAD_BYTES / (1024 * 1024)
                ),
                other => network_error(other),
            })?;
        if response.status != 200 {
            return Err(format!(
                "WoWInterface responded with an error (HTTP {}).",
                response.status
            ));
        }
        Ok(response.body)
    }
}

/// La risposta di un id che non esiste: `404` con un oggetto `ERROR`.
fn is_not_found(body: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(body)
        .is_ok_and(|value| value.get("ERROR").is_some_and(|e| e.is_string()))
}

fn network_error(error: TransportError) -> String {
    match error {
        TransportError::Timeout => "WoWInterface did not respond in time.".to_owned(),
        _ => "Could not reach WoWInterface: check the connection.".to_owned(),
    }
}

/// Un id WoWInterface valido: un numero positivo, senza spazi intorno.
fn project_id(text: &str) -> Option<String> {
    text.trim()
        .parse::<u64>()
        .ok()
        .filter(|&id| id > 0)
        .map(|id| id.to_string())
}

/// La versione senza spazi e senza una `v` iniziale: `v5.0.15` e `5.0.15` sono la stessa.
fn normalized(version: &str) -> &str {
    let version = version.trim();
    version
        .strip_prefix(['v', 'V'])
        .filter(|rest| rest.starts_with(|c: char| c.is_ascii_digit()))
        .unwrap_or(version)
}

/// Un file WoWInterface: nessun id numerico, la versione come nome (vedi `FileSummary`), la
/// data da `lastUpdate` in UTC.
fn summary(version: &str, last_update: Option<i64>) -> FileSummary {
    let date = last_update
        .and_then(|ms| u64::try_from(ms).ok())
        .and_then(|ms| UNIX_EPOCH.checked_add(Duration::from_millis(ms)))
        .map(backup::iso_utc)
        .unwrap_or_default();
    FileSummary {
        id: 0,
        name: version.to_owned(),
        date,
    }
}

/// Il progetto di ogni cartella offerta: quello del record che la nomina, altrimenti
/// `## X-WoWI-ID` del suo `.toc`.
fn assign(offered: &[InstalledAddon], records: &ProjectRecords) -> Vec<Option<String>> {
    offered
        .iter()
        .map(|addon| {
            records
                .iter()
                .find(|(_, record)| {
                    record
                        .folders
                        .iter()
                        .any(|folder| folder.eq_ignore_ascii_case(&addon.folder))
                })
                .and_then(|(key, _)| project_id(key))
                .or_else(|| addon.toc.wowi_id.as_deref().and_then(project_id))
        })
        .collect()
}

impl<T: PublicTransport> Source for WowInterfaceSource<T> {
    fn kind(&self) -> SourceKind {
        WOWI
    }

    fn check(
        &self,
        _installation: &Installation,
        offered: &[InstalledAddon],
        records: &ProjectRecords,
    ) -> Result<Vec<AddonUpdate>, String> {
        let keys = assign(offered, records);
        let ids: Vec<String> = keys
            .iter()
            .flatten()
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let files = self.fetch(&ids)?;
        // Id → le cartelle offerte del progetto, nell'ordine della scansione.
        let mut groups: HashMap<&str, Vec<&InstalledAddon>> = HashMap::new();
        for (addon, key) in offered.iter().zip(&keys) {
            if let Some(key) = key {
                groups.entry(key.as_str()).or_default().push(addon);
            }
        }
        let mut updates = Vec::new();
        for (addon, key) in offered.iter().zip(&keys) {
            let Some(key) = key else { continue };
            let Some(file) = files.get(key) else {
                // Non reclama la cartella: resta alle altre fonti, e l'errore si vede solo se
                // nessuna la riconosce.
                updates.push(AddonUpdate {
                    folder: addon.folder.clone(),
                    source: None,
                    project: None,
                    installed_file: None,
                    status: UpdateStatus::Error {
                        message: format!("WoWInterface does not list the addon with id {key}."),
                    },
                    version_override: None,
                    project_folders: Vec::new(),
                    installed_modules: Vec::new(),
                });
                continue;
            };
            let group = &groups[key.as_str()];
            let record = records.get(key);
            // Senza record conta il `.toc` di qualunque cartella del progetto che ha già la
            // versione della risposta: un modulo con una `## Version` propria non deve
            // proporre per sempre un aggiornamento. Altrimenti la prima versione dichiarata,
            // prima quelle delle cartelle con il tag.
            let installed_version = record.map(|r| r.file.clone()).or_else(|| {
                let versions = || group.iter().filter_map(|a| a.toc.version.as_deref());
                versions()
                    .find(|v| normalized(v) == normalized(&file.version))
                    .or_else(|| {
                        group
                            .iter()
                            .filter(|a| {
                                a.toc.wowi_id.as_deref().and_then(project_id).as_ref() == Some(key)
                            })
                            .find_map(|a| a.toc.version.as_deref())
                    })
                    .or_else(|| versions().next())
                    .map(str::to_owned)
            });
            let status = if installed_version
                .as_deref()
                .is_some_and(|v| normalized(v) == normalized(&file.version))
            {
                UpdateStatus::UpToDate
            } else {
                UpdateStatus::UpdateAvailable {
                    latest: summary(&file.version, file.last_update),
                }
            };
            let project_folders = match record {
                Some(record) => record.folders.clone(),
                None => group.iter().map(|a| a.folder.clone()).collect(),
            };
            updates.push(AddonUpdate {
                folder: addon.folder.clone(),
                source: Some(WOWI),
                project: Some(Project {
                    id: 0,
                    source: WOWI,
                    key: key.clone(),
                    name: file.title.clone(),
                    url: Some(page_url(key)),
                    allow_mod_distribution: true,
                }),
                installed_file: installed_version.as_deref().map(|v| summary(v, None)),
                status,
                version_override: None,
                project_folders,
                installed_modules: Vec::new(),
            });
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
                    .is_some_and(|p| p.source == WOWI && p.key == key)
            })
            .collect();
        let Some(project) = folders.first().and_then(|u| u.project.as_ref()) else {
            return UpdateOutcome::skipped(WOWI, key, "", "Not found among the installed addons.");
        };
        let name = project.name.as_str();
        if !folders
            .iter()
            .any(|u| matches!(u.status, UpdateStatus::UpdateAvailable { .. }))
        {
            return UpdateOutcome::skipped(WOWI, key, name, "Already up to date.");
        }
        let file = match self.file(key) {
            Ok(Some(file)) => file,
            Ok(None) => {
                return UpdateOutcome::skipped(
                    WOWI,
                    key,
                    name,
                    "WoWInterface no longer lists this addon.",
                );
            }
            Err(error) => return UpdateOutcome::error(WOWI, key, name, error),
        };
        let reporter = Reporter::new(target.progress, Some(WOWI), key, name);
        reporter.download(0, None);
        let bytes = match self.download(&file, &mut |received, total| {
            reporter.download(received, total)
        }) {
            Ok(bytes) => bytes,
            Err(error) => return UpdateOutcome::error(WOWI, key, name, error),
        };
        // Le cartelle del progetto sul disco: quelle che il nuovo zip non ha più si tolgono,
        // come per le altre fonti, invece di restare come addon sconosciuti.
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
            Err(error) => return UpdateOutcome::error(WOWI, key, name, error),
        };
        // Le cartelle sono già sostituite: se il registro non si salva l'aggiornamento resta
        // riuscito, e il messaggio lo dice.
        let message = record(
            WOWI,
            key,
            SourceRecord {
                file: file.version.clone(),
                folders: installed,
            },
        )
        .err()
        .map(|error| format!("Installed, but the app could not remember its folders: {error}"));
        UpdateOutcome::updated(
            WOWI,
            key,
            name,
            summary(&file.version, file.last_update),
            message,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::addons::toc::TocInfo;
    use crate::progress;
    use crate::sources::ProjectKey;
    use crate::sources::curseforge::{BytesResponse, HttpResponse};
    use crate::store::SourceRecords;
    use crate::test_support::TempDir;
    use std::fs;
    use std::io::{Cursor, Write};
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;

    const DOWNLOAD: &str =
        "https://cdn.wowinterface.com/downloads/getfile.php?id=5108&d=1792849912&minion";

    /// Una voce della risposta reale del 2026-10-26 (id 5108), ridotta: senza `description`,
    /// `changeLog` e immagini.
    fn clique(version: &str) -> serde_json::Value {
        serde_json::json!({
            "id": 5108, "categoryId": 21, "version": version, "lastUpdate": 1792849912000_i64,
            "checksum": "28cea520b35038629c54c85715115338",
            "fileName": "Clique-v5.0.15-release.zip", "downloadUri": DOWNLOAD,
            "pendingUpdate": 0, "title": "Clique", "author": "Cladhaire"
        })
    }

    fn ok(body: String) -> Result<HttpResponse, TransportError> {
        Ok(HttpResponse { status: 200, body })
    }

    #[derive(Default)]
    struct Fake {
        /// La risposta per ogni URL JSON; un URL non previsto non ha connessione.
        json: HashMap<String, Result<HttpResponse, TransportError>>,
        zip: Option<Vec<u8>>,
        calls: Arc<Mutex<Vec<String>>>,
    }

    impl PublicTransport for Fake {
        fn get(&self, url: &str) -> Result<HttpResponse, TransportError> {
            self.calls.lock().unwrap().push(url.to_owned());
            match self.json.get(url) {
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

    fn url(ids: &str) -> String {
        format!("{DETAILS_URL}{ids}.json")
    }

    fn source(
        responses: &[(&str, Result<HttpResponse, TransportError>)],
    ) -> WowInterfaceSource<Fake> {
        WowInterfaceSource::new(Fake {
            json: responses
                .iter()
                .map(|(ids, response)| {
                    let response = match response {
                        Ok(r) => Ok(HttpResponse {
                            status: r.status,
                            body: r.body.clone(),
                        }),
                        Err(e) => Err(*e),
                    };
                    (url(ids), response)
                })
                .collect(),
            ..Fake::default()
        })
    }

    fn clique_source(version: &str) -> WowInterfaceSource<Fake> {
        source(&[("5108", ok(serde_json::json!([clique(version)]).to_string()))])
    }

    fn installation() -> Installation {
        Installation {
            id: "test".to_owned(),
            path: PathBuf::from("unused"),
            flavor: "retail".to_owned(),
            name: "Test".to_owned(),
        }
    }

    fn addon(folder: &str, version: Option<&str>, wowi_id: Option<&str>) -> InstalledAddon {
        InstalledAddon {
            folder: folder.to_owned(),
            toc_file: Some(format!("{folder}.toc")),
            toc: TocInfo {
                version: version.map(str::to_owned),
                wowi_id: wowi_id.map(str::to_owned),
                ..TocInfo::default()
            },
            modified_ms: None,
            fingerprint: Some(1),
            error: None,
        }
    }

    fn statuses(updates: &[AddonUpdate]) -> Vec<(&str, Option<&str>, &UpdateStatus)> {
        updates
            .iter()
            .map(|u| {
                (
                    u.folder.as_str(),
                    u.project.as_ref().map(|p| p.key.as_str()),
                    &u.status,
                )
            })
            .collect()
    }

    #[test]
    fn the_toc_tag_matches_and_an_equal_version_is_up_to_date() {
        let offered = [
            addon("Clique", Some("v5.0.15-release"), Some(" 5108 ")),
            addon("Other", Some("1"), None),
        ];
        let updates = clique_source("v5.0.15-release")
            .check(&installation(), &offered, &ProjectRecords::new())
            .unwrap();
        assert_eq!(
            statuses(&updates),
            [("Clique", Some("5108"), &UpdateStatus::UpToDate)]
        );
        let update = &updates[0];
        assert_eq!(update.source, Some(SourceKind::Wowinterface));
        let project = update.project.as_ref().unwrap();
        assert_eq!(project.id, 0);
        assert_eq!(project.name, "Clique");
        assert_eq!(
            project.url.as_deref(),
            Some("https://www.wowinterface.com/downloads/info5108")
        );
        assert_eq!(update.project_folders, ["Clique"]);
    }

    #[test]
    fn a_different_version_is_an_update_and_a_leading_v_does_not_count() {
        let offered = [addon("Clique", Some("5.0.14"), Some("5108"))];
        let updates = clique_source("v5.0.15")
            .check(&installation(), &offered, &ProjectRecords::new())
            .unwrap();
        assert_eq!(
            updates[0].status,
            UpdateStatus::UpdateAvailable {
                latest: summary("v5.0.15", Some(1_792_849_912_000))
            }
        );
        assert_eq!(updates[0].installed_file, Some(summary("5.0.14", None)));
        assert_eq!(
            summary("x", Some(1_792_849_912_000)).date,
            "2026-10-24T13:51:52Z"
        );
        let offered = [addon("Clique", Some("5.0.15"), Some("5108"))];
        let updates = clique_source("v5.0.15")
            .check(&installation(), &offered, &ProjectRecords::new())
            .unwrap();
        assert_eq!(updates[0].status, UpdateStatus::UpToDate);
    }

    #[test]
    fn folders_with_the_same_id_are_one_project_in_one_request() {
        let details = serde_json::json!({
            "id": 23056, "version": "8.2.5", "lastUpdate": 1578325974000_i64,
            "downloadUri": "https://cdn.wowinterface.com/downloads/getfile.php?id=23056",
            "title": "Details! Damage Meter"
        });
        let calls = Arc::new(Mutex::new(Vec::new()));
        let wowi = WowInterfaceSource::new(Fake {
            json: HashMap::from([(
                url("23056,5108"),
                ok(serde_json::json!([clique("v5.0.15"), details]).to_string()),
            )]),
            calls: Arc::clone(&calls),
            ..Fake::default()
        });
        let offered = [
            addon("Details", Some("8.2.4"), Some("23056")),
            addon("Clique", Some("v5.0.15"), Some("5108")),
            addon("Details_Compare2", None, Some("23056")),
        ];
        let updates = wowi
            .check(&installation(), &offered, &ProjectRecords::new())
            .unwrap();
        assert_eq!(*calls.lock().unwrap(), [url("23056,5108")]);
        assert_eq!(updates.len(), 3);
        assert!(matches!(
            updates[0].status,
            UpdateStatus::UpdateAvailable { .. }
        ));
        // La cartella senza `## Version` prende quella del progetto.
        assert_eq!(updates[2].status, updates[0].status);
        assert_eq!(updates[2].installed_file, Some(summary("8.2.4", None)));
        assert_eq!(updates[0].project_folders, ["Details", "Details_Compare2"]);
        assert_eq!(updates[2].project_folders, ["Details", "Details_Compare2"]);
        assert_eq!(updates[1].status, UpdateStatus::UpToDate);
    }

    #[test]
    fn a_module_with_its_own_version_does_not_hide_an_up_to_date_project() {
        // Il modulo viene prima nella scansione e dichiara una `## Version` sua.
        let offered = [
            addon("Clique_Module", Some("1.0"), Some("5108")),
            addon("Clique", Some("v5.0.15"), Some("5108")),
        ];
        let updates = clique_source("v5.0.15")
            .check(&installation(), &offered, &ProjectRecords::new())
            .unwrap();
        assert_eq!(updates[0].status, UpdateStatus::UpToDate);
        assert_eq!(updates[0].installed_file, Some(summary("v5.0.15", None)));
        // Se nessuna cartella ha la versione nuova, c'è un aggiornamento.
        let updates = clique_source("v5.0.16")
            .check(&installation(), &offered, &ProjectRecords::new())
            .unwrap();
        assert!(matches!(
            updates[1].status,
            UpdateStatus::UpdateAvailable { .. }
        ));
    }

    #[test]
    fn many_ids_are_split_into_several_requests() {
        let offered: Vec<InstalledAddon> = (1..=IDS_PER_REQUEST + 1)
            .map(|id| addon(&format!("A{id}"), None, Some(&id.to_string())))
            .collect();
        let mut ids: Vec<String> = (1..=IDS_PER_REQUEST + 1).map(|id| id.to_string()).collect();
        ids.sort();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let wowi = WowInterfaceSource::new(Fake {
            json: HashMap::from([
                (url(&ids[..IDS_PER_REQUEST].join(",")), ok("[]".to_owned())),
                (url(&ids[IDS_PER_REQUEST..].join(",")), ok("[]".to_owned())),
            ]),
            calls: Arc::clone(&calls),
            ..Fake::default()
        });
        wowi.check(&installation(), &offered, &ProjectRecords::new())
            .unwrap();
        assert_eq!(calls.lock().unwrap().len(), 2);
    }

    #[test]
    fn the_record_matches_folders_and_gives_the_installed_version() {
        let records = ProjectRecords::from([(
            "5108".to_owned(),
            SourceRecord {
                file: "v5.0.15".to_owned(),
                folders: vec!["Clique".to_owned(), "Clique_Options".to_owned()],
            },
        )]);
        // Nessun tag e un `.toc` con un'altra versione: decide il record.
        let offered = [
            addon("clique", Some("old"), None),
            addon("Clique_Options", None, None),
        ];
        let updates = clique_source("v5.0.15")
            .check(&installation(), &offered, &records)
            .unwrap();
        assert_eq!(
            statuses(&updates),
            [
                ("clique", Some("5108"), &UpdateStatus::UpToDate),
                ("Clique_Options", Some("5108"), &UpdateStatus::UpToDate),
            ]
        );
        assert_eq!(updates[0].project_folders, ["Clique", "Clique_Options"]);
    }

    #[test]
    fn nothing_to_check_makes_no_request() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let wowi = WowInterfaceSource::new(Fake {
            calls: Arc::clone(&calls),
            ..Fake::default()
        });
        let offered = [
            addon("Plain", Some("1"), None),
            addon("Bad", Some("1"), Some("abc")),
            addon("Zero", Some("1"), Some("0")),
        ];
        let updates = wowi
            .check(&installation(), &offered, &ProjectRecords::new())
            .unwrap();
        assert!(updates.is_empty());
        assert!(calls.lock().unwrap().is_empty());
    }

    #[test]
    fn an_unknown_id_does_not_claim_the_folder() {
        let not_found = Ok(HttpResponse {
            status: 404,
            body: r#"{"ERROR":"No AddOn found."}"#.to_owned(),
        });
        let offered = [addon("Gone", Some("1"), Some("999999999"))];
        let updates = source(&[("999999999", not_found)])
            .check(&installation(), &offered, &ProjectRecords::new())
            .unwrap();
        assert_eq!(
            statuses(&updates),
            [(
                "Gone",
                None,
                &UpdateStatus::Error {
                    message: "WoWInterface does not list the addon with id 999999999.".to_owned()
                }
            )]
        );
        // Un array vuoto vale lo stesso.
        let updates = source(&[("999999999", ok("[]".to_owned()))])
            .check(&installation(), &offered, &ProjectRecords::new())
            .unwrap();
        assert!(updates[0].project.is_none());
    }

    #[test]
    fn a_folder_claimed_by_curseforge_is_not_offered_again() {
        // `check_updates` offre a WoWInterface solo quello che CurseForge non ha reclamato.
        struct Claims;
        impl Source for Claims {
            fn kind(&self) -> SourceKind {
                SourceKind::Curseforge
            }
            fn check(
                &self,
                _: &Installation,
                offered: &[InstalledAddon],
                _: &ProjectRecords,
            ) -> Result<Vec<AddonUpdate>, String> {
                Ok(offered
                    .iter()
                    .filter(|a| a.folder == "Clique")
                    .map(|a| AddonUpdate {
                        folder: a.folder.clone(),
                        source: Some(SourceKind::Curseforge),
                        project: Some(Project {
                            id: 5108,
                            source: SourceKind::Curseforge,
                            key: "5108".to_owned(),
                            name: "Clique".to_owned(),
                            url: None,
                            allow_mod_distribution: true,
                        }),
                        installed_file: None,
                        status: UpdateStatus::UpToDate,
                        version_override: None,
                        project_folders: Vec::new(),
                        installed_modules: Vec::new(),
                    })
                    .collect())
            }
            fn update(&self, _: &UpdateTarget<'_>, _: &mut RecordWriter<'_>) -> UpdateOutcome {
                unreachable!()
            }
        }
        let tmp = TempDir::new();
        let path = tmp.path().join("World of Warcraft").join("_retail_");
        let addons = path.join("Interface").join("AddOns");
        write_addon(&addons, "Clique", "## Version: 1\n## X-WoWI-ID: 5108\n");
        let calls = Arc::new(Mutex::new(Vec::new()));
        let wowi = WowInterfaceSource::new(Fake {
            calls: Arc::clone(&calls),
            ..Fake::default()
        });
        let updates = updates::check_updates(
            &Installation {
                path,
                ..installation()
            },
            &[&Claims, &wowi],
            &SourceRecords::new(),
        )
        .unwrap();
        assert_eq!(updates[0].source, Some(SourceKind::Curseforge));
        assert!(calls.lock().unwrap().is_empty());
    }

    #[test]
    fn errors_belong_to_wowinterface() {
        let check = |response| {
            source(&[("5108", response)]).check(
                &installation(),
                &[addon("Clique", Some("1"), Some("5108"))],
                &ProjectRecords::new(),
            )
        };
        assert_eq!(
            check(Err(TransportError::Timeout)),
            Err("WoWInterface did not respond in time.".to_owned())
        );
        assert_eq!(
            check(Err(TransportError::Connect)),
            Err("Could not reach WoWInterface: check the connection.".to_owned())
        );
        assert_eq!(
            check(Ok(HttpResponse {
                status: 503,
                body: String::new()
            })),
            Err("WoWInterface responded with an error (HTTP 503).".to_owned())
        );
        // Un 404 che non è "nessun addon" resta un errore.
        assert_eq!(
            check(Ok(HttpResponse {
                status: 404,
                body: "<html>".to_owned()
            })),
            Err("WoWInterface responded with an error (HTTP 404).".to_owned())
        );
        assert_eq!(
            check(ok("{".to_owned())),
            Err("Unrecognized response from WoWInterface.".to_owned())
        );
        assert_eq!(
            check(Err(TransportError::TooLarge)),
            Err("Unrecognized response from WoWInterface: it is too large.".to_owned())
        );
    }

    #[test]
    fn a_failing_wowinterface_does_not_stop_the_other_sources() {
        struct Fine;
        impl Source for Fine {
            fn kind(&self) -> SourceKind {
                SourceKind::Tukui
            }
            fn check(
                &self,
                _: &Installation,
                _: &[InstalledAddon],
                _: &ProjectRecords,
            ) -> Result<Vec<AddonUpdate>, String> {
                Ok(Vec::new())
            }
            fn update(&self, _: &UpdateTarget<'_>, _: &mut RecordWriter<'_>) -> UpdateOutcome {
                unreachable!()
            }
        }
        let tmp = TempDir::new();
        let path = tmp.path().join("World of Warcraft").join("_retail_");
        let addons = path.join("Interface").join("AddOns");
        write_addon(&addons, "Clique", "## X-WoWI-ID: 5108\n");
        let wowi = source(&[("5108", ok("{".to_owned()))]);
        let updates = updates::check_updates(
            &Installation {
                path,
                ..installation()
            },
            &[&Fine, &wowi],
            &SourceRecords::new(),
        )
        .unwrap();
        assert_eq!(
            updates[0].status,
            UpdateStatus::Error {
                message: "WoWInterface check failed: Unrecognized response from WoWInterface."
                    .to_owned()
            }
        );
    }

    #[test]
    fn a_malformed_entry_does_not_hide_the_others() {
        let body = serde_json::json!([{"id": 1, "title": "No version"}, clique("v5.0.15")]);
        let offered = [
            addon("Clique", Some("v5.0.15"), Some("5108")),
            addon("Broken", Some("1"), Some("1")),
        ];
        let updates = source(&[("1,5108", ok(body.to_string()))])
            .check(&installation(), &offered, &ProjectRecords::new())
            .unwrap();
        assert_eq!(updates[0].status, UpdateStatus::UpToDate);
        assert!(updates[1].project.is_none());
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
        write_addon(
            &addons,
            "Clique",
            "## Version: v5.0.14-release\n## X-WoWI-ID: 5108\n",
        );
        write_addon(&addons, "Clique_Old", "## X-WoWI-ID: 5108\n");
        write_addon(&addons, "Other", "## Version: 1\n");
        let calls = Arc::new(Mutex::new(Vec::new()));
        let wowi = WowInterfaceSource::new(Fake {
            json: HashMap::from([(
                url("5108"),
                ok(serde_json::json!([clique("v5.0.15-release")]).to_string()),
            )]),
            zip: Some(zip_of(&[(
                "Clique/Clique.toc",
                "## Version: v5.0.15-release\n## X-WoWI-ID: 5108\n",
            )])),
            calls: Arc::clone(&calls),
        });
        let mut written = Vec::new();
        let collected = progress::tests::Collected::default();
        let outcomes = updates::update_addons(
            &Installation {
                path,
                ..installation()
            },
            &[&wowi],
            &SourceRecords::new(),
            &[ProjectKey {
                source: WOWI,
                key: "5108".to_owned(),
            }],
            &mut |source, key, record| {
                written.push((source, key.to_owned(), record));
                Ok(())
            },
            &collected.sink(),
        )
        .unwrap();
        use progress::Phase::*;
        let key = || "5108".to_owned();
        assert_eq!(
            collected.phases(),
            [
                (key(), Checking),
                (key(), Downloading),
                (key(), Extracting),
                (key(), Installing)
            ]
        );
        assert_eq!(
            outcomes[0].status,
            updates::OutcomeStatus::Updated,
            "{outcomes:?}"
        );
        assert_eq!(outcomes[0].name, "Clique");
        assert_eq!(
            outcomes[0].file,
            Some(summary("v5.0.15-release", Some(1_792_849_912_000)))
        );
        assert!(
            fs::read_to_string(addons.join("Clique").join("Clique.toc"))
                .unwrap()
                .contains("v5.0.15-release")
        );
        // La cartella del progetto che il nuovo zip non ha più si toglie; le altre restano.
        let mut names: Vec<String> = fs::read_dir(&addons)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        assert_eq!(names, ["Clique", "Other"]);
        assert_eq!(
            written,
            [(
                WOWI,
                "5108".to_owned(),
                SourceRecord {
                    file: "v5.0.15-release".to_owned(),
                    folders: vec!["Clique".to_owned()],
                }
            )]
        );
        // I dettagli si leggono una volta: l'aggiornamento riusa quelli del controllo.
        assert_eq!(*calls.lock().unwrap(), [url("5108"), DOWNLOAD.to_owned()]);
    }

    #[test]
    fn an_up_to_date_project_is_skipped() {
        let tmp = TempDir::new();
        let path = tmp.path().join("World of Warcraft").join("_retail_");
        let addons = path.join("Interface").join("AddOns");
        write_addon(
            &addons,
            "Clique",
            "## Version: v5.0.15\n## X-WoWI-ID: 5108\n",
        );
        let outcomes = updates::update_addons(
            &Installation {
                path,
                ..installation()
            },
            &[&clique_source("v5.0.15")],
            &SourceRecords::new(),
            &[ProjectKey {
                source: WOWI,
                key: "5108".to_owned(),
            }],
            &mut |_, _, _| panic!("nothing to record"),
            &progress::ignore,
        )
        .unwrap();
        assert_eq!(outcomes[0].status, updates::OutcomeStatus::Skipped);
        assert_eq!(outcomes[0].message.as_deref(), Some("Already up to date."));
    }

    #[test]
    fn a_download_outside_wowinterface_is_refused() {
        let file = WowiFile {
            id: 5108,
            title: "Clique".to_owned(),
            version: "1".to_owned(),
            download_uri: "https://example.com/clique.zip".to_owned(),
            last_update: None,
        };
        let error = clique_source("1")
            .download(&file, &mut |_, _| {})
            .unwrap_err();
        assert!(error.contains("unrecognized download address"), "{error}");
        let file = WowiFile {
            download_uri: "http://cdn.wowinterface.com/x.zip".to_owned(),
            ..file
        };
        assert!(clique_source("1").download(&file, &mut |_, _| {}).is_err());
    }

    /// Legge l'API vera: `cargo test --locked live_wowinterface_details -- --ignored`.
    #[test]
    #[ignore = "needs the network"]
    fn live_wowinterface_details() {
        let transport = crate::sources::http::PublicReqwestTransport::new(&DOMAINS).unwrap();
        let wowi = WowInterfaceSource::new(transport);
        let files = wowi
            .fetch(&["5108".to_owned(), "23056".to_owned()])
            .unwrap();
        assert_eq!(files["5108"].title, "Clique");
        assert!(files.contains_key("23056"));
        assert!(
            reqwest::Url::parse(&files["5108"].download_uri)
                .is_ok_and(|url| is_allowed_url(&url, &DOMAINS))
        );
        assert!(wowi.fetch(&["999999999".to_owned()]).unwrap().is_empty());
    }
}
