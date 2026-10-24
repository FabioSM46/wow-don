//! Catalogo di CurseForge: ricerca degli addon di WoW e categorie, per la tab Search.
//!
//! I comandi restano sottili: qui si sceglie la versione del gioco, si traduce l'ordinamento
//! nei valori dell'API, si rispetta il limite dei risultati e si calcola `installed` dal disco
//! e dai record dell'app, senza chiamate di rete. Bloccante: va chiamato fuori dal thread
//! principale.

mod detail;
mod text;

pub use detail::{AddonDetail, addon_detail};

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::addons::toc;
use crate::installations::Installation;
use crate::sources::curseforge::{
    CatalogMod, Category, CurseForgeClient, ModSearch, RELEASE_TYPE_RELEASE, SortOrder, Transport,
};
use crate::store::InstalledRecords;
use crate::updates::{is_known_version_type, version_type_for};

/// Risultati per pagina: il massimo che l'API accetta.
pub const PAGE_SIZE: u32 = 50;
/// L'API rifiuta le ricerche con `index + pageSize` oltre questo valore.
pub const MAX_RESULTS: u32 = 10_000;

/// Gli ordinamenti della ricerca. Il frontend vede solo i nomi; i numeri di
/// `ModsSearchSortField` (docs.curseforge.com/rest-api, verificati il 2026-10-24) restano qui.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CatalogSort {
    Popularity,
    LastUpdated,
    Name,
    TotalDownloads,
}

impl CatalogSort {
    pub fn sort_field(self) -> u32 {
        match self {
            Self::Popularity => 2,
            Self::LastUpdated => 3,
            Self::Name => 4,
            Self::TotalDownloads => 6,
        }
    }
}

/// Una categoria per il frontend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogCategory {
    pub id: u32,
    pub name: String,
    pub parent_category_id: Option<u32>,
}

impl From<&Category> for CatalogCategory {
    fn from(category: &Category) -> Self {
        Self {
            id: category.id,
            name: category.name.clone(),
            parent_category_id: category.parent_category_id,
        }
    }
}

/// L'ultimo file Release di un progetto per una versione del gioco.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogFile {
    pub id: u32,
    /// Il nome dello zip; vuoto se l'API non lo indica.
    pub name: String,
    /// La versione del gioco del file, per esempio `12.1.5`.
    pub game_version: String,
}

/// Un addon del catalogo, come lo vede il frontend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogAddon {
    pub id: u32,
    pub name: String,
    pub summary: String,
    pub authors: Vec<String>,
    pub download_count: u64,
    /// Data ISO 8601 dell'ultima modifica del progetto.
    pub date_modified: String,
    pub categories: Vec<CatalogCategory>,
    pub thumbnail_url: Option<String>,
    pub url: Option<String>,
    /// Se CurseForge permette di scaricare i file da un'app di terzi. Sconosciuto vale `false`.
    pub allow_mod_distribution: bool,
    pub latest_file: Option<CatalogFile>,
    /// Il progetto è già nell'installazione (vedi [`installed_projects`]).
    pub installed: bool,
}

impl CatalogAddon {
    /// `version_type` sceglie l'ultimo file; `installed` è l'insieme di [`installed_projects`].
    pub fn new(found: &CatalogMod, version_type: u32, installed: &HashSet<u32>) -> Self {
        let project = &found.project;
        Self {
            id: project.id,
            name: project.name.clone(),
            summary: found.summary.clone(),
            authors: found
                .authors
                .iter()
                .map(|author| author.name.clone())
                .filter(|name| !name.is_empty())
                .collect(),
            download_count: found.download_count,
            date_modified: found.date_modified.clone(),
            categories: found.categories.iter().map(CatalogCategory::from).collect(),
            thumbnail_url: found
                .logo
                .as_ref()
                .and_then(|logo| logo.thumbnail_url.clone())
                .filter(|url| !url.is_empty()),
            url: project.links.website_url.clone(),
            allow_mod_distribution: project.allow_mod_distribution.unwrap_or(false),
            latest_file: latest_release(found, version_type),
            installed: installed.contains(&project.id),
        }
    }
}

/// Il file Release più recente per la versione, tra quelli che CurseForge indica come ultimi
/// del progetto. L'indice non ha la data: decide l'id, che CurseForge assegna in ordine
/// crescente.
fn latest_release(found: &CatalogMod, version_type: u32) -> Option<CatalogFile> {
    found
        .project
        .latest_files_indexes
        .iter()
        .filter(|index| index.release_type == RELEASE_TYPE_RELEASE)
        .filter(|index| index.game_version_type_id == Some(version_type))
        .max_by_key(|index| index.file_id)
        .map(|index| CatalogFile {
            id: index.file_id,
            name: index.filename.clone().unwrap_or_default(),
            game_version: index.game_version.clone(),
        })
}

/// Una pagina di risultati per il frontend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogPage {
    pub addons: Vec<CatalogAddon>,
    pub index: u32,
    pub page_size: u32,
    /// Tutti i risultati della ricerca, anche quelli oltre [`MAX_RESULTS`] che l'API non
    /// restituisce.
    pub total_count: u64,
}

/// Una ricerca come la chiede il frontend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogQuery {
    pub query: String,
    pub category_id: Option<u32>,
    /// Senza, vale la versione del gioco dell'installazione.
    pub version_type: Option<u32>,
    pub sort: CatalogSort,
    pub sort_order: SortOrder,
    pub index: u32,
}

/// La versione del gioco della ricerca: quella chiesta, purché l'app la conosca, altrimenti
/// quella dell'installazione.
fn search_version_type(installation: &Installation, requested: Option<u32>) -> Result<u32, String> {
    match requested {
        Some(id) if is_known_version_type(id) => Ok(id),
        Some(id) => Err(format!("Unknown game version type: {id}.")),
        None => version_type_for(&installation.flavor).ok_or_else(|| {
            format!(
                "Search does not support the game version \"{}\" yet.",
                installation.name
            )
        }),
    }
}

/// Una pagina della ricerca nel catalogo. Oltre [`MAX_RESULTS`] l'API rifiuterebbe la
/// richiesta: si chiede solo il conteggio (un risultato dall'inizio) e si restituisce una
/// pagina vuota con il `totalCount` reale.
pub fn search_catalog<T: Transport>(
    installation: &Installation,
    client: &CurseForgeClient<T>,
    records: &InstalledRecords,
    request: &CatalogQuery,
) -> Result<CatalogPage, String> {
    let version_type = search_version_type(installation, request.version_type)?;
    let query = request.query.trim();
    let mut search = ModSearch {
        search_filter: (!query.is_empty()).then(|| query.to_owned()),
        category_id: request.category_id,
        game_version_type_id: Some(version_type),
        sort_field: Some(request.sort.sort_field()),
        sort_order: Some(request.sort_order),
        index: Some(request.index),
        page_size: Some(PAGE_SIZE),
    };
    if request.index.saturating_add(PAGE_SIZE) > MAX_RESULTS {
        search.index = Some(0);
        search.page_size = Some(1);
        let page = client.search_mods(&search).map_err(|e| e.to_string())?;
        return Ok(CatalogPage {
            addons: Vec::new(),
            index: request.index,
            page_size: PAGE_SIZE,
            total_count: page.pagination.total_count,
        });
    }
    let page = client.search_mods(&search).map_err(|e| e.to_string())?;
    let installed = installed_projects(installation, records);
    Ok(CatalogPage {
        addons: page
            .data
            .iter()
            .map(|found| CatalogAddon::new(found, version_type, &installed))
            .collect(),
        index: page.pagination.index,
        page_size: PAGE_SIZE,
        total_count: page.pagination.total_count,
    })
}

/// Le categorie degli addon, ordinate per nome senza distinguere maiuscole e minuscole, senza
/// le classi.
pub fn list_categories<T: Transport>(
    client: &CurseForgeClient<T>,
) -> Result<Vec<CatalogCategory>, String> {
    let mut categories: Vec<CatalogCategory> = client
        .categories()
        .map_err(|e| e.to_string())?
        .iter()
        .filter(|category| category.is_class != Some(true))
        .map(CatalogCategory::from)
        .collect();
    categories.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then(a.id.cmp(&b.id))
    });
    Ok(categories)
}

/// I progetti CurseForge già presenti in un'installazione: quelli dei record dell'app con
/// almeno una cartella ancora in `Interface/AddOns`, e quelli che una cartella dichiara con
/// `## X-Curse-Project-ID` nel `.toc` del flavor. I nomi delle cartelle si confrontano senza
/// distinguere maiuscole e minuscole, come fa il gioco. Nessuna chiamata di rete e nessun
/// fingerprint: legge solo i nomi delle cartelle e i `.toc`.
///
/// È la regola di "già installato" della ricerca, e lo sarà dell'installazione di un addon
/// nuovo e delle sue dipendenze. Una cartella `AddOns` mancante o illeggibile vale come vuota.
pub(crate) fn installed_projects(
    installation: &Installation,
    records: &InstalledRecords,
) -> HashSet<u32> {
    let addons_dir = installation.path.join("Interface").join("AddOns");
    // Nome in minuscolo → id dichiarato dal `.toc`, se c'è.
    let mut folders: HashMap<String, Option<u32>> = HashMap::new();
    if let Ok(entries) = fs::read_dir(&addons_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || !entry.path().is_dir() {
                continue;
            }
            let declared = declared_project(&entry.path(), &name, &installation.flavor);
            folders.insert(name.to_lowercase(), declared);
        }
    }
    let mut projects: HashSet<u32> = folders.values().flatten().copied().collect();
    for (mod_id, record) in records {
        if record
            .folders
            .iter()
            .any(|folder| folders.contains_key(&folder.to_lowercase()))
        {
            projects.insert(*mod_id);
        }
    }
    projects
}

/// L'id di `## X-Curse-Project-ID` nel `.toc` della cartella per il flavor.
fn declared_project(dir: &Path, folder: &str, flavor: &str) -> Option<u32> {
    let file_names: Vec<String> = fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect();
    let toc_file = toc::select_toc(folder, &file_names, flavor)?;
    let bytes = fs::read(dir.join(toc_file)).ok()?;
    toc::parse_toc(&String::from_utf8_lossy(&bytes))
        .curse_project_id?
        .trim()
        .parse()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::curseforge::{ApiKey, BytesResponse, HttpResponse, TransportError};
    use crate::store::InstalledRecord;
    use crate::test_support::TempDir;
    use crate::updates::{CLASSIC_ERA, FOREVER};
    use std::sync::Mutex;

    /// Risponde sempre con lo stesso corpo e registra gli URL richiesti.
    struct Fake {
        status: u16,
        body: &'static str,
        seen: Mutex<Vec<String>>,
    }

    impl Transport for Fake {
        fn get(&self, url: &str, _key: &ApiKey) -> Result<HttpResponse, TransportError> {
            self.seen.lock().unwrap().push(url.to_owned());
            Ok(HttpResponse {
                status: self.status,
                body: self.body.to_owned(),
            })
        }

        fn post_json(
            &self,
            url: &str,
            _key: &ApiKey,
            _body: String,
        ) -> Result<HttpResponse, TransportError> {
            panic!("unexpected POST to {url}");
        }

        fn get_bytes(&self, url: &str, _max: u64) -> Result<BytesResponse, TransportError> {
            panic!("unexpected download from {url}");
        }
    }

    fn client(status: u16, body: &'static str) -> CurseForgeClient<Fake> {
        CurseForgeClient::new(
            Fake {
                status,
                body,
                seen: Mutex::new(Vec::new()),
            },
            Some("test-key-7f3a9c".to_owned()),
        )
    }

    fn seen(client: &CurseForgeClient<Fake>) -> Vec<String> {
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

    fn addon(root: &Path, folder: &str, toc_name: &str, toc: &str) {
        let dir = root.join("Interface").join("AddOns").join(folder);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(toc_name), toc).unwrap();
    }

    fn record(folders: &[&str]) -> InstalledRecord {
        InstalledRecord {
            file_id: 1,
            folders: folders.iter().map(|f| (*f).to_owned()).collect(),
        }
    }

    fn query(index: u32) -> CatalogQuery {
        CatalogQuery {
            query: "  details  ".to_owned(),
            category_id: None,
            version_type: None,
            sort: CatalogSort::Popularity,
            sort_order: SortOrder::Desc,
            index,
        }
    }

    const PAGE: &str = r#"{
      "data": [
        {"id": 61284, "name": "Details!", "summary": "Meter",
         "authors": [{"name": "Terciob"}, {"name": null}], "downloadCount": 1000,
         "dateModified": "2026-10-01T12:00:00Z",
         "categories": [{"id": 1, "name": "Combat", "parentCategoryId": 2}],
         "logo": {"thumbnailUrl": "https://media.forgecdn.net/thumb.png"},
         "links": {"websiteUrl": "https://www.curseforge.com/wow/addons/details"},
         "allowModDistribution": true,
         "latestFilesIndexes": [
           {"fileId": 10, "filename": "old.zip", "releaseType": 1, "gameVersion": "12.1.0", "gameVersionTypeId": 517},
           {"fileId": 12, "filename": "new.zip", "releaseType": 1, "gameVersion": "12.1.5", "gameVersionTypeId": 517},
           {"fileId": 13, "filename": "beta.zip", "releaseType": 2, "gameVersion": "12.1.5", "gameVersionTypeId": 517},
           {"fileId": 14, "filename": "era.zip", "releaseType": 1, "gameVersion": "1.15.7", "gameVersionTypeId": 67408}
         ]},
        {"id": 3358, "name": "DBM"}
      ],
      "pagination": {"index": 50, "pageSize": 50, "resultCount": 2, "totalCount": 12345}
    }"#;

    #[test]
    fn a_page_of_addons_with_the_installation_version() {
        let tmp = TempDir::new();
        addon(
            tmp.path(),
            "DBM-Core",
            "DBM-Core.toc",
            "## X-Curse-Project-ID: 3358\n",
        );
        let client = client(200, PAGE);
        let page = search_catalog(
            &installation(tmp.path(), "retail"),
            &client,
            &InstalledRecords::new(),
            &query(50),
        )
        .unwrap();
        assert_eq!(
            (page.index, page.page_size, page.total_count),
            (50, 50, 12345)
        );
        let details = &page.addons[0];
        assert_eq!(details.authors, ["Terciob"]);
        assert_eq!(
            details.latest_file,
            Some(CatalogFile {
                id: 12,
                name: "new.zip".to_owned(),
                game_version: "12.1.5".to_owned()
            })
        );
        assert!(details.allow_mod_distribution && !details.installed);
        assert_eq!(details.categories[0].parent_category_id, Some(2));
        let dbm = &page.addons[1];
        assert!(dbm.installed);
        assert!(!dbm.allow_mod_distribution);
        assert_eq!((dbm.latest_file.clone(), dbm.url.clone()), (None, None));

        assert_eq!(
            seen(&client),
            [
                "https://api.curseforge.com/v1/mods/search?gameId=1&searchFilter=details\
              &gameVersionTypeId=517&sortField=2&sortOrder=desc&index=50&pageSize=50"
            ]
        );
    }

    #[test]
    fn an_explicit_version_type_wins_and_must_be_known() {
        let tmp = TempDir::new();
        let forever = installation(tmp.path(), "classic_beta");
        let mut request = query(0);
        request.version_type = Some(CLASSIC_ERA);
        request.category_id = Some(7);
        request.query = String::new();
        let client = client(200, PAGE);
        let page = search_catalog(&forever, &client, &InstalledRecords::new(), &request).unwrap();
        assert_eq!(page.addons[0].latest_file.as_ref().unwrap().id, 14);
        assert_eq!(
            seen(&client),
            [
                "https://api.curseforge.com/v1/mods/search?gameId=1&categoryId=7\
              &gameVersionTypeId=67408&sortField=2&sortOrder=desc&index=0&pageSize=50"
            ]
        );

        // Senza versione esplicita vale quella dell'installazione.
        let client = self::client(200, PAGE);
        search_catalog(&forever, &client, &InstalledRecords::new(), &query(0)).unwrap();
        assert!(seen(&client)[0].contains(&format!("gameVersionTypeId={FOREVER}&")));

        request.version_type = Some(12345);
        let client = self::client(200, PAGE);
        assert_eq!(
            search_catalog(&forever, &client, &InstalledRecords::new(), &request),
            Err("Unknown game version type: 12345.".to_owned())
        );
        assert!(seen(&client).is_empty());

        let ptr = installation(tmp.path(), "ptr");
        let error = search_catalog(&ptr, &client, &InstalledRecords::new(), &query(0));
        assert_eq!(
            error,
            Err("Search does not support the game version \"Ptr\" yet.".to_owned())
        );
        assert!(seen(&client).is_empty());
    }

    #[test]
    fn beyond_the_result_limit_only_the_count_is_asked() {
        let tmp = TempDir::new();
        let retail = installation(tmp.path(), "retail");
        let client = client(200, PAGE);
        let page =
            search_catalog(&retail, &client, &InstalledRecords::new(), &query(9_951)).unwrap();
        assert!(page.addons.is_empty());
        assert_eq!(
            (page.index, page.page_size, page.total_count),
            (9_951, 50, 12345)
        );
        let urls = seen(&client);
        assert_eq!(urls.len(), 1);
        assert!(urls[0].ends_with("&index=0&pageSize=1"), "{}", urls[0]);

        // L'ultima pagina ammessa si chiede davvero; un indice enorme non trabocca.
        let client = self::client(200, PAGE);
        search_catalog(&retail, &client, &InstalledRecords::new(), &query(9_950)).unwrap();
        assert!(seen(&client)[0].ends_with("&index=9950&pageSize=50"));
        let client = self::client(200, PAGE);
        search_catalog(&retail, &client, &InstalledRecords::new(), &query(u32::MAX)).unwrap();
        assert!(seen(&client)[0].ends_with("&index=0&pageSize=1"));
    }

    #[test]
    fn api_errors_are_english_messages_without_the_key() {
        let tmp = TempDir::new();
        let client = client(429, "");
        let error = search_catalog(
            &installation(tmp.path(), "retail"),
            &client,
            &InstalledRecords::new(),
            &query(0),
        )
        .unwrap_err();
        assert!(error.starts_with("Too many requests"), "{error}");
        assert!(!error.contains("test-key-7f3a9c"));
        assert!(
            list_categories(&self::client(500, ""))
                .unwrap_err()
                .contains("HTTP 500")
        );
    }

    #[test]
    fn installed_comes_from_records_and_toc_files() {
        let tmp = TempDir::new();
        let retail = installation(tmp.path(), "retail");
        // Dichiarato nel `.toc` del flavor, con spazi.
        addon(
            tmp.path(),
            "Details",
            "Details.toc",
            "## X-Curse-Project-ID:  61284 \n",
        );
        // Solo nel `.toc` di un altro flavor: per Retail non conta.
        addon(
            tmp.path(),
            "EraOnly",
            "EraOnly_Vanilla.toc",
            "## X-Curse-Project-ID: 7\n",
        );
        // Un id non numerico non conta.
        addon(tmp.path(), "Odd", "Odd.toc", "## X-Curse-Project-ID: abc\n");
        // Registrato dall'app, con un nome che differisce solo nelle maiuscole.
        addon(
            tmp.path(),
            "WeakAuras",
            "WeakAuras.toc",
            "## Title: WeakAuras\n",
        );
        let mut records = InstalledRecords::new();
        records.insert(65387, record(&["weakauras", "WeakAurasOptions"]));
        // Un record le cui cartelle non ci sono più non conta.
        records.insert(999, record(&["Gone"]));

        let projects = installed_projects(&retail, &records);
        assert_eq!(projects, HashSet::from([61284, 65387]));

        // Senza `Interface/AddOns` non c'è niente, nemmeno dai record.
        let empty = TempDir::new();
        assert!(installed_projects(&installation(empty.path(), "retail"), &records).is_empty());
    }

    #[test]
    fn categories_are_sorted_without_classes() {
        let client = client(
            200,
            r#"{"data": [
              {"id": 3, "name": "quests", "parentCategoryId": 1},
              {"id": 1, "name": "Addons", "isClass": true},
              {"id": 2, "name": "Bags", "isClass": false},
              {"id": 4, "name": "Achievements", "isClass": null}
            ]}"#,
        );
        let names: Vec<(u32, String)> = list_categories(&client)
            .unwrap()
            .into_iter()
            .map(|c| (c.id, c.name))
            .collect();
        assert_eq!(
            names,
            [
                (4, "Achievements".to_owned()),
                (2, "Bags".to_owned()),
                (3, "quests".to_owned())
            ]
        );
        assert_eq!(
            seen(&client),
            ["https://api.curseforge.com/v1/categories?gameId=1"]
        );
    }

    #[test]
    fn the_payload_is_camel_case() {
        let sort: CatalogSort = serde_json::from_str(r#""totalDownloads""#).unwrap();
        assert_eq!(sort.sort_field(), 6);
        for (name, field) in [
            ("popularity", 2),
            ("lastUpdated", 3),
            ("name", 4),
            ("totalDownloads", 6),
        ] {
            let sort: CatalogSort = serde_json::from_str(&format!("\"{name}\"")).unwrap();
            assert_eq!(sort.sort_field(), field);
        }
        assert!(serde_json::from_str::<CatalogSort>(r#""rating""#).is_err());
        let order: SortOrder = serde_json::from_str(r#""asc""#).unwrap();
        assert_eq!(order, SortOrder::Asc);

        let page = CatalogPage {
            addons: vec![CatalogAddon {
                id: 1,
                name: "A".to_owned(),
                summary: String::new(),
                authors: Vec::new(),
                download_count: 5,
                date_modified: String::new(),
                categories: Vec::new(),
                thumbnail_url: None,
                url: None,
                allow_mod_distribution: false,
                latest_file: Some(CatalogFile {
                    id: 2,
                    name: "a.zip".to_owned(),
                    game_version: "12.1.5".to_owned(),
                }),
                installed: true,
            }],
            index: 0,
            page_size: PAGE_SIZE,
            total_count: 1,
        };
        let json = serde_json::to_value(&page).unwrap();
        assert_eq!(json["pageSize"], 50);
        assert_eq!(json["totalCount"], 1);
        let addon = &json["addons"][0];
        for key in [
            "downloadCount",
            "dateModified",
            "thumbnailUrl",
            "allowModDistribution",
            "latestFile",
        ] {
            assert!(addon.get(key).is_some(), "{key}");
        }
        assert_eq!(addon["latestFile"]["gameVersion"], "12.1.5");
    }
}
