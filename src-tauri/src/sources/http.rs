//! Trasporto HTTP per le fonti libere (D31), che non hanno chiave: una richiesta `GET` per le
//! risposte JSON e una per gli zip, con un limite di dimensione. Come per CurseForge, i test
//! usano risposte scritte a mano dietro [`PublicTransport`], senza rete.

use std::time::Duration;

use super::curseforge::{
    BytesResponse, HttpResponse, TransportError, read_limited, transport_error,
};

pub trait PublicTransport: Send + Sync {
    /// Una risposta JSON.
    fn get(&self, url: &str) -> Result<HttpResponse, TransportError>;
    /// Scarica un file. Un corpo più lungo di `max_len` byte dà [`TransportError::TooLarge`]
    /// senza tenerne in memoria più di `max_len`. Chi chiama valida l'URL con
    /// [`is_allowed_url`]; il trasporto reale applica la stessa regola ai redirect.
    fn get_bytes(&self, url: &str, max_len: u64) -> Result<BytesResponse, TransportError>;
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
        let response = self
            .client
            .get(url)
            .header("accept", "application/json")
            .send()
            .map_err(transport_error)?;
        let status = response.status().as_u16();
        let body = response.text().map_err(|_| TransportError::Other)?;
        Ok(HttpResponse { status, body })
    }

    fn get_bytes(&self, url: &str, max_len: u64) -> Result<BytesResponse, TransportError> {
        let mut response = self.client.get(url).send().map_err(transport_error)?;
        let status = response.status().as_u16();
        let declared = response.content_length();
        if declared.is_some_and(|len| len > max_len) {
            return Err(TransportError::TooLarge);
        }
        let body = read_limited(&mut response, max_len, declared)?;
        Ok(BytesResponse { status, body })
    }
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
