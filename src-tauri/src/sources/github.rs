//! GitHub releases as a source (D31, issue #108): only the projects the app has registered with a
//! `github` record (`id` = `owner/repo`). Addons installed by hand are never matched to a
//! repository.
//!
//! The latest release is read **without the API** (issue #179), only from `github.com`, which does
//! not count toward the API's limit of 60 requests per hour without a token:
//!
//! 1. the tag from the `Location` header of `https://github.com/{owner}/{repo}/releases/latest`,
//!    read without following the redirect: it must be `https://github.com/{owner}/{repo}/releases/tag/{tag}`
//!    for the same repository (a 404 means no release). A moved repository
//!    answers with a redirect to `releases/latest` of the new name, followed only once;
//!    a tag with `/` (`release/2.0`) does not fit in a segment and is read from the API;
//! 2. when the zip must be chosen, the packager's `release.json` from
//!    `https://github.com/{owner}/{repo}/releases/download/{tag}/release.json`;
//! 3. the zip from `https://github.com/{owner}/{repo}/releases/download/{tag}/{filename}`.
//!
//! Only when the release has no `release.json` (404 on that file) is the zip chosen from the
//! assets, which only the API lists: `GET https://api.github.com/repos/{owner}/{repo}/releases/latest`,
//! without a token. The exceeded limit and its message apply only to this read.
//!
//! Every response, even an error, is read once per run. Between one command and the next
//! (`Cache`) a release that was read is valid for ten minutes, and after a rate-limit-exceeded
//! response from the API no further request goes to the API until the reset.
//!
//! Verified on 2026-10-26: BigWigs' `releases/latest` answers `302` to
//! `/releases/tag/v427.8`, Auctionator's (no release) `404`, and
//! `releases/download/{tag}/release.json` redirects to `release-assets.githubusercontent.com`.
//!
//! API response verified on 2026-10-25 on BigWigs `v427.8`: `tag_name`, `published_at`,
//! `html_url` and `assets[]` with `name` and `browser_download_url` (`BigWigs-v427.8.zip` and
//! `release.json`). The packager's `release.json` (BigWigsMods/packager) lists
//! `releases[]` with `filename`, `nolib` and `metadata[]` of `flavor`/`interface`.
//!
//! The installed version is the record's tag; there is an update when the tag of `latest`
//! differs. The zip is chosen like this:
//!
//! 1. with `release.json`, the entry whose `metadata` contains the packager's flavor for the
//!    installation's game (see [`packager_flavor`]), preferring `nolib: false`;
//! 2. without `release.json`, the only `.zip` asset that does not end in `-nolib.zip`;
//! 3. otherwise the project is updated only from the site (`allowModDistribution` false).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Deserialize;

use super::curseforge::{MAX_DOWNLOAD_BYTES, TransportError};
use super::http::{DownloadProgress, PublicTransport, is_allowed_url};
use super::{FolderCheck, LinkInfo, RecordWriter, Source, SourceKind, UpdateTarget};
use crate::addons::scan::InstalledAddon;
use crate::addons::toc;
use crate::installations::Installation;
use crate::progress::{ProgressSink, Reporter};
use crate::store::{ProjectRecords, SourceRecord};
use crate::updates::{self, AddonUpdate, FileSummary, Project, UpdateOutcome, UpdateStatus};

pub const API_URL: &str = "https://api.github.com";
/// The site, from which the latest release's tag and its files are read without the API.
const WEB_URL: &str = "https://github.com";
/// The domains from which GitHub serves the API and the assets (`github.com` redirects assets to
/// `release-assets.githubusercontent.com`), with their subdomains.
pub const DOMAINS: [&str; 2] = ["github.com", "githubusercontent.com"];
/// The response type GitHub's API recommends.
const ACCEPT: &str = "application/vnd.github+json";
/// The name of the asset written by the packager.
const MANIFEST: &str = "release.json";

const GITHUB: SourceKind = SourceKind::Github;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct Release {
    pub(crate) tag_name: String,
    #[serde(default)]
    published_at: Option<String>,
    #[serde(default)]
    assets: Vec<Asset>,
    /// The repository (`owner/repo`) of a release read without the API (issue #179): the assets
    /// are not listed, the zip is chosen from the tag's `release.json`.
    #[serde(skip)]
    web_repo: Option<String>,
}

/// An asset of a release: the file name and the address to download it from.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct Asset {
    pub(crate) name: String,
    pub(crate) browser_download_url: String,
}

/// Why the latest release of a repository cannot be read. The variants separate the cases the
/// caller treats differently (the mirror of a CurseForge project, issue #162); the text is
/// the same English message the check shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ReleaseError {
    /// The limit of 60 requests per hour, with the reset time in the message.
    Limited(String),
    /// The repository is not valid, does not exist or has no published releases (HTTP 404).
    Missing(String),
    /// Network, unreadable response or another HTTP error.
    Other(String),
}

impl ReleaseError {
    pub(crate) fn message(&self) -> &str {
        match self {
            Self::Limited(message) | Self::Missing(message) | Self::Other(message) => message,
        }
    }
}

/// The packager's `release.json`.
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

/// The release's zip for the installation: an asset to download or `None`, only from the
/// site. `Err` is the reason why the release cannot be used.
type Choice = Result<Option<Asset>, String>;

/// Why a release's zip cannot be chosen, with the error type (issue #162).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ChoiceError {
    /// The `release.json` has no entry for the game, or the entry names a missing asset.
    NoFileForGame(String),
    /// The API's rate limit, read only for a release without `release.json`.
    Limited(String),
    /// The `release.json` cannot be read: network, unreadable response or another HTTP error.
    Other(String),
}

impl ChoiceError {
    pub(crate) fn message(&self) -> &str {
        match self {
            Self::NoFileForGame(message) | Self::Limited(message) | Self::Other(message) => message,
        }
    }
}

/// How long a release that was read is valid for later commands: every "Reload" redoes the check,
/// and without this memory N GitHub projects would cost N of the 60 requests per hour each round.
pub const RELEASE_TTL: Duration = Duration::from_secs(10 * 60);
/// How long to wait after an exceeded limit without `x-ratelimit-reset`.
const LIMIT_FALLBACK: Duration = Duration::from_secs(5 * 60);

/// The releases read successfully, by repository, with the instant of the read.
type Fresh = Mutex<HashMap<String, (Instant, Release)>>;

/// The GitHub responses that are valid between one command and the next: the releases read
/// successfully, for [`RELEASE_TTL`] (without the API and from the API), and the exceeded API rate
/// limit, until the reset time. The other errors apply only to the command that received them.
#[derive(Default)]
pub struct Cache {
    releases: Fresh,
    api_releases: Fresh,
    /// The limit's message and until when it applies.
    limited: Mutex<Option<(SystemTime, String)>>,
}

impl Cache {
    fn limit(&self) -> Option<String> {
        lock(&self.limited)
            .as_ref()
            .filter(|(until, _)| SystemTime::now() < *until)
            .map(|(_, message)| message.clone())
    }
}

/// The GitHub source. Every response, even an error, is kept for the whole lifetime of the source,
/// which lasts one command: the update reuses the check that precedes it. The releases read and
/// the exceeded limit also pass to later commands through [`Cache`].
pub struct GithubSource<T: PublicTransport> {
    transport: T,
    cache: Arc<Cache>,
    releases: Mutex<HashMap<String, Result<Release, ReleaseError>>>,
    api_releases: Mutex<HashMap<String, Result<Release, ReleaseError>>>,
    /// `None`: the `release.json` is not there (HTTP 404).
    manifests: Mutex<HashMap<String, Result<Option<Manifest>, String>>>,
    /// The zip chosen by the check for each project with an update.
    chosen: Mutex<HashMap<String, (Release, Asset)>>,
}

impl<T: PublicTransport> GithubSource<T> {
    #[cfg(test)]
    pub fn new(transport: T) -> Self {
        Self::with_cache(transport, Arc::default())
    }

    /// A source that shares `cache` with the others created by the app.
    pub fn with_cache(transport: T, cache: Arc<Cache>) -> Self {
        Self {
            transport,
            cache,
            releases: Mutex::new(HashMap::new()),
            api_releases: Mutex::new(HashMap::new()),
            manifests: Mutex::new(HashMap::new()),
            chosen: Mutex::new(HashMap::new()),
        }
    }

    /// The latest release of `repo`, read at most once per command and at most once
    /// per [`RELEASE_TTL`].
    fn latest(&self, repo: &str) -> Result<Release, String> {
        self.latest_release(repo)
            .map_err(|error| error.message().to_owned())
    }

    /// Like [`latest`](Self::latest), with the error type. Does not go through the API.
    pub(crate) fn latest_release(&self, repo: &str) -> Result<Release, ReleaseError> {
        remembered(&self.releases, &self.cache.releases, repo, || {
            self.fetch_latest(repo)
        })
    }

    /// The latest release of `repo` read from the API, with its assets: only for a release without
    /// `release.json`. Same memory as [`latest_release`](Self::latest_release).
    fn api_release(&self, repo: &str) -> Result<Release, ReleaseError> {
        remembered(&self.api_releases, &self.cache.api_releases, repo, || {
            self.fetch_api(repo)
        })
    }

    /// The tag of the latest release from the `releases/latest` redirect (see the module
    /// documentation), without the API and therefore without the rate-limit block.
    fn fetch_latest(&self, repo: &str) -> Result<Release, ReleaseError> {
        if !is_valid_repo(repo) {
            return Err(ReleaseError::Missing(invalid_repo(repo)));
        }
        let unrecognized = || {
            ReleaseError::Other(
                "GitHub gave an unrecognized address for the latest release.".to_owned(),
            )
        };
        let mut current = repo.to_owned();
        // At most one moved-repository redirect, then the one to the tag.
        for moves_left in [1, 0] {
            let url = format!("{WEB_URL}/{current}/releases/latest");
            let response = self
                .transport
                .get_redirect(&url)
                .map_err(|error| ReleaseError::Other(network_error(error)))?;
            match response.status {
                301 | 302 | 303 | 307 | 308 => {}
                404 => {
                    return Err(ReleaseError::Missing(format!(
                        "GitHub has no published release for {repo}."
                    )));
                }
                status => {
                    return Err(ReleaseError::Other(format!(
                        "GitHub responded with an error (HTTP {status})."
                    )));
                }
            }
            match latest_target(&url, response.location.as_deref(), &current) {
                Some(Target::Tag(tag)) => {
                    return Ok(Release {
                        tag_name: tag,
                        published_at: None,
                        assets: Vec::new(),
                        web_repo: Some(current),
                    });
                }
                Some(Target::Moved(moved)) if moves_left > 0 => current = moved,
                // The API limit applies here too: it is an API read.
                Some(Target::SlashedTag) => return self.api_release(&current),
                _ => return Err(unrecognized()),
            }
        }
        Err(unrecognized())
    }

    /// The latest release from the API, with the assets. It is the only read that counts toward the
    /// limit of 60 requests per hour, and the only one the exceeded limit blocks.
    fn fetch_api(&self, repo: &str) -> Result<Release, ReleaseError> {
        if !is_valid_repo(repo) {
            return Err(ReleaseError::Missing(invalid_repo(repo)));
        }
        if let Some(message) = self.cache.limit() {
            return Err(ReleaseError::Limited(message));
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
            })
            .map_err(ReleaseError::Other)?;
        match response.status {
            200 => serde_json::from_str(&response.body)
                .map_err(|_| ReleaseError::Other("Unrecognized response from GitHub.".to_owned())),
            404 => Err(ReleaseError::Missing(format!(
                "GitHub has no published release for {repo}."
            ))),
            403 | 429 if response.ratelimit_remaining == Some(0) => {
                let message = rate_limit_message(response.ratelimit_reset);
                let until = response.ratelimit_reset.map_or_else(
                    || SystemTime::now() + LIMIT_FALLBACK,
                    |reset| UNIX_EPOCH + Duration::from_secs(reset),
                );
                *lock(&self.cache.limited) = Some((until, message.clone()));
                Err(ReleaseError::Limited(message))
            }
            status => Err(ReleaseError::Other(format!(
                "GitHub responded with an error (HTTP {status})."
            ))),
        }
    }

    /// The `release.json` at address `url`, read at most once; `None` if it is not there (HTTP
    /// 404). Downloaded from `github.com`, not from the API: it does not count toward the limit.
    fn manifest(&self, url: &str) -> Result<Option<Manifest>, String> {
        if let Some(cached) = lock(&self.manifests).get(url) {
            return cached.clone();
        }
        let result = self.fetch_manifest(url);
        lock(&self.manifests).insert(url.to_owned(), result.clone());
        result
    }

    fn fetch_manifest(&self, url: &str) -> Result<Option<Manifest>, String> {
        if !is_allowed(url) {
            return Err("GitHub gave an unrecognized address for release.json.".to_owned());
        }
        let unreadable =
            || "The release.json of the latest GitHub release is unreadable.".to_owned();
        let response = self.transport.get(url).map_err(|error| match error {
            TransportError::TooLarge => unreadable(),
            other => network_error(other),
        })?;
        match response.status {
            200 => serde_json::from_str(&response.body)
                .map(Some)
                .map_err(|_| unreadable()),
            404 => Ok(None),
            status => Err(format!("GitHub responded with an error (HTTP {status}).")),
        }
    }

    /// The zip of `release` for the `flavor` game (see the module documentation).
    fn choose(&self, release: &Release, flavor: &str) -> Choice {
        self.choose_zip(release, flavor)
            .map_err(|error| error.message().to_owned())
    }

    /// Like [`choose`](Self::choose), with the error type.
    pub(crate) fn choose_zip(
        &self,
        release: &Release,
        flavor: &str,
    ) -> Result<Option<Asset>, ChoiceError> {
        if let Some(repo) = &release.web_repo {
            return self.choose_from_web(repo, release, flavor);
        }
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
        let manifest = self
            .manifest(&manifest.browser_download_url)
            .map_err(ChoiceError::Other)?
            .ok_or_else(|| {
                ChoiceError::Other("GitHub responded with an error (HTTP 404).".to_owned())
            })?;
        let file = manifest_file(&manifest, flavor)?;
        release
            .assets
            .iter()
            .find(|a| a.name == file.filename)
            .cloned()
            .map(Some)
            .ok_or_else(|| {
                ChoiceError::NoFileForGame(format!(
                    "The latest GitHub release does not include {}.",
                    file.filename
                ))
            })
    }

    /// The zip of a release read without the API: from the tag's `release.json`, with the same rule
    /// as the API branch. Without `release.json` (404) the assets are read from the API, and the
    /// release must still be the tag's.
    fn choose_from_web(
        &self,
        repo: &str,
        release: &Release,
        flavor: &str,
    ) -> Result<Option<Asset>, ChoiceError> {
        let tag = &release.tag_name;
        let url = download_url(repo, tag, MANIFEST).ok_or_else(|| {
            ChoiceError::Other("GitHub gave an unrecognized address for release.json.".to_owned())
        })?;
        let Some(manifest) = self.manifest(&url).map_err(ChoiceError::Other)? else {
            let listed = self.api_release(repo).map_err(|error| match error {
                ReleaseError::Limited(message) => ChoiceError::Limited(message),
                ReleaseError::Missing(message) | ReleaseError::Other(message) => {
                    ChoiceError::Other(message)
                }
            })?;
            if listed.tag_name != *tag {
                return Err(ChoiceError::Other(
                    "The latest GitHub release changed while it was read: check again.".to_owned(),
                ));
            }
            return self.choose_zip(&listed, flavor);
        };
        let file = manifest_file(&manifest, flavor)?;
        // `filename` is the name of a file of the release, never a path.
        let url = is_file_name(&file.filename)
            .then(|| download_url(repo, tag, &file.filename))
            .flatten()
            .ok_or_else(|| {
                ChoiceError::NoFileForGame(format!(
                    "The latest GitHub release does not include {}.",
                    file.filename
                ))
            })?;
        Ok(Some(Asset {
            name: file.filename.clone(),
            browser_download_url: url,
        }))
    }

    /// Downloads `asset` only from GitHub's domains and with the download size limit: it is
    /// also used by the mirror of blocked CurseForge projects (issue #163).
    pub(crate) fn download(
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

    /// Downloads `asset` and installs it in place of `old_folders`, then registers the tag. `check`
    /// can reject the zip's folders before they touch the disk. The outcome uses the shape of
    /// an update: `updated` means installed.
    #[allow(clippy::too_many_arguments)]
    fn install(
        &self,
        key: &str,
        release: &Release,
        asset: &Asset,
        addons_dir: &std::path::Path,
        old_folders: &[String],
        check: &FolderCheck<'_>,
        record: &mut RecordWriter<'_>,
        progress: &ProgressSink<'_>,
    ) -> UpdateOutcome {
        match self.fetch_asset(key, asset, progress) {
            Ok(zip) => install_zip(
                key,
                release,
                &zip,
                addons_dir,
                old_folders,
                check,
                record,
                progress,
            ),
            Err(outcome) => *outcome,
        }
    }

    /// Downloads `asset` reporting progress: the network part of an installation.
    fn fetch_asset(
        &self,
        key: &str,
        asset: &Asset,
        progress: &ProgressSink<'_>,
    ) -> Result<Vec<u8>, Box<UpdateOutcome>> {
        let name = repo_name(key);
        let reporter = Reporter::new(progress, Some(GITHUB), key, name);
        reporter.download(0, None);
        let zip = self
            .download(asset, &mut |received, total| {
                reporter.download(received, total)
            })
            .map_err(|error| Box::new(UpdateOutcome::error(GITHUB, key, name, error)))?;
        reporter.downloaded(zip.len() as u64);
        Ok(zip)
    }

    /// Installs a GitHub project that is not there yet (the import, issue #134, and links, issue
    /// #109), with the same zip choice as the check. `check` can reject the zip's folders
    /// before they touch the disk.
    pub fn install_new(
        &self,
        installation: &Installation,
        key: &str,
        check: &FolderCheck<'_>,
        record: &mut RecordWriter<'_>,
        progress: &ProgressSink<'_>,
    ) -> UpdateOutcome {
        match self.download_new(installation, key, progress) {
            Ok(download) => {
                install_downloaded(installation, key, &download, check, record, progress)
            }
            Err(outcome) => *outcome,
        }
    }

    /// The network part of [`install_new`](Self::install_new): the latest release, the zip for the
    /// game and its download, without touching the disk. The import does it for several projects
    /// together (issue #180).
    pub fn download_new(
        &self,
        installation: &Installation,
        key: &str,
        progress: &ProgressSink<'_>,
    ) -> Result<GithubDownload, Box<UpdateOutcome>> {
        let name = repo_name(key);
        let release = self
            .latest(key)
            .map_err(|error| Box::new(UpdateOutcome::error(GITHUB, key, name, error)))?;
        let asset = match self.choose(&release, &installation.flavor) {
            Ok(Some(asset)) => asset,
            Ok(None) => {
                return Err(Box::new(UpdateOutcome::skipped(
                    GITHUB,
                    key,
                    name,
                    WEBSITE_ONLY,
                )));
            }
            Err(error) => {
                return Err(Box::new(UpdateOutcome::skipped(GITHUB, key, name, &error)));
            }
        };
        let zip = self.fetch_asset(key, &asset, progress)?;
        Ok(GithubDownload { release, zip })
    }

    /// The preview of a link (issue #109): the latest release and whether it has a zip for the
    /// installation's game, with the same choice as the installation. The reads (without the API,
    /// except for a release without `release.json`) are reused from the cache at installation.
    /// `Err` if the release cannot be read.
    pub fn preview(&self, installation: &Installation, key: &str) -> Result<LinkInfo, String> {
        let release = self.latest(key)?;
        let (downloadable, message) = match self.choose(&release, &installation.flavor) {
            Ok(Some(_)) => (true, None),
            Ok(None) => (false, Some(WEBSITE_ONLY.to_owned())),
            Err(error) => (false, Some(error)),
        };
        Ok(LinkInfo {
            name: repo_name(key).to_owned(),
            version: release.tag_name,
            downloadable,
            message,
            url: format!("https://github.com/{key}"),
        })
    }
}

/// The release of a GitHub project with its zip, downloaded and not yet installed. Neither
/// `Clone` nor a `Debug` with the bytes: the zip can weigh tens of MB.
pub struct GithubDownload {
    release: Release,
    zip: Vec<u8>,
}

impl std::fmt::Debug for GithubDownload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GithubDownload")
            .field("tag", &self.release.tag_name)
            .field("zip_bytes", &self.zip.len())
            .finish()
    }
}

/// The name of the `owner/repo` project in outcomes and progress: the repository.
pub fn project_name(key: &str) -> &str {
    repo_name(key)
}

/// The on-disk part of [`GithubSource::install_new`]: installs `download` and registers the tag.
pub fn install_downloaded(
    installation: &Installation,
    key: &str,
    download: &GithubDownload,
    check: &FolderCheck<'_>,
    record: &mut RecordWriter<'_>,
    progress: &ProgressSink<'_>,
) -> UpdateOutcome {
    let addons_dir = installation.path.join("Interface").join("AddOns");
    install_zip(
        key,
        &download.release,
        &download.zip,
        &addons_dir,
        &[],
        check,
        record,
        progress,
    )
}

/// Installs the downloaded zip of `release` in place of `old_folders`, then registers the tag.
#[allow(clippy::too_many_arguments)]
fn install_zip(
    key: &str,
    release: &Release,
    zip: &[u8],
    addons_dir: &std::path::Path,
    old_folders: &[String],
    check: &FolderCheck<'_>,
    record: &mut RecordWriter<'_>,
    progress: &ProgressSink<'_>,
) -> UpdateOutcome {
    let name = repo_name(key);
    let reporter = Reporter::new(progress, Some(GITHUB), key, name);
    let folders = match updates::install_bytes(zip, addons_dir, old_folders, check, &reporter) {
        Ok(folders) => folders,
        Err(error) => return UpdateOutcome::error(GITHUB, key, name, error),
    };
    // The folders are already replaced: if the registry is not saved the installation still
    // succeeded, and the message says so.
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

const WEBSITE_ONLY: &str = "The latest GitHub release has no single zip WoW-don can pick: download it from the release page, then drop the zip into this window.";

fn lock<V>(mutex: &Mutex<V>) -> std::sync::MutexGuard<'_, V> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// The release of `repo` from the command's memory (`local`, errors too), then from the one
/// between commands (`shared`, only successes for [`RELEASE_TTL`]), otherwise from `fetch`.
fn remembered(
    local: &Mutex<HashMap<String, Result<Release, ReleaseError>>>,
    shared: &Fresh,
    repo: &str,
    fetch: impl FnOnce() -> Result<Release, ReleaseError>,
) -> Result<Release, ReleaseError> {
    if let Some(cached) = lock(local).get(repo) {
        return cached.clone();
    }
    let fresh = lock(shared)
        .get(repo)
        .filter(|(read, _)| read.elapsed() < RELEASE_TTL)
        .map(|(_, release)| release.clone());
    let result = fresh.map(Ok).unwrap_or_else(|| {
        let fetched = fetch();
        if let Ok(release) = &fetched {
            lock(shared).insert(repo.to_owned(), (Instant::now(), release.clone()));
        }
        fetched
    });
    lock(local).insert(repo.to_owned(), result.clone());
    result
}

fn invalid_repo(repo: &str) -> String {
    format!("\"{repo}\" is not a GitHub repository (owner/repo).")
}

/// The `release.json` entry for the `flavor` game: the one whose `metadata` contains the
/// packager's flavor, preferring `nolib: false`.
fn manifest_file<'a>(
    manifest: &'a Manifest,
    flavor: &str,
) -> Result<&'a ManifestFile, ChoiceError> {
    let no_file = || {
        ChoiceError::NoFileForGame(
            "The latest GitHub release has no file for this game version.".to_owned(),
        )
    };
    let wanted = packager_flavor(flavor).ok_or_else(no_file)?;
    manifest
        .releases
        .iter()
        .filter(|file| file.metadata.iter().any(|m| m.flavor == wanted))
        .min_by_key(|file| file.nolib)
        .ok_or_else(no_file)
}

/// Where the `releases/latest` redirect leads.
#[derive(Debug, PartialEq, Eq)]
enum Target {
    /// `/{owner}/{repo}/releases/tag/{tag}` of the same repository: the tag, decoded.
    Tag(String),
    /// `/{owner}/{repo}/releases/latest` of another name: the repository has been moved.
    Moved(String),
    /// The tag of the same repository contains `/` (`release/2.0`, a valid tag name for
    /// git): it does not fit in a segment of `releases/download/{tag}/`, so the release is read
    /// from the API, which gives the right addresses of its assets.
    SlashedTag,
}

/// The `releases/latest` redirect of `repo` requested from `from`: only `https://github.com`,
/// without query or fragment, to the tag of the same repository (case-insensitively, like
/// GitHub) or to `releases/latest` of a moved repository. Otherwise
/// `None`.
fn latest_target(from: &str, location: Option<&str>, repo: &str) -> Option<Target> {
    let url = reqwest::Url::parse(from).ok()?.join(location?).ok()?;
    if !is_allowed_url(&url, &DOMAINS)
        || url.host_str() != Some("github.com")
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let segments: Vec<&str> = url.path_segments()?.collect();
    match segments.as_slice() {
        [owner, name, "releases", "tag", tag @ ..]
            if !tag.is_empty() && format!("{owner}/{name}").eq_ignore_ascii_case(repo) =>
        {
            let parts: Vec<String> = tag
                .iter()
                .map(|part| percent_decode(part))
                .collect::<Option<_>>()?;
            match parts.as_slice() {
                [tag] if is_file_name(tag) => Some(Target::Tag(tag.clone())),
                // Every piece between the `/` must be a name: no `..` and no empty pieces.
                _ if parts
                    .iter()
                    .flat_map(|part| part.split('/'))
                    .all(is_file_name) =>
                {
                    Some(Target::SlashedTag)
                }
                _ => None,
            }
        }
        [owner, name, "releases", "latest"] => {
            let moved = format!("{owner}/{name}");
            (is_valid_repo(&moved) && !moved.eq_ignore_ascii_case(repo))
                .then_some(Target::Moved(moved))
        }
        _ => None,
    }
}

/// A path segment with the `%XX` sequences decoded, if it is valid UTF-8.
fn percent_decode(segment: &str) -> Option<String> {
    let bytes = segment.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%' {
            let hex = bytes.get(at + 1..at + 3)?;
            if !hex.iter().all(u8::is_ascii_hexdigit) {
                return None;
            }
            decoded.push(u8::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok()?);
            at += 3;
        } else {
            decoded.push(bytes[at]);
            at += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

/// A name that fits in a single path segment: not empty, without `/` or `\`, other than
/// `.` and `..`, without control characters.
fn is_file_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name
            .chars()
            .any(|c| c == '/' || c == '\\' || c.is_control())
}

/// `https://github.com/{owner}/{repo}/releases/download/{tag}/{file}`, with `tag` and `file`
/// encoded as path segments.
fn download_url(repo: &str, tag: &str, file: &str) -> Option<String> {
    let (owner, name) = repo.split_once('/')?;
    let mut url = reqwest::Url::parse(WEB_URL).ok()?;
    url.path_segments_mut()
        .ok()?
        .clear()
        .extend([owner, name, "releases", "download", tag, file]);
    Some(url.into())
}

fn is_allowed(url: &str) -> bool {
    reqwest::Url::parse(url).is_ok_and(|url| is_allowed_url(&url, &DOMAINS))
}

/// `owner/repo` with the characters GitHub allows in names: ASCII letters and digits, `-`, `_` and
/// `.`, without `.` or `..` on their own. The text ends up in an API URL.
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

/// The name shown for a project: the part of the repository after the owner.
fn repo_name(repo: &str) -> &str {
    repo.split_once('/').map_or(repo, |(_, name)| name)
}

/// The packager's flavor for the installation's game, derived from the same family as
/// `toc::toc_suffixes`, so the two tables do not drift apart. Verified on BigWigs'
/// `v427.8` `release.json` (2026-10-25):
///
/// | Folder | Game | Packager flavor |
/// | --- | --- | --- |
/// | `_retail_` (e PTR, beta) | Retail | `mainline` |
/// | `_classic_era_` | Classic Era | `classic` |
/// | `_forever_`, `_classic_beta_` | WoW Forever | `forever` |
/// | `_anniversary_` | Anniversary (TBC) | `bcc` |
/// | `_classic_` | Progressive Classic (Mists) | `mists` |
///
/// `wrath`, `cata` and `titan` have no client folder today.
fn packager_flavor(flavor: &str) -> Option<&'static str> {
    match *toc::toc_suffixes(flavor).first()? {
        "Standard" => Some("mainline"),
        "Forever" => Some("forever"),
        "Vanilla" => Some("classic"),
        "TBC" => Some("bcc"),
        "Mists" => Some("mists"),
        // A family that `toc_suffixes` adds in the future does not become Retail by mistake.
        _ => None,
    }
}

fn network_error(error: TransportError) -> String {
    match error {
        TransportError::Timeout => "GitHub did not respond in time.".to_owned(),
        _ => "Could not reach GitHub: check the connection.".to_owned(),
    }
}

/// The message for the 60 requests per hour limit, with the time (UTC) at which GitHub resets it.
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

/// A GitHub file: no numeric id, the tag as the name (see `FileSummary`).
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
                    .is_some_and(|p| p.source == GITHUB && p.key == key)
            })
            .collect();
        let Some(project) = folders.first().and_then(|u| u.project.as_ref()) else {
            return target.not_found(GITHUB);
        };
        let name = project.name.as_str();
        // With `reinstall` (issue #148) an up-to-date addon is downloaded again from its release.
        let wanted = |status: &UpdateStatus| match status {
            UpdateStatus::UpdateAvailable { .. } => true,
            UpdateStatus::UpToDate => target.reinstall,
            _ => false,
        };
        if !folders.iter().any(|u| wanted(&u.status)) {
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
        let chosen = lock(&self.chosen).get(key).cloned();
        let (release, asset) = match chosen {
            Some(chosen) => chosen,
            // For an up-to-date project the check does not choose the file (it would cost a read
            // of `release.json` on every check): it is chosen here, from the release already read.
            None if target.reinstall => {
                let release = match self.latest(key) {
                    Ok(release) => release,
                    Err(error) => return UpdateOutcome::error(GITHUB, key, name, error),
                };
                match self.choose(&release, target.flavor) {
                    Ok(Some(asset)) => (release, asset),
                    Ok(None) => return UpdateOutcome::skipped(GITHUB, key, name, WEBSITE_ONLY),
                    Err(message) => return UpdateOutcome::skipped(GITHUB, key, name, &message),
                }
            }
            None => return UpdateOutcome::skipped(GITHUB, key, name, "Check for updates again."),
        };
        // The project's folders on disk: those the new zip no longer has are removed.
        let old_folders: Vec<String> = folders.iter().map(|u| u.folder.clone()).collect();
        self.install(
            key,
            &release,
            &asset,
            target.addons_dir,
            &old_folders,
            &|_| Ok(()),
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
    use crate::sources::http::{ApiResponse, BytesResponse, HttpResponse, RedirectResponse};
    use crate::store::SourceRecords;
    use crate::test_support::TempDir;
    use std::fs;
    use std::io::{Cursor, Write};
    use std::path::PathBuf;
    use std::sync::Arc;
    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;

    const REPO: &str = "BigWigsMods/BigWigs";
    /// `releases/latest` on the site, read without following the redirect.
    const LATEST: &str = "https://github.com/BigWigsMods/BigWigs/releases/latest";
    /// The same release from the API: only for a release without `release.json`.
    const API_LATEST: &str = "https://api.github.com/repos/BigWigsMods/BigWigs/releases/latest";
    const DOWNLOADS: &str = "https://github.com/BigWigsMods/BigWigs/releases/download/v427.8";

    fn downloads(tag: &str) -> String {
        format!("https://github.com/{REPO}/releases/download/{tag}")
    }

    /// The fake transport's responses, by URL.
    #[derive(Default)]
    struct Fake {
        /// `releases/latest` → (status, `Location`); without an entry, 404.
        redirects: HashMap<String, Result<(u16, Option<String>), TransportError>>,
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

        fn get_redirect(&self, url: &str) -> Result<RedirectResponse, TransportError> {
            self.calls.lock().unwrap().push(url.to_owned());
            let (status, location) = self
                .redirects
                .get(url)
                .cloned()
                .unwrap_or(Ok((404, None)))?;
            Ok(RedirectResponse { status, location })
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

    /// The real release of 2026-10-25, reduced to the fields used, with the assets `assets`.
    fn release(tag: &str, assets: &[&str]) -> String {
        let assets: Vec<_> = assets
            .iter()
            .map(|name| {
                serde_json::json!({
                    "name": name,
                    "browser_download_url": format!("{}/{name}", downloads(tag)),
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

    /// BigWigs' real `v427.8` `release.json` (reduced flavors), plus a `-nolib` variant.
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

    fn with_manifest(fake: Fake, tag: &str, body: &str) -> Fake {
        let mut fake = fake;
        fake.json.insert(
            format!("{}/{MANIFEST}", downloads(tag)),
            HttpResponse {
                status: 200,
                body: body.to_owned(),
            },
        );
        fake
    }

    /// The `releases/latest` redirect to `location`.
    fn found(location: &str) -> Result<(u16, Option<String>), TransportError> {
        Ok((302, Some(location.to_owned())))
    }

    fn tag_url(tag: &str) -> String {
        format!("https://github.com/{REPO}/releases/tag/{tag}")
    }

    /// A release `tag` with the assets `assets`: the site answers with the redirect to the tag,
    /// and the `release.json` is there only if it is among the assets. The API lists the assets.
    fn fake(tag: &str, assets: &[&str]) -> Fake {
        let mut fake = Fake::default();
        fake.redirects
            .insert(LATEST.to_owned(), found(&tag_url(tag)));
        fake.api
            .insert(API_LATEST.to_owned(), ok(release(tag, assets)));
        if assets.contains(&MANIFEST) {
            fake = with_manifest(fake, tag, &manifest());
        }
        fake
    }

    fn no_api_call(calls: &Mutex<Vec<String>>) -> bool {
        !calls
            .lock()
            .unwrap()
            .iter()
            .any(|url| url.starts_with(API_URL))
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
                // Without the API the publication date is not known.
                latest: summary("v427.8", None)
            }
        );
        // `mainline` is both in the `-nolib` file and in the full one: `nolib: false` wins.
        assert_eq!(chosen(&source).as_deref(), Some("BigWigs-v427.8.zip"));
    }

    #[test]
    fn a_flavor_missing_from_release_json_is_an_error() {
        let manifest = serde_json::json!({"releases": [
            {"filename": "BigWigs-v427.8.zip", "nolib": false,
             "metadata": [{"flavor": "mainline", "interface": 120100}]}
        ]})
        .to_string();
        let fake = with_manifest(
            fake("v427.8", &["BigWigs-v427.8.zip", MANIFEST]),
            "v427.8",
            &manifest,
        );
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
        assert_eq!(packager_flavor("forever"), Some("forever"));
        assert_eq!(packager_flavor("anniversary"), Some("bcc"));
        assert_eq!(packager_flavor("classic"), Some("mists"));
        // Every flavor in the table exists in the real `release.json`.
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
    fn without_release_json_the_only_full_zip_is_picked_from_the_api() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut fake = fake("v2", &["Addon-v2.zip", "Addon-v2-nolib.zip"]);
        fake.calls = Arc::clone(&calls);
        let source = GithubSource::new(fake);
        let updates = source
            .check(&installation("retail"), &offered(), &records("v1"))
            .unwrap();
        assert!(matches!(
            updates[0].status,
            UpdateStatus::UpdateAvailable { .. }
        ));
        assert!(updates[0].project.as_ref().unwrap().allow_mod_distribution);
        assert_eq!(chosen(&source).as_deref(), Some("Addon-v2.zip"));
        // The `release.json` is missing (404): only then is the API read.
        assert_eq!(
            *calls.lock().unwrap(),
            [
                LATEST.to_owned(),
                format!("{}/{MANIFEST}", downloads("v2")),
                API_LATEST.to_owned()
            ]
        );
    }

    /// Issue #179: tag from the redirect, `release.json` and zip from `releases/download/{tag}/`,
    /// without any request to the API.
    #[test]
    fn a_release_with_release_json_is_read_without_the_api() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut fake = fake("v427.8", &["BigWigs-v427.8.zip", MANIFEST]);
        // The API does not answer: it must not be needed.
        fake.api.clear();
        fake.calls = Arc::clone(&calls);
        let source = GithubSource::new(fake);
        let release = source.latest_release(REPO).unwrap();
        assert_eq!(release.tag_name, "v427.8");
        let asset = source.choose_zip(&release, "retail").unwrap().unwrap();
        assert_eq!(
            asset,
            Asset {
                name: "BigWigs-v427.8.zip".to_owned(),
                browser_download_url: format!("{DOWNLOADS}/BigWigs-v427.8.zip"),
            }
        );
        assert_eq!(
            *calls.lock().unwrap(),
            [LATEST.to_owned(), format!("{DOWNLOADS}/{MANIFEST}")]
        );
        assert!(no_api_call(&calls));
    }

    #[test]
    fn no_release_on_the_website_asks_nothing_else() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let source = GithubSource::new(Fake {
            calls: Arc::clone(&calls),
            ..Fake::default()
        });
        assert_eq!(
            source.latest_release("Auctionator/Auctionator"),
            Err(ReleaseError::Missing(
                "GitHub has no published release for Auctionator/Auctionator.".to_owned()
            ))
        );
        assert_eq!(
            *calls.lock().unwrap(),
            ["https://github.com/Auctionator/Auctionator/releases/latest"]
        );
    }

    #[test]
    fn the_redirect_must_point_to_a_tag_of_the_same_repository() {
        let latest = |location: Option<&str>| {
            let mut fake = Fake::default();
            fake.redirects
                .insert(LATEST.to_owned(), Ok((302, location.map(str::to_owned))));
            GithubSource::new(fake).latest_release(REPO)
        };
        let unrecognized = Err(ReleaseError::Other(
            "GitHub gave an unrecognized address for the latest release.".to_owned(),
        ));
        for bad in [
            None,
            Some("https://github.com/someone/Other/releases/tag/v1"),
            Some("https://evil.example.com/BigWigsMods/BigWigs/releases/tag/v1"),
            Some("http://github.com/BigWigsMods/BigWigs/releases/tag/v1"),
            Some("https://api.github.com/BigWigsMods/BigWigs/releases/tag/v1"),
            Some("https://github.com/BigWigsMods/BigWigs/releases"),
            Some("https://github.com/BigWigsMods/BigWigs/releases/tag/"),
            Some("https://github.com/BigWigsMods/BigWigs/releases/tag/v1?x=1"),
            Some("https://github.com/BigWigsMods/BigWigs/releases/tag/%2E%2E"),
            Some("https://github.com/BigWigsMods/BigWigs/releases/tag/a/.."),
            Some("https://github.com/BigWigsMods/BigWigs/releases/tag/a%2F%2E%2E"),
            Some("https://github.com/someone/Other/releases/tag/a/b"),
            Some("https://github.com/BigWigsMods/BigWigs/releases/tag/%zz"),
        ] {
            assert_eq!(latest(bad), unrecognized, "{bad:?}");
        }
        // A relative address, different case and an encoded tag are fine.
        let tag = |location: &str| latest(Some(location)).map(|release| release.tag_name);
        assert_eq!(
            tag("/BigWigsMods/BigWigs/releases/tag/v427.8"),
            Ok("v427.8".to_owned())
        );
        assert_eq!(
            tag("https://github.com/bigwigsmods/bigwigs/releases/tag/v1"),
            Ok("v1".to_owned())
        );
        assert_eq!(
            tag("https://github.com/BigWigsMods/BigWigs/releases/tag/1.0%2Bbeta%20x"),
            Ok("1.0+beta x".to_owned())
        );
        // The zip of an encoded tag is downloaded from the same tag, encoded again.
        assert_eq!(
            download_url(REPO, "1.0+beta x", "A.zip").as_deref(),
            Some("https://github.com/BigWigsMods/BigWigs/releases/download/1.0+beta%20x/A.zip")
        );
        // Other site statuses.
        let mut fake = Fake::default();
        fake.redirects.insert(LATEST.to_owned(), Ok((500, None)));
        assert_eq!(
            GithubSource::new(fake).latest_release(REPO),
            Err(ReleaseError::Other(
                "GitHub responded with an error (HTTP 500).".to_owned()
            ))
        );
        let mut fake = Fake::default();
        fake.redirects
            .insert(LATEST.to_owned(), Err(TransportError::Timeout));
        assert_eq!(
            GithubSource::new(fake).latest_release(REPO),
            Err(ReleaseError::Other(
                "GitHub did not respond in time.".to_owned()
            ))
        );
    }

    /// A tag with `/` (valid for git) does not fit in `releases/download/{tag}/`: the release is
    /// read from the API, with its assets, instead of failing.
    #[test]
    fn a_tag_with_a_slash_is_read_from_the_api() {
        for location in [
            "https://github.com/BigWigsMods/BigWigs/releases/tag/release/2.0",
            "https://github.com/BigWigsMods/BigWigs/releases/tag/release%2F2.0",
        ] {
            let calls = Arc::new(Mutex::new(Vec::new()));
            let mut fake = fake("release/2.0", &["A.zip"]);
            fake.redirects.insert(LATEST.to_owned(), found(location));
            fake.calls = Arc::clone(&calls);
            let source = GithubSource::new(fake);
            let release = source.latest_release(REPO).unwrap();
            assert_eq!(release.tag_name, "release/2.0", "{location}");
            assert_eq!(release.web_repo, None, "{location}");
            let asset = source.choose_zip(&release, "retail").unwrap().unwrap();
            assert_eq!(asset.name, "A.zip");
            assert_eq!(
                *calls.lock().unwrap(),
                [LATEST.to_owned(), API_LATEST.to_owned()],
                "{location}"
            );
        }
    }

    /// A moved repository redirects to `releases/latest` of the new name: it is followed once.
    #[test]
    fn a_moved_repository_is_followed_once() {
        let moved = "https://github.com/NewOwner/BigWigs/releases/latest";
        let mut fake = fake("v427.8", &["BigWigs-v427.8.zip", MANIFEST]);
        fake.redirects.insert(LATEST.to_owned(), found(moved));
        fake.redirects.insert(
            moved.to_owned(),
            found("https://github.com/NewOwner/BigWigs/releases/tag/v1"),
        );
        let release = GithubSource::new(fake).latest_release(REPO).unwrap();
        assert_eq!(release.tag_name, "v1");
        assert_eq!(release.web_repo.as_deref(), Some("NewOwner/BigWigs"));
        // Two moves in a row, or one to the same name, are not.
        let mut fake = Fake::default();
        fake.redirects.insert(LATEST.to_owned(), found(moved));
        fake.redirects.insert(
            moved.to_owned(),
            found("https://github.com/Third/BigWigs/releases/latest"),
        );
        assert!(GithubSource::new(fake).latest_release(REPO).is_err());
        let mut fake = Fake::default();
        fake.redirects.insert(
            LATEST.to_owned(),
            found("https://github.com/bigwigsmods/bigwigs/releases/latest"),
        );
        assert!(GithubSource::new(fake).latest_release(REPO).is_err());
    }

    #[test]
    fn a_file_name_in_release_json_must_not_be_a_path() {
        for filename in ["../x.zip", "a/b.zip", "a\\b.zip", ".."] {
            let manifest = serde_json::json!({"releases": [
                {"filename": filename, "nolib": false, "metadata": [{"flavor": "mainline"}]}
            ]})
            .to_string();
            let mut fake = fake("v427.8", &[]);
            fake = with_manifest(fake, "v427.8", &manifest);
            let source = GithubSource::new(fake);
            let release = source.latest_release(REPO).unwrap();
            assert!(
                matches!(
                    source.choose_zip(&release, "retail"),
                    Err(ChoiceError::NoFileForGame(_))
                ),
                "{filename}"
            );
        }
    }

    /// Between the redirect and the API a new release may come out: they are not mixed.
    #[test]
    fn the_api_fallback_must_find_the_same_tag() {
        let mut fake = fake("v2", &["A.zip"]);
        fake.api
            .insert(API_LATEST.to_owned(), ok(release("v3", &["A.zip"])));
        let source = GithubSource::new(fake);
        let release = source.latest_release(REPO).unwrap();
        assert_eq!(
            source.choose_zip(&release, "retail"),
            Err(ChoiceError::Other(
                "The latest GitHub release changed while it was read: check again.".to_owned()
            ))
        );
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
        // A record whose folders are no longer on disk asks for nothing.
        let updates = source
            .check(&installation("retail"), &[addon("Other")], &records("v1"))
            .unwrap();
        assert!(updates.is_empty());
        assert!(calls.lock().unwrap().is_empty());
    }

    #[test]
    fn the_rate_limit_is_a_clear_error_and_stops_further_api_requests() {
        // The limit expires in one hour according to the test's clock: a date written in the test
        // sooner or later passes, and an already expired limit no longer stops the requests.
        let reset = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 3_600;
        let at = reset % 86_400;
        let expected = UpdateStatus::Error {
            message: format!(
                "GitHub's limit of 60 requests an hour has been reached: try again after {:02}:{:02} UTC.",
                at / 3_600,
                at % 3_600 / 60
            ),
        };
        for status in [403, 429] {
            let calls = Arc::new(Mutex::new(Vec::new()));
            let mut fake = Fake {
                calls: Arc::clone(&calls),
                ..Fake::default()
            };
            // Two releases without `release.json`: the zip is chosen from the API.
            for repo in ["a/one", "a/two"] {
                fake.redirects.insert(
                    format!("https://github.com/{repo}/releases/latest"),
                    found(&format!("https://github.com/{repo}/releases/tag/2")),
                );
            }
            fake.api.insert(
                format!("{API_URL}/repos/a/one/releases/latest"),
                Ok(ApiResponse {
                    status,
                    body: "{}".to_owned(),
                    ratelimit_remaining: Some(0),
                    ratelimit_reset: Some(reset),
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
            assert_eq!(updates[0].status, expected, "HTTP {status}");
            assert_eq!(updates[1].status, expected, "HTTP {status}");
            // The second project does not start a request to the API.
            let api_calls = calls
                .lock()
                .unwrap()
                .iter()
                .filter(|url| url.starts_with(API_URL))
                .count();
            assert_eq!(api_calls, 1, "HTTP {status}");
        }
        assert_eq!(
            rate_limit_message(None),
            "GitHub's limit of 60 requests an hour has been reached: try again later."
        );
    }

    /// Issue #179: the API limit does not stop a release with `release.json`.
    #[test]
    fn the_rate_limit_does_not_block_the_website() {
        let cache = Arc::new(Cache::default());
        *lock(&cache.limited) = Some((
            SystemTime::now() + Duration::from_secs(3_600),
            "limited".to_owned(),
        ));
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut web = fake("v427.8", &["BigWigs-v427.8.zip", MANIFEST]);
        web.calls = Arc::clone(&calls);
        let source = GithubSource::with_cache(web, Arc::clone(&cache));
        source
            .check(&installation("retail"), &offered(), &records("v427.7"))
            .unwrap();
        assert_eq!(chosen(&source).as_deref(), Some("BigWigs-v427.8.zip"));
        assert!(no_api_call(&calls));
        // Without `release.json` the API is needed, and the limit blocks it without a request.
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut fake = fake("v2", &["A.zip"]);
        fake.calls = Arc::clone(&calls);
        let updates = GithubSource::with_cache(fake, cache)
            .check(&installation("retail"), &offered(), &records("v1"))
            .unwrap();
        assert_eq!(
            updates[0].status,
            UpdateStatus::Error {
                message: "limited".to_owned()
            }
        );
        assert!(no_api_call(&calls));
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
        // The second command finds the release still valid.
        assert_eq!(*calls.lock().unwrap(), [LATEST]);
        // Expired, it is read again.
        lock(&cache.releases).get_mut(REPO).unwrap().0 -= RELEASE_TTL;
        run();
        assert_eq!(calls.lock().unwrap().len(), 2);

        // The release read from the API (without `release.json`) is also valid for later commands.
        let calls = Arc::new(Mutex::new(Vec::new()));
        let listed = Arc::new(Cache::default());
        for _ in 0..2 {
            let mut fake = fake("v2", &["A.zip"]);
            fake.calls = Arc::clone(&calls);
            let source = GithubSource::with_cache(fake, Arc::clone(&listed));
            source
                .check(&installation("retail"), &offered(), &records("v1"))
                .unwrap();
            assert_eq!(chosen(&source).as_deref(), Some("A.zip"));
        }
        // The second round reads nothing again: the release and the API are cached; the 404 of the
        // `release.json` applies only to the command, so it is read again.
        assert_eq!(
            *calls.lock().unwrap(),
            [
                LATEST.to_owned(),
                format!("{}/{MANIFEST}", downloads("v2")),
                API_LATEST.to_owned(),
                format!("{}/{MANIFEST}", downloads("v2")),
            ]
        );

        // The exceeded limit also applies to later commands, until the reset.
        let limited = Arc::new(Cache::default());
        let in_an_hour = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 3_600;
        let mut first = fake("v2", &["A.zip"]);
        first.api.insert(
            API_LATEST.to_owned(),
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
        let mut second = fake("v3", &["A.zip"]);
        second.calls = Arc::clone(&calls);
        let updates = GithubSource::with_cache(second, limited)
            .check(&installation("retail"), &offered(), &records("v1"))
            .unwrap();
        assert!(matches!(updates[0].status, UpdateStatus::Error { .. }));
        assert!(no_api_call(&calls));
    }

    #[test]
    fn errors_belong_to_github() {
        // Errors of the API, read for a release `v2` without `release.json`.
        let status = |response: Result<ApiResponse, TransportError>| {
            let mut fake = fake("v2", &[]);
            fake.api.insert(API_LATEST.to_owned(), response);
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
        // A 403 with requests still available is not the limit.
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
        // Site errors.
        assert_eq!(
            check(Fake::default(), "retail", "v1")[0].status,
            error("GitHub has no published release for BigWigsMods/BigWigs.")
        );
        // An unreadable `release.json`.
        let fake = with_manifest(fake("v2", &["A.zip", MANIFEST]), "v2", "not json");
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

    // ── Update and installation ──

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
        assert_eq!(outcomes[0].file, Some(summary("v427.8", None)));
        assert_eq!(
            fs::read_to_string(addons.join("BigWigs").join("BigWigs.toc")).unwrap(),
            "new"
        );
        // The folder that the new zip no longer has is removed.
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
        // The release and the `release.json` are read once: the update reuses the check.
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

    /// Issue #148: with `reinstall` an up-to-date project is downloaded again from its release.
    #[test]
    fn reinstall_downloads_the_installed_release_again() {
        let tmp = TempDir::new();
        let (installation, addons) = game(&tmp);
        fs::create_dir_all(addons.join("BigWigs")).unwrap();
        fs::write(addons.join("BigWigs").join("BigWigs.toc"), "broken").unwrap();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut fake = fake("v427.8", &["BigWigs-v427.8.zip", MANIFEST]);
        fake.calls = Arc::clone(&calls);
        fake.bytes.insert(
            format!("{DOWNLOADS}/BigWigs-v427.8.zip"),
            zip_of(&[("BigWigs/BigWigs.toc", "restored")]),
        );
        let source = GithubSource::new(fake);
        let run = |reinstall: bool| {
            updates::update_addons_with(
                &installation,
                None,
                &[&source],
                &SourceRecords::from([(GITHUB, records("v427.8"))]),
                &[super::super::ProjectKey {
                    source: GITHUB,
                    key: REPO.to_owned(),
                }],
                reinstall,
                &mut |_, _, _| Ok(()),
                &progress::ignore,
            )
            .unwrap()
            .remove(0)
        };

        let plain = run(false);
        assert_eq!(plain.message.as_deref(), Some("Already up to date."));
        let again = run(true);
        assert_eq!(again.status, updates::OutcomeStatus::Updated, "{again:?}");
        assert_eq!(
            fs::read_to_string(addons.join("BigWigs").join("BigWigs.toc")).unwrap(),
            "restored"
        );
        // The check of an up-to-date project does not read `release.json`: only the reinstall does.
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
            &|_| Ok(()),
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

    #[test]
    fn the_download_touches_no_disk_and_the_install_uses_it() {
        // The import (issue #180): first release and zip, then the installation with that zip.
        let tmp = TempDir::new();
        let (installation, addons) = game(&tmp);
        let mut fake = fake("v427.8", &["BigWigs-v427.8.zip", MANIFEST]);
        fake.bytes.insert(
            format!("{DOWNLOADS}/BigWigs-v427.8.zip"),
            zip_of(&[("BigWigs/BigWigs.toc", "new")]),
        );
        let source = GithubSource::new(fake);
        let download = source
            .download_new(&installation, REPO, &progress::ignore)
            .unwrap();
        assert!(!addons.join("BigWigs").exists());
        let outcome = install_downloaded(
            &installation,
            REPO,
            &download,
            &|_| Ok(()),
            &mut |_, _, _| Ok(()),
            &progress::ignore,
        );
        assert_eq!(
            outcome.status,
            updates::OutcomeStatus::Updated,
            "{outcome:?}"
        );
        assert!(addons.join("BigWigs").is_dir());
        assert_eq!(project_name(REPO), "BigWigs");
    }

    /// Reads real GitHub, without the API: `cargo test --locked live_github_release -- --ignored`.
    #[test]
    #[ignore = "needs the network"]
    fn live_github_release() {
        let transport = crate::sources::http::PublicReqwestTransport::new(&DOMAINS).unwrap();
        let source = GithubSource::new(transport);
        for repo in [REPO, "Nevcairiel/Bartender4"] {
            let release = source.latest_release(repo).unwrap();
            assert!(release.web_repo.is_some(), "{repo}");
            let asset = source.choose(&release, "retail").unwrap().expect("a zip");
            assert!(asset.name.ends_with(".zip"), "{repo}");
            assert!(is_allowed(&asset.browser_download_url), "{repo}");
        }
        assert!(matches!(
            source.latest_release("Auctionator/Auctionator"),
            Err(ReleaseError::Missing(_))
        ));
    }
}
