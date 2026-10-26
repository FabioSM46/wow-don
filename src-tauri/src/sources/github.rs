//! Le release GitHub come fonte (D31, issue #108): solo i progetti che l'app ha registrato con un
//! record `github` (`id` = `owner/repo`). Gli addon installati a mano non si abbinano mai a un
//! repository.
//!
//! L'ultima release si legge **senza l'API** (issue #179), solo da `github.com`, che non conta
//! nel limite di 60 richieste l'ora dell'API senza token:
//!
//! 1. il tag dall'intestazione `Location` di `https://github.com/{owner}/{repo}/releases/latest`,
//!    letta senza seguire il redirect: deve essere `https://github.com/{owner}/{repo}/releases/tag/{tag}`
//!    per lo stesso repository (un 404 vuol dire nessuna release). Un repository spostato
//!    risponde con un redirect verso `releases/latest` del nuovo nome, seguito una volta sola;
//!    un tag con `/` (`release/2.0`) non sta in un segmento e si legge dall'API;
//! 2. quando serve scegliere lo zip, il `release.json` del packager da
//!    `https://github.com/{owner}/{repo}/releases/download/{tag}/release.json`;
//! 3. lo zip da `https://github.com/{owner}/{repo}/releases/download/{tag}/{filename}`.
//!
//! Solo quando la release non ha un `release.json` (404 su quel file) lo zip si sceglie dagli
//! asset, che elenca solo l'API: `GET https://api.github.com/repos/{owner}/{repo}/releases/latest`,
//! senza token. Il limite superato e il suo messaggio valgono solo per questa lettura.
//!
//! Ogni risposta, anche un errore, si legge una volta per esecuzione. Tra un comando e l'altro
//! (`Cache`) una release letta vale dieci minuti, e dopo una risposta di limite superato
//! dell'API non parte nessun'altra richiesta all'API fino al ripristino.
//!
//! Verificato il 2026-10-26: `releases/latest` di BigWigs risponde `302` verso
//! `/releases/tag/v427.8`, quello di Auctionator (nessuna release) `404`, e
//! `releases/download/{tag}/release.json` rimanda a `release-assets.githubusercontent.com`.
//!
//! Risposta dell'API verificata il 2026-10-25 su BigWigs `v427.8`: `tag_name`, `published_at`,
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
use super::{FolderCheck, LinkInfo, RecordWriter, Source, SourceKind, UpdateTarget};
use crate::addons::scan::InstalledAddon;
use crate::addons::toc;
use crate::installations::Installation;
use crate::progress::{ProgressSink, Reporter};
use crate::store::{ProjectRecords, SourceRecord};
use crate::updates::{self, AddonUpdate, FileSummary, Project, UpdateOutcome, UpdateStatus};

pub const API_URL: &str = "https://api.github.com";
/// Il sito, da cui si leggono il tag dell'ultima release e i suoi file senza l'API.
const WEB_URL: &str = "https://github.com";
/// I domini da cui GitHub serve l'API e gli asset (`github.com` rimanda gli asset a
/// `release-assets.githubusercontent.com`), con i loro sottodomini.
pub const DOMAINS: [&str; 2] = ["github.com", "githubusercontent.com"];
/// Il tipo di risposta che l'API di GitHub raccomanda.
const ACCEPT: &str = "application/vnd.github+json";
/// Il nome dell'asset scritto dal packager.
const MANIFEST: &str = "release.json";

const GITHUB: SourceKind = SourceKind::Github;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct Release {
    pub(crate) tag_name: String,
    #[serde(default)]
    published_at: Option<String>,
    #[serde(default)]
    assets: Vec<Asset>,
    /// Il repository (`owner/repo`) di una release letta senza l'API (issue #179): gli asset
    /// non sono elencati, lo zip si sceglie dal `release.json` del tag.
    #[serde(skip)]
    web_repo: Option<String>,
}

/// Un asset di una release: il nome del file e l'indirizzo da cui scaricarlo.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct Asset {
    pub(crate) name: String,
    pub(crate) browser_download_url: String,
}

/// Perché l'ultima release di un repository non si legge. Le varianti separano i casi che chi
/// chiama tratta in modo diverso (il mirror di un progetto CurseForge, issue #162); il testo è
/// lo stesso messaggio in inglese che il controllo mostra.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ReleaseError {
    /// Il limite di 60 richieste l'ora, con l'ora di ripristino nel messaggio.
    Limited(String),
    /// Il repository non è valido, non esiste o non ha release pubblicate (HTTP 404).
    Missing(String),
    /// Rete, risposta illeggibile o un altro errore HTTP.
    Other(String),
}

impl ReleaseError {
    pub(crate) fn message(&self) -> &str {
        match self {
            Self::Limited(message) | Self::Missing(message) | Self::Other(message) => message,
        }
    }
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

/// Perché lo zip di una release non si sceglie, con il tipo dell'errore (issue #162).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ChoiceError {
    /// Il `release.json` non ha una voce per il gioco, o la voce nomina un asset che manca.
    NoFileForGame(String),
    /// Il limite di richieste dell'API, letta solo per una release senza `release.json`.
    Limited(String),
    /// Il `release.json` non si legge: rete, risposta illeggibile o un altro errore HTTP.
    Other(String),
}

impl ChoiceError {
    pub(crate) fn message(&self) -> &str {
        match self {
            Self::NoFileForGame(message) | Self::Limited(message) | Self::Other(message) => message,
        }
    }
}

/// Quanto vale una release letta per i comandi successivi: ogni "Reload" rifà il controllo, e
/// senza questa memoria N progetti GitHub costerebbero N delle 60 richieste l'ora a ogni giro.
pub const RELEASE_TTL: Duration = Duration::from_secs(10 * 60);
/// Quanto si aspetta dopo un limite superato senza `x-ratelimit-reset`.
const LIMIT_FALLBACK: Duration = Duration::from_secs(5 * 60);

/// Le release lette con successo, per repository, con l'istante della lettura.
type Fresh = Mutex<HashMap<String, (Instant, Release)>>;

/// Le risposte di GitHub che valgono tra un comando e l'altro: le release lette con successo,
/// per [`RELEASE_TTL`] (senza l'API e dall'API), e il limite di richieste dell'API superato,
/// fino all'ora di ripristino. Gli altri errori valgono solo per il comando che li ha ricevuti.
#[derive(Default)]
pub struct Cache {
    releases: Fresh,
    api_releases: Fresh,
    /// Il messaggio del limite e fino a quando vale.
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

/// La fonte GitHub. Ogni risposta, anche un errore, si tiene per tutta la vita della fonte, che
/// dura un comando: l'aggiornamento riusa il controllo che lo precede. Le release lette e il
/// limite superato passano anche ai comandi successivi attraverso [`Cache`].
pub struct GithubSource<T: PublicTransport> {
    transport: T,
    cache: Arc<Cache>,
    releases: Mutex<HashMap<String, Result<Release, ReleaseError>>>,
    api_releases: Mutex<HashMap<String, Result<Release, ReleaseError>>>,
    /// `None`: il `release.json` non c'è (HTTP 404).
    manifests: Mutex<HashMap<String, Result<Option<Manifest>, String>>>,
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
            api_releases: Mutex::new(HashMap::new()),
            manifests: Mutex::new(HashMap::new()),
            chosen: Mutex::new(HashMap::new()),
        }
    }

    /// L'ultima release di `repo`, letta al massimo una volta per comando e al massimo una
    /// volta per [`RELEASE_TTL`].
    fn latest(&self, repo: &str) -> Result<Release, String> {
        self.latest_release(repo)
            .map_err(|error| error.message().to_owned())
    }

    /// Come [`latest`](Self::latest), con il tipo dell'errore. Non passa dall'API.
    pub(crate) fn latest_release(&self, repo: &str) -> Result<Release, ReleaseError> {
        remembered(&self.releases, &self.cache.releases, repo, || {
            self.fetch_latest(repo)
        })
    }

    /// L'ultima release di `repo` letta dall'API, con i suoi asset: solo per una release senza
    /// `release.json`. Stessa memoria di [`latest_release`](Self::latest_release).
    fn api_release(&self, repo: &str) -> Result<Release, ReleaseError> {
        remembered(&self.api_releases, &self.cache.api_releases, repo, || {
            self.fetch_api(repo)
        })
    }

    /// Il tag dell'ultima release dal redirect di `releases/latest` (vedi la documentazione del
    /// modulo), senza l'API e quindi senza il blocco del limite di richieste.
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
        // Al massimo un redirect di repository spostato, poi quello verso il tag.
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
                // Il limite dell'API vale anche qui: è una lettura dell'API.
                Some(Target::SlashedTag) => return self.api_release(&current),
                _ => return Err(unrecognized()),
            }
        }
        Err(unrecognized())
    }

    /// L'ultima release dall'API, con gli asset. È l'unica lettura che conta nel limite di 60
    /// richieste l'ora, e l'unica che il limite superato blocca.
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

    /// Il `release.json` all'indirizzo `url`, letto al massimo una volta; `None` se non c'è
    /// (HTTP 404). Si scarica da `github.com`, non dall'API: non conta nel limite di richieste.
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

    /// Lo zip di `release` per il gioco `flavor` (vedi la documentazione del modulo).
    fn choose(&self, release: &Release, flavor: &str) -> Choice {
        self.choose_zip(release, flavor)
            .map_err(|error| error.message().to_owned())
    }

    /// Come [`choose`](Self::choose), con il tipo dell'errore.
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

    /// Lo zip di una release letta senza l'API: dal `release.json` del tag, con la stessa regola
    /// del ramo dell'API. Senza `release.json` (404) gli asset si leggono dall'API, e la release
    /// deve essere ancora quella del tag.
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
        // `filename` è il nome di un file della release, mai un percorso.
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

    /// Scarica `asset` solo dai domini di GitHub e con il limite di dimensione dei download: lo
    /// usa anche il mirror dei progetti CurseForge bloccati (issue #163).
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

    /// Scarica `asset` e lo installa al posto di `old_folders`, poi registra il tag. `check`
    /// può rifiutare le cartelle dello zip prima che tocchino il disco. L'esito usa la forma di
    /// un aggiornamento: `updated` vuol dire installato.
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

    /// Scarica `asset` riportando l'avanzamento: la parte di rete di un'installazione.
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

    /// Installa un progetto GitHub che non c'è ancora (l'import, issue #134, e i link, issue
    /// #109), con la stessa scelta dello zip del controllo. `check` può rifiutare le cartelle
    /// dello zip prima che tocchino il disco.
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

    /// La parte di rete di [`install_new`](Self::install_new): l'ultima release, lo zip per il
    /// gioco e il suo download, senza toccare il disco. L'import la fa per più progetti insieme
    /// (issue #180).
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

    /// L'anteprima di un link (issue #109): l'ultima release e se ha uno zip per il gioco
    /// dell'installazione, con la stessa scelta dell'installazione. Le letture (senza l'API, salvo
    /// una release senza `release.json`) si riusano dalla cache all'installazione. `Err` se la
    /// release non si legge.
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

/// La release di un progetto GitHub con il suo zip, scaricato e non ancora installato. Né
/// `Clone` né un `Debug` con i byte: lo zip può pesare decine di MB.
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

/// Il nome del progetto `owner/repo` negli esiti e nell'avanzamento: il repository.
pub fn project_name(key: &str) -> &str {
    repo_name(key)
}

/// La parte su disco di [`GithubSource::install_new`]: installa `download` e registra il tag.
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

/// Installa lo zip già scaricato di `release` al posto di `old_folders`, poi registra il tag.
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

const WEBSITE_ONLY: &str = "The latest GitHub release has no single zip WoW-don can pick: download it from the release page, then drop the zip into this window.";

fn lock<V>(mutex: &Mutex<V>) -> std::sync::MutexGuard<'_, V> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// La release di `repo` dalla memoria del comando (`local`, anche gli errori), poi da quella
/// tra i comandi (`shared`, solo i successi per [`RELEASE_TTL`]), altrimenti da `fetch`.
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

/// La voce del `release.json` per il gioco `flavor`: quella il cui `metadata` contiene il
/// flavor del packager, preferendo `nolib: false`.
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

/// Dove porta il redirect di `releases/latest`.
#[derive(Debug, PartialEq, Eq)]
enum Target {
    /// `/{owner}/{repo}/releases/tag/{tag}` dello stesso repository: il tag, decodificato.
    Tag(String),
    /// `/{owner}/{repo}/releases/latest` di un altro nome: il repository è stato spostato.
    Moved(String),
    /// Il tag dello stesso repository contiene `/` (`release/2.0`, un nome di tag valido per
    /// git): non sta in un segmento di `releases/download/{tag}/`, quindi la release si legge
    /// dall'API, che dà gli indirizzi giusti dei suoi asset.
    SlashedTag,
}

/// Il redirect di `releases/latest` di `repo` richiesto a `from`: solo `https://github.com`,
/// senza query né frammento, verso il tag dello stesso repository (senza distinguere maiuscole
/// e minuscole, come GitHub) o verso `releases/latest` di un repository spostato. Altrimenti
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
                // Ogni pezzo tra le `/` deve essere un nome: niente `..` né pezzi vuoti.
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

/// Un segmento di percorso con le sequenze `%XX` decodificate, se è UTF-8 valido.
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

/// Un nome che sta in un solo segmento di percorso: non vuoto, senza `/` né `\`, diverso da
/// `.` e `..`, senza caratteri di controllo.
fn is_file_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name
            .chars()
            .any(|c| c == '/' || c == '\\' || c.is_control())
}

/// `https://github.com/{owner}/{repo}/releases/download/{tag}/{file}`, con `tag` e `file`
/// codificati come segmenti di percorso.
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
/// | `_forever_`, `_classic_beta_` | WoW Forever | `forever` |
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
        // Con `reinstall` (issue #148) un addon aggiornato si riscarica dalla release installata.
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
            // Per un progetto aggiornato il controllo non sceglie il file (costerebbe una lettura
            // di `release.json` a ogni controllo): lo si sceglie qui, dalla release già letta.
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
        // Le cartelle del progetto sul disco: quelle che il nuovo zip non ha più si tolgono.
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
    /// `releases/latest` sul sito, letto senza seguire il redirect.
    const LATEST: &str = "https://github.com/BigWigsMods/BigWigs/releases/latest";
    /// La stessa release dall'API: solo per una release senza `release.json`.
    const API_LATEST: &str = "https://api.github.com/repos/BigWigsMods/BigWigs/releases/latest";
    const DOWNLOADS: &str = "https://github.com/BigWigsMods/BigWigs/releases/download/v427.8";

    fn downloads(tag: &str) -> String {
        format!("https://github.com/{REPO}/releases/download/{tag}")
    }

    /// Le risposte del trasporto finto, per URL.
    #[derive(Default)]
    struct Fake {
        /// `releases/latest` → (stato, `Location`); senza voce, 404.
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

    /// La release reale del 2026-10-25, ridotta ai campi usati, con gli asset `assets`.
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

    /// Il redirect di `releases/latest` verso `location`.
    fn found(location: &str) -> Result<(u16, Option<String>), TransportError> {
        Ok((302, Some(location.to_owned())))
    }

    fn tag_url(tag: &str) -> String {
        format!("https://github.com/{REPO}/releases/tag/{tag}")
    }

    /// Una release `tag` con gli asset `assets`: il sito risponde con il redirect verso il tag,
    /// e il `release.json` c'è solo se è tra gli asset. L'API elenca gli asset.
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
                // Senza l'API la data di pubblicazione non si conosce.
                latest: summary("v427.8", None)
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
        // Il `release.json` manca (404): solo allora si legge l'API.
        assert_eq!(
            *calls.lock().unwrap(),
            [
                LATEST.to_owned(),
                format!("{}/{MANIFEST}", downloads("v2")),
                API_LATEST.to_owned()
            ]
        );
    }

    /// Issue #179: tag dal redirect, `release.json` e zip da `releases/download/{tag}/`, senza
    /// nessuna richiesta all'API.
    #[test]
    fn a_release_with_release_json_is_read_without_the_api() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut fake = fake("v427.8", &["BigWigs-v427.8.zip", MANIFEST]);
        // L'API non risponde: non deve servire.
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
        // Un indirizzo relativo, maiuscole diverse e un tag codificato vanno bene.
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
        // Lo zip di un tag codificato si scarica dallo stesso tag, codificato di nuovo.
        assert_eq!(
            download_url(REPO, "1.0+beta x", "A.zip").as_deref(),
            Some("https://github.com/BigWigsMods/BigWigs/releases/download/1.0+beta%20x/A.zip")
        );
        // Altri stati del sito.
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

    /// Un tag con `/` (valido per git) non sta in `releases/download/{tag}/`: la release si legge
    /// dall'API, con i suoi asset, invece di fallire.
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

    /// Un repository spostato rimanda a `releases/latest` del nuovo nome: si segue una volta.
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
        // Due spostamenti di fila, o uno verso lo stesso nome, no.
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

    /// Tra il redirect e l'API può uscire una release nuova: non si mescolano.
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
        // Un record le cui cartelle non sono più sul disco non chiede niente.
        let updates = source
            .check(&installation("retail"), &[addon("Other")], &records("v1"))
            .unwrap();
        assert!(updates.is_empty());
        assert!(calls.lock().unwrap().is_empty());
    }

    #[test]
    fn the_rate_limit_is_a_clear_error_and_stops_further_api_requests() {
        for status in [403, 429] {
            let calls = Arc::new(Mutex::new(Vec::new()));
            let mut fake = Fake {
                calls: Arc::clone(&calls),
                ..Fake::default()
            };
            // Due release senza `release.json`: lo zip si sceglie dall'API.
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
                    // Le 14:32:00 UTC di un giorno lontano: un orario già passato farebbe
                    // scadere il limite durante il test.
                    ratelimit_reset: Some(1_793_025_120 + 36_500 * 86_400),
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
            // Il secondo progetto non fa partire una richiesta all'API.
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

    /// Issue #179: il limite dell'API non ferma una release con `release.json`.
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
        // Senza `release.json` serve l'API, e il limite la blocca senza richiesta.
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
        // Il secondo comando trova la release ancora valida.
        assert_eq!(*calls.lock().unwrap(), [LATEST]);
        // Scaduta, si rilegge.
        lock(&cache.releases).get_mut(REPO).unwrap().0 -= RELEASE_TTL;
        run();
        assert_eq!(calls.lock().unwrap().len(), 2);

        // Anche la release letta dall'API (senza `release.json`) vale per i comandi successivi.
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
        // Il secondo giro non rilegge niente: la release e l'API sono in cache, il 404 del
        // `release.json` vale solo per il comando, quindi si rilegge.
        assert_eq!(
            *calls.lock().unwrap(),
            [
                LATEST.to_owned(),
                format!("{}/{MANIFEST}", downloads("v2")),
                API_LATEST.to_owned(),
                format!("{}/{MANIFEST}", downloads("v2")),
            ]
        );

        // Il limite superato vale anche per i comandi successivi, fino al ripristino.
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
        // Errori dell'API, letta per una release `v2` senza `release.json`.
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
        // Errori del sito.
        assert_eq!(
            check(Fake::default(), "retail", "v1")[0].status,
            error("GitHub has no published release for BigWigsMods/BigWigs.")
        );
        // Un `release.json` illeggibile.
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
        assert_eq!(outcomes[0].file, Some(summary("v427.8", None)));
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

    /// Issue #148: con `reinstall` un progetto aggiornato si riscarica dalla release installata.
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
        // Il controllo di un progetto aggiornato non legge `release.json`: solo la reinstallazione.
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
        // L'import (issue #180): prima release e zip, poi l'installazione con quello zip.
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

    /// Legge GitHub vero, senza l'API: `cargo test --locked live_github_release -- --ignored`.
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
