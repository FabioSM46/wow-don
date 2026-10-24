//! Installazione di un addon nuovo dal catalogo (`install_addon`, issue #70).
//!
//! Segue il flusso di `updates::update_addons` e ne riusa `install_file`: download, estrazione
//! sicura in staging, sostituzione con ripristino. In più, prima di toccare il disco:
//! - un progetto già presente (stessa regola del flag `installed` della ricerca,
//!   [`catalog::installed_projects`]) si salta senza nessuna chiamata di rete;
//! - una cartella dello zip che esiste già e appartiene a un altro progetto (nei record di
//!   `installed-addons.json` o, senza record, per il `## X-Curse-Project-ID` del suo `.toc`)
//!   fa fallire l'installazione; una cartella esistente senza record né id di progetto
//!   (installata a mano) si sostituisce.
//!
//! Bloccante: va chiamato fuori dal thread principale.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::Path;

use serde::Serialize;

use crate::catalog;
use crate::installations::Installation;
use crate::sources::curseforge::{CurseForgeClient, File, Mod, RELEASE_TYPE_RELEASE, Transport};
use crate::store::{InstalledRecord, InstalledRecords};
use crate::updates::{self, FileSummary, VersionOverrides};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum InstallStatus {
    Installed,
    Skipped,
    Error,
}

/// L'esito dell'installazione di un progetto, con la stessa forma di `UpdateOutcome`. Le
/// dipendenze obbligatorie (issue #71) potranno aggiungere un campo `dependencyOf` con il
/// progetto che le ha richieste.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallOutcome {
    pub mod_id: u32,
    /// Il nome del progetto; vuoto se non è stato letto da CurseForge (per esempio quando il
    /// progetto è già installato e non serve chiederlo).
    pub name: String,
    pub status: InstallStatus,
    /// Perché è stato saltato, cosa è andato storto o un avviso su un'installazione riuscita.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Il file installato, solo per `installed`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<FileSummary>,
}

impl InstallOutcome {
    fn skipped(mod_id: u32, name: &str, message: &str) -> Self {
        Self {
            mod_id,
            name: name.to_owned(),
            status: InstallStatus::Skipped,
            message: Some(message.to_owned()),
            file: None,
        }
    }

    fn error(mod_id: u32, name: &str, message: String) -> Self {
        Self {
            mod_id,
            name: name.to_owned(),
            status: InstallStatus::Error,
            message: Some(message),
            file: None,
        }
    }
}

/// Installa il progetto `mod_id` nell'installazione: il file `file_id` se indicato (purché sia
/// di quel progetto), altrimenti l'ultimo file Release per la versione del gioco forzata per il
/// progetto (`overrides`) o per quella dell'installazione. `records` sono i record di
/// `installed-addons.json` dell'installazione; `record` salva quello appena installato.
///
/// Non restituisce mai `Err`: ogni problema è nell'esito, come per un progetto di
/// `update_addons`.
pub fn install_addon<T: Transport>(
    installation: &Installation,
    client: &CurseForgeClient<T>,
    overrides: &VersionOverrides,
    records: &InstalledRecords,
    mod_id: u32,
    file_id: Option<u32>,
    record: &mut dyn FnMut(u32, InstalledRecord) -> Result<(), String>,
) -> InstallOutcome {
    if catalog::installed_projects(installation, records).contains(&mod_id) {
        return InstallOutcome::skipped(mod_id, "", "Already installed.");
    }

    let project = match client.get_mods(&[mod_id]) {
        Ok(mods) => mods.into_iter().find(|m| m.id == mod_id),
        Err(error) => return InstallOutcome::error(mod_id, "", error.to_string()),
    };
    let Some(project) = project else {
        return InstallOutcome::error(mod_id, "", "Addon not found on CurseForge.".to_owned());
    };
    let name = project.name.as_str();
    // D13: sconosciuto vale `false`.
    if project.allow_mod_distribution != Some(true) {
        return InstallOutcome::skipped(
            mod_id,
            name,
            "The author does not allow downloads from third-party apps: install it from the CurseForge website.",
        );
    }

    let file_id = match file_id {
        Some(id) => id,
        None => {
            let Some(version_type) = overrides
                .get(&mod_id)
                .copied()
                .or_else(|| updates::version_type_for(&installation.flavor))
            else {
                return InstallOutcome::error(
                    mod_id,
                    name,
                    format!(
                        "Installing addons does not support the game version \"{}\" yet.",
                        installation.name
                    ),
                );
            };
            match latest_release(&project, version_type) {
                Some(id) => id,
                None => {
                    return InstallOutcome::skipped(
                        mod_id,
                        name,
                        "There is no release file for this game version.",
                    );
                }
            }
        }
    };

    let file = match client.get_file(mod_id, file_id) {
        Ok(file) => file,
        Err(error) => return InstallOutcome::error(mod_id, name, error.to_string()),
    };
    if file.mod_id != mod_id {
        return InstallOutcome::error(
            mod_id,
            name,
            "The requested file does not belong to this addon.".to_owned(),
        );
    }
    if file.download_url.is_none() {
        return InstallOutcome::skipped(
            mod_id,
            name,
            "CurseForge does not provide a download link for this file: install it from the CurseForge website.",
        );
    }

    install_checked(installation, client, records, &file, name, record)
}

/// Il file Release più recente per la versione tra quelli che CurseForge indica come ultimi del
/// progetto. L'indice non ha la data: decide l'id, che CurseForge assegna in ordine crescente
/// (come l'ultimo file della ricerca).
fn latest_release(project: &Mod, version_type: u32) -> Option<u32> {
    project
        .latest_files_indexes
        .iter()
        .filter(|index| index.release_type == RELEASE_TYPE_RELEASE)
        .filter(|index| index.game_version_type_id == Some(version_type))
        .map(|index| index.file_id)
        .max()
}

/// Scarica e installa `file`, rifiutando le cartelle di altri progetti, poi lo registra.
fn install_checked<T: Transport>(
    installation: &Installation,
    client: &CurseForgeClient<T>,
    records: &InstalledRecords,
    file: &File,
    name: &str,
    record: &mut dyn FnMut(u32, InstalledRecord) -> Result<(), String>,
) -> InstallOutcome {
    let mod_id = file.mod_id;
    let addons_dir = installation.path.join("Interface").join("AddOns");
    let flavor = installation.flavor.as_str();
    let check =
        |folders: &[String]| check_foreign_folders(&addons_dir, flavor, records, mod_id, folders);
    let folders = match updates::install_file(client, file, &addons_dir, &[], &check) {
        Ok(folders) => folders,
        Err(message) => return InstallOutcome::error(mod_id, name, message),
    };
    // Le cartelle sono già al loro posto: se il registro non si salva l'installazione resta
    // riuscita, e il messaggio lo dice (come in `update_addons`).
    let message = record(
        mod_id,
        InstalledRecord {
            file_id: file.id,
            folders,
        },
    )
    .err()
    .map(|error| format!("Installed, but the app could not remember its folders: {error}"));
    InstallOutcome {
        mod_id,
        name: name.to_owned(),
        status: InstallStatus::Installed,
        message,
        file: Some(FileSummary::from(file)),
    }
}

/// Rifiuta lo zip se una delle sue cartelle esiste già in `AddOns` e appartiene a un altro
/// progetto: lo dice un record, oppure, per una cartella senza record, il
/// `## X-Curse-Project-ID` del suo `.toc` per il flavor (la stessa regola di
/// [`catalog::installed_projects`]). Una cartella senza record né id si sostituisce. Nomi
/// confrontati senza distinguere maiuscole e minuscole, come fa il gioco. Una cartella `AddOns`
/// che non esiste vale come vuota; una illeggibile fa fallire l'installazione, perché non si
/// potrebbe sapere cosa si sostituisce.
fn check_foreign_folders(
    addons_dir: &Path,
    flavor: &str,
    records: &InstalledRecords,
    mod_id: u32,
    folders: &[String],
) -> Result<(), String> {
    let on_disk = folder_names(addons_dir)?;
    for folder in folders {
        let lower = folder.to_lowercase();
        let Some(disk_name) = on_disk.get(&lower) else {
            continue;
        };
        let recorded: Vec<u32> = records
            .iter()
            .filter(|(_, record)| record.folders.iter().any(|f| f.to_lowercase() == lower))
            .map(|(id, _)| *id)
            .collect();
        // Il `.toc` conta solo per le cartelle che nessun record conosce, come nella ricerca.
        let owner = if recorded.is_empty() {
            catalog::declared_project(&addons_dir.join(disk_name), disk_name, flavor)
                .filter(|id| *id != mod_id)
        } else {
            recorded.into_iter().find(|id| *id != mod_id)
        };
        if let Some(owner) = owner {
            return Err(format!(
                "The folder \"{folder}\" belongs to another installed addon (CurseForge project {owner}): nothing was changed."
            ));
        }
    }
    Ok(())
}

/// Le cartelle in `AddOns`: nome in minuscolo → nome sul disco.
fn folder_names(addons_dir: &Path) -> Result<HashMap<String, String>, String> {
    let entries = match fs::read_dir(addons_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(HashMap::new()),
        Err(error) => {
            return Err(format!(
                "Could not read the AddOns folder ({}).",
                error.kind()
            ));
        }
    };
    let mut names = HashMap::new();
    for entry in entries {
        let entry = entry
            .map_err(|error| format!("Could not read the AddOns folder ({}).", error.kind()))?;
        if entry.path().is_dir() {
            let name = entry.file_name().to_string_lossy().into_owned();
            names.insert(name.to_lowercase(), name);
        }
    }
    Ok(names)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::io::{Cursor, Write};
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    use serde_json::{Value, json};
    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;

    use super::*;
    use crate::sources::curseforge::{ApiKey, BytesResponse, HttpResponse, TransportError};
    use crate::test_support::TempDir;
    use crate::updates::{CLASSIC_ERA, RETAIL};

    const API: &str = "https://api.curseforge.com";

    /// Trasporto finto che risponde per URL e registra le richieste.
    #[derive(Default)]
    struct Routes {
        mods: String,
        files: HashMap<String, String>,
        zips: HashMap<String, Vec<u8>>,
        /// Condiviso con il test, che non può rileggere il trasporto dal client.
        log: Arc<Log>,
    }

    #[derive(Default)]
    struct Log {
        requests: Mutex<Vec<String>>,
        downloads: Mutex<Vec<String>>,
    }

    fn reply(body: Option<&String>) -> Result<HttpResponse, TransportError> {
        Ok(match body {
            Some(body) => HttpResponse {
                status: 200,
                body: body.clone(),
            },
            None => HttpResponse {
                status: 404,
                body: String::new(),
            },
        })
    }

    impl Transport for Routes {
        fn get(&self, url: &str, _key: &ApiKey) -> Result<HttpResponse, TransportError> {
            self.log.requests.lock().unwrap().push(url.to_owned());
            reply(self.files.get(url))
        }

        fn post_json(
            &self,
            url: &str,
            _key: &ApiKey,
            _body: String,
        ) -> Result<HttpResponse, TransportError> {
            self.log.requests.lock().unwrap().push(url.to_owned());
            if url == format!("{API}/v1/mods") {
                reply(Some(&self.mods))
            } else {
                reply(None)
            }
        }

        fn get_bytes(&self, url: &str, _max: u64) -> Result<BytesResponse, TransportError> {
            self.log.downloads.lock().unwrap().push(url.to_owned());
            Ok(match self.zips.get(url) {
                Some(body) => BytesResponse {
                    status: 200,
                    body: body.clone(),
                },
                None => BytesResponse {
                    status: 404,
                    body: Vec::new(),
                },
            })
        }
    }

    fn zip_of(files: &[(&str, &str)]) -> Vec<u8> {
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        for (name, data) in files {
            writer
                .start_file(*name, SimpleFileOptions::default())
                .unwrap();
            writer.write_all(data.as_bytes()).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    fn download_url(id: u32) -> String {
        format!("https://edge.forgecdn.net/files/{id}/file-{id}.zip")
    }

    fn file_json(mod_id: u32, id: u32, with_url: bool) -> Value {
        let mut file = json!({
            "id": id, "modId": mod_id, "displayName": format!("File {id}"),
            "fileName": format!("file-{id}.zip"), "fileDate": "2026-10-01T00:00:00Z",
            "releaseType": 1,
        });
        if with_url {
            file["downloadUrl"] = json!(download_url(id));
        }
        file
    }

    /// Un progetto con un file Release per Retail (`id·10`) e uno per Classic Era (`id·10+1`),
    /// più una beta Retail più recente che non va scelta.
    fn project(mod_id: u32, allow: Option<bool>) -> Value {
        let mut project = json!({
            "id": mod_id, "name": format!("Project {mod_id}"),
            "latestFilesIndexes": [
                {"fileId": mod_id * 10, "releaseType": 1, "gameVersionTypeId": RETAIL},
                {"fileId": mod_id * 10 + 1, "releaseType": 1, "gameVersionTypeId": CLASSIC_ERA},
                {"fileId": mod_id * 10 + 2, "releaseType": 2, "gameVersionTypeId": RETAIL},
            ],
        });
        if let Some(allow) = allow {
            project["allowModDistribution"] = json!(allow);
        }
        project
    }

    struct Fixture {
        _tmp: TempDir,
        installation: Installation,
        addons: PathBuf,
    }

    fn fixture() -> Fixture {
        let tmp = TempDir::new();
        let path = tmp.path().join("World of Warcraft").join("_retail_");
        let addons = path.join("Interface").join("AddOns");
        fs::create_dir_all(&addons).unwrap();
        Fixture {
            _tmp: tmp,
            installation: Installation {
                id: "test".to_owned(),
                path,
                flavor: "retail".to_owned(),
                name: "Retail".to_owned(),
            },
            addons,
        }
    }

    fn write_folder(addons: &Path, folder: &str, toc: &str) {
        let dir = addons.join(folder);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(format!("{folder}.toc")), toc).unwrap();
        fs::write(dir.join(format!("{folder}.lua")), "old").unwrap();
    }

    /// Le rotte di un progetto installabile: file `file_id` con lo zip `zip`.
    fn routes(mod_id: u32, file_id: u32, zip: &[(&str, &str)]) -> Routes {
        Routes {
            mods: json!({"data": [project(mod_id, Some(true))]}).to_string(),
            files: HashMap::from([(
                format!("{API}/v1/mods/{mod_id}/files/{file_id}"),
                json!({"data": file_json(mod_id, file_id, true)}).to_string(),
            )]),
            zips: HashMap::from([(download_url(file_id), zip_of(zip))]),
            ..Routes::default()
        }
    }

    fn client(routes: Routes) -> CurseForgeClient<Routes> {
        CurseForgeClient::new(routes, Some("test-key".to_owned()))
    }

    fn record(file_id: u32, folders: &[&str]) -> InstalledRecord {
        InstalledRecord {
            file_id,
            folders: folders.iter().map(|f| (*f).to_owned()).collect(),
        }
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }

    /// Installa senza versioni forzate, raccogliendo i record scritti.
    fn install(
        fx: &Fixture,
        routes: Routes,
        records: &InstalledRecords,
        mod_id: u32,
        file_id: Option<u32>,
    ) -> (InstallOutcome, Vec<(u32, InstalledRecord)>) {
        let mut written = Vec::new();
        let outcome = install_addon(
            &fx.installation,
            &client(routes),
            &VersionOverrides::new(),
            records,
            mod_id,
            file_id,
            &mut |id, r| {
                written.push((id, r));
                Ok(())
            },
        );
        (outcome, written)
    }

    #[test]
    fn installs_the_latest_release_and_records_it() {
        let fx = fixture();
        let routes = routes(
            1,
            10,
            &[
                ("Alpha/Alpha.toc", "## Title: Alpha"),
                ("Alpha_Opt/x.lua", ""),
            ],
        );
        let log = Arc::clone(&routes.log);

        let (outcome, written) = install(&fx, routes, &InstalledRecords::new(), 1, None);

        assert_eq!(outcome.status, InstallStatus::Installed, "{outcome:?}");
        assert_eq!(outcome.name, "Project 1");
        assert_eq!(outcome.message, None);
        assert_eq!(outcome.file.as_ref().unwrap().id, 10);
        assert_eq!(names(&fx.addons), ["Alpha", "Alpha_Opt"]);
        assert_eq!(written, [(1, record(10, &["Alpha", "Alpha_Opt"]))]);
        assert_eq!(*log.downloads.lock().unwrap(), [download_url(10)]);
        // Niente rimane accanto ad AddOns.
        assert_eq!(names(fx.addons.parent().unwrap()), ["AddOns"]);
    }

    #[test]
    fn the_project_override_picks_the_file() {
        let fx = fixture();
        let routes = routes(1, 11, &[("Alpha/Alpha.toc", "")]);
        let outcome = install_addon(
            &fx.installation,
            &client(routes),
            &VersionOverrides::from([(1, CLASSIC_ERA)]),
            &InstalledRecords::new(),
            1,
            None,
            &mut |_, _| Ok(()),
        );
        assert_eq!(outcome.status, InstallStatus::Installed, "{outcome:?}");
        assert_eq!(outcome.file.unwrap().id, 11);
    }

    #[test]
    fn an_explicit_file_id_is_installed() {
        let fx = fixture();
        let (outcome, written) = install(
            &fx,
            routes(1, 99, &[("Alpha/Alpha.toc", "")]),
            &InstalledRecords::new(),
            1,
            Some(99),
        );
        assert_eq!(outcome.status, InstallStatus::Installed, "{outcome:?}");
        assert_eq!(written[0].1.file_id, 99);
    }

    #[test]
    fn a_file_of_another_project_is_refused() {
        let fx = fixture();
        let mut routes = routes(1, 99, &[("Alpha/Alpha.toc", "")]);
        routes.files.insert(
            format!("{API}/v1/mods/1/files/99"),
            json!({"data": file_json(2, 99, true)}).to_string(),
        );
        let log = Arc::clone(&routes.log);

        let (outcome, written) = install(&fx, routes, &InstalledRecords::new(), 1, Some(99));

        assert_eq!(outcome.status, InstallStatus::Error);
        assert_eq!(
            outcome.message.as_deref(),
            Some("The requested file does not belong to this addon.")
        );
        assert!(written.is_empty());
        assert!(log.downloads.lock().unwrap().is_empty());
        assert!(names(&fx.addons).is_empty());
    }

    #[test]
    fn distribution_not_allowed_or_unknown_is_skipped_without_downloading() {
        for allow in [Some(false), None] {
            let fx = fixture();
            let mut routes = routes(1, 10, &[("Alpha/Alpha.toc", "")]);
            routes.mods = json!({"data": [project(1, allow)]}).to_string();
            let log = Arc::clone(&routes.log);

            let (outcome, written) = install(&fx, routes, &InstalledRecords::new(), 1, None);

            assert_eq!(outcome.status, InstallStatus::Skipped, "{allow:?}");
            assert!(
                outcome
                    .message
                    .unwrap()
                    .contains("install it from the CurseForge website")
            );
            assert!(written.is_empty());
            assert!(log.downloads.lock().unwrap().is_empty());
            assert!(names(&fx.addons).is_empty());
        }
    }

    #[test]
    fn a_file_without_download_url_is_skipped() {
        let fx = fixture();
        let mut routes = routes(1, 10, &[("Alpha/Alpha.toc", "")]);
        routes.files.insert(
            format!("{API}/v1/mods/1/files/10"),
            json!({"data": file_json(1, 10, false)}).to_string(),
        );
        let log = Arc::clone(&routes.log);

        let (outcome, _) = install(&fx, routes, &InstalledRecords::new(), 1, None);

        assert_eq!(outcome.status, InstallStatus::Skipped);
        assert!(
            outcome
                .message
                .unwrap()
                .contains("does not provide a download link")
        );
        assert!(log.downloads.lock().unwrap().is_empty());
    }

    #[test]
    fn no_release_for_the_game_version_is_skipped() {
        let mut fx = fixture();
        fx.installation.flavor = "classic".to_owned();
        let (outcome, _) = install(
            &fx,
            routes(1, 10, &[("Alpha/Alpha.toc", "")]),
            &InstalledRecords::new(),
            1,
            None,
        );
        assert_eq!(outcome.status, InstallStatus::Skipped);
        assert_eq!(
            outcome.message.as_deref(),
            Some("There is no release file for this game version.")
        );
    }

    #[test]
    fn a_folder_of_another_project_leaves_the_disk_unchanged() {
        let fx = fixture();
        write_folder(&fx.addons, "SharedLib", "## Title: Shared\n");
        let records = InstalledRecords::from([(2, record(20, &["sharedlib"]))]);
        let routes = routes(
            1,
            10,
            &[("Alpha/Alpha.toc", ""), ("SharedLib/SharedLib.toc", "new")],
        );

        let (outcome, written) = install(&fx, routes, &records, 1, None);

        assert_eq!(outcome.status, InstallStatus::Error);
        let message = outcome.message.unwrap();
        assert!(message.contains("\"SharedLib\""), "{message}");
        assert!(message.contains("project 2"), "{message}");
        assert!(written.is_empty());
        assert_eq!(names(&fx.addons), ["SharedLib"]);
        assert_eq!(
            fs::read_to_string(fx.addons.join("SharedLib").join("SharedLib.lua")).unwrap(),
            "old"
        );
        assert_eq!(names(fx.addons.parent().unwrap()), ["AddOns"]);
    }

    #[test]
    fn an_unrecorded_folder_of_another_project_leaves_the_disk_unchanged() {
        let fx = fixture();
        // Nessun record, ma il `.toc` dice che `SharedLib` è del progetto 2.
        write_folder(&fx.addons, "sharedlib", "## X-Curse-Project-ID: 2\n");
        let routes = routes(
            1,
            10,
            &[("Alpha/Alpha.toc", ""), ("SharedLib/SharedLib.toc", "new")],
        );

        let (outcome, written) = install(&fx, routes, &InstalledRecords::new(), 1, None);

        assert_eq!(outcome.status, InstallStatus::Error);
        let message = outcome.message.unwrap();
        assert!(message.contains("\"SharedLib\""), "{message}");
        assert!(message.contains("project 2"), "{message}");
        assert!(written.is_empty());
        assert_eq!(names(&fx.addons), ["sharedlib"]);
        assert_eq!(
            fs::read_to_string(fx.addons.join("sharedlib").join("sharedlib.lua")).unwrap(),
            "old"
        );
    }

    #[test]
    fn a_recorded_folder_that_is_gone_does_not_block() {
        let fx = fixture();
        // Il progetto 2 ha registrato `Alpha`, ma la cartella non c'è più.
        let records = InstalledRecords::from([(2, record(20, &["Alpha"]))]);
        let (outcome, _) = install(
            &fx,
            routes(1, 10, &[("Alpha/Alpha.toc", "")]),
            &records,
            1,
            None,
        );
        assert_eq!(outcome.status, InstallStatus::Installed, "{outcome:?}");
    }

    #[test]
    fn a_manually_installed_folder_is_replaced() {
        let fx = fixture();
        write_folder(&fx.addons, "alpha", "## Title: Alpha\n");
        let (outcome, written) = install(
            &fx,
            routes(1, 10, &[("Alpha/Alpha.lua", "new")]),
            &InstalledRecords::new(),
            1,
            None,
        );
        assert_eq!(outcome.status, InstallStatus::Installed, "{outcome:?}");
        assert_eq!(names(&fx.addons), ["Alpha"]);
        assert_eq!(
            fs::read_to_string(fx.addons.join("Alpha").join("Alpha.lua")).unwrap(),
            "new"
        );
        assert_eq!(written[0].1.folders, ["Alpha"]);
    }

    #[test]
    fn an_installed_project_is_skipped_without_network() {
        let fx = fixture();
        write_folder(&fx.addons, "Beta", "## X-Curse-Project-ID: 3\n");
        write_folder(&fx.addons, "Alpha", "## Title: Alpha\n");
        let records = InstalledRecords::from([(1, record(10, &["ALPHA"]))]);
        for mod_id in [1, 3] {
            let routes = routes(mod_id, mod_id * 10, &[("Alpha/Alpha.toc", "")]);
            let log = Arc::clone(&routes.log);

            let (outcome, written) = install(&fx, routes, &records, mod_id, None);

            assert_eq!(outcome.status, InstallStatus::Skipped);
            assert_eq!(outcome.message.as_deref(), Some("Already installed."));
            assert!(written.is_empty());
            assert!(log.requests.lock().unwrap().is_empty());
            assert!(log.downloads.lock().unwrap().is_empty());
        }
    }

    #[test]
    fn a_record_that_cannot_be_saved_leaves_the_install_done() {
        let fx = fixture();
        let outcome = install_addon(
            &fx.installation,
            &client(routes(1, 10, &[("Alpha/Alpha.toc", "")])),
            &VersionOverrides::new(),
            &InstalledRecords::new(),
            1,
            None,
            &mut |_, _| Err("disk full".to_owned()),
        );
        assert_eq!(outcome.status, InstallStatus::Installed);
        assert!(outcome.file.is_some());
        assert_eq!(
            outcome.message.as_deref(),
            Some("Installed, but the app could not remember its folders: disk full")
        );
        assert_eq!(names(&fx.addons), ["Alpha"]);
    }

    #[test]
    fn network_errors_and_unknown_projects_are_errors() {
        let fx = fixture();
        let mut unknown = routes(1, 10, &[]);
        unknown.mods = json!({"data": []}).to_string();
        let (outcome, _) = install(&fx, unknown, &InstalledRecords::new(), 1, None);
        assert_eq!(outcome.status, InstallStatus::Error);
        assert_eq!(
            outcome.message.as_deref(),
            Some("Addon not found on CurseForge.")
        );

        // Il file indicato non esiste: 404 dall'API.
        let (outcome, _) = install(
            &fx,
            routes(1, 10, &[]),
            &InstalledRecords::new(),
            1,
            Some(55),
        );
        assert_eq!(outcome.status, InstallStatus::Error);
        assert!(!outcome.message.unwrap().contains("test-key"));
    }

    #[test]
    fn the_outcome_payload_is_camel_case() {
        let json = serde_json::to_value(InstallOutcome::skipped(7, "Foo", "why")).unwrap();
        assert_eq!(
            json,
            json!({"modId": 7, "name": "Foo", "status": "skipped", "message": "why"})
        );
        let json = serde_json::to_value(InstallOutcome {
            mod_id: 7,
            name: "Foo".to_owned(),
            status: InstallStatus::Installed,
            message: None,
            file: Some(FileSummary {
                id: 1,
                name: "x".to_owned(),
                date: "d".to_owned(),
            }),
        })
        .unwrap();
        assert_eq!(json["status"], "installed");
        assert_eq!(json["file"]["id"], 1);
        assert!(json.get("message").is_none());
        assert_eq!(serde_json::to_value(InstallStatus::Error).unwrap(), "error");
    }
}
