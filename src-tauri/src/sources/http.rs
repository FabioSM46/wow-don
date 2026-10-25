//! Lo strato HTTP comune alle fonti: le risposte, gli errori di trasporto e la lettura con un
//! limite di dimensione, usati anche dal client di CurseForge. Per le fonti libere (D31), che
//! non hanno chiave, c'è [`PublicTransport`]: una richiesta `GET` per le risposte JSON e una per
//! gli zip, entrambe con un limite. I test usano risposte scritte a mano dietro il trait, senza
//! rete.

use std::io::Read;
use std::time::Duration;

/// Dimensione massima di una risposta JSON di una fonte libera: l'elenco di Tukui pesa pochi
/// KiB, il dettaglio di un file WoWInterface meno ancora.
pub const MAX_JSON_BYTES: u64 = 4 * 1024 * 1024;

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
    /// Il corpo supera il limite chiesto dal chiamante.
    TooLarge,
    Other,
}

pub fn transport_error(error: reqwest::Error) -> TransportError {
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
pub fn read_limited(
    reader: &mut impl Read,
    max_len: u64,
    declared: Option<u64>,
) -> Result<Vec<u8>, TransportError> {
    read_limited_with(reader, max_len, declared, &mut |_| {})
}

/// Come [`read_limited`], e dopo ogni blocco aggiunto chiama `on_read` con i byte letti finora
/// (issue #127). Limiti ed errori sono gli stessi.
pub fn read_limited_with(
    reader: &mut impl Read,
    max_len: u64,
    declared: Option<u64>,
    on_read: &mut ReadProgress<'_>,
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
        on_read(body.len() as u64);
    }
}

/// Riceve i byte letti finora da [`read_limited_with`].
pub type ReadProgress<'a> = dyn FnMut(u64) + 'a;

/// Riceve i byte scaricati finora e quelli dichiarati dal server, se li dichiara.
pub type DownloadProgress<'a> = dyn FnMut(u64, Option<u64>) + 'a;

pub trait PublicTransport: Send + Sync {
    /// Una risposta JSON, al massimo [`MAX_JSON_BYTES`]: oltre, [`TransportError::TooLarge`].
    fn get(&self, url: &str) -> Result<HttpResponse, TransportError>;
    /// Scarica un file. Un corpo più lungo di `max_len` byte dà [`TransportError::TooLarge`]
    /// senza tenerne in memoria più di `max_len`. Chi chiama valida l'URL con
    /// [`is_allowed_url`]; il trasporto reale applica la stessa regola ai redirect.
    fn get_bytes(&self, url: &str, max_len: u64) -> Result<BytesResponse, TransportError>;
    /// Come [`get_bytes`](Self::get_bytes), riportando a `progress` i byte scaricati. Senza una
    /// versione propria il trasporto riporta solo la fine.
    fn get_bytes_with_progress(
        &self,
        url: &str,
        max_len: u64,
        progress: &mut DownloadProgress<'_>,
    ) -> Result<BytesResponse, TransportError> {
        let response = self.get_bytes(url, max_len)?;
        let len = response.body.len() as u64;
        progress(len, Some(len));
        Ok(response)
    }
}

/// Il trasporto reale: `reqwest` bloccante con TLS `rustls`, che segue i redirect solo verso i
/// domini della fonte. Va usato fuori dal thread principale.
pub struct PublicReqwestTransport {
    client: reqwest::blocking::Client,
}

impl PublicReqwestTransport {
    pub fn new(domains: &'static [&'static str]) -> Result<Self, TransportError> {
        let client = reqwest::blocking::Client::builder()
            // Vale per la connessione e per ogni lettura, non per l'intero download.
            .timeout(Duration::from_secs(30))
            .user_agent(concat!("wow-don/", env!("CARGO_PKG_VERSION")))
            .redirect(reqwest::redirect::Policy::custom(move |attempt| {
                if attempt.previous().len() >= 5 || !is_allowed_url(attempt.url(), domains) {
                    attempt.stop()
                } else {
                    attempt.follow()
                }
            }))
            .build()
            .map_err(|_| TransportError::Other)?;
        Ok(Self { client })
    }
}

impl PublicTransport for PublicReqwestTransport {
    fn get(&self, url: &str) -> Result<HttpResponse, TransportError> {
        let mut response = self
            .client
            .get(url)
            .header("accept", "application/json")
            .send()
            .map_err(transport_error)?;
        let status = response.status().as_u16();
        let declared = response.content_length();
        if declared.is_some_and(|len| len > MAX_JSON_BYTES) {
            return Err(TransportError::TooLarge);
        }
        let body = read_limited(&mut response, MAX_JSON_BYTES, declared)?;
        let body = String::from_utf8(body).map_err(|_| TransportError::Other)?;
        Ok(HttpResponse { status, body })
    }

    fn get_bytes(&self, url: &str, max_len: u64) -> Result<BytesResponse, TransportError> {
        self.get_bytes_with_progress(url, max_len, &mut |_, _| {})
    }

    fn get_bytes_with_progress(
        &self,
        url: &str,
        max_len: u64,
        progress: &mut DownloadProgress<'_>,
    ) -> Result<BytesResponse, TransportError> {
        download(self.client.get(url), max_len, progress)
    }
}

/// Invia la richiesta di un download e ne legge il corpo con [`read_limited_with`], riportando
/// a `progress` i byte ricevuti e il `Content-Length`. Comune ai trasporti reali.
pub fn download(
    request: reqwest::blocking::RequestBuilder,
    max_len: u64,
    progress: &mut DownloadProgress<'_>,
) -> Result<BytesResponse, TransportError> {
    let mut response = request.send().map_err(transport_error)?;
    let status = response.status().as_u16();
    let declared = response.content_length();
    if declared.is_some_and(|len| len > max_len) {
        return Err(TransportError::TooLarge);
    }
    let body = read_limited_with(&mut response, max_len, declared, &mut |received| {
        progress(received, declared)
    })?;
    Ok(BytesResponse { status, body })
}

/// Un URL ammesso: `https`, senza credenziali né porta esplicita, verso uno dei `domains` o un
/// suo sottodominio.
pub fn is_allowed_url(url: &reqwest::Url, domains: &[&str]) -> bool {
    let Some(host) = url.host_str() else {
        return false;
    };
    url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && url.port().is_none()
        && domains.iter().any(|domain| {
            host == *domain
                || host
                    .strip_suffix(domain)
                    .is_some_and(|rest| rest.ends_with('.'))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allowed(url: &str) -> bool {
        is_allowed_url(&reqwest::Url::parse(url).unwrap(), &["tukui.org"])
    }

    #[test]
    fn reading_reports_the_bytes_read_after_each_chunk() {
        // Un lettore che consegna 100 KiB a pezzi da 40 KiB.
        struct Chunks(Vec<u8>, usize);
        impl Read for Chunks {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                let n = (self.0.len() - self.1).min(40 * 1024).min(buf.len());
                buf[..n].copy_from_slice(&self.0[self.1..self.1 + n]);
                self.1 += n;
                Ok(n)
            }
        }
        let mut reader = Chunks(vec![7; 100 * 1024], 0);
        let mut seen = Vec::new();
        let body = read_limited_with(&mut reader, 200 * 1024, None, &mut |n| seen.push(n)).unwrap();
        assert_eq!(body.len(), 100 * 1024);
        assert_eq!(seen, [40 * 1024, 80 * 1024, 100 * 1024]);
        // Oltre il limite si ferma come prima, senza riportare il blocco di troppo.
        let mut reader = Chunks(vec![7; 100 * 1024], 0);
        let mut seen = Vec::new();
        let error = read_limited_with(&mut reader, 50 * 1024, None, &mut |n| seen.push(n));
        assert_eq!(error.unwrap_err(), TransportError::TooLarge);
        assert_eq!(seen, [40 * 1024]);
    }

    #[test]
    fn only_https_urls_of_the_source_domains_are_allowed() {
        assert!(allowed("https://api.tukui.org/v1/download/elvui/x"));
        assert!(allowed("https://tukui.org/elvui"));
        assert!(!allowed("http://api.tukui.org/v1/addons"));
        assert!(!allowed("https://eviltukui.org/x"));
        assert!(!allowed("https://tukui.org.example.com/x"));
        let mut with_user = reqwest::Url::parse("https://api.tukui.org/x").unwrap();
        with_user.set_username("someone").unwrap();
        assert!(!is_allowed_url(&with_user, &["tukui.org"]));
        assert!(!allowed("https://api.tukui.org:8443/x"));
    }
}
