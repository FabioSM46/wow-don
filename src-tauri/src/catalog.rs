//! CurseForge catalog: search for WoW addons and categories, for the Search tab.
//!
//! The commands stay thin: here the game version is chosen, the sort order is translated
//! into the API's values, the result limit is respected and `installed` is computed from disk
//! and from the app's records, without network calls. Blocking: it must be called off the main
//! thread.

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
    CatalogMod, Category, CurseForgeClient, ModSearch, SortOrder, Transport, pick_file,
};
use crate::sources::github_mirror;
use crate::store::InstalledRecords;
use crate::updates::{is_known_version_type, version_type_for};

/// Results per page: the maximum the API accepts.
pub const PAGE_SIZE: u32 = 50;
/// The API rejects searches with `index + pageSize` beyond this value.
pub const MAX_RESULTS: u32 = 10_000;

/// The search sort orders. The frontend sees only the names; the numbers of
/// `ModsSearchSortField` (docs.curseforge.com/rest-api, verified on 2026-10-24) stay here.
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

/// A category for the frontend.
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

/// The latest Release file of a project for a game version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogFile {
    pub id: u32,
    /// The zip's name; empty if the API does not give it.
    pub name: String,
    /// The game version of the file, for example `12.1.5`.
    pub game_version: String,
    /// CurseForge's `releaseType`: 1 Release, 2 Beta (only with the option from issue #141).
    pub release_type: u8,
}

/// A catalog addon, as the frontend sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogAddon {
    pub id: u32,
    pub name: String,
    pub summary: String,
    pub authors: Vec<String>,
    pub download_count: u64,
    /// ISO 8601 date of the project's last modification.
    pub date_modified: String,
    pub categories: Vec<CatalogCategory>,
    pub thumbnail_url: Option<String>,
    pub url: Option<String>,
    /// Whether CurseForge allows downloading the files from a third-party app. Unknown counts
    /// as `false`.
    pub allow_mod_distribution: bool,
    /// The project's GitHub repository `owner/repo`, derived from `links.sourceUrl` with the
    /// mirror rule ([`github_mirror::github_repo`]); `null` if `sourceUrl` is missing or is not
    /// a GitHub repository. No network request: it comes from the search or detail data. It
    /// is there for every project, but it only matters with `allowModDistribution = false`:
    /// without it, the GitHub mirror (D34) is impossible up front and Search can send the user
    /// straight to the website; with it, the frontend can ask for `github_mirror` on its own
    /// (issue #222, #223). The repository only says the mirror is possible, not that the same
    /// version is there.
    pub github_repo: Option<String>,
    pub latest_file: Option<CatalogFile>,
    /// The project is already in the installation (see [`installed_projects`]).
    pub installed: bool,
}

impl CatalogAddon {
    /// `version_type` picks the latest file, a Beta only with `allow_beta` and no Release
    /// (issue #141); `installed` is the set from [`installed_projects`].
    pub fn new(
        found: &CatalogMod,
        version_type: u32,
        allow_beta: bool,
        installed: &HashSet<u32>,
    ) -> Self {
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
            github_repo: github_mirror::github_repo(project.links.source_url.as_deref()),
            latest_file: latest_file(found, version_type, allow_beta),
            installed: installed.contains(&project.id),
        }
    }
}

/// The file to install for the version, among those CurseForge lists as the project's latest,
/// with the rule of [`pick_file`]. The index has no date: the id decides, which CurseForge
/// assigns in ascending order.
fn latest_file(found: &CatalogMod, version_type: u32, allow_beta: bool) -> Option<CatalogFile> {
    let for_version = found
        .project
        .latest_files_indexes
        .iter()
        .filter(|index| index.game_version_type_id == Some(version_type));
    pick_file(
        for_version,
        |index| index.release_type,
        |index| index.file_id,
        allow_beta,
    )
    .map(|index| CatalogFile {
        id: index.file_id,
        name: index.filename.clone().unwrap_or_default(),
        game_version: index.game_version.clone(),
        release_type: index.release_type,
    })
}

/// A page of results for the frontend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogPage {
    pub addons: Vec<CatalogAddon>,
    pub index: u32,
    pub page_size: u32,
    /// All the search results, including those beyond [`MAX_RESULTS`] that the API does not
    /// return. It also includes those hidden for lacking a Release file.
    pub total_count: u64,
    /// How many API results this page read, hidden ones included: zero means the API has
    /// nothing more, even if `addons` is empty because of the filter.
    pub fetched: u32,
    /// How many of the results read were removed because they have no Release file for the
    /// searched version (issue #140): they could not be installed.
    pub hidden: u32,
}

/// A search as the frontend requests it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogQuery {
    pub query: String,
    pub category_id: Option<u32>,
    /// Without it, the installation's game version applies.
    pub version_type: Option<u32>,
    /// The "Beta when there is no Release" option (issue #141).
    pub allow_beta: bool,
    pub sort: CatalogSort,
    pub sort_order: SortOrder,
    pub index: u32,
}

/// The game version of the search: the requested one, provided the app knows it, otherwise
/// the installation's.
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

/// A page of the catalog search. Beyond [`MAX_RESULTS`] the API would reject the request:
/// only the count is asked for (one result from the start) and an empty page with the real
/// `totalCount` is returned.
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
            fetched: 0,
            hidden: 0,
        });
    }
    let page = client.search_mods(&search).map_err(|e| e.to_string())?;
    let installed = installed_projects(installation, records);
    // `gameVersionTypeId` includes projects with a file of any type for the version (for
    // example only a Beta): those the installation would not take are removed here, with the
    // same rule (`pick_file`, Beta only with the option).
    let all: Vec<CatalogAddon> = page
        .data
        .iter()
        .map(|found| CatalogAddon::new(found, version_type, request.allow_beta, &installed))
        .collect();
    let fetched = u32::try_from(all.len()).unwrap_or(u32::MAX);
    let addons: Vec<CatalogAddon> = all
        .into_iter()
        .filter(|addon| addon.latest_file.is_some())
        .collect();
    let hidden = fetched - u32::try_from(addons.len()).unwrap_or(u32::MAX);
    Ok(CatalogPage {
        addons,
        index: page.pagination.index,
        page_size: PAGE_SIZE,
        total_count: page.pagination.total_count,
        fetched,
        hidden,
    })
}

/// The addon categories, sorted by name case-insensitively, without the classes.
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

/// The CurseForge projects already present in an installation: those in the app's records
/// with at least one folder still in `Interface/AddOns`, and those a folder declares with
/// `## X-Curse-Project-ID` in the flavor's `.toc`. Folder names are compared
/// case-insensitively, as the game does. No network call and no fingerprint: it reads only
/// the folder names and the `.toc` files.
///
/// It is the search's "already installed" rule, and it will be the rule for installing a new
/// addon and its dependencies. A missing or unreadable `AddOns` folder counts as empty.
pub(crate) fn installed_projects(
    installation: &Installation,
    records: &InstalledRecords,
) -> HashSet<u32> {
    let addons_dir = installation.path.join("Interface").join("AddOns");
    // Lowercase name → name on disk, folders only.
    let mut folders: HashMap<String, String> = HashMap::new();
    if let Ok(entries) = fs::read_dir(&addons_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || !entry.path().is_dir() {
                continue;
            }
            folders.insert(name.to_lowercase(), name);
        }
    }
    // Records first, in memory: the folders a record claims are not needed for the `.toc`.
    let mut projects = HashSet::new();
    let mut claimed = HashSet::new();
    for (mod_id, record) in records {
        let present: Vec<String> = record
            .folders
            .iter()
            .map(|folder| folder.to_lowercase())
            .filter(|folder| folders.contains_key(folder))
            .collect();
        if !present.is_empty() {
            projects.insert(*mod_id);
            claimed.extend(present);
        }
    }
    // Then the `.toc`, only for the folders no record knows.
    for (lower, name) in &folders {
        if claimed.contains(lower) {
            continue;
        }
        if let Some(id) = declared_project(&addons_dir.join(name), name, &installation.flavor) {
            projects.insert(id);
        }
    }
    projects
}

/// The id from `## X-Curse-Project-ID` in the folder's `.toc` for the flavor. Also used by
/// the installation of a new addon to recognise the folders of other projects.
pub(crate) fn declared_project(dir: &Path, folder: &str, flavor: &str) -> Option<u32> {
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

    /// Always answers with the same body and records the requested URLs.
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
            allow_beta: false,
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
        {"id": 3358, "name": "DBM",
         "latestFilesIndexes": [
           {"fileId": 20, "filename": "dbm.zip", "releaseType": 1, "gameVersion": "12.1.5", "gameVersionTypeId": 517},
           {"fileId": 21, "filename": "dbm-era.zip", "releaseType": 1, "gameVersion": "1.15.7", "gameVersionTypeId": 67408}
         ]},
        {"id": 84208, "name": "RareScanner", "allowModDistribution": true,
         "latestFilesIndexes": [
           {"fileId": 30, "filename": "rs-beta.zip", "releaseType": 2, "gameVersion": "12.1.5", "gameVersionTypeId": 517},
           {"fileId": 31, "filename": "rs-era.zip", "releaseType": 1, "gameVersion": "1.15.7", "gameVersionTypeId": 67408}
         ]}
      ],
      "pagination": {"index": 50, "pageSize": 50, "resultCount": 3, "totalCount": 12345}
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
                game_version: "12.1.5".to_owned(),
                release_type: 1,
            })
        );
        assert!(details.allow_mod_distribution && !details.installed);
        assert_eq!(details.categories[0].parent_category_id, Some(2));
        let dbm = &page.addons[1];
        assert!(dbm.installed);
        assert!(!dbm.allow_mod_distribution);
        assert_eq!(dbm.latest_file.as_ref().map(|f| f.id), Some(20));
        assert_eq!(dbm.url, None);
        // RareScanner has only a Beta for Retail: it could not be installed and does not appear.
        assert_eq!(page.addons.len(), 2);
        assert_eq!((page.fetched, page.hidden), (3, 1));

        assert_eq!(
            seen(&client),
            [
                "https://api.curseforge.com/v1/mods/search?gameId=1&searchFilter=details\
              &gameVersionTypeId=517&sortField=2&sortOrder=desc&index=50&pageSize=50"
            ]
        );
    }

    #[test]
    fn with_the_beta_option_a_beta_only_addon_is_shown_as_a_beta() {
        let tmp = TempDir::new();
        let client = client(200, PAGE);
        let mut request = query(50);
        request.allow_beta = true;
        let page = search_catalog(
            &installation(tmp.path(), "retail"),
            &client,
            &InstalledRecords::new(),
            &request,
        )
        .unwrap();
        assert_eq!((page.fetched, page.hidden), (3, 0));
        let rare = page.addons.iter().find(|a| a.id == 84208).unwrap();
        assert_eq!(
            rare.latest_file.as_ref().map(|f| (f.id, f.release_type)),
            Some((30, 2))
        );
        // A Release always wins over the Beta, even with the option.
        let details = page.addons.iter().find(|a| a.id == 61284).unwrap();
        assert_eq!(details.latest_file.as_ref().map(|f| f.id), Some(12));
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
        // For Classic Era RareScanner has a Release: with another version the filter changes.
        assert_eq!(page.addons.len(), 3);
        assert_eq!((page.fetched, page.hidden), (3, 0));
        assert_eq!(
            seen(&client),
            [
                "https://api.curseforge.com/v1/mods/search?gameId=1&categoryId=7\
              &gameVersionTypeId=67408&sortField=2&sortOrder=desc&index=0&pageSize=50"
            ]
        );

        // Without an explicit version the installation's applies.
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
        assert_eq!((page.fetched, page.hidden), (0, 0));
        let urls = seen(&client);
        assert_eq!(urls.len(), 1);
        assert!(urls[0].ends_with("&index=0&pageSize=1"), "{}", urls[0]);

        // The last allowed page is really requested; a huge index does not overflow.
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
        // Declared in the flavor's `.toc`, with spaces.
        addon(
            tmp.path(),
            "Details",
            "Details.toc",
            "## X-Curse-Project-ID:  61284 \n",
        );
        // Only in another flavor's `.toc`: it does not count for Retail.
        addon(
            tmp.path(),
            "EraOnly",
            "EraOnly_Vanilla.toc",
            "## X-Curse-Project-ID: 7\n",
        );
        // A non-numeric id does not count.
        addon(tmp.path(), "Odd", "Odd.toc", "## X-Curse-Project-ID: abc\n");
        // Recorded by the app, with a name that differs only in case.
        addon(
            tmp.path(),
            "WeakAuras",
            "WeakAuras.toc",
            "## Title: WeakAuras\n",
        );
        let mut records = InstalledRecords::new();
        records.insert(65387, record(&["weakauras", "WeakAurasOptions"]));
        // A record whose folders are gone does not count.
        records.insert(999, record(&["Gone"]));

        let projects = installed_projects(&retail, &records);
        assert_eq!(projects, HashSet::from([61284, 65387]));

        // Without `Interface/AddOns` there is nothing, not even from the records.
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
                github_repo: Some("owner/repo".to_owned()),
                latest_file: Some(CatalogFile {
                    id: 2,
                    name: "a.zip".to_owned(),
                    game_version: "12.1.5".to_owned(),
                    release_type: 1,
                }),
                installed: true,
            }],
            index: 0,
            page_size: PAGE_SIZE,
            total_count: 1,
            fetched: 1,
            hidden: 0,
        };
        let json = serde_json::to_value(&page).unwrap();
        assert_eq!(json["pageSize"], 50);
        assert_eq!(json["totalCount"], 1);
        assert_eq!(
            (json["fetched"].clone(), json["hidden"].clone()),
            (1.into(), 0.into())
        );
        let addon = &json["addons"][0];
        for key in [
            "downloadCount",
            "dateModified",
            "thumbnailUrl",
            "allowModDistribution",
            "githubRepo",
            "latestFile",
        ] {
            assert!(addon.get(key).is_some(), "{key}");
        }
        assert_eq!(addon["latestFile"]["gameVersion"], "12.1.5");
        assert_eq!(addon["githubRepo"], "owner/repo");
        assert!(addon.get("github_repo").is_none());
    }

    /// A search project with the given `sourceUrl` (`None`: without `links.sourceUrl`).
    fn with_source(source_url: Option<&str>) -> CatalogAddon {
        let links = match source_url {
            Some(url) => serde_json::json!({ "sourceUrl": url }),
            None => serde_json::json!({ "websiteUrl": "https://www.curseforge.com/wow/addons/a" }),
        };
        let found: CatalogMod = serde_json::from_value(serde_json::json!({
            "id": 7, "name": "A", "allowModDistribution": false, "links": links
        }))
        .unwrap();
        CatalogAddon::new(&found, 517, false, &HashSet::new())
    }

    #[test]
    fn the_github_repo_comes_from_the_source_url() {
        assert_eq!(
            with_source(Some("https://github.com/Owner/Repo.git")).github_repo,
            Some("Owner/Repo".to_owned())
        );
        assert_eq!(
            with_source(Some("https://www.github.com/owner/repo/tree/main")).github_repo,
            Some("owner/repo".to_owned())
        );
        assert_eq!(
            with_source(Some("https://gitlab.com/owner/repo")).github_repo,
            None
        );
        assert_eq!(with_source(Some("")).github_repo, None);
        assert_eq!(with_source(None).github_repo, None);
        let json = serde_json::to_value(with_source(None)).unwrap();
        assert_eq!(json["githubRepo"], serde_json::Value::Null);
    }
}
