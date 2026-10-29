//! App configuration read from the environment.
//!
//! The CurseForge API key comes from `CURSEFORGE_API_KEY`. In development it is
//! loaded from the `.env` file at the repo root (gitignored).

use std::path::Path;

const CURSEFORGE_API_KEY: &str = "CURSEFORGE_API_KEY";

pub struct Config {
    pub curseforge_api_key: Option<String>,
}

impl Config {
    pub fn load() -> Self {
        // In debug builds the .env sits one folder above src-tauri.
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
