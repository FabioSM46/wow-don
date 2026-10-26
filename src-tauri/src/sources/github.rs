//! Le release GitHub come fonte (D31, issue #108): solo i progetti che l'app ha registrato con un
//! record `github` (`id` = `owner/repo`). Gli addon installati a mano non si abbinano mai a un
//! repository.
//!
//! Il controllo legge `GET https://api.github.com/repos/{owner}/{repo}/releases/latest`, senza
//! token: 60 richieste l'ora per indirizzo. Ogni risposta, anche un errore, si legge una volta
//! per esecuzione. Tra un comando e l'altro (`Cache`) una release letta vale dieci minuti, e
//! dopo una risposta di limite superato non parte nessun'altra richiesta fino al ripristino.
//!
//! Risposta verificata il 2026-10-25 su BigWigs `v427.8`: `tag_name`, `published_at`,
//! `html_url` e `assets[]` con `name` e `browser_download_url` (`BigWigs-v427.8.zip` e
//! `release.json`). Il `release.json` del packager (BigWigsMods/packager) elenca
//! `releases[]` con `filename`, `nolib` e `metadata[]` di `flavor`/`interface`.
//!
//! La versione installata è il tag del record; c'è un aggiornamento quando il tag di `latest` è
//! diverso. Lo zip si sceglie così:
//!
//! 1. con `release.json`, la voce il cui `metadata` contiene il flavor del packager per il gioco
//!    dell'installazione (vedi [`packager_flavor`]), preferendo `nolib: false`;
//! 2. senza `release.json`, l'unico asset `.zip` che non termina in `-nolib.zip`;
//! 3. altrimenti il progetto si aggiorna solo dal sito (`allowModDistribution` falso).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Deserialize;

use super::curseforge::{MAX_DOWNLOAD_BYTES, TransportError};
use super::http::{DownloadProgress, PublicTransport, is_allowed_url};
use super::{RecordWriter, Source, SourceKind, UpdateTarget};
use crate::addons::scan::InstalledAddon;
use crate::addons::toc;
use crate::installations::Installation;
use crate::progress::{ProgressSink, Reporter};
use crate::store::{ProjectRecords, SourceRecord};
use crate::updates::{self, AddonUpdate, FileSummary, Project, UpdateOutcome, UpdateStatus};

pub const API_URL: &str = "https://api.github.com";
/// I domini da cui GitHub serve l'API e gli asset (`github.com` rimanda gli asset a
/// `release-assets.githubusercontent.com`), con i loro sottodomini.
pub const DOMAINS: [&str; 2] = ["github.com", "githubusercontent.com"];
/// Il tipo di risposta che l'API di GitHub raccomanda.
const ACCEPT: &str = "application/vnd.github+json";
/// Il nome dell'asset scritto dal packager.
const MANIFEST: &str = "release.json";

const GITHUB: SourceKind = SourceKind::Github;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct Release {
    tag_name: String,
    #[serde(default)]
    published_at: Option<String>,
    #[serde(default)]
    assets: Vec<Asset>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

/// Il `release.json` del packager.
#[derive(Debug, Clone, Deserialize)]
struct Manifest {
    releases: Vec<ManifestFile>,
}

#[derive(Debug, Clone, Deserialize)]
struct ManifestFile {
    filename: String,
    #[serde(default)]
    nolib: bool,
    #[serde(default)]
    metadata: Vec<ManifestFlavor>,
}

#[derive(Debug, Clone, Deserialize)]
struct ManifestFlavor {
    flavor: String,
}

/// Lo zip della release per l'installazione: un asset da scaricare oppure `None`, solo dal
/// sito. `Err` è il motivo per cui la release non si può usare.
type Choice = Result<Option<Asset>, String>;

/// Quanto vale una release letta per i comandi successivi: ogni "Reload" rifà il controllo, e
/// senza questa memoria N progetti GitHub costerebbero N delle 60 richieste l'ora a ogni giro.
pub const RELEASE_TTL: Duration = Duration::from_secs(10 * 60);
/// Quanto si aspetta dopo un limite superato senza `x-ratelimit-reset`.
const LIMIT_FALLBACK: Duration = Duration::from_secs(5 * 60);

/// Le risposte di GitHub che valgono tra un comando e l'altro: le release lette con successo,
/// per [`RELEASE_TTL`], e il limite di richieste superato, fino all'ora di ripristino. Gli altri
/// errori valgono solo per il comando che li ha ricevuti.
#[derive(Default)]
pub struct Cache {
    releases: Mutex<HashMap<String, (Instant, Release)>>,
    /// Il messaggio del limite e fino a quando vale.
    limited: Mutex<Option<(SystemTime, String)>>,
}

impl Cache {
    fn release(&self, repo: &str) -> Option<Release> {
        lock(&self.releases)
            .get(repo)
            .filter(|(read, _)| read.elapsed() < RELEASE_TTL)
            .map(|(_, release)| release.clone())
    }

    fn limit(&self) -> Option<String> {
        lock(&self.limited)
            .as_ref()
            .filter(|(until, _)| SystemTime::now() < *until)
            .map(|(_, message)| message.clone())
    }
}

/// La fonte GitHub. Ogni risposta, anche un errore, si tiene per tutta la vita della fonte, che
/// dura un comando: l'aggiornamento riusa il controllo che lo precede. Le release lette e il
/// limite superato passano anche ai comandi successivi attraverso [`Cache`].
pub struct GithubSource<T: PublicTransport> {
    transport: T,
    cache: Arc<Cache>,
    releases: Mutex<HashMap<String, Result<Release, String>>>,
    manifests: Mutex<HashMap<String, Result<Manifest, String>>>,
    /// Lo zip scelto dal controllo per ogni progetto con un aggiornamento.
    chosen: Mutex<HashMap<String, (Release, Asset)>>,
}

impl<T: PublicTransport> GithubSource<T> {
    #[cfg(test)]
    pub fn new(transport: T) -> Self {
        Self::with_cache(transport, Arc::default())
    }

    /// Una fonte che condivide `cache` con le altre create dall'app.
    pub fn with_cache(transport: T, cache: Arc<Cache>) -> Self {
        Self {
            transport,
            cache,
            releases: Mutex::new(HashMap::new()),
            manifests: Mutex::new(HashMap::new()),
            chosen: Mutex::new(HashMap::new()),
        }
    }

    /// L'ultima release di `repo`, letta al massimo una volta per comando e al massimo una
    /// volta per [`RELEASE_TTL`].
    fn latest(&self, repo: &str) -> Result<Release, String> {
        if let Some(cached) = lock(&self.releases).get(repo) {
            return cached.clone();
        }
        let result = self.cache.release(repo).map(Ok).unwrap_or_else(|| {
            let fetched = self.fetch_latest(repo);
            if let Ok(release) = &fetched {
                lock(&self.cache.releases)
                    .insert(repo.to_owned(), (Instant::now(), release.clone()));
            }
            fetched
        });
        lock(&self.releases).insert(repo.to_owned(), result.clone());
        result
    }

    fn fetch_latest(&self, repo: &str) -> Result<Release, String> {
        if !is_valid_repo(repo) {
            return Err(format!(
                "\"{repo}\" is not a GitHub repository (owner/repo)."
            ));
        }
        if let Some(message) = self.cache.limit() {
            return Err(message);
        }
        let url = format!("{API_URL}/repos/{repo}/releases/latest");
        let response = self
            .transport
            .get_api(&url, ACCEPT)
            .map_err(|error| match error {
                TransportError::TooLarge => {
                    "Unrecognized response from GitHub: it is too large.".to_owned()
                }
                other => network_error(other),
            })?;
        match response.status {
            200 => serde_json::from_str(&response.body)
                .map_err(|_| "Unrecognized response from GitHub.".to_owned()),
            404 => Err(format!("GitHub has no published release for {repo}.")),
            403 | 429 if response.ratelimit_remaining == Some(0) => {
                let message = rate_limit_message(response.ratelimit_reset);
                let until = response.ratelimit_reset.map_or_else(
                    || SystemTime::now() + LIMIT_FALLBACK,
                    |reset| UNIX_EPOCH + Duration::from_secs(reset),
                );
                *lock(&self.cache.limited) = Some((until, message.clone()));
                Err(message)
            }
            status => Err(format!("GitHub responded with an error (HTTP {status}).")),
        }
    }

    /// Il `release.json` all'indirizzo `url`, letto al massimo una volta. Si scarica da
    /// `github.com`, non dall'API: non conta nel limite di richieste.
    fn manifest(&self, url: &str) -> Result<Manifest, String> {
        if let Some(cached) = lock(&self.manifests).get(url) {
            return cached.clone();
        }
        let result = self.fetch_manifest(url);
        lock(&self.manifests).insert(url.to_owned(), result.clone());
        result
    }

    fn fetch_manifest(&self, url: &str) -> Result<Manifest, String> {
        if !is_allowed(url) {
            return Err("GitHub gave an unrecognized address for release.json.".to_owned());
        }
        let unreadable =
            || "The release.json of the latest GitHub release is unreadable.".to_owned();
        let response = self.transport.get(url).map_err(|error| match error {
            TransportError::TooLarge => unreadable(),
            other => network_error(other),
        })?;
        if response.status != 200 {
            return Err(format!(
                "GitHub responded with an error (HTTP {}).",
                response.status
            ));
        }
        serde_json::from_str(&response.body).map_err(|_| unreadable())
    }

    /// Lo zip di `release` per il gioco `flavor` (vedi la documentazione del modulo).
    fn choose(&self, release: &Release, flavor: &str) -> Choice {
        let Some(manifest) = release.assets.iter().find(|a| a.name == MANIFEST) else {
            let mut zips = release.assets.iter().filter(|a| {
                let name = a.name.to_ascii_lowercase();
                name.ends_with(".zip") && !name.ends_with("-nolib.zip")
            });
            return Ok(match (zips.next(), zips.next()) {
                (Some(only), None) => Some(only.clone()),
                _ => None,
            });
        };
        let manifest = self.manifest(&manifest.browser_download_url)?;
        let no_file = || "The latest GitHub release has no file for this game version.".to_owned();
        let wanted = packager_flavor(flavor).ok_or_else(no_file)?;
        let file = manifest
            .releases
            .iter()
            .filter(|file| file.metadata.iter().any(|m| m.flavor == wanted))
            .min_by_key(|file| file.nolib)
            .ok_or_else(no_file)?;
        release
            .assets
            .iter()
            .find(|a| a.name == file.filename)
            .cloned()
            .map(Some)
            .ok_or_else(|| {
                format!(
                    "The latest GitHub release does not include {}.",
                    file.filename
                )
            })
    }

    fn download(
        &self,
        asset: &Asset,
        progress: &mut DownloadProgress<'_>,
    ) -> Result<Vec<u8>, String> {
        if !is_allowed(&asset.browser_download_url) {
            return Err("GitHub gave an unrecognized download address: download cancelled.".into());
        }
        let response = self
            .transport
            .get_bytes_with_progress(&asset.browser_download_url, MAX_DOWNLOAD_BYTES, progress)
            .map_err(|error| match error {
                TransportError::TooLarge => format!(
                    "The file to download exceeds the {} MiB limit: download cancelled.",
                    MAX_DOWNLOAD_BYTES / (1024 * 1024)
                ),
                other => network_error(other),
            })?;
        if response.status != 200 {
            return Err(format!(
                "GitHub responded with an error (HTTP {}).",
                response.status
            ));
        }
        Ok(response.body)
    }

    /// Scarica `asset` e lo installa al posto di `old_folders`, poi registra il tag. L'esito usa
    /// la forma di un aggiornamento: `updated` vuol dire installato.
    #[allow(clippy::too_many_arguments)]
    fn install(
        &self,
        key: &str,
        release: &Release,
        asset: &Asset,
        addons_dir: &std::path::Path,
        old_folders: &[String],
        record: &mut RecordWriter<'_>,
        progress: &ProgressSink<'_>,
    ) -> UpdateOutcome {
        let name = repo_name(key);
        let reporter = Reporter::new(progress, Some(GITHUB), key, name);
        reporter.download(0, None);
        let bytes = match self.download(asset, &mut |received, total| {
            reporter.download(received, total)
        }) {
            Ok(bytes) => bytes,
            Err(error) => return UpdateOutcome::error(GITHUB, key, name, error),
        };
        reporter.downloaded(bytes.len() as u64);
        let folders =
            match updates::install_bytes(&bytes, addons_dir, old_folders, &|_| Ok(()), &reporter) {
                Ok(folders) => folders,
                Err(error) => return UpdateOutcome::error(GITHUB, key, name, error),
            };
        // Le cartelle sono già sostituite: se il registro non si salva l'installazione resta
        // riuscita, e il messaggio lo dice.
        let message = record(
            GITHUB,
            key,
            SourceRecord {
                file: release.tag_name.clone(),
                folders,
            },
        )
        .err()
        .map(|error| format!("Installed, but the app could not remember its folders: {error}"));
        UpdateOutcome::updated(GITHUB, key, name, summary_of(release), message)
    }

    /// Installa un progetto GitHub che non c'è ancora (l'import, issue #134), con la stessa
    /// scelta dello zip del controllo.
    pub fn install_new(
        &self,
        installation: &Installation,
        key: &str,
        record: &mut RecordWriter<'_>,
        progress: &ProgressSink<'_>,
    ) -> UpdateOutcome {
        let name = repo_name(key);
        let release = match self.latest(key) {
            Ok(release) => release,
            Err(error) => return UpdateOutcome::error(GITHUB, key, name, error),
        };
        let asset = match self.choose(&release, &installation.flavor) {
            Ok(Some(asset)) => asset,
            Ok(None) => return UpdateOutcome::skipped(GITHUB, key, name, WEBSITE_ONLY),
            Err(error) => return UpdateOutcome::skipped(GITHUB, key, name, &error),
        };
        let addons_dir = installation.path.join("Interface").join("AddOns");
        self.install(key, &release, &asset, &addons_dir, &[], record, progress)
    }
}

const WEBSITE_ONLY: &str = "The latest GitHub release has no single zip WoW-don can pick: download it from the release page, then drop the zip into this window.";

fn lock<V>(mutex: &Mutex<V>) -> std::sync::MutexGuard<'_, V> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

fn is_allowed(url: &str) -> bool {
    reqwest::Url::parse(url).is_ok_and(|url| is_allowed_url(&url, &DOMAINS))
}

/// `owner/repo` con i caratteri che GitHub ammette nei nomi: lettere e cifre ASCII, `-`, `_` e
/// `.`, senza `.` o `..` da soli. Il testo finisce in un URL dell'API.
pub(crate) fn is_valid_repo(repo: &str) -> bool {
    let Some((owner, name)) = repo.split_once('/') else {
        return false;
    };
    [owner, name].iter().all(|part| {
        !part.is_empty()
            && part.len() <= 100
            && *part != "."
            && *part != ".."
            && part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    })
}

/// Il nome mostrato per un progetto: la parte del repository dopo il proprietario.
fn repo_name(repo: &str) -> &str {
    repo.split_once('/').map_or(repo, |(_, name)| name)
}

/// Il flavor del packager per il gioco dell'installazione, ricavato dalla stessa famiglia di
/// `toc::toc_suffixes`, così le due tabelle non si separano. Verificata sul `release.json` di
/// BigWigs `v427.8` (2026-10-25):
///
/// | Cartella | Gioco | Flavor del packager |
/// | --- | --- | --- |
/// | `_retail_` (e PTR, beta) | Retail | `mainline` |
/// | `_classic_era_` | Classic Era | `classic` |
/// | `_classic_beta_` | WoW Forever | `forever` |
/// | `_anniversary_` | Anniversary (TBC) | `bcc` |
/// | `_classic_` | Classic progressiva (Mists) | `mists` |
///
/// `wrath`, `cata` e `titan` non hanno oggi una cartella del client.
fn packager_flavor(flavor: &str) -> Option<&'static str> {
    match *toc::toc_suffixes(flavor).first()? {
        "Standard" => Some("mainline"),
        "Forever" => Some("forever"),
        "Vanilla" => Some("classic"),
        "TBC" => Some("bcc"),
        "Mists" => Some("mists"),
        // Una famiglia che `toc_suffixes` aggiungerà in futuro non diventa Retail per sbaglio.
        _ => None,
    }
}

fn network_error(error: TransportError) -> String {
    match error {
        TransportError::Timeout => "GitHub did not respond in time.".to_owned(),
        _ => "Could not reach GitHub: check the connection.".to_owned(),
    }
}

/// Il messaggio del limite di 60 richieste l'ora, con l'ora (UTC) in cui GitHub lo azzera.
fn rate_limit_message(reset: Option<u64>) -> String {
    match reset {
        Some(reset) => {
            let seconds = reset % 86_400;
            format!(
                "GitHub's limit of 60 requests an hour has been reached: try again after {:02}:{:02} UTC.",
                seconds / 3_600,
                seconds % 3_600 / 60
            )
        }
        None => {
            "GitHub's limit of 60 requests an hour has been reached: try again later.".to_owned()
        }
    }
}

/// Un file GitHub: nessun id numerico, il tag come nome (vedi `FileSummary`).
fn summary(tag: &str, date: Option<&str>) -> FileSummary {
    FileSummary {
        id: 0,
        name: tag.to_owned(),
        date: date.unwrap_or_default().to_owned(),
    }
}

fn summary_of(release: &Release) -> FileSummary {
    summary(&release.tag_name, release.published_at.as_deref())
}

impl<T: PublicTransport> Source for GithubSource<T> {
    fn kind(&self) -> SourceKind {
        GITHUB
    }

    fn check(
        &self,
        installation: &Installation,
        offered: &[InstalledAddon],
        records: &ProjectRecords,
    ) -> Result<Vec<AddonUpdate>, String> {
        let mut updates = Vec::new();
        for (key, record) in records {
            let present: Vec<&InstalledAddon> = offered
                .iter()
                .filter(|a| {
                    record
                        .folders
                        .iter()
                        .any(|f| f.eq_ignore_ascii_case(&a.folder))
                })
                .collect();
            if present.is_empty() {
                continue;
            }
            let mut downloadable = true;
            let status = match self.latest(key) {
                Err(message) => UpdateStatus::Error { message },
                Ok(release) if release.tag_name == record.file => UpdateStatus::UpToDate,
                Ok(release) => match self.choose(&release, &installation.flavor) {
                    Err(message) => UpdateStatus::Error { message },
                    Ok(asset) => {
                        match asset {
                            Some(asset) => {
                                lock(&self.chosen).insert(key.clone(), (release.clone(), asset));
                            }
                            None => downloadable = false,
                        }
                        UpdateStatus::UpdateAvailable {
                            latest: summary_of(&release),
                        }
                    }
                },
            };
            for folder in present {
                updates.push(AddonUpdate {
                    folder: folder.folder.clone(),
                    source: Some(GITHUB),
                    project: Some(Project {
                        id: 0,
                        source: GITHUB,
                        key: key.clone(),
                        name: repo_name(key).to_owned(),
                        url: is_valid_repo(key).then(|| format!("https://github.com/{key}")),
                        allow_mod_distribution: downloadable,
                    }),
                    installed_file: Some(summary(&record.file, None)),
                    status: status.clone(),
                    version_override: None,
                    project_folders: record.folders.clone(),
                    installed_modules: Vec::new(),
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
                    .is_some_and(|p| p.source == GITHUB && p.key == key)
            })
            .collect();
        let Some(project) = folders.first().and_then(|u| u.project.as_ref()) else {
            return UpdateOutcome::skipped(
                GITHUB,
                key,
                "",
                "Not found among the installed addons.",
            );
        };
        let name = project.name.as_str();
        if !folders
            .iter()
            .any(|u| matches!(u.status, UpdateStatus::UpdateAvailable { .. }))
        {
            let reason = folders.iter().find_map(|u| match &u.status {
                UpdateStatus::Error { message } => Some(message.as_str()),
                _ => None,
            });
            return UpdateOutcome::skipped(
                GITHUB,
                key,
                name,
                reason.unwrap_or("Already up to date."),
            );
        }
        if !project.allow_mod_distribution {
            return UpdateOutcome::skipped(GITHUB, key, name, WEBSITE_ONLY);
        }
        let Some((release, asset)) = lock(&self.chosen).get(key).cloned() else {
            return UpdateOutcome::skipped(GITHUB, key, name, "Check for updates again.");
        };
        // Le cartelle del progetto sul disco: quelle che il nuovo zip non ha più si tolgono.
        let old_folders: Vec<String> = folders.iter().map(|u| u.folder.clone()).collect();
        self.install(
            key,
            &release,
            &asset,
            target.addons_dir,
            &old_folders,
            record,
            target.progress,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::addons::toc::TocInfo;
    use crate::progress;
    use crate::sources::http::{ApiResponse, BytesResponse, HttpResponse};
    use crate::store::SourceRecords;
    use crate::test_support::TempDir;
    use std::fs;
    use std::io::{Cursor, Write};
    use std::path::PathBuf;
    use std::sync::Arc;
    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;

    const REPO: &str = "BigWigsMods/BigWigs";
    const LATEST: &str = "https://api.github.com/repos/BigWigsMods/BigWigs/releases/latest";
    const DOWNLOADS: &str = "https://github.com/BigWigsMods/BigWigs/releases/download/v427.8";

    /// Le risposte del trasporto finto, per URL.
    #[derive(Default)]
    struct Fake {
        api: HashMap<String, Result<ApiResponse, TransportError>>,
        json: HashMap<String, HttpResponse>,
        bytes: HashMap<String, Vec<u8>>,
        calls: Arc<Mutex<Vec<String>>>,
    }

    impl PublicTransport for Fake {
        fn get(&self, url: &str) -> Result<HttpResponse, TransportError> {
            self.calls.lock().unwrap().push(url.to_owned());
            match self.json.get(url) {
                Some(response) => Ok(HttpResponse {
                    status: response.status,
                    body: response.body.clone(),
                }),
                None => Ok(HttpResponse {
                    status: 404,
                    body: String::new(),
                }),
            }
        }

        fn get_api(&self, url: &str, accept: &str) -> Result<ApiResponse, TransportError> {
            assert_eq!(accept, ACCEPT);
            self.calls.lock().unwrap().push(url.to_owned());
            match self.api.get(url) {
                Some(Ok(response)) => Ok(ApiResponse {
                    status: response.status,
                    body: response.body.clone(),
                    ratelimit_remaining: response.ratelimit_remaining,
                    ratelimit_reset: response.ratelimit_reset,
                }),
                Some(Err(error)) => Err(*error),
                None => Err(TransportError::Connect),
            }
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

    fn ok(body: String) -> Result<ApiResponse, TransportError> {
        Ok(ApiResponse {
            status: 200,
            body,
            ratelimit_remaining: Some(59),
            ratelimit_reset: None,
        })
    }

    /// La release reale del 2026-10-25, ridotta ai campi usati, con gli asset `assets`.
    fn release(tag: &str, assets: &[&str]) -> String {
        let assets: Vec<_> = assets
            .iter()
            .map(|name| {
                serde_json::json!({
                    "name": name,
                    "browser_download_url": format!("{DOWNLOADS}/{name}"),
                    "size": 1,
                })
            })
            .collect();
        serde_json::json!({
            "tag_name": tag, "name": tag, "draft": false, "prerelease": false,
            "published_at": "2026-10-23T16:33:18Z",
            "html_url": format!("https://github.com/{REPO}/releases/tag/{tag}"),
            "assets": assets,
        })
        .to_string()
    }

    /// Il `release.json` reale di BigWigs `v427.8` (flavor ridotti), più una variante `-nolib`.
    fn manifest() -> String {
        serde_json::json!({"releases": [
            {"name": "BigWigs", "version": "v427.8", "filename": "BigWigs-v427.8-nolib.zip",
             "nolib": true, "metadata": [{"flavor": "mainline", "interface": 120100}]},
            {"name": "BigWigs", "version": "v427.8", "filename": "BigWigs-v427.8.zip",
             "nolib": false, "metadata": [
                {"flavor": "mists", "interface": 50503},
                {"flavor": "mainline", "interface": 120100},
                {"flavor": "classic", "interface": 11508},
                {"flavor": "bcc", "interface": 20505},
                {"flavor": "forever", "interface": 16001}
             ]}
        ]})
        .to_string()
    }

    fn with_manifest(fake: Fake, body: &str) -> Fake {
        let mut fake = fake;
        fake.json.insert(
            format!("{DOWNLOADS}/{MANIFEST}"),
            HttpResponse {
                status: 200,
                body: body.to_owned(),
            },
        );
        fake
    }

    fn fake(tag: &str, assets: &[&str]) -> Fake {
        let mut fake = Fake::default();
        fake.api.insert(LATEST.to_owned(), ok(release(tag, assets)));
        with_manifest(fake, &manifest())
    }

    fn installation(flavor: &str) -> Installation {
        Installation {
            id: "test".to_owned(),
            path: PathBuf::from("unused"),
            flavor: flavor.to_owned(),
            name: "Test".to_owned(),
        }
    }

    fn addon(folder: &str) -> InstalledAddon {
        InstalledAddon {
            folder: folder.to_owned(),
            toc_file: Some(format!("{folder}.toc")),
            toc: TocInfo::default(),
            modified_ms: None,
            fingerprint: Some(1),
            error: None,
        }
    }

    fn records(tag: &str) -> ProjectRecords {
        ProjectRecords::from([(
            REPO.to_owned(),
            SourceRecord {
                file: tag.to_owned(),
                folders: vec!["BigWigs".to_owned(), "BigWigs_Core".to_owned()],
            },
        )])
    }

    fn offered() -> Vec<InstalledAddon> {
        vec![addon("BigWigs"), addon("bigwigs_core"), addon("Other")]
    }

    fn check(fake: Fake, flavor: &str, tag: &str) -> Vec<AddonUpdate> {
        GithubSource::new(fake)
            .check(&installation(flavor), &offered(), &records(tag))
            .unwrap()
    }

    fn chosen(source: &GithubSource<Fake>) -> Option<String> {
        lock(&source.chosen).get(REPO).map(|(_, a)| a.name.clone())
    }

    #[test]
    fn the_same_tag_is_up_to_date_and_reads_nothing_else() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut fake = fake("v427.8", &["BigWigs-v427.8.zip", MANIFEST]);
        fake.calls = Arc::clone(&calls);
        let updates = check(fake, "retail", "v427.8");
        let folders: Vec<_> = updates
            .iter()
            .map(|u| (u.folder.as_str(), &u.status))
            .collect();
        assert_eq!(
            folders,
            [
                ("BigWigs", &UpdateStatus::UpToDate),
                ("bigwigs_core", &UpdateStatus::UpToDate)
            ]
        );
        let project = updates[0].project.as_ref().unwrap();
        assert_eq!(project.source, SourceKind::Github);
        assert_eq!(project.key, REPO);
        assert_eq!(project.name, "BigWigs");
        assert_eq!(
            project.url.as_deref(),
            Some("https://github.com/BigWigsMods/BigWigs")
        );
        assert!(project.allow_mod_distribution);
        assert_eq!(updates[0].installed_file, Some(summary("v427.8", None)));
        assert_eq!(*calls.lock().unwrap(), [LATEST]);
    }

    #[test]
    fn a_different_tag_picks_the_zip_for_the_flavor_from_release_json() {
        let source = GithubSource::new(fake(
            "v427.8",
            &["BigWigs-v427.8-nolib.zip", "BigWigs-v427.8.zip", MANIFEST],
        ));
        let updates = source
            .check(&installation("retail"), &offered(), &records("v427.7"))
            .unwrap();
        assert_eq!(
            updates[0].status,
            UpdateStatus::UpdateAvailable {
                latest: summary("v427.8", Some("2026-10-23T16:33:18Z"))
            }
        );
        // `mainline` è sia nel file `-nolib` sia in quello completo: vince `nolib: false`.
        assert_eq!(chosen(&source).as_deref(), Some("BigWigs-v427.8.zip"));
    }

    #[test]
    fn a_flavor_missing_from_release_json_is_an_error() {
        let manifest = serde_json::json!({"releases": [
            {"filename": "BigWigs-v427.8.zip", "nolib": false,
             "metadata": [{"flavor": "mainline", "interface": 120100}]}
        ]})
        .to_string();
        let fake = with_manifest(fake("v427.8", &["BigWigs-v427.8.zip", MANIFEST]), &manifest);
        let updates = check(fake, "anniversary", "v427.7");
        assert_eq!(
            updates[0].status,
            UpdateStatus::Error {
                message: "The latest GitHub release has no file for this game version.".to_owned()
            }
        );
    }

    #[test]
    fn packager_flavors_follow_the_toc_families() {
        assert_eq!(packager_flavor("retail"), Some("mainline"));
        assert_eq!(packager_flavor("_retail_"), Some("mainline"));
        assert_eq!(packager_flavor("ptr"), Some("mainline"));
        assert_eq!(packager_flavor("classic_era"), Some("classic"));
        assert_eq!(packager_flavor("classic_beta"), Some("forever"));
        assert_eq!(packager_flavor("anniversary"), Some("bcc"));
        assert_eq!(packager_flavor("classic"), Some("mists"));
        // Ogni flavor della tabella esiste nel `release.json` reale.
        let manifest: Manifest = serde_json::from_str(&manifest()).unwrap();
        for flavor in [
            "retail",
            "classic_era",
            "classic_beta",
            "anniversary",
            "classic",
        ] {
            let wanted = packager_flavor(flavor).unwrap();
            assert!(
                manifest.releases[1]
                    .metadata
                    .iter()
                    .any(|m| m.flavor == wanted),
                "{flavor}"
            );
        }
    }

    #[test]
    fn without_release_json_the_only_full_zip_is_picked() {
        let source = GithubSource::new(fake("v2", &["Addon-v2.zip", "Addon-v2-nolib.zip"]));
        let updates = source
            .check(&installation("retail"), &offered(), &records("v1"))
            .unwrap();
        assert!(matches!(
            updates[0].status,
            UpdateStatus::UpdateAvailable { .. }
        ));
        assert!(updates[0].project.as_ref().unwrap().allow_mod_distribution);
        assert_eq!(chosen(&source).as_deref(), Some("Addon-v2.zip"));
    }

    #[test]
    fn two_zips_or_only_nolib_are_website_only() {
        for assets in [
            &["A-v2.zip", "B-v2.zip"][..],
            &["A-v2-nolib.zip"][..],
            &[][..],
        ] {
            let source = GithubSource::new(fake("v2", assets));
            let updates = source
                .check(&installation("retail"), &offered(), &records("v1"))
                .unwrap();
            assert!(
                matches!(updates[0].status, UpdateStatus::UpdateAvailable { .. }),
                "{assets:?}"
            );
            assert!(!updates[0].project.as_ref().unwrap().allow_mod_distribution);
            assert_eq!(chosen(&source), None);
        }
    }

    #[test]
    fn only_recorded_projects_are_checked() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut fake = fake("v2", &[]);
        fake.calls = Arc::clone(&calls);
        let source = GithubSource::new(fake);
        let updates = source
            .check(&installation("retail"), &offered(), &ProjectRecords::new())
            .unwrap();
        assert!(updates.is_empty());
        // Un record le cui cartelle non sono più sul disco non chiede niente.
        let updates = source
            .check(&installation("retail"), &[addon("Other")], &records("v1"))
            .unwrap();
        assert!(updates.is_empty());
        assert!(calls.lock().unwrap().is_empty());
    }

    #[test]
    fn the_rate_limit_is_a_clear_error_and_stops_further_requests() {
        for status in [403, 429] {
            let calls = Arc::new(Mutex::new(Vec::new()));
            let mut fake = Fake {
                calls: Arc::clone(&calls),
                ..Fake::default()
            };
            fake.api.insert(
                format!("{API_URL}/repos/a/one/releases/latest"),
                Ok(ApiResponse {
                    status,
                    body: "{}".to_owned(),
                    ratelimit_remaining: Some(0),
                    // 2026-10-26 14:32:00 UTC.
                    ratelimit_reset: Some(1_793_025_120),
                }),
            );
            let record = |folder: &str| SourceRecord {
                file: "1".to_owned(),
                folders: vec![folder.to_owned()],
            };
            let records = ProjectRecords::from([
                ("a/one".to_owned(), record("One")),
                ("a/two".to_owned(), record("Two")),
            ]);
            let updates = GithubSource::new(fake)
                .check(
                    &installation("retail"),
                    &[addon("One"), addon("Two")],
                    &records,
                )
                .unwrap();
            let expected = UpdateStatus::Error {
                message: "GitHub's limit of 60 requests an hour has been reached: try again after 14:32 UTC.".to_owned(),
            };
            assert_eq!(updates[0].status, expected, "HTTP {status}");
            assert_eq!(updates[1].status, expected, "HTTP {status}");
            // Il secondo progetto non fa partire una richiesta.
            assert_eq!(calls.lock().unwrap().len(), 1, "HTTP {status}");
        }
        assert_eq!(
            rate_limit_message(None),
            "GitHub's limit of 60 requests an hour has been reached: try again later."
        );
    }

    #[test]
    fn a_shared_cache_spares_requests_across_commands() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let cache = Arc::new(Cache::default());
        let run = || {
            let mut fake = fake("v427.8", &[]);
            fake.calls = Arc::clone(&calls);
            GithubSource::with_cache(fake, Arc::clone(&cache))
                .check(&installation("retail"), &offered(), &records("v427.8"))
                .unwrap()
        };
        assert_eq!(run()[0].status, UpdateStatus::UpToDate);
        assert_eq!(run()[0].status, UpdateStatus::UpToDate);
        // Il secondo comando trova la release ancora valida.
        assert_eq!(*calls.lock().unwrap(), [LATEST]);
        // Scaduta, si rilegge.
        lock(&cache.releases).get_mut(REPO).unwrap().0 -= RELEASE_TTL;
        run();
        assert_eq!(calls.lock().unwrap().len(), 2);

        // Il limite superato vale anche per i comandi successivi, fino al ripristino.
        let limited = Arc::new(Cache::default());
        let in_an_hour = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 3_600;
        let mut first = Fake::default();
        first.api.insert(
            LATEST.to_owned(),
            Ok(ApiResponse {
                status: 403,
                body: String::new(),
                ratelimit_remaining: Some(0),
                ratelimit_reset: Some(in_an_hour),
            }),
        );
        GithubSource::with_cache(first, Arc::clone(&limited))
            .check(&installation("retail"), &offered(), &records("v1"))
            .unwrap();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut second = fake("v2", &[]);
        second.calls = Arc::clone(&calls);
        let updates = GithubSource::with_cache(second, limited)
            .check(&installation("retail"), &offered(), &records("v1"))
            .unwrap();
        assert!(matches!(updates[0].status, UpdateStatus::Error { .. }));
        assert!(calls.lock().unwrap().is_empty());
    }

    #[test]
    fn errors_belong_to_github() {
        let status = |response: Result<ApiResponse, TransportError>| {
            let mut fake = Fake::default();
            fake.api.insert(LATEST.to_owned(), response);
            check(fake, "retail", "v1").remove(0).status
        };
        let error = |message: &str| UpdateStatus::Error {
            message: message.to_owned(),
        };
        assert_eq!(
            status(Ok(ApiResponse {
                status: 200,
                body: "{".to_owned(),
                ratelimit_remaining: None,
                ratelimit_reset: None,
            })),
            error("Unrecognized response from GitHub.")
        );
        assert_eq!(
            status(Ok(ApiResponse {
                status: 404,
                body: String::new(),
                ratelimit_remaining: Some(50),
                ratelimit_reset: None,
            })),
            error("GitHub has no published release for BigWigsMods/BigWigs.")
        );
        // Un 403 con richieste ancora disponibili non è il limite.
        assert_eq!(
            status(Ok(ApiResponse {
                status: 403,
                body: String::new(),
                ratelimit_remaining: Some(50),
                ratelimit_reset: Some(1),
            })),
            error("GitHub responded with an error (HTTP 403).")
        );
        assert_eq!(
            status(Err(TransportError::Timeout)),
            error("GitHub did not respond in time.")
        );
        // Un `release.json` illeggibile.
        let fake = with_manifest(fake("v2", &["A.zip", MANIFEST]), "not json");
        assert_eq!(
            check(fake, "retail", "v1")[0].status,
            error("The release.json of the latest GitHub release is unreadable.")
        );
    }

    #[test]
    fn repositories_are_validated_before_any_request() {
        assert!(is_valid_repo("BigWigsMods/BigWigs"));
        assert!(is_valid_repo("a-b/c_d.e"));
        for bad in [
            "", "owner", "/repo", "owner/", "a/b/c", "a/..", "a/b?x=1", "a/b#c", "a /b",
        ] {
            assert!(!is_valid_repo(bad), "{bad}");
        }
        let calls = Arc::new(Mutex::new(Vec::new()));
        let source = GithubSource::new(Fake {
            calls: Arc::clone(&calls),
            ..Fake::default()
        });
        assert!(source.latest("a/../../x").is_err());
        assert!(calls.lock().unwrap().is_empty());
    }

    // ── Aggiornamento e installazione ──

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

    fn game(tmp: &TempDir) -> (Installation, PathBuf) {
        let path = tmp.path().join("World of Warcraft").join("_retail_");
        let addons = path.join("Interface").join("AddOns");
        fs::create_dir_all(&addons).unwrap();
        let installation = Installation {
            id: "test".to_owned(),
            path,
            flavor: "retail".to_owned(),
            name: "Retail".to_owned(),
        };
        (installation, addons)
    }

    #[test]
    fn an_update_downloads_the_chosen_zip_and_records_the_new_tag() {
        let tmp = TempDir::new();
        let (installation, addons) = game(&tmp);
        for folder in ["BigWigs", "BigWigs_Core"] {
            fs::create_dir_all(addons.join(folder)).unwrap();
            fs::write(addons.join(folder).join(format!("{folder}.toc")), "old").unwrap();
        }
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut fake = fake("v427.8", &["BigWigs-v427.8.zip", MANIFEST]);
        fake.calls = Arc::clone(&calls);
        fake.bytes.insert(
            format!("{DOWNLOADS}/BigWigs-v427.8.zip"),
            zip_of(&[("BigWigs/BigWigs.toc", "new")]),
        );
        let source = GithubSource::new(fake);
        let mut written = Vec::new();
        let outcomes = updates::update_addons(
            &installation,
            &[&source],
            &SourceRecords::from([(GITHUB, records("v427.7"))]),
            &[super::super::ProjectKey {
                source: GITHUB,
                key: REPO.to_owned(),
            }],
            &mut |source, key, record| {
                written.push((source, key.to_owned(), record));
                Ok(())
            },
            &progress::ignore,
        )
        .unwrap();
        assert_eq!(
            outcomes[0].status,
            updates::OutcomeStatus::Updated,
            "{outcomes:?}"
        );
        assert_eq!(
            outcomes[0].file,
            Some(summary("v427.8", Some("2026-10-23T16:33:18Z")))
        );
        assert_eq!(
            fs::read_to_string(addons.join("BigWigs").join("BigWigs.toc")).unwrap(),
            "new"
        );
        // La cartella che il nuovo zip non ha più si toglie.
        assert!(!addons.join("BigWigs_Core").exists());
        assert_eq!(
            written,
            [(
                GITHUB,
                REPO.to_owned(),
                SourceRecord {
                    file: "v427.8".to_owned(),
                    folders: vec!["BigWigs".to_owned()],
                }
            )]
        );
        // La release e il `release.json` si leggono una volta: l'aggiornamento riusa il controllo.
        assert_eq!(
            *calls.lock().unwrap(),
            [
                LATEST.to_owned(),
                format!("{DOWNLOADS}/{MANIFEST}"),
                format!("{DOWNLOADS}/BigWigs-v427.8.zip")
            ]
        );
    }

    #[test]
    fn a_website_only_update_is_skipped() {
        let tmp = TempDir::new();
        let (installation, addons) = game(&tmp);
        fs::create_dir_all(addons.join("BigWigs")).unwrap();
        let source = GithubSource::new(fake("v2", &["A.zip", "B.zip"]));
        let outcomes = updates::update_addons(
            &installation,
            &[&source],
            &SourceRecords::from([(GITHUB, records("v1"))]),
            &[super::super::ProjectKey {
                source: GITHUB,
                key: REPO.to_owned(),
            }],
            &mut |_, _, _| panic!("nothing to record"),
            &progress::ignore,
        )
        .unwrap();
        assert_eq!(outcomes[0].status, updates::OutcomeStatus::Skipped);
        assert_eq!(outcomes[0].message.as_deref(), Some(WEBSITE_ONLY));
    }

    #[test]
    fn a_download_outside_github_is_refused() {
        let asset = Asset {
            name: "a.zip".to_owned(),
            browser_download_url: "https://example.com/a.zip".to_owned(),
        };
        let error = GithubSource::new(Fake::default())
            .download(&asset, &mut |_, _| {})
            .unwrap_err();
        assert!(error.contains("unrecognized download address"), "{error}");
    }

    #[test]
    fn a_new_project_is_installed_and_recorded() {
        let tmp = TempDir::new();
        let (installation, addons) = game(&tmp);
        let mut fake = fake("v427.8", &["BigWigs-v427.8.zip", MANIFEST]);
        fake.bytes.insert(
            format!("{DOWNLOADS}/BigWigs-v427.8.zip"),
            zip_of(&[
                ("BigWigs/BigWigs.toc", "new"),
                ("BigWigs_Core/BigWigs_Core.toc", ""),
            ]),
        );
        let mut written = Vec::new();
        let outcome = GithubSource::new(fake).install_new(
            &installation,
            REPO,
            &mut |_, key, record| {
                written.push(format!(
                    "{key}={}:{}",
                    record.file,
                    record.folders.join(",")
                ));
                Ok(())
            },
            &progress::ignore,
        );
        assert_eq!(
            outcome.status,
            updates::OutcomeStatus::Updated,
            "{outcome:?}"
        );
        assert_eq!(outcome.name, "BigWigs");
        assert!(addons.join("BigWigs_Core").is_dir());
        assert_eq!(written, ["BigWigsMods/BigWigs=v427.8:BigWigs,BigWigs_Core"]);
    }

    /// Legge l'API vera: `cargo test --locked live_github_release -- --ignored`.
    #[test]
    #[ignore = "needs the network"]
    fn live_github_release() {
        let transport = crate::sources::http::PublicReqwestTransport::new(&DOMAINS).unwrap();
        let source = GithubSource::new(transport);
        let release = source.latest(REPO).unwrap();
        let asset = source.choose(&release, "retail").unwrap().expect("a zip");
        assert!(asset.name.ends_with(".zip"));
        assert!(is_allowed(&asset.browser_download_url));
    }
}
