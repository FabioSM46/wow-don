//! Client per l'API di CurseForge (`https://api.curseforge.com`, documentazione su
//! docs.curseforge.com/rest-api).
//!
//! La chiave viaggia solo nell'header `x-api-key` e non compare mai in messaggi d'errore, nel
//! `Debug` o nei log (D10). Il trasporto HTTP sta dietro [`Transport`]: i test provano il
//! parsing e gli errori con risposte scritte a mano, senza rete e senza chiave.

// Alcune parti servono solo al test live (`version_types`) o arriveranno con M3 (campi dei file
// e dei mod per il download): sono già qui perché descrivono l'API, non il loro uso.
#![allow(dead_code)]

use std::fmt;
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

pub const BASE_URL: &str = "https://api.curseforge.com";
/// Id di World of Warcraft nell'API di CurseForge.
pub const WOW_GAME_ID: u32 = 1;
/// `releaseType` dei file stabili.
pub const RELEASE_TYPE_RELEASE: u8 = 1;

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

/// Perché una richiesta non ha avuto risposta. Non contiene testo del trasporto, che
/// potrebbe riportare dettagli della richiesta.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportError {
    Timeout,
    Connect,
    Other,
}

pub trait Transport: Send + Sync {
    fn get(&self, url: &str, api_key: &ApiKey) -> Result<HttpResponse, TransportError>;
    fn post_json(
        &self,
        url: &str,
        api_key: &ApiKey,
        body: String,
    ) -> Result<HttpResponse, TransportError>;
}

/// Il trasporto reale: `reqwest` bloccante con TLS `rustls`. Va usato fuori dal thread
/// principale (i comandi lo chiamano dentro `spawn_blocking`).
pub struct ReqwestTransport {
    client: reqwest::blocking::Client,
}

impl ReqwestTransport {
    pub fn new() -> Result<Self, CurseForgeError> {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent(concat!("wow-don/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|_| CurseForgeError::Network(TransportError::Other))?;
        Ok(Self { client })
    }

    fn send(
        &self,
        request: reqwest::blocking::RequestBuilder,
    ) -> Result<HttpResponse, TransportError> {
        let response = request.send().map_err(|error| {
            if error.is_timeout() {
                TransportError::Timeout
            } else if error.is_connect() {
                TransportError::Connect
            } else {
                TransportError::Other
            }
        })?;
        let status = response.status().as_u16();
        let body = response.text().map_err(|_| TransportError::Other)?;
        Ok(HttpResponse { status, body })
    }
}

impl Transport for ReqwestTransport {
    fn get(&self, url: &str, api_key: &ApiKey) -> Result<HttpResponse, TransportError> {
        self.send(
            self.client
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
            self.client
                .post(url)
                .header("x-api-key", api_key.expose())
                .header("accept", "application/json")
                .header("content-type", "application/json")
                .body(body),
        )
    }
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
}

impl fmt::Display for CurseForgeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingKey => {
                f.write_str("Chiave CurseForge mancante: imposta CURSEFORGE_API_KEY nel file .env.")
            }
            Self::KeyRejected => {
                f.write_str("CurseForge ha rifiutato la chiave: controlla CURSEFORGE_API_KEY.")
            }
            Self::RateLimited => {
                f.write_str("Troppe richieste a CurseForge: riprova tra qualche minuto.")
            }
            Self::Http(status) => {
                write!(f, "CurseForge ha risposto con un errore (HTTP {status}).")
            }
            Self::Network(TransportError::Timeout) => {
                f.write_str("CurseForge non ha risposto in tempo.")
            }
            Self::Network(_) => {
                f.write_str("Impossibile raggiungere CurseForge: controlla la connessione.")
            }
            Self::InvalidResponse => f.write_str("Risposta di CurseForge non riconosciuta."),
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
    pub exact_matches: Vec<FingerprintMatch>,
    pub exact_fingerprints: Vec<u64>,
    pub unmatched_fingerprints: Vec<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FingerprintMatch {
    /// Id del mod (progetto).
    pub id: u32,
    pub file: File,
    #[serde(default)]
    pub latest_files: Vec<File>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct File {
    pub id: u32,
    pub mod_id: u32,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub file_name: String,
    /// Data ISO 8601, per esempio `2026-10-01T12:00:00Z`.
    #[serde(default)]
    pub file_date: String,
    pub release_type: u8,
    #[serde(default)]
    pub game_versions: Vec<String>,
    #[serde(default)]
    pub sortable_game_versions: Vec<SortableGameVersion>,
    #[serde(default)]
    pub file_fingerprint: Option<u64>,
    #[serde(default)]
    pub modules: Vec<FileModule>,
    #[serde(default = "available")]
    pub is_available: bool,
}

fn available() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SortableGameVersion {
    #[serde(default)]
    pub game_version_name: String,
    #[serde(default)]
    pub game_version_type_id: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileModule {
    #[serde(default)]
    pub name: String,
    pub fingerprint: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Mod {
    pub id: u32,
    pub name: String,
    #[serde(default)]
    pub slug: String,
    /// Assente vuol dire sconosciuto: chi lo usa per i download lo tratta come `false`.
    #[serde(default)]
    pub allow_mod_distribution: Option<bool>,
    #[serde(default)]
    pub links: ModLinks,
    #[serde(default)]
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
    pub filename: String,
    pub release_type: u8,
    #[serde(default)]
    pub game_version: String,
    #[serde(default)]
    pub game_version_type_id: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GameVersionType {
    pub id: u32,
    #[serde(default)]
    pub game_id: u32,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
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
        seen: Mutex<Vec<(String, String, Option<String>)>>,
    }

    impl Fake {
        fn new(reply: Result<(u16, &'static str), TransportError>) -> Self {
            Self {
                reply,
                seen: Mutex::new(Vec::new()),
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

    const MODS: &str = r#"{"data": [
          {"id": 1, "name": "Full", "slug": "full", "allowModDistribution": false,
           "links": {"websiteUrl": "https://www.curseforge.com/wow/addons/full"},
           "latestFilesIndexes": [{"gameVersion": "12.1.5", "fileId": 9, "filename": "f.zip",
             "releaseType": 1, "gameVersionTypeId": 517}]},
          {"id": 2, "name": "Minimal"}
        ]}"#;

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
    /// corrispondere a niente, ma la richiesta deve riuscire (non 404).
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
        let result = client
            .match_fingerprints(&[1])
            .expect("richiesta a CurseForge");
        println!(
            "exact: {}, unmatched: {:?}",
            result.exact_matches.len(),
            result.unmatched_fingerprints
        );
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
}
