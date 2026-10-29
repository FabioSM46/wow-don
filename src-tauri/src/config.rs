//! App configuration read from the environment.
//!
//! The app holds no CurseForge key (D44): it reaches CurseForge through the wow-don-proxy
//! Worker. `WOWDON_CURSEFORGE_PROXY` points it at another proxy, for example a local
//! `wrangler dev`.

use crate::sources::curseforge::PROXY_URL;

const PROXY_OVERRIDE: &str = "WOWDON_CURSEFORGE_PROXY";

pub struct Config {
    /// The base URL of the CurseForge API as the app sees it: the proxy.
    pub curseforge_url: String,
}

impl Config {
    pub fn load() -> Self {
        let curseforge_url = std::env::var(PROXY_OVERRIDE)
            .ok()
            .and_then(|value| proxy_override(&value, cfg!(debug_assertions)))
            .unwrap_or_else(|| PROXY_URL.to_owned());
        Self { curseforge_url }
    }
}

/// The override, if it is usable: an `https` URL, or plain `http` to the loopback address only in
/// debug builds (`wrangler dev`). Anything else is ignored and the built-in proxy is used.
fn proxy_override(value: &str, debug: bool) -> Option<String> {
    let url = reqwest::Url::parse(value.trim()).ok()?;
    let host = url.host_str()?;
    let allowed = match url.scheme() {
        "https" => true,
        "http" => debug && matches!(host, "127.0.0.1" | "localhost" | "[::1]"),
        _ => false,
    };
    let plain = url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none();
    (allowed && plain).then(|| url.as_str().trim_end_matches('/').to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn https_overrides_are_accepted() {
        assert_eq!(
            proxy_override(" https://proxy.example.com/ ", false).as_deref(),
            Some("https://proxy.example.com")
        );
        assert_eq!(
            proxy_override("https://proxy.example.com/base", false).as_deref(),
            Some("https://proxy.example.com/base")
        );
    }

    #[test]
    fn loopback_http_only_in_debug() {
        for value in ["http://127.0.0.1:8787", "http://localhost:8787"] {
            assert!(proxy_override(value, true).is_some(), "{value}");
            assert_eq!(proxy_override(value, false), None, "{value}");
        }
        assert_eq!(proxy_override("http://proxy.example.com", true), None);
    }

    #[test]
    fn anything_else_is_ignored() {
        for value in [
            "",
            "proxy.example.com",
            "ftp://proxy.example.com",
            "https://proxy.example.com/?key=1",
            "https://proxy.example.com/#x",
        ] {
            assert_eq!(proxy_override(value, true), None, "{value}");
        }
    }
}
