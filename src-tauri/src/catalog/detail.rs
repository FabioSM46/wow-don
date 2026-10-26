//! Il dettaglio di un addon del catalogo: descrizione in testo, immagini della gallery, file
//! recenti e dipendenze obbligatorie, per decidere se installarlo e quale versione arriverà. Bloccante: va
//! chiamato fuori dal thread principale.

use std::collections::{HashMap, HashSet};

use serde::Serialize;

use super::text::plain_text;
use super::{CatalogAddon, MAX_RESULTS, installed_projects, search_version_type};
use crate::installations::Installation;
use crate::sources::curseforge::{
    CurseForgeClient, File, ModAsset, RELATION_REQUIRED_DEPENDENCY, RELEASE_TYPE_BETA,
    RELEASE_TYPE_RELEASE, Transport, is_media_url,
};
use crate::store::InstalledRecords;
use crate::updates::VersionOverrides;

/// I file mostrati nel dettaglio, al massimo.
pub const MAX_FILES: usize = 20;
/// I file chiesti all'API per pagina: il massimo, perché gli Alpha si scartano dopo.
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

/// Un'immagine della gallery. Gli URL sono indirizzi pubblici del CDN di CurseForge (vedi
/// [`is_media_url`]): il frontend li carica con `<img src>`, il backend non li scarica.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Screenshot {
    /// Testo semplice, vuoto se manca; si mostra solo come testo, come `description`.
    pub title: String,
    /// Testo semplice, vuoto se manca; si mostra solo come testo.
    pub description: String,
    /// L'anteprima, oppure `url` se l'anteprima manca o non è sul CDN.
    pub thumbnail_url: String,
    /// L'immagine intera.
    pub url: String,
}

impl Screenshot {
    /// L'immagine se `url` è sul CDN, altrimenti niente.
    fn from_asset(asset: &ModAsset) -> Option<Self> {
        let url = asset.url.as_deref().filter(|url| is_media_url(url))?;
        let thumbnail_url = asset
            .thumbnail_url
            .as_deref()
            .filter(|thumb| is_media_url(thumb))
            .unwrap_or(url);
        Some(Self {
            title: plain_text(asset.title.as_deref().unwrap_or_default()),
            description: plain_text(asset.description.as_deref().unwrap_or_default()),
            thumbnail_url: thumbnail_url.to_owned(),
            url: url.to_owned(),
        })
    }
}

/// Le immagini della gallery mostrabili, nell'ordine dell'API.
fn screenshots(assets: &[ModAsset]) -> Vec<Screenshot> {
    assets.iter().filter_map(Screenshot::from_asset).collect()
}

/// Il dettaglio per il frontend: i campi di [`CatalogAddon`] più descrizione, gallery, file e
/// dipendenze.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AddonDetail {
    #[serde(flatten)]
    pub addon: CatalogAddon,
    /// La versione del gioco usata per i file e le dipendenze.
    pub version_type: u32,
    /// La descrizione in testo semplice: nessun HTML dell'autore arriva al frontend. Può
    /// contenere `<` e `>` decodificati dalle entità, quindi si mostra solo come testo, mai
    /// con `innerHTML` (vedi `text::plain_text`).
    pub description: String,
    /// Le immagini della gallery, dalla stessa `get_mod`: nessuna richiesta in più.
    pub screenshots: Vec<Screenshot>,
    /// I file Release e Beta per la versione, dal più recente, al massimo [`MAX_FILES`].
    pub files: Vec<DetailFile>,
    /// Le dipendenze obbligatorie dell'ultimo file Release: quello di `latestFile` se c'è,
    /// altrimenti il Release più recente tra tutti i file letti, anche quelli non mostrati.
    pub dependencies: Vec<Dependency>,
}

/// Il dettaglio del progetto `mod_id` per l'installazione. La versione del gioco è quella
/// chiesta, altrimenti quella forzata per il progetto, altrimenti quella dell'installazione.
/// Al massimo sei richieste: il mod, la descrizione, una o due pagine di file, il file
/// dell'ultimo Release se non è tra quelli letti e, se ci sono dipendenze, i loro nomi con una
/// sola `get_mods`.
pub fn addon_detail<T: Transport>(
    installation: &Installation,
    client: &CurseForgeClient<T>,
    records: &InstalledRecords,
    overrides: &VersionOverrides,
    mod_id: u32,
    version_type: Option<u32>,
    allow_beta: bool,
) -> Result<AddonDetail, String> {
    let requested = version_type.or_else(|| overrides.get(&mod_id).copied());
    let version_type = search_version_type(installation, requested)?;
    let found = client.get_mod(mod_id).map_err(|e| e.to_string())?;
    let description = plain_text(&client.mod_description(mod_id).map_err(|e| e.to_string())?);
    let mut files = recent_files(client, mod_id, version_type)?;
    let installed = installed_projects(installation, records);
    let addon = CatalogAddon::new(&found, version_type, allow_beta, &installed);
    let screenshots = screenshots(&found.screenshots);
    let latest = addon.latest_file.as_ref().map(|file| file.id);
    // Le dipendenze si leggono dalla lista intera: tagliarla prima perderebbe l'ultimo
    // Release dietro più di [`MAX_FILES`] Beta.
    let dependencies = required_dependencies(client, mod_id, latest, &files)?;
    files.truncate(MAX_FILES);
    Ok(AddonDetail {
        addon,
        version_type,
        description,
        screenshots,
        files: files.iter().map(DetailFile::from).collect(),
        dependencies,
    })
}

/// I file Release e Beta per la versione, dal più recente, senza ripetizioni.
///
/// L'ordine della risposta di `/files` non è documentato, quindi non conta: si legge la
/// prima pagina e, se `totalCount` ne supera una, anche l'ultima (con `index + pageSize` al
/// massimo [`MAX_RESULTS`], come nella ricerca). I file più recenti stanno in una delle due
/// sia con un ordine crescente sia con uno decrescente; poi decide l'id, che CurseForge
/// assegna in ordine crescente, come per l'ultimo file della ricerca.
fn recent_files<T: Transport>(
    client: &CurseForgeClient<T>,
    mod_id: u32,
    version_type: u32,
) -> Result<Vec<File>, String> {
    let first = client
        .mod_files(mod_id, version_type, 0, FILES_PAGE_SIZE)
        .map_err(|e| e.to_string())?;
    let total = first.pagination.total_count;
    let mut files = first.data;
    let page = u64::from(FILES_PAGE_SIZE);
    if total > page {
        let last_reachable = MAX_RESULTS - FILES_PAGE_SIZE;
        // Il `min` con un `u32` rende la conversione esatta.
        let index = (total - page).min(u64::from(last_reachable)) as u32;
        let last = client
            .mod_files(mod_id, version_type, index, FILES_PAGE_SIZE)
            .map_err(|e| e.to_string())?;
        // Con meno di due pagine piene le due letture si sovrappongono.
        let mut seen: HashSet<u32> = files.iter().map(|file| file.id).collect();
        files.extend(last.data.into_iter().filter(|file| seen.insert(file.id)));
    }
    files.retain(|file| matches!(file.release_type, RELEASE_TYPE_RELEASE | RELEASE_TYPE_BETA));
    files.sort_by_key(|file| std::cmp::Reverse(file.id));
    Ok(files)
}

/// Le dipendenze obbligatorie dell'ultimo file Release, senza ripetizioni e senza il progetto
/// stesso, con i nomi letti da una sola `get_mods`. L'ultimo Release è `latest`, quello che
/// `latestFilesIndexes` indica per la versione, letto con `get_file` se non è tra `files`;
/// senza indice è il Release più recente tra `files` (già ordinati).
fn required_dependencies<T: Transport>(
    client: &CurseForgeClient<T>,
    mod_id: u32,
    latest: Option<u32>,
    files: &[File],
) -> Result<Vec<Dependency>, String> {
    let fetched;
    let release = match latest {
        Some(id) => match files.iter().find(|file| file.id == id) {
            Some(file) => Some(file),
            None => {
                fetched = client.get_file(mod_id, id).map_err(|e| e.to_string())?;
                Some(&fetched)
            }
        },
        None => files
            .iter()
            .find(|file| file.release_type == RELEASE_TYPE_RELEASE),
    };
    let Some(release) = release else {
        return Ok(Vec::new());
    };
    let mut seen = HashSet::new();
    let ids: Vec<u32> = release
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
    use std::collections::HashMap;
    use std::fs;
    use std::path::Path;
    use std::sync::Mutex;

    /// Risponde in base all'URL e registra le richieste (URL e corpo).
    struct Fake {
        project: String,
        /// Le pagine di `/files`, per `index`.
        pages: HashMap<u32, String>,
        /// La risposta di `/files/{fileId}`.
        file: String,
        seen: Mutex<Vec<(String, Option<String>)>>,
    }

    /// L'indice dice che l'ultimo Release per Retail (517) è il file 30.
    const MOD: &str = r#"{"data": {"id": 61284, "name": "Details!", "summary": "Meter",
      "allowModDistribution": true,
      "screenshots": [
        {"title": "Meter", "description": "<b>The</b> window",
         "thumbnailUrl": "https://media.forgecdn.net/1-thumb.png", "url": "https://media.forgecdn.net/1.png"},
        {"title": null, "description": null, "thumbnailUrl": "https://media.forgecdn.net/2-thumb.png",
         "url": "http://media.forgecdn.net/2.png"},
        {"title": "Plain", "url": "https://media.forgecdn.net/3.png"}
      ],
      "latestFilesIndexes": [
        {"fileId": 30, "filename": "details-30.zip", "releaseType": 1, "gameVersion": "12.1.5", "gameVersionTypeId": 517},
        {"fileId": 29, "filename": "details-29.zip", "releaseType": 2, "gameVersion": "12.1.5", "gameVersionTypeId": 517}
      ]}}"#;
    /// Un progetto senza `latestFilesIndexes`: l'ultimo Release si cerca tra i file letti.
    const BARE_MOD: &str = r#"{"data": {"id": 61284, "name": "Details!"}}"#;
    /// Il file letto con `get_file`.
    const FILE_30: &str = r#"{"data": {"id": 30, "modId": 61284, "displayName": "Details 30",
      "releaseType": 1, "dependencies": [{"modId": 3358, "relationType": 3}]}}"#;
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

    /// Una pagina di `/files` con i file Release `ids` (senza dipendenze) e il totale.
    fn page(ids: impl IntoIterator<Item = u32>, total: u64) -> String {
        let files: Vec<String> = ids
            .into_iter()
            .map(|id| format!(r#"{{"id": {id}, "modId": 61284, "releaseType": 1}}"#))
            .collect();
        format!(
            r#"{{"data": [{}], "pagination": {{"index": 0, "pageSize": 50, "resultCount": {}, "totalCount": {total}}}}}"#,
            files.join(","),
            files.len()
        )
    }

    /// L'`index` della richiesta di `/files`.
    fn index_of(url: &str) -> u32 {
        let start = url.find("index=").expect("index") + "index=".len();
        url[start..]
            .split('&')
            .next()
            .and_then(|value| value.parse().ok())
            .expect("numeric index")
    }

    impl Transport for Fake {
        fn get(&self, url: &str, _key: &ApiKey) -> Result<HttpResponse, TransportError> {
            self.seen.lock().unwrap().push((url.to_owned(), None));
            let body = if url.ends_with("/description") {
                DESCRIPTION
            } else if url.contains("/files?") {
                let index = index_of(url);
                self.pages
                    .get(&index)
                    .unwrap_or_else(|| panic!("unexpected files page {index}"))
                    .as_str()
            } else if url.contains("/files/") {
                self.file.as_str()
            } else {
                self.project.as_str()
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
        client_with(MOD, [(0, files.to_owned())])
    }

    fn client_with(
        project: &str,
        pages: impl IntoIterator<Item = (u32, String)>,
    ) -> CurseForgeClient<Fake> {
        CurseForgeClient::new(
            Fake {
                project: project.to_owned(),
                pages: pages.into_iter().collect(),
                file: FILE_30.to_owned(),
                seen: Mutex::new(Vec::new()),
            },
            Some("test-key-7f3a9c".to_owned()),
        )
    }

    fn retail(tmp: &TempDir, client: &CurseForgeClient<Fake>) -> AddonDetail {
        detail(
            &installation(tmp.path(), "retail"),
            client,
            &VersionOverrides::new(),
            None,
        )
        .unwrap()
    }

    fn file_ids(detail: &AddonDetail) -> Vec<u32> {
        detail.files.iter().map(|file| file.id).collect()
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
            false,
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
        assert_eq!(detail.addon.latest_file.as_ref().unwrap().id, 30);
        assert_eq!(detail.version_type, RETAIL);
        assert_eq!(detail.description, "A meter.\n\nFast");
        // Dalla stessa `get_mod`: l'immagine su `http` si scarta, l'anteprima mancante è l'URL.
        assert_eq!(
            detail.screenshots,
            [
                Screenshot {
                    title: "Meter".to_owned(),
                    description: "The window".to_owned(),
                    thumbnail_url: "https://media.forgecdn.net/1-thumb.png".to_owned(),
                    url: "https://media.forgecdn.net/1.png".to_owned(),
                },
                Screenshot {
                    title: "Plain".to_owned(),
                    description: String::new(),
                    thumbnail_url: "https://media.forgecdn.net/3.png".to_owned(),
                    url: "https://media.forgecdn.net/3.png".to_owned(),
                },
            ]
        );
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
                "https://api.curseforge.com/v1/mods/61284/files?gameVersionTypeId=517&index=0&pageSize=50",
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
        ], "pagination": {"index": 0, "pageSize": 50, "resultCount": 1, "totalCount": 1}}"#;
        let client = client_with(BARE_MOD, [(0, files.to_owned())]);
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
        let client = client(&page(1..=30, 30));
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
    fn dependencies_come_from_the_full_list_before_truncation() {
        let tmp = TempDir::new();
        // Venticinque Beta più recenti del Release 50, che non entra nei venti mostrati.
        let betas: Vec<String> = (100..125)
            .map(|id| format!(r#"{{"id": {id}, "modId": 61284, "releaseType": 2}}"#))
            .collect();
        let files = format!(
            r#"{{"data": [{}, {{"id": 50, "modId": 61284, "releaseType": 1,
              "dependencies": [{{"modId": 3358, "relationType": 3}}]}}],
              "pagination": {{"index": 0, "pageSize": 50, "resultCount": 26, "totalCount": 26}}}}"#,
            betas.join(",")
        );
        let client = client_with(BARE_MOD, [(0, files)]);
        let detail = retail(&tmp, &client);
        assert_eq!(detail.files.len(), MAX_FILES);
        assert!(!file_ids(&detail).contains(&50));
        assert_eq!(
            detail.dependencies,
            [Dependency {
                id: 3358,
                name: "DBM".to_owned()
            }]
        );
    }

    #[test]
    fn the_indexed_release_is_read_when_it_is_not_among_the_files() {
        let tmp = TempDir::new();
        // L'indice dice 30; tra i file letti c'è solo il Release 28, con un'altra dipendenza.
        let files = r#"{"data": [
          {"id": 29, "modId": 61284, "releaseType": 2},
          {"id": 28, "modId": 61284, "releaseType": 1, "dependencies": [{"modId": 77, "relationType": 3}]}
        ], "pagination": {"index": 0, "pageSize": 50, "resultCount": 2, "totalCount": 2}}"#;
        let client = client(files);
        let detail = retail(&tmp, &client);
        assert_eq!(file_ids(&detail), [29, 28]);
        assert_eq!(detail.dependencies[0].id, 3358);
        let seen = seen(&client);
        assert_eq!(
            seen[3].0,
            "https://api.curseforge.com/v1/mods/61284/files/30"
        );
        assert_eq!(seen[4].1.as_deref(), Some(r#"{"modIds":[3358]}"#));
    }

    #[test]
    fn the_last_page_is_read_whatever_the_order() {
        let tmp = TempDir::new();
        // In ordine crescente i più recenti stanno nell'ultima pagina (index = 120 - 50).
        let client = client_with(
            BARE_MOD,
            [(0, page(1..=50, 120)), (70, page(71..=120, 120))],
        );
        let detail = retail(&tmp, &client);
        assert_eq!(file_ids(&detail), (101..=120).rev().collect::<Vec<_>>());
        let files_urls: Vec<String> = seen(&client)
            .into_iter()
            .map(|(url, _)| url)
            .filter(|url| url.contains("/files?"))
            .collect();
        assert_eq!(files_urls.len(), 2);
        assert!(files_urls[1].contains("&index=70&pageSize=50"));

        // In ordine decrescente bastava la prima: l'ultima non aggiunge niente di nuovo.
        let client = client_with(
            BARE_MOD,
            [
                (0, page((71..=120).rev(), 120)),
                (70, page((1..=50).rev(), 120)),
            ],
        );
        assert_eq!(
            file_ids(&retail(&tmp, &client)),
            (101..=120).rev().collect::<Vec<_>>()
        );

        // Due pagine che si sovrappongono non ripetono i file.
        let client = client_with(BARE_MOD, [(0, page(1..=50, 70)), (20, page(21..=70, 70))]);
        assert_eq!(
            file_ids(&retail(&tmp, &client)),
            (51..=70).rev().collect::<Vec<_>>()
        );

        // Oltre i 10.000 file l'ultima pagina raggiungibile finisce a 10.000.
        let client = client_with(
            BARE_MOD,
            [
                (0, page(1..=50, 20_000)),
                (9_950, page(9_951..=10_000, 20_000)),
            ],
        );
        assert_eq!(retail(&tmp, &client).files[0].id, 10_000);

        // Con una sola pagina non c'è una seconda richiesta.
        let client = client_with(BARE_MOD, [(0, page(1..=50, 50))]);
        retail(&tmp, &client);
        assert_eq!(
            seen(&client)
                .iter()
                .filter(|(url, _)| url.contains("/files?"))
                .count(),
            1
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
            "screenshots",
            "files",
            "dependencies",
        ] {
            assert!(json.get(key).is_some(), "{key}");
        }
        assert_eq!(
            json["screenshots"][0],
            serde_json::json!({
                "title": "Meter",
                "description": "The window",
                "thumbnailUrl": "https://media.forgecdn.net/1-thumb.png",
                "url": "https://media.forgecdn.net/1.png",
            })
        );
        assert!(json.get("addon").is_none());
        assert_eq!(json["files"][0]["releaseType"], 1);
        assert_eq!(json["dependencies"][0]["name"], "DBM");
    }

    fn asset(thumbnail_url: Option<&str>, url: Option<&str>) -> ModAsset {
        ModAsset {
            thumbnail_url: thumbnail_url.map(str::to_owned),
            url: url.map(str::to_owned),
            title: None,
            description: None,
        }
    }

    #[test]
    fn only_cdn_images_reach_the_gallery() {
        let cdn = "https://media.forgecdn.net/a.png";
        let thumb = "https://media.forgecdn.net/a-thumb.png";
        let assets = [
            asset(Some(thumb), Some(cdn)),
            asset(Some(thumb), Some("http://media.forgecdn.net/a.png")),
            asset(Some(thumb), Some("https://example.com/a.png")),
            asset(Some(thumb), Some("")),
            asset(Some(thumb), Some("not a url")),
            asset(Some(thumb), None),
            // Senza anteprima, o con un'anteprima fuori dal CDN, vale l'URL.
            asset(None, Some(cdn)),
            asset(Some(""), Some(cdn)),
            asset(Some("https://example.com/t.png"), Some(cdn)),
        ];
        let all = screenshots(&assets);
        let kept: Vec<(&str, &str)> = all
            .iter()
            .map(|s| (s.thumbnail_url.as_str(), s.url.as_str()))
            .collect();
        assert_eq!(kept, [(thumb, cdn), (cdn, cdn), (cdn, cdn), (cdn, cdn)]);
        assert!(
            all.iter()
                .all(|s| s.title.is_empty() && s.description.is_empty())
        );
    }
}
