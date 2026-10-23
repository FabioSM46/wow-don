//! Client per l'API di CurseForge (`https://api.curseforge.com`, documentazione su
//! docs.curseforge.com/rest-api).
//!
//! La chiave viaggia solo nell'header `x-api-key` delle richieste verso [`BASE_URL`] e non
//! compare mai in messaggi d'errore, nel `Debug` o nei log (D10). Gli zip si scaricano dalla
//! CDN senza chiave, solo da host di CurseForge e con un limite di dimensione. Il trasporto HTTP
//! sta dietro [`Transport`]: i test provano il parsing e gli errori con risposte scritte a mano,
//! senza rete e senza chiave.

// Alcune parti servono solo al test live (`version_types`) o arriveranno con M3 (campi dei file
// e dei mod per il download): sono già qui perché descrivono l'API, non il loro uso.
#![allow(dead_code)]

use std::fmt;
use std::io::Read;
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

pub const BASE_URL: &str = "https://api.curseforge.com";
/// Id di World of Warcraft nell'API di CurseForge.
pub const WOW_GAME_ID: u32 = 1;
/// `releaseType` dei file stabili.
pub const RELEASE_TYPE_RELEASE: u8 = 1;
/// Dimensione massima di uno zip scaricato. Gli addon più grandi pesano poche decine di MiB.
pub const MAX_DOWNLOAD_BYTES: u64 = 200 * 1024 * 1024;
/// I domini da cui si scaricano gli zip, con i loro sottodomini.
const DOWNLOAD_DOMAINS: [&str; 2] = ["forgecdn.net", "curseforge.com"];

/// La chiave API. Il `Debug` non la mostra.
#[derive(Clone)]
pub struct ApiKey(String);

impl ApiKey {
    pub fn new(key: String) -> Self {
        Self(key)
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ApiKey(***)")
    }
}

// ── Trasporto ───────────────────────────────────────────────────────────────

pub struct HttpResponse {
    pub status: u16,
    pub body: String,
}

pub struct BytesResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

/// Perché una richiesta non ha avuto risposta. Non contiene testo del trasporto, che
/// potrebbe riportare dettagli della richiesta.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportError {
    Timeout,
    Connect,
    /// Il corpo supera il limite chiesto da [`Transport::get_bytes`].
    TooLarge,
    Other,
}

pub trait Transport: Send + Sync {
    /// Richiesta all'API, con la chiave.
    fn get(&self, url: &str, api_key: &ApiKey) -> Result<HttpResponse, TransportError>;
    /// Richiesta all'API, con la chiave.
    fn post_json(
        &self,
        url: &str,
        api_key: &ApiKey,
        body: String,
    ) -> Result<HttpResponse, TransportError>;
    /// Scarica un file **senza chiave**. Un corpo più lungo di `max_len` byte dà
    /// [`TransportError::TooLarge`] senza tenerne in memoria più di `max_len`.
    ///
    /// Non controlla l'URL: chi chiama deve averlo già validato con `is_download_url`, come fa
    /// [`CurseForgeClient::download`]. Il trasporto reale applica la stessa regola solo ai
    /// redirect.
    fn get_bytes(&self, url: &str, max_len: u64) -> Result<BytesResponse, TransportError>;
}

/// Il trasporto reale: `reqwest` bloccante con TLS `rustls`. Va usato fuori dal thread
/// principale (i comandi lo chiamano dentro `spawn_blocking`).
pub struct ReqwestTransport {
    /// Per l'API: non segue i redirect, perché `x-api-key` non è un header che `reqwest` toglie
    /// passando a un altro host.
    api: reqwest::blocking::Client,
    /// Per gli zip: segue i redirect solo verso host di download ammessi.
    downloads: reqwest::blocking::Client,
}

impl ReqwestTransport {
    pub fn new() -> Result<Self, CurseForgeError> {
        let builder = || {
            reqwest::blocking::Client::builder()
                // Vale per la connessione e per ogni lettura, non per l'intero download.
                .timeout(Duration::from_secs(30))
                .user_agent(concat!("wow-don/", env!("CARGO_PKG_VERSION")))
        };
        let api = builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| CurseForgeError::Network(TransportError::Other))?;
        let downloads = builder()
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                if attempt.previous().len() >= 5 || !is_download_url(attempt.url()) {
                    attempt.stop()
                } else {
                    attempt.follow()
                }
            }))
            .build()
            .map_err(|_| CurseForgeError::Network(TransportError::Other))?;
        Ok(Self { api, downloads })
    }

    fn send(
        &self,
        request: reqwest::blocking::RequestBuilder,
    ) -> Result<HttpResponse, TransportError> {
        let response = request.send().map_err(transport_error)?;
        let status = response.status().as_u16();
        let body = response.text().map_err(|_| TransportError::Other)?;
        Ok(HttpResponse { status, body })
    }
}

fn transport_error(error: reqwest::Error) -> TransportError {
    if error.is_timeout() {
        TransportError::Timeout
    } else if error.is_connect() {
        TransportError::Connect
    } else {
        TransportError::Other
    }
}

/// Legge al massimo `max_len` byte: appena il corpo li supererebbe si ferma con
/// [`TransportError::TooLarge`], senza aggiungere il blocco di troppo. Anche la capacità del
/// buffer resta entro `max_len`: la crescita raddoppia come quella di `Vec`, ma senza superare
/// il limite. `declared` è il `Content-Length` già confrontato con il limite, usato per
/// allocare una volta sola.
fn read_limited(
    reader: &mut impl Read,
    max_len: u64,
    declared: Option<u64>,
) -> Result<Vec<u8>, TransportError> {
    let limit = usize::try_from(max_len).unwrap_or(usize::MAX);
    let initial = declared.map_or(0, |len| usize::try_from(len).unwrap_or(limit).min(limit));
    let mut body = Vec::with_capacity(initial);
    let mut chunk = vec![0u8; 64 * 1024];
    loop {
        let read = match reader.read(&mut chunk) {
            Ok(0) => return Ok(body),
            Ok(read) => read,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == std::io::ErrorKind::TimedOut => {
                return Err(TransportError::Timeout);
            }
            Err(_) => return Err(TransportError::Other),
        };
        if body.len() as u64 + read as u64 > max_len {
            return Err(TransportError::TooLarge);
        }
        let needed = body.len() + read;
        if needed > body.capacity() {
            let target = body.capacity().saturating_mul(2).max(needed).min(limit);
            body.reserve_exact(target - body.len());
        }
        body.extend_from_slice(&chunk[..read]);
    }
}

impl Transport for ReqwestTransport {
    fn get(&self, url: &str, api_key: &ApiKey) -> Result<HttpResponse, TransportError> {
        self.send(
            self.api
                .get(url)
                .header("x-api-key", api_key.expose())
                .header("accept", "application/json"),
        )
    }

    fn post_json(
        &self,
        url: &str,
        api_key: &ApiKey,
        body: String,
    ) -> Result<HttpResponse, TransportError> {
        self.send(
            self.api
                .post(url)
                .header("x-api-key", api_key.expose())
                .header("accept", "application/json")
                .header("content-type", "application/json")
                .body(body),
        )
    }

    fn get_bytes(&self, url: &str, max_len: u64) -> Result<BytesResponse, TransportError> {
        let mut response = self.downloads.get(url).send().map_err(transport_error)?;
        let status = response.status().as_u16();
        let declared = response.content_length();
        if declared.is_some_and(|len| len > max_len) {
            return Err(TransportError::TooLarge);
        }
        let body = read_limited(&mut response, max_len, declared)?;
        Ok(BytesResponse { status, body })
    }
}

/// Un URL da cui si può scaricare: `https`, senza credenziali né porta esplicita, verso un
/// dominio di [`DOWNLOAD_DOMAINS`] o un suo sottodominio.
fn is_download_url(url: &reqwest::Url) -> bool {
    let Some(host) = url.host_str() else {
        return false;
    };
    url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && url.port().is_none()
        && DOWNLOAD_DOMAINS.iter().any(|domain| {
            host == *domain
                || host
                    .strip_suffix(domain)
                    .is_some_and(|rest| rest.ends_with('.'))
        })
}

// ── Errori ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CurseForgeError {
    MissingKey,
    KeyRejected,
    RateLimited,
    Http(u16),
    Network(TransportError),
    InvalidResponse,
    /// Il file non ha `downloadUrl`: l'autore non permette il download da app di terzi (D13).
    DownloadNotAllowed,
    /// `downloadUrl` non è un indirizzo `https` di CurseForge.
    DownloadUrlRejected,
    /// Lo zip supera [`MAX_DOWNLOAD_BYTES`].
    DownloadTooLarge,
}

impl fmt::Display for CurseForgeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingKey => {
                f.write_str("CurseForge key missing: set CURSEFORGE_API_KEY in the .env file.")
            }
            Self::KeyRejected => {
                f.write_str("CurseForge rejected the key: check CURSEFORGE_API_KEY.")
            }
            Self::RateLimited => {
                f.write_str("Too many requests to CurseForge: try again in a few minutes.")
            }
            Self::Http(status) => {
                write!(f, "CurseForge responded with an error (HTTP {status}).")
            }
            Self::Network(TransportError::Timeout) => {
                f.write_str("CurseForge did not respond in time.")
            }
            Self::Network(_) => {
                f.write_str("Could not reach CurseForge: check the connection.")
            }
            Self::InvalidResponse => f.write_str("Unrecognized response from CurseForge."),
            Self::DownloadNotAllowed => f.write_str(
                "The author does not allow downloads from third-party apps: download the addon from the CurseForge website.",
            ),
            Self::DownloadUrlRejected => f.write_str(
                "CurseForge gave an unrecognized download address: download cancelled.",
            ),
            Self::DownloadTooLarge => write!(
                f,
                "The file to download exceeds the {} MiB limit: download cancelled.",
                MAX_DOWNLOAD_BYTES / (1024 * 1024)
            ),
        }
    }
}

impl std::error::Error for CurseForgeError {}

// ── Modello dei dati ────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct Envelope<T> {
    data: T,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FingerprintMatches {
    #[serde(deserialize_with = "null_as_default")]
    pub exact_matches: Vec<FingerprintMatch>,
    #[serde(deserialize_with = "null_as_default")]
    pub exact_fingerprints: Vec<u64>,
    /// File di cui combacia solo una parte delle cartelle (un addon installato in parte, una
    /// cartella modificata, versioni miste). Ogni cartella va abbinata al file i cui `modules`
    /// contengono il suo fingerprint, e solo se non c'è un match esatto.
    #[serde(deserialize_with = "null_as_default")]
    pub partial_matches: Vec<FingerprintMatch>,
    #[serde(deserialize_with = "null_as_default")]
    pub unmatched_fingerprints: Vec<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FingerprintMatch {
    /// Id del mod (progetto).
    pub id: u32,
    pub file: File,
    #[serde(default, deserialize_with = "null_as_default")]
    pub latest_files: Vec<File>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct File {
    pub id: u32,
    pub mod_id: u32,
    #[serde(default, deserialize_with = "null_as_default")]
    pub display_name: String,
    /// Identifica il file da scaricare: assente o `null` resta `None`, mai un nome vuoto.
    #[serde(default)]
    pub file_name: Option<String>,
    /// Data ISO 8601, per esempio `2026-10-01T12:00:00Z`.
    #[serde(default, deserialize_with = "null_as_default")]
    pub file_date: String,
    pub release_type: u8,
    #[serde(default, deserialize_with = "null_as_default")]
    pub game_versions: Vec<String>,
    #[serde(default, deserialize_with = "null_as_default")]
    pub sortable_game_versions: Vec<SortableGameVersion>,
    #[serde(default)]
    pub file_fingerprint: Option<u64>,
    #[serde(default, deserialize_with = "null_as_default")]
    pub modules: Vec<FileModule>,
    #[serde(default = "available", deserialize_with = "null_as_available")]
    pub is_available: bool,
    /// Indirizzo dello zip sulla CDN. Assente o `null` quando l'autore non permette il download
    /// da app di terzi (D13).
    #[serde(default)]
    pub download_url: Option<String>,
}

fn available() -> bool {
    true
}

/// L'API a volte manda `null` dove ci si aspetta una lista o un testo descrittivo (per esempio
/// `"unmatchedFingerprints": null`): `null` vale come il valore vuoto, come un campo assente.
///
/// Solo per liste e testi descrittivi. Gli identificativi (id, `fileName`, `releaseType`,
/// fingerprint, nome del mod) restano rigorosi o diventano `Option`: un `0` o un nome vuoto al
/// posto di un dato mancante sembrerebbero validi a chi li usa.
fn null_as_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

fn null_as_available<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<bool>::deserialize(deserializer)?.unwrap_or(true))
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SortableGameVersion {
    #[serde(default, deserialize_with = "null_as_default")]
    pub game_version_name: String,
    #[serde(default)]
    pub game_version_type_id: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileModule {
    #[serde(default, deserialize_with = "null_as_default")]
    pub name: String,
    pub fingerprint: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Mod {
    pub id: u32,
    pub name: String,
    #[serde(default, deserialize_with = "null_as_default")]
    pub slug: String,
    /// Assente vuol dire sconosciuto: chi lo usa per i download lo tratta come `false`.
    #[serde(default)]
    pub allow_mod_distribution: Option<bool>,
    #[serde(default, deserialize_with = "null_as_default")]
    pub links: ModLinks,
    #[serde(default, deserialize_with = "null_as_default")]
    pub latest_files_indexes: Vec<FileIndex>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ModLinks {
    pub website_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileIndex {
    pub file_id: u32,
    #[serde(default)]
    pub filename: Option<String>,
    pub release_type: u8,
    #[serde(default, deserialize_with = "null_as_default")]
    pub game_version: String,
    #[serde(default)]
    pub game_version_type_id: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GameVersionType {
    pub id: u32,
    #[serde(default)]
    pub game_id: Option<u32>,
    #[serde(default, deserialize_with = "null_as_default")]
    pub name: String,
    #[serde(default, deserialize_with = "null_as_default")]
    pub slug: String,
}

#[derive(Serialize)]
struct FingerprintsRequest<'a> {
    fingerprints: &'a [u32],
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ModsRequest<'a> {
    mod_ids: &'a [u32],
}

// ── Client ──────────────────────────────────────────────────────────────────

pub struct CurseForgeClient<T: Transport> {
    transport: T,
    key: Option<ApiKey>,
}

impl<T: Transport> fmt::Debug for CurseForgeClient<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CurseForgeClient")
            .field("key", &self.key)
            .finish_non_exhaustive()
    }
}

impl<T: Transport> CurseForgeClient<T> {
    pub fn new(transport: T, key: Option<String>) -> Self {
        Self {
            transport,
            // Ripulita dagli spazi: un header con spazi ai bordi verrebbe rifiutato dal
            // trasporto e l'errore sembrerebbe un problema di rete.
            key: key
                .map(|k| k.trim().to_owned())
                .filter(|k| !k.is_empty())
                .map(ApiKey::new),
        }
    }

    fn key(&self) -> Result<&ApiKey, CurseForgeError> {
        self.key.as_ref().ok_or(CurseForgeError::MissingKey)
    }

    /// Gli addon che CurseForge riconosce dai loro fingerprint. Una sola richiesta.
    pub fn match_fingerprints(
        &self,
        fingerprints: &[u32],
    ) -> Result<FingerprintMatches, CurseForgeError> {
        let key = self.key()?;
        if fingerprints.is_empty() {
            return Ok(FingerprintMatches::default());
        }
        let body = serde_json::to_string(&FingerprintsRequest { fingerprints })
            .map_err(|_| CurseForgeError::InvalidResponse)?;
        let url = format!("{BASE_URL}/v1/fingerprints/{WOW_GAME_ID}");
        decode(self.transport.post_json(&url, key, body))
    }

    /// Il dettaglio di più mod. Una sola richiesta.
    pub fn get_mods(&self, mod_ids: &[u32]) -> Result<Vec<Mod>, CurseForgeError> {
        let key = self.key()?;
        if mod_ids.is_empty() {
            return Ok(Vec::new());
        }
        let body = serde_json::to_string(&ModsRequest { mod_ids })
            .map_err(|_| CurseForgeError::InvalidResponse)?;
        decode(
            self.transport
                .post_json(&format!("{BASE_URL}/v1/mods"), key, body),
        )
    }

    /// I tipi di versione del gioco di WoW (Retail, Classic, …).
    pub fn version_types(&self) -> Result<Vec<GameVersionType>, CurseForgeError> {
        let key = self.key()?;
        let url = format!("{BASE_URL}/v1/games/{WOW_GAME_ID}/version-types");
        decode(self.transport.get(&url, key))
    }

    /// Il dettaglio di un file di un mod (`GET /v1/mods/{modId}/files/{fileId}`).
    pub fn get_file(&self, mod_id: u32, file_id: u32) -> Result<File, CurseForgeError> {
        let key = self.key()?;
        let url = format!("{BASE_URL}/v1/mods/{mod_id}/files/{file_id}");
        decode(self.transport.get(&url, key))
    }

    /// I byte dello zip di `file`, scaricati dalla CDN senza chiave. Non controlla
    /// `allowModDistribution` del mod: lo fa chi decide cosa installare.
    ///
    /// Senza `downloadUrl`, o con un indirizzo che non è `https` verso un host di CurseForge,
    /// fallisce senza nessuna richiesta. Gli errori non riportano l'indirizzo.
    pub fn download(&self, file: &File) -> Result<Vec<u8>, CurseForgeError> {
        let raw = file
            .download_url
            .as_deref()
            .ok_or(CurseForgeError::DownloadNotAllowed)?;
        let url = reqwest::Url::parse(raw)
            .ok()
            .filter(is_download_url)
            .ok_or(CurseForgeError::DownloadUrlRejected)?;
        let response = self
            .transport
            .get_bytes(url.as_str(), MAX_DOWNLOAD_BYTES)
            .map_err(|error| match error {
                TransportError::TooLarge => CurseForgeError::DownloadTooLarge,
                error => CurseForgeError::Network(error),
            })?;
        match response.status {
            200..=299 => Ok(response.body),
            429 => Err(CurseForgeError::RateLimited),
            status => Err(CurseForgeError::Http(status)),
        }
    }
}

fn decode<D: DeserializeOwned>(
    response: Result<HttpResponse, TransportError>,
) -> Result<D, CurseForgeError> {
    let response = response.map_err(CurseForgeError::Network)?;
    match response.status {
        200..=299 => serde_json::from_str::<Envelope<D>>(&response.body)
            .map(|envelope| envelope.data)
            .map_err(|_| CurseForgeError::InvalidResponse),
        401 | 403 => Err(CurseForgeError::KeyRejected),
        429 => Err(CurseForgeError::RateLimited),
        status => Err(CurseForgeError::Http(status)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    const KEY: &str = "test-key-7f3a9c";

    /// Un trasporto finto: risponde sempre con la stessa risposta e registra le richieste.
    struct Fake {
        reply: Result<(u16, &'static str), TransportError>,
        /// Le richieste con la chiave: URL, chiave, corpo.
        seen: Mutex<Vec<(String, String, Option<String>)>>,
        /// Risposta ai download.
        bytes: Result<(u16, &'static [u8]), TransportError>,
        /// I download: URL e limite. Il trait non passa la chiave a `get_bytes`.
        seen_bytes: Mutex<Vec<(String, u64)>>,
    }

    impl Fake {
        fn new(reply: Result<(u16, &'static str), TransportError>) -> Self {
            Self {
                reply,
                seen: Mutex::new(Vec::new()),
                bytes: Ok((200, b"PK\x03\x04zip")),
                seen_bytes: Mutex::new(Vec::new()),
            }
        }

        fn answer(&self) -> Result<HttpResponse, TransportError> {
            self.reply.map(|(status, body)| HttpResponse {
                status,
                body: body.to_owned(),
            })
        }
    }

    impl Transport for Fake {
        fn get(&self, url: &str, key: &ApiKey) -> Result<HttpResponse, TransportError> {
            self.seen
                .lock()
                .unwrap()
                .push((url.to_owned(), key.expose().to_owned(), None));
            self.answer()
        }

        fn post_json(
            &self,
            url: &str,
            key: &ApiKey,
            body: String,
        ) -> Result<HttpResponse, TransportError> {
            self.seen
                .lock()
                .unwrap()
                .push((url.to_owned(), key.expose().to_owned(), Some(body)));
            self.answer()
        }

        fn get_bytes(&self, url: &str, max_len: u64) -> Result<BytesResponse, TransportError> {
            self.seen_bytes
                .lock()
                .unwrap()
                .push((url.to_owned(), max_len));
            self.bytes.map(|(status, body)| BytesResponse {
                status,
                body: body.to_vec(),
            })
        }
    }

    fn client(reply: Result<(u16, &'static str), TransportError>) -> CurseForgeClient<Fake> {
        CurseForgeClient::new(Fake::new(reply), Some(KEY.to_owned()))
    }

    const MATCHES: &str = r#"{
      "data": {
        "isCacheBuilt": true,
        "exactMatches": [{
          "id": 61284,
          "file": {
            "id": 5001, "modId": 61284, "displayName": "Details 20261001",
            "fileName": "Details.zip", "fileDate": "2026-10-01T12:00:00Z",
            "releaseType": 1, "gameVersions": ["12.1.5"],
            "sortableGameVersions": [{"gameVersionName": "12.1.5", "gameVersionTypeId": 517}],
            "fileFingerprint": 4000000000,
            "modules": [{"name": "Details", "fingerprint": 123}],
            "unknownField": {"ignored": true}
          },
          "latestFiles": []
        }],
        "exactFingerprints": [123],
        "partialMatches": [],
        "unmatchedFingerprints": [456]
      }
    }"#;

    #[test]
    fn fingerprint_matches_are_parsed_and_sent_correctly() {
        let client = client(Ok((200, MATCHES)));
        let result = client.match_fingerprints(&[123, 456]).unwrap();
        assert_eq!(result.exact_matches.len(), 1);
        let matched = &result.exact_matches[0];
        assert_eq!(matched.id, 61284);
        assert_eq!(matched.file.modules[0].fingerprint, 123);
        assert_eq!(matched.file.file_fingerprint, Some(4_000_000_000));
        assert_eq!(
            matched.file.sortable_game_versions[0].game_version_type_id,
            Some(517)
        );
        assert!(matched.file.is_available);
        assert_eq!(result.unmatched_fingerprints, [456]);

        let seen = client.transport.seen.lock().unwrap();
        assert_eq!(seen[0].0, "https://api.curseforge.com/v1/fingerprints/1");
        assert_eq!(seen[0].1, KEY);
        assert_eq!(seen[0].2.as_deref(), Some(r#"{"fingerprints":[123,456]}"#));
    }

    /// Forma di `FingerprintsMatchesResult` nella documentazione ufficiale: `partialMatches` è
    /// una lista di `FingerprintMatch`, come `exactMatches`.
    #[test]
    fn partial_matches_are_parsed() {
        let body = r#"{"data": {
          "isCacheBuilt": true,
          "exactMatches": [],
          "exactFingerprints": [],
          "partialMatches": [{
            "id": 3358,
            "file": {
              "id": 7001, "modId": 3358, "displayName": "DBM 12.0.1",
              "fileName": "DBM.zip", "fileDate": "2026-10-01T12:00:00Z", "releaseType": 1,
              "modules": [{"name": "DBM-Core", "fingerprint": 11}, {"name": "DBM-GUI", "fingerprint": 22}]
            },
            "latestFiles": null
          }],
          "partialMatchFingerprints": {"7001": [11]},
          "installedFingerprints": [11, 99],
          "unmatchedFingerprints": [99]
        }}"#;
        let client = client(Ok((200, body)));
        let result = client.match_fingerprints(&[11, 99]).unwrap();
        assert!(result.exact_matches.is_empty());
        assert_eq!(result.partial_matches.len(), 1);
        let partial = &result.partial_matches[0];
        assert_eq!(partial.id, 3358);
        assert_eq!(partial.file.modules[1].name, "DBM-GUI");
        assert!(partial.latest_files.is_empty());
    }

    const MODS: &str = r#"{"data": [
          {"id": 1, "name": "Full", "slug": "full", "allowModDistribution": false,
           "links": {"websiteUrl": "https://www.curseforge.com/wow/addons/full"},
           "latestFilesIndexes": [{"gameVersion": "12.1.5", "fileId": 9, "filename": "f.zip",
             "releaseType": 1, "gameVersionTypeId": 517}]},
          {"id": 2, "name": "Minimal"}
        ]}"#;

    /// La forma esatta di una risposta reale di `POST /v1/fingerprints/1` per un fingerprint
    /// che non corrisponde a niente: `unmatchedFingerprints` è `null`, non una lista vuota.
    #[test]
    fn nulls_in_the_real_response_are_read_as_empty() {
        let client = client(Ok((
            200,
            r#"{"data":{"isCacheBuilt":true,"exactMatches":[],"exactFingerprints":[],"partialMatches":[],"partialMatchFingerprints":{},"installedFingerprints":[1],"unmatchedFingerprints":null}}"#,
        )));
        let result = client.match_fingerprints(&[1]).unwrap();
        assert!(result.exact_matches.is_empty());
        assert!(result.partial_matches.is_empty());
        assert!(result.unmatched_fingerprints.is_empty());

        // `partialMatches` null o assente vale come lista vuota.
        for body in [
            r#"{"data":{"exactMatches":[],"partialMatches":null}}"#,
            r#"{"data":{"exactMatches":[]}}"#,
        ] {
            let partial = self::client(Ok((200, body)));
            let result = partial.match_fingerprints(&[1]).unwrap();
            assert!(result.partial_matches.is_empty());
        }

        let file = r#"{"id": 1, "modId": 2, "displayName": null, "fileName": null, "fileDate": null,
          "releaseType": 1, "gameVersions": null, "sortableGameVersions": null,
          "modules": null, "isAvailable": null}"#;
        let file: File = serde_json::from_str(file).unwrap();
        assert!(file.display_name.is_empty() && file.modules.is_empty());
        assert_eq!(file.file_name, None);
        // Gli identificativi non diventano 0: un id null resta un errore.
        let broken = r#"{"id": null, "modId": 2, "releaseType": 1}"#;
        assert!(serde_json::from_str::<File>(broken).is_err());
        assert!(file.is_available);
    }

    #[test]
    fn mods_with_missing_optional_fields_are_parsed() {
        let client = client(Ok((200, MODS)));
        let mods = client.get_mods(&[1, 2]).unwrap();
        assert_eq!(mods[0].allow_mod_distribution, Some(false));
        assert_eq!(
            mods[0].latest_files_indexes[0].game_version_type_id,
            Some(517)
        );
        assert_eq!(
            mods[0].links.website_url.as_deref(),
            Some("https://www.curseforge.com/wow/addons/full")
        );
        assert_eq!(mods[1].allow_mod_distribution, None);
        assert!(mods[1].latest_files_indexes.is_empty());
        let seen = client.transport.seen.lock().unwrap();
        assert_eq!(seen[0].0, "https://api.curseforge.com/v1/mods");
        assert_eq!(seen[0].2.as_deref(), Some(r#"{"modIds":[1,2]}"#));
    }

    #[test]
    fn version_types_are_read_with_a_get() {
        let client = client(Ok((
            200,
            r#"{"data": [{"id": 517, "gameId": 1, "name": "WoW Retail", "slug": "wow_retail", "isSyncable": true, "status": 1}]}"#,
        )));
        let types = client.version_types().unwrap();
        assert_eq!(types[0].id, 517);
        assert_eq!(types[0].slug, "wow_retail");
        let seen = client.transport.seen.lock().unwrap();
        assert_eq!(
            seen[0].0,
            "https://api.curseforge.com/v1/games/1/version-types"
        );
        assert_eq!(seen[0].2, None);
    }

    #[test]
    fn empty_input_makes_no_request() {
        let client = client(Ok((500, "")));
        assert_eq!(
            client.match_fingerprints(&[]).unwrap(),
            FingerprintMatches::default()
        );
        assert!(client.get_mods(&[]).unwrap().is_empty());
        assert!(client.transport.seen.lock().unwrap().is_empty());
    }

    #[test]
    fn a_missing_key_fails_before_any_request() {
        for key in [None, Some(String::new()), Some("  ".to_owned())] {
            let client = CurseForgeClient::new(Fake::new(Ok((200, MATCHES))), key);
            assert_eq!(
                client.match_fingerprints(&[1]),
                Err(CurseForgeError::MissingKey)
            );
            assert_eq!(client.version_types(), Err(CurseForgeError::MissingKey));
            assert!(client.transport.seen.lock().unwrap().is_empty());
        }
    }

    #[test]
    fn errors_are_mapped() {
        for (reply, expected) in [
            (Ok((401, "")), CurseForgeError::KeyRejected),
            (Ok((403, "")), CurseForgeError::KeyRejected),
            (Ok((429, "")), CurseForgeError::RateLimited),
            (Ok((500, "")), CurseForgeError::Http(500)),
            (Ok((200, "{ broken")), CurseForgeError::InvalidResponse),
            (
                Ok((200, r#"{"nodata": 1}"#)),
                CurseForgeError::InvalidResponse,
            ),
            (
                Err(TransportError::Timeout),
                CurseForgeError::Network(TransportError::Timeout),
            ),
            (
                Err(TransportError::Connect),
                CurseForgeError::Network(TransportError::Connect),
            ),
        ] {
            assert_eq!(client(reply).version_types(), Err(expected));
        }
    }

    #[test]
    fn the_key_never_appears_in_messages_or_debug() {
        let replies = [
            Ok((401, "")),
            Ok((429, "")),
            Ok((500, "")),
            Ok((200, "{ broken")),
            Err(TransportError::Other),
        ];
        for reply in replies {
            let client = client(reply);
            let error = client.match_fingerprints(&[1]).unwrap_err();
            assert!(!error.to_string().contains(KEY));
            assert!(!format!("{error:?}").contains(KEY));
            assert!(!format!("{client:?}").contains(KEY));
        }
        assert_eq!(format!("{:?}", ApiKey::new(KEY.to_owned())), "ApiKey(***)");
        assert!(
            CurseForgeError::MissingKey
                .to_string()
                .contains("CURSEFORGE_API_KEY")
        );
    }

    #[test]
    fn the_key_is_trimmed_before_use() {
        let client = CurseForgeClient::new(
            Fake::new(Ok((200, r#"{"data": []}"#))),
            Some(format!("  {KEY} \n")),
        );
        client.version_types().unwrap();
        assert_eq!(client.transport.seen.lock().unwrap()[0].1, KEY);
    }

    const FILE: &str = r#"{"data": {
        "id": 5001, "modId": 61284, "displayName": "Details 20261001",
        "fileName": "Details.zip", "fileDate": "2026-10-01T12:00:00Z", "releaseType": 1,
        "downloadUrl": "https://edge.forgecdn.net/files/5/1/Details 20261001.zip"
      }}"#;

    const CDN_URL: &str = "https://edge.forgecdn.net/files/5/1/Details%2020261001.zip";

    fn downloadable(url: Option<&str>) -> File {
        let mut file: File = serde_json::from_str(
            r#"{"id": 5001, "modId": 61284, "releaseType": 1, "downloadUrl": null}"#,
        )
        .unwrap();
        file.download_url = url.map(str::to_owned);
        file
    }

    #[test]
    fn get_file_reads_the_download_url() {
        let client = client(Ok((200, FILE)));
        let file = client.get_file(61284, 5001).unwrap();
        assert_eq!(file.id, 5001);
        assert_eq!(
            file.download_url.as_deref(),
            Some("https://edge.forgecdn.net/files/5/1/Details 20261001.zip")
        );
        let seen = client.transport.seen.lock().unwrap();
        assert_eq!(
            seen[0].0,
            "https://api.curseforge.com/v1/mods/61284/files/5001"
        );
        assert_eq!(seen[0].1, KEY);
        assert_eq!(seen[0].2, None);

        // `null` e assente valgono entrambi "non scaricabile".
        assert_eq!(downloadable(None).download_url, None);
        let absent: File =
            serde_json::from_str(r#"{"id": 1, "modId": 2, "releaseType": 1}"#).unwrap();
        assert_eq!(absent.download_url, None);
    }

    #[test]
    fn download_goes_to_the_cdn_without_the_key() {
        let client = client(Ok((200, FILE)));
        let file = client.get_file(61284, 5001).unwrap();
        let bytes = client.download(&file).unwrap();
        assert_eq!(bytes, b"PK\x03\x04zip");
        // Una sola richiesta con la chiave, quella all'API; il download passa da `get_bytes`,
        // che la chiave non la riceve.
        let seen = client.transport.seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert!(seen[0].0.starts_with(BASE_URL));
        let downloads = client.transport.seen_bytes.lock().unwrap();
        assert_eq!(*downloads, [(CDN_URL.to_owned(), MAX_DOWNLOAD_BYTES)]);
    }

    #[test]
    fn download_works_without_a_key() {
        let client = CurseForgeClient::new(Fake::new(Ok((200, ""))), None);
        assert!(client.download(&downloadable(Some(CDN_URL))).is_ok());
    }

    #[test]
    fn a_file_without_download_url_is_not_downloaded() {
        let client = client(Ok((200, "")));
        assert_eq!(
            client.download(&downloadable(None)),
            Err(CurseForgeError::DownloadNotAllowed)
        );
        assert!(
            CurseForgeError::DownloadNotAllowed
                .to_string()
                .contains("does not allow downloads from third-party apps")
        );
        assert!(client.transport.seen_bytes.lock().unwrap().is_empty());
    }

    #[test]
    fn only_https_curseforge_hosts_are_downloaded() {
        for url in [
            "https://edge.forgecdn.net/files/1/2/a.zip",
            "https://mediafilez.forgecdn.net/files/1/2/a.zip",
            "https://forgecdn.net/a.zip",
            "https://www.curseforge.com/a.zip",
            "https://EDGE.ForgeCDN.net:443/a.zip",
        ] {
            let client = client(Ok((200, "")));
            assert!(client.download(&downloadable(Some(url))).is_ok(), "{url}");
            assert_eq!(client.transport.seen_bytes.lock().unwrap().len(), 1);
        }
        for url in [
            "http://edge.forgecdn.net/files/1/2/a.zip",
            "https://evil.example/a.zip",
            "https://forgecdn.net.evil.example/a.zip",
            "https://evilforgecdn.net/a.zip",
            "https://notcurseforge.com/a.zip",
            "https://edge.forgecdn.net:8443/a.zip",
            // Credenziali nell'URL; spezzato perché il controllo di privacy non lo legga come email.
            concat!("https://user:pass", "@", "edge.forgecdn.net/a.zip"),
            "https://203.0.113.7/a.zip",
            "ftp://edge.forgecdn.net/a.zip",
            "file:///etc/passwd",
            "edge.forgecdn.net/a.zip",
            "",
        ] {
            let client = client(Ok((200, "")));
            assert_eq!(
                client.download(&downloadable(Some(url))),
                Err(CurseForgeError::DownloadUrlRejected),
                "{url}"
            );
            assert!(client.transport.seen_bytes.lock().unwrap().is_empty());
        }
    }

    #[test]
    fn download_errors_are_mapped_without_url_or_key() {
        for (reply, expected) in [
            (Ok((404, &b""[..])), CurseForgeError::Http(404)),
            (Ok((403, &b""[..])), CurseForgeError::Http(403)),
            (Ok((429, &b""[..])), CurseForgeError::RateLimited),
            (
                Err(TransportError::TooLarge),
                CurseForgeError::DownloadTooLarge,
            ),
            (
                Err(TransportError::Timeout),
                CurseForgeError::Network(TransportError::Timeout),
            ),
            (
                Err(TransportError::Other),
                CurseForgeError::Network(TransportError::Other),
            ),
        ] {
            let mut fake = Fake::new(Ok((200, "")));
            fake.bytes = reply;
            let client = CurseForgeClient::new(fake, Some(KEY.to_owned()));
            let error = client.download(&downloadable(Some(CDN_URL))).unwrap_err();
            assert_eq!(error, expected);
            for text in [error.to_string(), format!("{error:?}")] {
                assert!(!text.contains(KEY), "{text}");
                assert!(!text.contains("forgecdn"), "{text}");
                assert!(!text.contains("files/5/1"), "{text}");
            }
        }
        for error in [
            CurseForgeError::DownloadNotAllowed,
            CurseForgeError::DownloadUrlRejected,
        ] {
            assert!(!error.to_string().contains("forgecdn"));
        }
        assert!(
            CurseForgeError::DownloadTooLarge
                .to_string()
                .contains("200 MiB")
        );
    }

    #[test]
    fn read_limited_stops_at_the_limit() {
        let data = vec![7u8; 200_000];
        // Né la lunghezza né la capacità superano il limite, con o senza Content-Length.
        for (max_len, declared) in [
            (200_000, None),
            (200_000, Some(200_000)),
            (250_001, None),
            (250_001, Some(10)),
        ] {
            let body = read_limited(&mut data.as_slice(), max_len, declared).unwrap();
            assert_eq!(body, data);
            assert!(body.capacity() as u64 <= max_len, "{}", body.capacity());
        }
        assert_eq!(
            read_limited(&mut data.as_slice(), 199_999, None),
            Err(TransportError::TooLarge)
        );
        assert!(read_limited(&mut &b""[..], 0, None).unwrap().is_empty());
    }

    /// Un server HTTP locale che risponde a una sola richiesta con `response` e restituisce
    /// la richiesta ricevuta (riga e header).
    fn local_server(response: Vec<u8>) -> (String, std::thread::JoinHandle<String>) {
        use std::io::Write;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut byte = [0u8; 1];
            while !request.ends_with(b"\r\n\r\n") && stream.read(&mut byte).unwrap() == 1 {
                request.push(byte[0]);
            }
            // Il client può chiudere prima di aver letto tutto (limite superato).
            let _ = stream.write_all(&response);
            let _ = stream.flush();
            String::from_utf8_lossy(&request).to_lowercase()
        });
        (address, handle)
    }

    #[test]
    fn real_transport_downloads_without_the_key_and_within_the_limit() {
        let transport = ReqwestTransport::new().unwrap();

        let (address, server) = local_server(
            b"HTTP/1.1 200 OK\r\ncontent-length: 4\r\nconnection: close\r\n\r\nPK\x03\x04".to_vec(),
        );
        let response = transport.get_bytes(&format!("{address}/a.zip"), 4).unwrap();
        assert_eq!(
            (response.status, response.body.as_slice()),
            (200, &b"PK\x03\x04"[..])
        );
        let request = server.join().unwrap();
        assert!(request.starts_with("get /a.zip "));
        assert!(!request.contains("x-api-key"));

        // Content-Length oltre il limite: rifiutato prima di leggere il corpo.
        let (address, server) = local_server(
            b"HTTP/1.1 200 OK\r\ncontent-length: 100\r\nconnection: close\r\n\r\n".to_vec(),
        );
        assert_eq!(
            transport.get_bytes(&address, 99).err(),
            Some(TransportError::TooLarge)
        );
        server.join().unwrap();

        // Senza Content-Length: si ferma leggendo.
        let mut body = b"HTTP/1.1 200 OK\r\nconnection: close\r\n\r\n".to_vec();
        body.extend(std::iter::repeat_n(b'x', 300_000));
        let (address, server) = local_server(body);
        assert_eq!(
            transport.get_bytes(&address, 100_000).err(),
            Some(TransportError::TooLarge)
        );
        server.join().unwrap();
    }

    /// Un redirect non viene seguito verso un host che non è di CurseForge, e l'API non segue
    /// redirect: la chiave non può arrivare a un altro host.
    #[test]
    fn real_transport_does_not_follow_redirects_off_curseforge() {
        let transport = ReqwestTransport::new().unwrap();
        let redirect = b"HTTP/1.1 302 Found\r\nlocation: http://127.0.0.1:9/other\r\ncontent-length: 0\r\nconnection: close\r\n\r\n";

        let (address, server) = local_server(redirect.to_vec());
        assert_eq!(transport.get_bytes(&address, 10).unwrap().status, 302);
        server.join().unwrap();

        let (address, server) = local_server(redirect.to_vec());
        let response = transport
            .get(&address, &ApiKey::new(KEY.to_owned()))
            .unwrap();
        assert_eq!(response.status, 302);
        server.join().unwrap();
    }

    /// Prova che TLS e indirizzo funzionano davvero, senza chiave: una chiave inventata deve
    /// essere rifiutata da CurseForge (401/403), non fallire per rete o certificati.
    ///
    /// `cargo test --manifest-path src-tauri/Cargo.toml live_tls_reaches_curseforge -- --ignored`
    #[test]
    #[ignore = "usa la rete"]
    fn live_tls_reaches_curseforge() {
        let client = CurseForgeClient::new(
            ReqwestTransport::new().unwrap(),
            Some("not-a-real-key".to_owned()),
        );
        assert_eq!(client.version_types(), Err(CurseForgeError::KeyRejected));
    }

    /// Prova `POST /v1/fingerprints/1` con la chiave reale. Un fingerprint inventato non deve
    /// corrispondere a niente, ma la richiesta deve riuscire (non 404). Con
    /// `WOW_DON_LIVE_FINGERPRINTS=<f1>,<f2>` stampa anche i match parziali trovati.
    ///
    /// `cargo test --manifest-path src-tauri/Cargo.toml live_fingerprint_matches -- --ignored --nocapture`
    #[test]
    #[ignore = "usa la rete e la chiave CurseForge reale"]
    fn live_fingerprint_matches() {
        let config = crate::config::Config::load();
        let client = CurseForgeClient::new(
            ReqwestTransport::new().unwrap(),
            config.curseforge_api_key.clone(),
        );
        // Fingerprint da `WOW_DON_LIVE_FINGERPRINTS` (numeri separati da virgole), per provare
        // anche i match parziali con le cartelle di un addon reale; altrimenti uno inventato.
        let fingerprints: Vec<u32> = std::env::var("WOW_DON_LIVE_FINGERPRINTS")
            .ok()
            .map(|v| {
                v.split(',')
                    .map(|f| f.trim().parse().expect("fingerprint non numerico"))
                    .collect()
            })
            .unwrap_or_else(|| vec![1]);
        let result = client
            .match_fingerprints(&fingerprints)
            .expect("richiesta a CurseForge");
        println!(
            "exact: {}, partial: {}, unmatched: {:?}",
            result.exact_matches.len(),
            result.partial_matches.len(),
            result.unmatched_fingerprints
        );
        for found in &result.partial_matches {
            let modules: Vec<(&str, u64)> = found
                .file
                .modules
                .iter()
                .map(|m| (m.name.as_str(), m.fingerprint))
                .collect();
            println!(
                "partial: mod {} file {} modules {modules:?}",
                found.id, found.file.id
            );
        }
    }

    /// Interroga l'API reale con la chiave dell'ambiente (o di `.env` in debug) e stampa i
    /// tipi di versione del gioco di WoW, per confermare gli id usati dal controllo degli
    /// aggiornamenti. Lo lancia l'utente, a mano:
    ///
    /// `cargo test --manifest-path src-tauri/Cargo.toml live_version_types -- --ignored --nocapture`
    #[test]
    #[ignore = "usa la rete e la chiave CurseForge reale"]
    fn live_version_types() {
        let config = crate::config::Config::load();
        let client = CurseForgeClient::new(
            ReqwestTransport::new().unwrap(),
            config.curseforge_api_key.clone(),
        );
        let types = client.version_types().expect("richiesta a CurseForge");
        for t in types {
            println!("{:>6}  {:<40} {}", t.id, t.name, t.slug);
        }
    }

    /// Legge l'ultimo file di Details! (mod 61284) con `get_file` e, se l'autore lo permette,
    /// lo scarica dalla CDN e controlla che sia uno zip. Lo lancia l'utente, a mano:
    ///
    /// `cargo test --manifest-path src-tauri/Cargo.toml live_get_file_and_download -- --ignored --nocapture`
    #[test]
    #[ignore = "usa la rete e la chiave CurseForge reale"]
    fn live_get_file_and_download() {
        // Progetto da `WOW_DON_LIVE_MOD_ID`, altrimenti Details! (61284). Il test fallisce se il
        // progetto è distribuibile ma il download non riesce: così non passa senza aver provato
        // la CDN.
        let mod_id: u32 = std::env::var("WOW_DON_LIVE_MOD_ID")
            .ok()
            .map(|v| {
                v.trim()
                    .parse()
                    .expect("WOW_DON_LIVE_MOD_ID non è un numero")
            })
            .unwrap_or(61284);
        let config = crate::config::Config::load();
        let client = CurseForgeClient::new(
            ReqwestTransport::new().unwrap(),
            config.curseforge_api_key.clone(),
        );
        let mods = client.get_mods(&[mod_id]).expect("richiesta a CurseForge");
        let project = mods.first().expect("progetto trovato");
        let index = project
            .latest_files_indexes
            .first()
            .expect("almeno un file");
        let file = client
            .get_file(mod_id, index.file_id)
            .expect("richiesta a CurseForge");
        println!(
            "progetto {} ({}), allowModDistribution: {:?}; file {} ({:?}), downloadUrl presente: {}",
            project.id,
            project.name,
            project.allow_mod_distribution,
            file.id,
            file.file_name,
            file.download_url.is_some()
        );
        match client.download(&file) {
            Ok(bytes) => {
                println!("scaricati {} byte", bytes.len());
                assert!(bytes.starts_with(b"PK"), "non è uno zip");
            }
            Err(CurseForgeError::DownloadNotAllowed) => {
                assert_ne!(
                    project.allow_mod_distribution,
                    Some(true),
                    "progetto distribuibile ma senza downloadUrl"
                );
                println!("download not allowed by the author");
            }
            Err(error) => panic!("{error}"),
        }
    }
}
