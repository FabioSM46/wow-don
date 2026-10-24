//! Il dettaglio di un addon del catalogo: descrizione in testo, file recenti e dipendenze
//! obbligatorie, per decidere se installarlo e quale versione arriverà. Bloccante: va
//! chiamato fuori dal thread principale.

use std::collections::{HashMap, HashSet};

use serde::Serialize;

use super::text::plain_text;
use super::{CatalogAddon, installed_projects, search_version_type};
use crate::installations::Installation;
use crate::sources::curseforge::{
    CurseForgeClient, File, RELATION_REQUIRED_DEPENDENCY, RELEASE_TYPE_BETA, RELEASE_TYPE_RELEASE,
    Transport,
};
use crate::store::InstalledRecords;
use crate::updates::VersionOverrides;

/// I file mostrati nel dettaglio, al massimo.
pub const MAX_FILES: usize = 20;
/// I file chiesti all'API: il massimo di una pagina, perché gli Alpha si scartano dopo.
const FILES_PAGE_SIZE: u32 = 50;

/// Un file del progetto nel dettaglio.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DetailFile {
    pub id: u32,
    /// Il nome mostrato da CurseForge, altrimenti quello dello zip; vuoto se mancano entrambi.
    pub name: String,
    /// Data ISO 8601 del file.
    pub date: String,
    /// 1 Release, 2 Beta.
    pub release_type: u8,
}

impl From<&File> for DetailFile {
    fn from(file: &File) -> Self {
        let name = if file.display_name.is_empty() {
            file.file_name.clone().unwrap_or_default()
        } else {
            file.display_name.clone()
        };
        Self {
            id: file.id,
            name,
            date: file.file_date.clone(),
            release_type: file.release_type,
        }
    }
}

/// Una dipendenza obbligatoria.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Dependency {
    pub id: u32,
    /// Vuoto se CurseForge non restituisce il progetto.
    pub name: String,
}

/// Il dettaglio per il frontend: i campi di [`CatalogAddon`] più descrizione, file e
/// dipendenze.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AddonDetail {
    #[serde(flatten)]
    pub addon: CatalogAddon,
    /// La versione del gioco usata per i file e le dipendenze.
    pub version_type: u32,
    /// La descrizione in testo semplice: nessun HTML dell'autore arriva al frontend.
    pub description: String,
    /// I file Release e Beta per la versione, dal più recente, al massimo [`MAX_FILES`].
    pub files: Vec<DetailFile>,
    /// Le dipendenze obbligatorie dell'ultimo file Release.
    pub dependencies: Vec<Dependency>,
}

/// Il dettaglio del progetto `mod_id` per l'installazione. La versione del gioco è quella
/// chiesta, altrimenti quella forzata per il progetto, altrimenti quella dell'installazione.
/// Al massimo quattro richieste: il mod, la descrizione, i file e, se ci sono dipendenze,
/// i loro nomi con una sola `get_mods`.
pub fn addon_detail<T: Transport>(
    installation: &Installation,
    client: &CurseForgeClient<T>,
    records: &InstalledRecords,
    overrides: &VersionOverrides,
    mod_id: u32,
    version_type: Option<u32>,
) -> Result<AddonDetail, String> {
    let requested = version_type.or_else(|| overrides.get(&mod_id).copied());
    let version_type = search_version_type(installation, requested)?;
    let found = client.get_mod(mod_id).map_err(|e| e.to_string())?;
    let description = plain_text(&client.mod_description(mod_id).map_err(|e| e.to_string())?);
    let mut files: Vec<File> = client
        .mod_files(mod_id, version_type, FILES_PAGE_SIZE)
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter(|file| matches!(file.release_type, RELEASE_TYPE_RELEASE | RELEASE_TYPE_BETA))
        .collect();
    // L'ordine della risposta non è documentato: decide l'id, che CurseForge assegna in
    // ordine crescente, come per l'ultimo file della ricerca.
    files.sort_by_key(|file| std::cmp::Reverse(file.id));
    files.truncate(MAX_FILES);
    let dependencies = required_dependencies(client, mod_id, &files)?;
    let installed = installed_projects(installation, records);
    Ok(AddonDetail {
        addon: CatalogAddon::new(&found, version_type, &installed),
        version_type,
        description,
        files: files.iter().map(DetailFile::from).collect(),
        dependencies,
    })
}

/// Le dipendenze obbligatorie del file Release più recente tra `files` (già ordinati), senza
/// ripetizioni e senza il progetto stesso, con i nomi letti da una sola `get_mods`.
fn required_dependencies<T: Transport>(
    client: &CurseForgeClient<T>,
    mod_id: u32,
    files: &[File],
) -> Result<Vec<Dependency>, String> {
    let Some(latest) = files
        .iter()
        .find(|file| file.release_type == RELEASE_TYPE_RELEASE)
    else {
        return Ok(Vec::new());
    };
    let mut seen = HashSet::new();
    let ids: Vec<u32> = latest
        .dependencies
        .iter()
        .filter(|dependency| dependency.relation_type == RELATION_REQUIRED_DEPENDENCY)
        .map(|dependency| dependency.mod_id)
        .filter(|id| *id != mod_id && seen.insert(*id))
        .collect();
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let names: HashMap<u32, String> = client
        .get_mods(&ids)
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|project| (project.id, project.name))
        .collect();
    Ok(ids
        .into_iter()
        .map(|id| Dependency {
            id,
            name: names.get(&id).cloned().unwrap_or_default(),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::curseforge::{ApiKey, BytesResponse, HttpResponse, TransportError};
    use crate::test_support::TempDir;
    use crate::updates::{CLASSIC_ERA, FOREVER, RETAIL};
    use std::fs;
    use std::path::Path;
    use std::sync::Mutex;

    /// Risponde in base all'URL e registra le richieste (URL e corpo).
    struct Fake {
        files: String,
        seen: Mutex<Vec<(String, Option<String>)>>,
    }

    const MOD: &str = r#"{"data": {"id": 61284, "name": "Details!", "summary": "Meter",
      "allowModDistribution": true,
      "latestFilesIndexes": [
        {"fileId": 12, "filename": "new.zip", "releaseType": 1, "gameVersion": "12.1.5", "gameVersionTypeId": 517}
      ]}}"#;
    const DESCRIPTION: &str =
        r#"{"data": "<p>A meter.</p><script>alert(1)</script><ul><li>Fast</li></ul>"}"#;
    const DEPENDENCY_NAMES: &str = r#"{"data": [{"id": 3358, "name": "DBM"}]}"#;

    /// File 30 Release con dipendenze, 31 Alpha, 29 Beta, 28 Release più vecchio.
    const FILES: &str = r#"{"data": [
      {"id": 29, "modId": 61284, "displayName": "Details Beta", "fileDate": "2026-10-02T00:00:00Z", "releaseType": 2,
       "dependencies": [{"modId": 77, "relationType": 3}]},
      {"id": 31, "modId": 61284, "displayName": "Details Alpha", "releaseType": 3},
      {"id": 30, "modId": 61284, "displayName": "", "fileName": "details-30.zip",
       "fileDate": "2026-10-03T00:00:00Z", "releaseType": 1,
       "dependencies": [
         {"modId": 3358, "relationType": 3}, {"modId": 14, "relationType": 2},
         {"modId": 3358, "relationType": 3}, {"modId": 61284, "relationType": 3},
         {"modId": 15, "relationType": 1}, {"modId": 404, "relationType": 3}
       ]},
      {"id": 28, "modId": 61284, "displayName": "Details 28", "releaseType": 1, "dependencies": null}
    ], "pagination": {"index": 0, "pageSize": 50, "resultCount": 4, "totalCount": 4}}"#;

    impl Transport for Fake {
        fn get(&self, url: &str, _key: &ApiKey) -> Result<HttpResponse, TransportError> {
            self.seen.lock().unwrap().push((url.to_owned(), None));
            let body = if url.ends_with("/description") {
                DESCRIPTION
            } else if url.contains("/files?") {
                self.files.as_str()
            } else {
                MOD
            };
            Ok(HttpResponse {
                status: 200,
                body: body.to_owned(),
            })
        }

        fn post_json(
            &self,
            url: &str,
            _key: &ApiKey,
            body: String,
        ) -> Result<HttpResponse, TransportError> {
            self.seen.lock().unwrap().push((url.to_owned(), Some(body)));
            Ok(HttpResponse {
                status: 200,
                body: DEPENDENCY_NAMES.to_owned(),
            })
        }

        fn get_bytes(&self, url: &str, _max: u64) -> Result<BytesResponse, TransportError> {
            panic!("unexpected download from {url}");
        }
    }

    fn client(files: &str) -> CurseForgeClient<Fake> {
        CurseForgeClient::new(
            Fake {
                files: files.to_owned(),
                seen: Mutex::new(Vec::new()),
            },
            Some("test-key-7f3a9c".to_owned()),
        )
    }

    fn seen(client: &CurseForgeClient<Fake>) -> Vec<(String, Option<String>)> {
        client.transport().seen.lock().unwrap().clone()
    }

    fn installation(root: &Path, flavor: &str) -> Installation {
        Installation {
            id: "inst".to_owned(),
            path: root.to_path_buf(),
            flavor: flavor.to_owned(),
            name: crate::installations::display_name(flavor),
        }
    }

    fn detail(
        installation: &Installation,
        client: &CurseForgeClient<Fake>,
        overrides: &VersionOverrides,
        version_type: Option<u32>,
    ) -> Result<AddonDetail, String> {
        addon_detail(
            installation,
            client,
            &InstalledRecords::new(),
            overrides,
            61284,
            version_type,
        )
    }

    #[test]
    fn the_detail_has_text_files_and_required_dependencies() {
        let tmp = TempDir::new();
        let dir = tmp.path().join("Interface").join("AddOns").join("Details");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("Details.toc"), "## X-Curse-Project-ID: 61284\n").unwrap();
        let client = client(FILES);
        let detail = detail(
            &installation(tmp.path(), "retail"),
            &client,
            &VersionOverrides::new(),
            None,
        )
        .unwrap();

        assert_eq!(detail.addon.name, "Details!");
        assert!(detail.addon.installed);
        assert_eq!(detail.addon.latest_file.as_ref().unwrap().id, 12);
        assert_eq!(detail.version_type, RETAIL);
        assert_eq!(detail.description, "A meter.\n\nFast");
        let files: Vec<(u32, &str, u8)> = detail
            .files
            .iter()
            .map(|f| (f.id, f.name.as_str(), f.release_type))
            .collect();
        assert_eq!(
            files,
            [
                (30, "details-30.zip", 1),
                (29, "Details Beta", 2),
                (28, "Details 28", 1)
            ]
        );
        assert_eq!(detail.files[0].date, "2026-10-03T00:00:00Z");
        // Solo le obbligatorie del file 30, senza ripetizioni né il progetto stesso; il nome
        // di un progetto che CurseForge non restituisce resta vuoto.
        assert_eq!(
            detail.dependencies,
            [
                Dependency {
                    id: 3358,
                    name: "DBM".to_owned()
                },
                Dependency {
                    id: 404,
                    name: String::new()
                }
            ]
        );

        let seen = seen(&client);
        let urls: Vec<&str> = seen.iter().map(|(url, _)| url.as_str()).collect();
        assert_eq!(
            urls,
            [
                "https://api.curseforge.com/v1/mods/61284",
                "https://api.curseforge.com/v1/mods/61284/description",
                "https://api.curseforge.com/v1/mods/61284/files?gameVersionTypeId=517&pageSize=50",
                "https://api.curseforge.com/v1/mods",
            ]
        );
        assert_eq!(seen[3].1.as_deref(), Some(r#"{"modIds":[3358,404]}"#));
    }

    #[test]
    fn the_version_is_explicit_then_forced_then_the_installation() {
        let tmp = TempDir::new();
        let forever = installation(tmp.path(), "classic_beta");
        let files_url = |client: &CurseForgeClient<Fake>| seen(client)[2].0.clone();
        let forced = VersionOverrides::from([(61284, CLASSIC_ERA), (1, RETAIL)]);

        let client = client(FILES);
        let detail = detail(&forever, &client, &VersionOverrides::new(), None).unwrap();
        assert_eq!(detail.version_type, FOREVER);
        assert!(files_url(&client).contains(&format!("gameVersionTypeId={FOREVER}&")));

        let client = self::client(FILES);
        let detail = self::detail(&forever, &client, &forced, None).unwrap();
        assert_eq!(detail.version_type, CLASSIC_ERA);
        assert!(files_url(&client).contains(&format!("gameVersionTypeId={CLASSIC_ERA}&")));
        // L'indice della ricerca non ha un file Release per Classic Era.
        assert_eq!(detail.addon.latest_file, None);

        let client = self::client(FILES);
        let detail = self::detail(&forever, &client, &forced, Some(RETAIL)).unwrap();
        assert_eq!(detail.version_type, RETAIL);

        let client = self::client(FILES);
        assert_eq!(
            self::detail(&forever, &client, &forced, Some(12345)),
            Err("Unknown game version type: 12345.".to_owned())
        );
        assert!(seen(&client).is_empty());
    }

    #[test]
    fn without_a_release_file_there_are_no_dependencies() {
        let tmp = TempDir::new();
        let files = r#"{"data": [
          {"id": 29, "modId": 61284, "releaseType": 2, "dependencies": [{"modId": 77, "relationType": 3}]}
        ]}"#;
        let client = client(files);
        let detail = detail(
            &installation(tmp.path(), "retail"),
            &client,
            &VersionOverrides::new(),
            None,
        )
        .unwrap();
        assert_eq!(detail.files.len(), 1);
        assert!(detail.dependencies.is_empty());
        // Nessuna `get_mods` senza dipendenze.
        assert_eq!(seen(&client).len(), 3);
    }

    #[test]
    fn at_most_twenty_files_from_the_newest() {
        let tmp = TempDir::new();
        let many: String = (1..=30)
            .map(|id| format!(r#"{{"id": {id}, "modId": 61284, "releaseType": 1}}"#))
            .collect::<Vec<_>>()
            .join(",");
        let client = client(&format!(r#"{{"data": [{many}]}}"#));
        let detail = detail(
            &installation(tmp.path(), "retail"),
            &client,
            &VersionOverrides::new(),
            None,
        )
        .unwrap();
        assert_eq!(detail.files.len(), MAX_FILES);
        assert_eq!(
            (detail.files[0].id, detail.files[MAX_FILES - 1].id),
            (30, 11)
        );
    }

    #[test]
    fn the_payload_is_flat_and_camel_case() {
        let tmp = TempDir::new();
        let client = client(FILES);
        let detail = detail(
            &installation(tmp.path(), "retail"),
            &client,
            &VersionOverrides::new(),
            None,
        )
        .unwrap();
        let json = serde_json::to_value(&detail).unwrap();
        for key in [
            "id",
            "name",
            "allowModDistribution",
            "latestFile",
            "installed",
            "versionType",
            "description",
            "files",
            "dependencies",
        ] {
            assert!(json.get(key).is_some(), "{key}");
        }
        assert!(json.get("addon").is_none());
        assert_eq!(json["files"][0]["releaseType"], 1);
        assert_eq!(json["dependencies"][0]["name"], "DBM");
    }
}
