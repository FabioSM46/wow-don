//! Configurazione dell'app letta dall'ambiente.
//!
//! La chiave API di CurseForge arriva da `CURSEFORGE_API_KEY`. In sviluppo viene
//! caricata dal file `.env` nella radice del repo (gitignored).

use std::path::Path;

const CURSEFORGE_API_KEY: &str = "CURSEFORGE_API_KEY";

pub struct Config {
    pub curseforge_api_key: Option<String>,
}

impl Config {
    pub fn load() -> Self {
        // In debug il .env sta una cartella sopra src-tauri.
        if cfg!(debug_assertions) {
            let dev_env = Path::new(env!("CARGO_MANIFEST_DIR")).join("../.env");
            let _ = dotenvy::from_path(dev_env);
        }
        let _ = dotenvy::dotenv();

        let curseforge_api_key = std::env::var(CURSEFORGE_API_KEY)
            .ok()
            .map(|key| key.trim().to_owned())
            .filter(|key| !key.is_empty());

        Self { curseforge_api_key }
    }
}
