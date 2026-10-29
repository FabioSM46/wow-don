//! The HTTP layer shared by the sources: the responses, the transport errors and reading with a
//! size limit, also used by the CurseForge client. For the free sources (D31), which have no
//! key, there is [`PublicTransport`]: one `GET` request for JSON responses and one for zips,
//! both with a limit. The tests use hand-written responses behind the trait, without the
//! network.

use std::io::Read;
use std::time::Duration;

/// Maximum size of a free source's JSON response: the Tukui list weighs a few KiB, the
/// detail of a WoWInterface file even less.
pub const MAX_JSON_BYTES: u64 = 4 * 1024 * 1024;

pub struct HttpResponse {
    pub status: u16,
    pub body: String,
}

/// A JSON response with the headers a source reads: today only GitHub's rate limit
/// (`x-ratelimit-remaining`, `x-ratelimit-reset` in Unix seconds).
pub struct ApiResponse {
    pub status: u16,
    pub body: String,
    pub ratelimit_remaining: Option<u64>,
    pub ratelimit_reset: Option<u64>,
}

/// The response to a request that does not follow redirects: the status and the `Location`
/// header, without the body. GitHub uses it to read the tag of `releases/latest` (issue #179).
pub struct RedirectResponse {
    pub status: u16,
    pub location: Option<String>,
}

pub struct BytesResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

/// Why a request got no response. It contains no transport text, which could carry
/// details of the request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportError {
    Timeout,
    Connect,
    /// The body exceeds the limit requested by the caller.
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

/// Reads at most `max_len` bytes: as soon as the body would exceed them it stops with
/// [`TransportError::TooLarge`], without appending the excess chunk. The buffer's capacity
/// also stays within `max_len`: growth doubles like `Vec`'s, but without exceeding the
/// limit. `declared` is the `Content-Length` already compared with the limit, used to
/// allocate only once.
pub fn read_limited(
    reader: &mut impl Read,
    max_len: u64,
    declared: Option<u64>,
) -> Result<Vec<u8>, TransportError> {
    read_limited_with(reader, max_len, declared, &mut |_| {})
}

/// Like [`read_limited`], and after every appended chunk calls `on_read` with the bytes read so
/// far (issue #127). Limits and errors are the same.
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

/// Receives the bytes read so far from [`read_limited_with`].
pub type ReadProgress<'a> = dyn FnMut(u64) + 'a;

/// Receives the bytes downloaded so far and those declared by the server, if it declares them.
pub type DownloadProgress<'a> = dyn FnMut(u64, Option<u64>) + 'a;

pub trait PublicTransport: Send + Sync {
    /// A JSON response, at most [`MAX_JSON_BYTES`]: beyond that, [`TransportError::TooLarge`].
    fn get(&self, url: &str) -> Result<HttpResponse, TransportError>;
    /// Like [`get`](Self::get) with the given `Accept` header, and with the rate-limit
    /// headers. Without its own version the transport does not report them.
    fn get_api(&self, url: &str, accept: &str) -> Result<ApiResponse, TransportError> {
        let _ = accept;
        let response = self.get(url)?;
        Ok(ApiResponse {
            status: response.status,
            body: response.body,
            ratelimit_remaining: None,
            ratelimit_reset: None,
        })
    }
    /// A `GET` request that does **not** follow redirects, to read `Location`; the body is not
    /// read. The caller validates the URL and the `Location` with [`is_allowed_url`]. Without its
    /// own version the transport cannot do it and answers [`TransportError::Other`].
    fn get_redirect(&self, url: &str) -> Result<RedirectResponse, TransportError> {
        let _ = url;
        Err(TransportError::Other)
    }
    /// Downloads a file. A body longer than `max_len` bytes gives [`TransportError::TooLarge`]
    /// without holding more than `max_len` of it in memory. The caller validates the URL with
    /// [`is_allowed_url`]; the real transport applies the same rule to redirects.
    fn get_bytes(&self, url: &str, max_len: u64) -> Result<BytesResponse, TransportError>;
    /// Like [`get_bytes`](Self::get_bytes), reporting the downloaded bytes to `progress`. Without
    /// its own version the transport reports only the end.
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

/// The real transport: blocking `reqwest` with `rustls` TLS, which follows redirects only to the
/// source's domains, plus a second client that never follows them ([`get_redirect`]). Must be
/// used off the main thread.
///
/// [`get_redirect`]: PublicTransport::get_redirect
pub struct PublicReqwestTransport {
    client: reqwest::blocking::Client,
    no_redirect: reqwest::blocking::Client,
}

impl PublicReqwestTransport {
    pub fn new(domains: &'static [&'static str]) -> Result<Self, TransportError> {
        let builder = || {
            reqwest::blocking::Client::builder()
                // Applies to the connection and to every read, not to the whole download.
                .timeout(Duration::from_secs(30))
                .user_agent(concat!("wow-don/", env!("CARGO_PKG_VERSION")))
        };
        let client = builder()
            .redirect(reqwest::redirect::Policy::custom(move |attempt| {
                if attempt.previous().len() >= 5 || !is_allowed_url(attempt.url(), domains) {
                    attempt.stop()
                } else {
                    attempt.follow()
                }
            }))
            .build()
            .map_err(|_| TransportError::Other)?;
        let no_redirect = builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| TransportError::Other)?;
        Ok(Self {
            client,
            no_redirect,
        })
    }
}

impl PublicTransport for PublicReqwestTransport {
    fn get(&self, url: &str) -> Result<HttpResponse, TransportError> {
        let response = self.get_api(url, "application/json")?;
        Ok(HttpResponse {
            status: response.status,
            body: response.body,
        })
    }

    fn get_api(&self, url: &str, accept: &str) -> Result<ApiResponse, TransportError> {
        let mut response = self
            .client
            .get(url)
            .header("accept", accept)
            .send()
            .map_err(transport_error)?;
        let status = response.status().as_u16();
        let header = |name: &str| {
            response
                .headers()
                .get(name)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.trim().parse::<u64>().ok())
        };
        let ratelimit_remaining = header("x-ratelimit-remaining");
        let ratelimit_reset = header("x-ratelimit-reset");
        let declared = response.content_length();
        if declared.is_some_and(|len| len > MAX_JSON_BYTES) {
            return Err(TransportError::TooLarge);
        }
        let body = read_limited(&mut response, MAX_JSON_BYTES, declared)?;
        let body = String::from_utf8(body).map_err(|_| TransportError::Other)?;
        Ok(ApiResponse {
            status,
            body,
            ratelimit_remaining,
            ratelimit_reset,
        })
    }

    fn get_redirect(&self, url: &str) -> Result<RedirectResponse, TransportError> {
        let response = self.no_redirect.get(url).send().map_err(transport_error)?;
        let location = response
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        Ok(RedirectResponse {
            status: response.status().as_u16(),
            location,
        })
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

/// Sends a download request and reads its body with [`read_limited_with`], reporting the
/// received bytes and the `Content-Length` to `progress`. Shared by the real transports.
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

/// An allowed URL: `https`, without credentials or an explicit port, to one of the `domains` or
/// one of their subdomains.
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
        // A reader that delivers 100 KiB in 40 KiB pieces.
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
        // Beyond the limit it stops as before, without reporting the excess chunk.
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
