//! Installazione da un link di GitHub, WoWInterface o Tukui (issue #109).
//!
//! [`parse_link`] riconosce il link senza nessuna richiesta di rete e lo traduce nel progetto
//! di una fonte (`ProjectKey`): `github.com/<owner>/<repo>` (anche con `/releases` e simili),
//! `wowinterface.com/downloads/info<ID>-<nome>.html` e `tukui.org/elvui` o `/tukui`, con o
//! senza `www`. Il testo si legge con `reqwest::Url`, mai confrontando il testo intero.
//!
//! [`preview`] chiede alla fonte nome, versione e se il progetto si può scaricare per
//! l'installazione; [`install`] lo installa con le fonti esistenti (`install_new` di ognuna),
//! con le regole di `install_new::install_addon`:
//! - un progetto già installato (un record della stessa fonte con almeno una cartella ancora in
//!   `AddOns`) si salta senza rete;
//! - una cartella dello zip che esiste già e appartiene a un altro progetto (un record di
//!   qualunque fonte, o senza record il `## X-Curse-Project-ID` del suo `.toc`) fa fallire
//!   l'installazione prima di toccare il disco; una cartella senza record né id si sostituisce.
//!
//! Bloccante: va chiamato fuori dal thread principale.

use std::path::Path;

use serde::Serialize;

use crate::catalog;
use crate::install_new;
use crate::installations::Installation;
use crate::progress::ProgressSink;
use crate::sources::github::{self, GithubSource};
use crate::sources::http::PublicTransport;
use crate::sources::tukui::TukuiSource;
use crate::sources::wowinterface::WowInterfaceSource;
use crate::sources::{LinkInfo, ProjectKey, RecordWriter, SourceKind};
use crate::store::SourceRecords;
use crate::updates::UpdateOutcome;

const UNRECOGNIZED: &str = "WoW-don cannot install from this link: paste a GitHub repository, a WoWInterface addon page or tukui.org/elvui or /tukui.";
const MALFORMED: &str = "This is not a valid link.";
const CURSEFORGE: &str = "CurseForge addons are installed from the Search tab.";

/// I primi segmenti di `github.com` che non sono un proprietario di repository.
const GITHUB_RESERVED: [&str; 14] = [
    "about",
    "apps",
    "collections",
    "enterprise",
    "explore",
    "features",
    "login",
    "marketplace",
    "notifications",
    "orgs",
    "settings",
    "sponsors",
    "topics",
    "users",
];

/// Il progetto indicato da un link. Nessuna richiesta di rete: un link non riconosciuto, di un
/// altro host o malformato dà un errore che dice cosa incollare. Senza schema vale `https://`.
pub fn parse_link(text: &str) -> Result<ProjectKey, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("Paste the link of an addon.".to_owned());
    }
    let with_scheme = if text.contains("://") {
        text.to_owned()
    } else {
        format!("https://{text}")
    };
    let url = reqwest::Url::parse(&with_scheme).map_err(|_| MALFORMED.to_owned())?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(UNRECOGNIZED.to_owned());
    }
    let host = url.host_str().ok_or_else(|| MALFORMED.to_owned())?;
    let host = host.strip_prefix("www.").unwrap_or(host);
    let segments: Vec<&str> = url
        .path_segments()
        .map(|segments| segments.filter(|s| !s.is_empty()).collect())
        .unwrap_or_default();
    let project = |source, key: String| ProjectKey { source, key };
    match host {
        "github.com" => github_repo(&segments).map(|key| project(SourceKind::Github, key)),
        "wowinterface.com" => {
            wowinterface_id(&url, &segments).map(|key| project(SourceKind::Wowinterface, key))
        }
        "tukui.org" => match segments.as_slice() {
            [name] if name.eq_ignore_ascii_case("elvui") => {
                Ok(project(SourceKind::Tukui, "-2".to_owned()))
            }
            [name] if name.eq_ignore_ascii_case("tukui") => {
                Ok(project(SourceKind::Tukui, "-1".to_owned()))
            }
            _ => Err("From tukui.org WoW-don installs ElvUI (tukui.org/elvui) and Tukui (tukui.org/tukui).".to_owned()),
        },
        "curseforge.com" => Err(CURSEFORGE.to_owned()),
        _ => Err(UNRECOGNIZED.to_owned()),
    }
}

/// `owner/repo` dai primi due segmenti, senza un `.git` finale.
fn github_repo(segments: &[&str]) -> Result<String, String> {
    let not_a_repo =
        || "This GitHub link does not point to a repository (github.com/owner/repo).".to_owned();
    let [owner, repo, ..] = segments else {
        return Err(not_a_repo());
    };
    if GITHUB_RESERVED
        .iter()
        .any(|reserved| owner.eq_ignore_ascii_case(reserved))
    {
        return Err(not_a_repo());
    }
    let repo = repo.strip_suffix(".git").unwrap_or(repo);
    let key = format!("{owner}/{repo}");
    if github::is_valid_repo(&key) {
        Ok(key)
    } else {
        Err(not_a_repo())
    }
}

/// L'id di `downloads/info<ID>-<nome>.html` (anche `download<ID>…` e `fileinfo.php?id=<ID>`).
fn wowinterface_id(url: &reqwest::Url, segments: &[&str]) -> Result<String, String> {
    let not_an_addon = || {
        "This WoWInterface link does not point to an addon page (wowinterface.com/downloads/info…)."
            .to_owned()
    };
    let [downloads, page] = segments else {
        return Err(not_an_addon());
    };
    if !downloads.eq_ignore_ascii_case("downloads") {
        return Err(not_an_addon());
    }
    let page = page.to_ascii_lowercase();
    let digits: String = if page == "fileinfo.php" {
        url.query_pairs()
            .find(|(name, _)| name == "id")
            .map(|(_, value)| value.into_owned())
            .ok_or_else(not_an_addon)?
    } else {
        let rest = page
            .strip_prefix("info")
            .or_else(|| page.strip_prefix("download"))
            .ok_or_else(not_an_addon)?;
        let end = rest
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(rest.len());
        let tail = &rest[end..];
        if !(tail.is_empty() || tail.starts_with('-') || tail == ".html") {
            return Err(not_an_addon());
        }
        rest[..end].to_owned()
    };
    is_wowinterface_id(&digits)
        .then(|| digits.trim_start_matches('0').to_owned())
        .ok_or_else(not_an_addon)
}

/// Solo cifre, non zero, al massimo nove: l'id finisce nell'URL dell'API.
fn is_wowinterface_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 9
        && id.bytes().all(|b| b.is_ascii_digit())
        && id.bytes().any(|b| b != b'0')
}

/// Le fonti che sanno installare da un link.
pub struct LinkSources<'a, T: PublicTransport> {
    pub github: &'a GithubSource<T>,
    pub wowinterface: &'a WowInterfaceSource<T>,
    pub tukui: &'a TukuiSource<T>,
}

/// L'anteprima di un link per l'installazione.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkPreview {
    pub source: SourceKind,
    /// L'id del progetto nella fonte, da passare a `install_from_link`.
    pub id: String,
    pub name: String,
    pub version: String,
    /// Se WoW-don lo può scaricare per il gioco dell'installazione.
    pub downloadable: bool,
    /// Perché non si può scaricare, o un avviso.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// La pagina del progetto sul sito della fonte.
    pub url: String,
    /// Se l'app l'ha già installato (un record della fonte con le cartelle ancora presenti).
    pub installed: bool,
}

/// L'id `key` è un id valido per la fonte: un controllo senza rete prima di comporre un URL.
fn check_key(source: SourceKind, key: &str) -> Result<(), String> {
    let valid = match source {
        SourceKind::Github => github::is_valid_repo(key),
        SourceKind::Wowinterface => is_wowinterface_id(key),
        SourceKind::Tukui => key.parse::<i64>().is_ok(),
        SourceKind::Curseforge => return Err(CURSEFORGE.to_owned()),
    };
    if valid {
        Ok(())
    } else {
        Err(format!(
            "\"{key}\" is not a valid {} project.",
            source.label()
        ))
    }
}

/// Se il record `(source_b, key_b)` è dello stesso progetto: i repository GitHub non
/// distinguono maiuscole e minuscole.
fn same_project(source: SourceKind, key: &str, source_b: SourceKind, key_b: &str) -> bool {
    source == source_b
        && (key == key_b || (source == SourceKind::Github && key.eq_ignore_ascii_case(key_b)))
}

/// Se l'app ha installato il progetto: un record della stessa fonte nomina una cartella che
/// c'è ancora in `AddOns`. Solo disco, nessuna rete. Una cartella `AddOns` illeggibile è un
/// errore, non "non installato": l'installazione fallirebbe comunque nel controllo delle cartelle.
fn is_installed(
    addons_dir: &Path,
    records: &SourceRecords,
    source: SourceKind,
    key: &str,
) -> Result<bool, String> {
    let Some(projects) = records.get(&source) else {
        return Ok(false);
    };
    let on_disk = install_new::folder_names(addons_dir)?;
    Ok(projects
        .iter()
        .filter(|(recorded, _)| same_project(source, key, source, recorded))
        .flat_map(|(_, record)| &record.folders)
        .any(|folder| on_disk.contains_key(&folder.to_lowercase())))
}

/// Rifiuta lo zip se una delle sue cartelle esiste già e appartiene a un altro progetto, con la
/// regola di `install_new`: lo dice un record di qualunque fonte, oppure, per una cartella
/// senza record, il `## X-Curse-Project-ID` del suo `.toc` per il flavor. Nomi confrontati senza
/// distinguere maiuscole e minuscole, come fa il gioco.
fn check_foreign_folders(
    addons_dir: &Path,
    flavor: &str,
    records: &SourceRecords,
    source: SourceKind,
    key: &str,
    folders: &[String],
) -> Result<(), String> {
    let on_disk = install_new::folder_names(addons_dir)?;
    for folder in folders {
        let lower = folder.to_lowercase();
        let Some(disk_name) = on_disk.get(&lower) else {
            continue;
        };
        let recorded: Vec<(SourceKind, &str)> = records
            .iter()
            .flat_map(|(kind, projects)| {
                projects
                    .iter()
                    .filter(|(_, record)| record.folders.iter().any(|f| f.to_lowercase() == lower))
                    .map(move |(id, _)| (*kind, id.as_str()))
            })
            .collect();
        let owner = if recorded.is_empty() {
            catalog::declared_project(&addons_dir.join(disk_name), disk_name, flavor)
                .map(|id| (SourceKind::Curseforge, id.to_string()))
        } else {
            recorded
                .into_iter()
                .find(|(kind, id)| !same_project(source, key, *kind, id))
                .map(|(kind, id)| (kind, id.to_owned()))
        };
        if let Some((kind, id)) = owner {
            return Err(format!(
                "The folder \"{folder}\" belongs to another installed addon ({} project {id}): nothing was changed.",
                kind.label()
            ));
        }
    }
    Ok(())
}

/// L'anteprima del progetto `target` per l'installazione: una richiesta alla sua fonte. `Err`
/// se la fonte non risponde o non conosce il progetto.
pub fn preview<T: PublicTransport>(
    installation: &Installation,
    sources: &LinkSources<'_, T>,
    records: &SourceRecords,
    target: &ProjectKey,
) -> Result<LinkPreview, String> {
    check_key(target.source, &target.key)?;
    let key = target.key.as_str();
    // Prima il disco: un `AddOns` illeggibile fallisce senza rete.
    let addons_dir = installation.path.join("Interface").join("AddOns");
    let installed = is_installed(&addons_dir, records, target.source, key)?;
    let info: LinkInfo = match target.source {
        SourceKind::Github => sources.github.preview(installation, key)?,
        SourceKind::Wowinterface => sources.wowinterface.preview(key)?,
        SourceKind::Tukui => sources.tukui.preview(installation, key)?,
        SourceKind::Curseforge => return Err(CURSEFORGE.to_owned()),
    };
    Ok(LinkPreview {
        source: target.source,
        id: target.key.clone(),
        name: info.name,
        version: info.version,
        downloadable: info.downloadable,
        message: info.message,
        url: info.url,
        installed,
    })
}

/// Installa il progetto `key` della fonte `source`. `records` sono i record di tutte le fonti
/// per l'installazione, letti sotto `InstallLock`; `record` salva quello appena installato.
/// L'esito ha la forma di `UpdateOutcome`: `updated` vuol dire installato.
pub fn install<T: PublicTransport>(
    installation: &Installation,
    sources: &LinkSources<'_, T>,
    records: &SourceRecords,
    source: SourceKind,
    key: &str,
    record: &mut RecordWriter<'_>,
    progress: &ProgressSink<'_>,
) -> UpdateOutcome {
    if let Err(error) = check_key(source, key) {
        return UpdateOutcome::error(source, key, "", error);
    }
    let addons_dir = installation.path.join("Interface").join("AddOns");
    match is_installed(&addons_dir, records, source, key) {
        Ok(true) => return UpdateOutcome::skipped(source, key, "", "Already installed."),
        Ok(false) => {}
        Err(error) => return UpdateOutcome::error(source, key, "", error),
    }
    let flavor = installation.flavor.as_str();
    let check = |folders: &[String]| {
        check_foreign_folders(&addons_dir, flavor, records, source, key, folders)
    };
    match source {
        SourceKind::Github => {
            sources
                .github
                .install_new(installation, key, &check, record, progress)
        }
        SourceKind::Wowinterface => {
            sources
                .wowinterface
                .install_new(installation, key, &check, record, progress)
        }
        SourceKind::Tukui => sources
            .tukui
            .install_new(installation, key, &check, record, progress),
        SourceKind::Curseforge => UpdateOutcome::error(source, key, "", CURSEFORGE.to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::fs;
    use std::io::{Cursor, Write};
    use std::sync::{Arc, Mutex};

    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;

    use super::*;
    use crate::progress;
    use crate::sources::curseforge::TransportError;
    use crate::sources::http::{ApiResponse, BytesResponse, HttpResponse, RedirectResponse};
    use crate::sources::tukui::ADDONS_URL;
    use crate::sources::wowinterface::DETAILS_URL;
    use crate::store::{ProjectRecords, SourceRecord};
    use crate::test_support::TempDir;
    use crate::updates::OutcomeStatus;

    fn parsed(text: &str) -> (SourceKind, String) {
        let key = parse_link(text).unwrap_or_else(|e| panic!("{text}: {e}"));
        (key.source, key.key)
    }

    #[test]
    fn github_links_name_the_repository() {
        for text in [
            "https://github.com/BigWigsMods/BigWigs",
            "https://www.github.com/BigWigsMods/BigWigs/",
            "http://github.com/BigWigsMods/BigWigs/releases",
            "https://github.com/BigWigsMods/BigWigs/releases/tag/v427.8",
            "https://github.com/BigWigsMods/BigWigs.git",
            "https://github.com/BigWigsMods/BigWigs?tab=readme#install",
            "  github.com/BigWigsMods/BigWigs  ",
            "HTTPS://GitHub.com/BigWigsMods/BigWigs",
        ] {
            assert_eq!(
                parsed(text),
                (SourceKind::Github, "BigWigsMods/BigWigs".to_owned()),
                "{text}"
            );
        }
    }

    #[test]
    fn wowinterface_links_name_the_addon_id() {
        for text in [
            "https://www.wowinterface.com/downloads/info5108-Clique.html",
            "https://wowinterface.com/downloads/info5108",
            "https://www.wowinterface.com/downloads/info5108.html",
            "https://www.wowinterface.com/downloads/download5108-Clique",
            "https://www.wowinterface.com/downloads/fileinfo.php?id=5108",
            "www.wowinterface.com/downloads/info05108-Clique.html#info",
        ] {
            assert_eq!(
                parsed(text),
                (SourceKind::Wowinterface, "5108".to_owned()),
                "{text}"
            );
        }
    }

    #[test]
    fn tukui_links_name_elvui_or_tukui() {
        for (text, id) in [
            ("https://www.tukui.org/elvui", "-2"),
            ("https://tukui.org/ElvUI/", "-2"),
            ("tukui.org/elvui", "-2"),
            ("https://tukui.org/tukui", "-1"),
            ("https://www.tukui.org/tukui?x=1", "-1"),
        ] {
            assert_eq!(parsed(text), (SourceKind::Tukui, id.to_owned()), "{text}");
        }
    }

    #[test]
    fn other_links_are_clear_errors() {
        for text in [
            "",
            "   ",
            "https://example.com/BigWigsMods/BigWigs",
            "https://github.com.example.com/BigWigsMods/BigWigs",
            "https://gist.github.com/someone/abc",
            "https://github.com/BigWigsMods",
            "https://github.com/orgs/BigWigsMods/repositories",
            "https://github.com/a/..",
            "https://github.com/a%20b/c",
            "ftp://github.com/BigWigsMods/BigWigs",
            "file:///github.com/BigWigsMods/BigWigs",
            "https://www.wowinterface.com/downloads/info.html",
            "https://www.wowinterface.com/downloads/infoX5108.html",
            "https://www.wowinterface.com/downloads/info5108x.html",
            "https://www.wowinterface.com/downloads/info0",
            "https://www.wowinterface.com/downloads/info12345678901",
            "https://www.wowinterface.com/forums/info5108",
            "https://www.wowinterface.com/downloads/fileinfo.php?id=abc",
            "https://www.tukui.org/addons",
            "https://www.tukui.org/elvui/download",
            "https://www.curseforge.com/wow/addons/details",
            "https://addons.wago.io/addons/details",
            "https://",
            "http://[::1",
        ] {
            let error = parse_link(text).expect_err(text);
            assert!(!error.is_empty(), "{text}");
        }
        assert_eq!(
            parse_link("https://www.curseforge.com/wow/addons/details").unwrap_err(),
            CURSEFORGE
        );
    }

    // ── Comandi con un trasporto finto ──

    const REPO: &str = "BigWigsMods/BigWigs";
    const LATEST: &str = "https://api.github.com/repos/BigWigsMods/BigWigs/releases/latest";
    const GITHUB_ZIP: &str =
        "https://github.com/BigWigsMods/BigWigs/releases/download/v427.8/BigWigs-v427.8.zip";
    const WOWI_ZIP: &str = "https://cdn.wowinterface.com/downloads/file5108/Clique.zip";
    const TUKUI_ZIP: &str = "https://api.tukui.org/v1/download/main/elvui.zip?token=x";

    /// Risponde per URL e registra ogni richiesta.
    #[derive(Default)]
    struct Fake {
        json: HashMap<String, String>,
        bytes: HashMap<String, Vec<u8>>,
        calls: Arc<Mutex<Vec<String>>>,
    }

    impl PublicTransport for Fake {
        fn get(&self, url: &str) -> Result<HttpResponse, TransportError> {
            self.calls.lock().unwrap().push(url.to_owned());
            Ok(match self.json.get(url) {
                Some(body) => HttpResponse {
                    status: 200,
                    body: body.clone(),
                },
                // Come MMOUI per un id che non esiste.
                None => HttpResponse {
                    status: 404,
                    body: r#"{"ERROR":"No AddOn found."}"#.to_owned(),
                },
            })
        }

        /// `releases/latest` sul sito rimanda al tag della release che l'API elenca per lo
        /// stesso repository; senza release, 404.
        fn get_redirect(&self, url: &str) -> Result<RedirectResponse, TransportError> {
            self.calls.lock().unwrap().push(url.to_owned());
            let api = url.replace("https://github.com/", "https://api.github.com/repos/");
            let tag = self
                .json
                .get(&api)
                .and_then(|body| serde_json::from_str::<serde_json::Value>(body).ok())
                .and_then(|release| release["tag_name"].as_str().map(str::to_owned));
            let repo = url.trim_end_matches("/releases/latest");
            Ok(match tag {
                Some(tag) => RedirectResponse {
                    status: 302,
                    location: Some(format!("{repo}/releases/tag/{tag}")),
                },
                None => RedirectResponse {
                    status: 404,
                    location: None,
                },
            })
        }

        fn get_api(&self, url: &str, _accept: &str) -> Result<ApiResponse, TransportError> {
            let response = self.get(url)?;
            Ok(ApiResponse {
                status: response.status,
                body: response.body,
                ratelimit_remaining: Some(59),
                ratelimit_reset: None,
            })
        }

        fn get_bytes(&self, url: &str, _max_len: u64) -> Result<BytesResponse, TransportError> {
            self.calls.lock().unwrap().push(url.to_owned());
            Ok(match self.bytes.get(url) {
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
        for (name, text) in files {
            writer
                .start_file(*name, SimpleFileOptions::default())
                .unwrap();
            writer.write_all(text.as_bytes()).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    /// Le risposte delle tre fonti: una release GitHub senza `release.json` con un solo zip,
    /// il file 5108 di WoWInterface e l'elenco di Tukui con ElvUI.
    fn fake() -> Fake {
        let mut fake = Fake::default();
        fake.json.insert(
            LATEST.to_owned(),
            serde_json::json!({
                "tag_name": "v427.8",
                "published_at": "2026-10-23T16:33:18Z",
                "assets": [{"name": "BigWigs-v427.8.zip", "browser_download_url": GITHUB_ZIP}],
            })
            .to_string(),
        );
        fake.json.insert(
            format!("{DETAILS_URL}5108.json"),
            serde_json::json!([{
                "id": 5108, "title": "Clique", "version": "v5.0.15-release",
                "downloadUri": WOWI_ZIP, "lastUpdate": 1_761_000_000_000_i64,
            }])
            .to_string(),
        );
        fake.json.insert(
            ADDONS_URL.to_owned(),
            serde_json::json!([{
                "id": -2, "name": "ElvUI", "url": TUKUI_ZIP, "version": "16.00",
                "patch": ["12.1.0"], "web_url": "https://tukui.org/elvui",
                "last_update": "2026-10-13", "directories": ["ElvUI"],
            }])
            .to_string(),
        );
        fake.bytes.insert(
            GITHUB_ZIP.to_owned(),
            zip_of(&[("BigWigs/BigWigs.toc", "## Interface: 120100\n")]),
        );
        fake.bytes.insert(
            WOWI_ZIP.to_owned(),
            zip_of(&[("Clique/Clique.toc", "## Interface: 120100\n")]),
        );
        fake.bytes.insert(
            TUKUI_ZIP.to_owned(),
            zip_of(&[("ElvUI/ElvUI.toc", "## Interface: 120100\n")]),
        );
        fake
    }

    struct Game {
        _tmp: TempDir,
        installation: Installation,
        addons: std::path::PathBuf,
    }

    fn game() -> Game {
        let tmp = TempDir::new();
        let path = tmp.path().join("World of Warcraft").join("_retail_");
        let addons = path.join("Interface").join("AddOns");
        fs::create_dir_all(&addons).unwrap();
        Game {
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

    /// Le tre fonti sullo stesso trasporto finto.
    struct Sources {
        github: GithubSource<Fake>,
        wowinterface: WowInterfaceSource<Fake>,
        tukui: TukuiSource<Fake>,
        calls: Arc<Mutex<Vec<String>>>,
    }

    impl Sources {
        fn new() -> Self {
            let calls = Arc::new(Mutex::new(Vec::new()));
            let make = || Fake {
                calls: Arc::clone(&calls),
                ..fake()
            };
            Self {
                github: GithubSource::new(make()),
                wowinterface: WowInterfaceSource::new(make()),
                tukui: TukuiSource::new(make()),
                calls: Arc::clone(&calls),
            }
        }

        fn links(&self) -> LinkSources<'_, Fake> {
            LinkSources {
                github: &self.github,
                wowinterface: &self.wowinterface,
                tukui: &self.tukui,
            }
        }

        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    /// Le tre fonti con il progetto, la cartella dello zip, il nome e la versione.
    const CASES: [(SourceKind, &str, &str, &str, &str); 3] = [
        (SourceKind::Github, REPO, "BigWigs", "BigWigs", "v427.8"),
        (
            SourceKind::Wowinterface,
            "5108",
            "Clique",
            "Clique",
            "v5.0.15-release",
        ),
        (SourceKind::Tukui, "-2", "ElvUI", "ElvUI", "16.00"),
    ];

    fn target(source: SourceKind, key: &str) -> ProjectKey {
        ProjectKey {
            source,
            key: key.to_owned(),
        }
    }

    fn run_install(
        game: &Game,
        sources: &Sources,
        records: &SourceRecords,
        source: SourceKind,
        key: &str,
    ) -> (UpdateOutcome, Vec<(SourceKind, String, SourceRecord)>) {
        let mut written = Vec::new();
        let outcome = install(
            &game.installation,
            &sources.links(),
            records,
            source,
            key,
            &mut |source, key, record| {
                written.push((source, key.to_owned(), record));
                Ok(())
            },
            &progress::ignore,
        );
        (outcome, written)
    }

    fn recorded(source: SourceKind, key: &str, folder: &str) -> SourceRecords {
        SourceRecords::from([(
            source,
            ProjectRecords::from([(
                key.to_owned(),
                SourceRecord {
                    file: "old".to_owned(),
                    folders: vec![folder.to_owned()],
                },
            )]),
        )])
    }

    #[test]
    fn the_preview_reads_name_version_and_page_from_each_source() {
        let game = game();
        for (source, key, _, name, version) in CASES {
            let sources = Sources::new();
            let preview = preview(
                &game.installation,
                &sources.links(),
                &SourceRecords::new(),
                &target(source, key),
            )
            .unwrap();
            assert_eq!(preview.source, source);
            assert_eq!(preview.id, key);
            assert_eq!(preview.name, name);
            assert_eq!(preview.version, version);
            assert!(preview.downloadable, "{preview:?}");
            assert_eq!(preview.message, None);
            assert!(!preview.installed);
            assert!(preview.url.starts_with("https://"), "{}", preview.url);
            // GitHub legge il tag senza l'API, poi, senza `release.json`, gli asset dall'API.
            let requests = if source == SourceKind::Github { 3 } else { 1 };
            assert_eq!(sources.calls().len(), requests, "{:?}", sources.calls());
        }
    }

    #[test]
    fn the_preview_says_when_a_github_release_has_no_zip_to_pick() {
        let game = game();
        let mut sources = Sources::new();
        let mut two_zips = fake();
        two_zips.json.insert(
            LATEST.to_owned(),
            serde_json::json!({
                "tag_name": "v1",
                "assets": [
                    {"name": "A.zip", "browser_download_url": "https://github.com/a/A.zip"},
                    {"name": "B.zip", "browser_download_url": "https://github.com/a/B.zip"},
                ],
            })
            .to_string(),
        );
        sources.github = GithubSource::new(two_zips);
        let preview = preview(
            &game.installation,
            &sources.links(),
            &SourceRecords::new(),
            &target(SourceKind::Github, REPO),
        )
        .unwrap();
        assert!(!preview.downloadable);
        assert!(preview.message.is_some());
    }

    #[test]
    fn the_preview_marks_an_installed_project_and_fails_on_an_unknown_one() {
        let game = game();
        fs::create_dir_all(game.addons.join("clique")).unwrap();
        let sources = Sources::new();
        let records = recorded(SourceKind::Wowinterface, "5108", "Clique");
        let preview = preview(
            &game.installation,
            &sources.links(),
            &records,
            &target(SourceKind::Wowinterface, "5108"),
        )
        .unwrap();
        assert!(preview.installed);

        let error = super::preview(
            &game.installation,
            &sources.links(),
            &records,
            &target(SourceKind::Wowinterface, "999"),
        )
        .unwrap_err();
        assert!(error.contains("999"), "{error}");
    }

    #[test]
    fn each_source_installs_and_writes_its_record() {
        for (source, key, folder, name, version) in CASES {
            let game = game();
            let sources = Sources::new();
            let (outcome, written) =
                run_install(&game, &sources, &SourceRecords::new(), source, key);
            assert_eq!(outcome.status, OutcomeStatus::Updated, "{outcome:?}");
            assert_eq!((outcome.source, outcome.key.as_str()), (source, key));
            assert_eq!(outcome.name, name);
            assert_eq!(outcome.file.as_ref().unwrap().name, version);
            assert!(game.addons.join(folder).is_dir());
            assert_eq!(written.len(), 1);
            let (written_source, written_key, record) = &written[0];
            assert_eq!((*written_source, written_key.as_str()), (source, key));
            assert_eq!(record.file, version);
            assert_eq!(record.folders, [folder]);
        }
    }

    #[test]
    fn an_installed_project_is_skipped_without_network() {
        for (source, key, folder, _, _) in CASES {
            let game = game();
            fs::create_dir_all(game.addons.join(folder)).unwrap();
            let sources = Sources::new();
            let records = recorded(source, key, folder);
            let (outcome, written) = run_install(&game, &sources, &records, source, key);
            assert_eq!(outcome.status, OutcomeStatus::Skipped, "{outcome:?}");
            assert_eq!(outcome.message.as_deref(), Some("Already installed."));
            assert!(written.is_empty());
            assert!(sources.calls().is_empty(), "{:?}", sources.calls());
        }
    }

    #[test]
    fn a_record_whose_folders_are_gone_does_not_count_as_installed() {
        let game = game();
        let sources = Sources::new();
        let records = recorded(SourceKind::Tukui, "-2", "ElvUI");
        let (outcome, _) = run_install(&game, &sources, &records, SourceKind::Tukui, "-2");
        assert_eq!(outcome.status, OutcomeStatus::Updated, "{outcome:?}");
    }

    #[test]
    fn a_github_record_matches_whatever_the_case() {
        let game = game();
        fs::create_dir_all(game.addons.join("BigWigs")).unwrap();
        let sources = Sources::new();
        let records = recorded(SourceKind::Github, "bigwigsmods/bigwigs", "BigWigs");
        let (outcome, _) = run_install(&game, &sources, &records, SourceKind::Github, REPO);
        assert_eq!(outcome.status, OutcomeStatus::Skipped, "{outcome:?}");
    }

    #[test]
    fn a_folder_of_another_project_fails_without_touching_the_disk() {
        for (source, key, folder, _, _) in CASES {
            // Un record di un'altra fonte nomina la cartella dello zip.
            let game = game();
            let existing = game.addons.join(folder.to_lowercase());
            fs::create_dir_all(&existing).unwrap();
            fs::write(existing.join("Mine.lua"), "mine").unwrap();
            let sources = Sources::new();
            let records = recorded(SourceKind::Curseforge, "61284", folder);
            let (outcome, written) = run_install(&game, &sources, &records, source, key);
            assert_eq!(outcome.status, OutcomeStatus::Error, "{outcome:?}");
            let message = outcome.message.unwrap();
            assert!(
                message.contains("belongs to another installed addon"),
                "{message}"
            );
            assert!(message.contains("61284"), "{message}");
            assert!(written.is_empty());
            assert!(existing.join("Mine.lua").is_file());

            // Senza record, il `## X-Curse-Project-ID` del `.toc` installato.
            let game = super::tests::game();
            let existing = game.addons.join(folder);
            fs::create_dir_all(&existing).unwrap();
            fs::write(
                existing.join(format!("{folder}.toc")),
                "## Interface: 120100\n## X-Curse-Project-ID: 1234\n",
            )
            .unwrap();
            let sources = Sources::new();
            let (outcome, _) = run_install(&game, &sources, &SourceRecords::new(), source, key);
            assert_eq!(outcome.status, OutcomeStatus::Error, "{outcome:?}");
            assert!(outcome.message.unwrap().contains("CurseForge project 1234"));
            assert_eq!(
                fs::read_to_string(existing.join(format!("{folder}.toc"))).unwrap(),
                "## Interface: 120100\n## X-Curse-Project-ID: 1234\n"
            );
        }
    }

    /// Un `AddOns` che non si legge (qui un file al suo posto) è un errore senza rete, non
    /// "non installato".
    #[test]
    fn an_unreadable_addons_folder_fails_without_network() {
        let game = game();
        fs::remove_dir(&game.addons).unwrap();
        fs::write(&game.addons, "not a folder").unwrap();
        let sources = Sources::new();
        let records = recorded(SourceKind::Tukui, "-2", "ElvUI");
        let error = preview(
            &game.installation,
            &sources.links(),
            &records,
            &target(SourceKind::Tukui, "-2"),
        )
        .unwrap_err();
        assert!(error.contains("AddOns"), "{error}");
        let (outcome, written) = run_install(&game, &sources, &records, SourceKind::Tukui, "-2");
        assert_eq!(outcome.status, OutcomeStatus::Error, "{outcome:?}");
        assert!(written.is_empty());
        assert!(sources.calls().is_empty(), "{:?}", sources.calls());
    }

    #[test]
    fn a_folder_installed_by_hand_is_replaced() {
        let game = game();
        let existing = game.addons.join("Clique");
        fs::create_dir_all(&existing).unwrap();
        fs::write(existing.join("Old.lua"), "old").unwrap();
        let sources = Sources::new();
        let (outcome, _) = run_install(
            &game,
            &sources,
            &SourceRecords::new(),
            SourceKind::Wowinterface,
            "5108",
        );
        assert_eq!(outcome.status, OutcomeStatus::Updated, "{outcome:?}");
        assert!(!existing.join("Old.lua").exists());
        assert!(existing.join("Clique.toc").is_file());
    }

    #[test]
    fn invalid_ids_fail_without_network() {
        let game = game();
        let sources = Sources::new();
        for (source, key) in [
            (SourceKind::Github, "../evil"),
            (SourceKind::Github, "owner"),
            (SourceKind::Wowinterface, "5108,23056"),
            (SourceKind::Wowinterface, "../x"),
            (SourceKind::Tukui, "elvui"),
            (SourceKind::Curseforge, "61284"),
        ] {
            let (outcome, _) = run_install(&game, &sources, &SourceRecords::new(), source, key);
            assert_eq!(outcome.status, OutcomeStatus::Error, "{key}");
            assert!(
                preview(
                    &game.installation,
                    &sources.links(),
                    &SourceRecords::new(),
                    &target(source, key)
                )
                .is_err()
            );
        }
        assert!(sources.calls().is_empty(), "{:?}", sources.calls());
    }
}
