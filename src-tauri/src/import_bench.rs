//! Banco di prova dell'import (issue #180): misura un import reale, con la rete vera, nello
//! stesso percorso del comando `import_addons` (`import_availability`, `CommandInstaller`,
//! `addon_import::run_import`), in un'installazione finta sotto una cartella temporanea. Non tocca
//! mai una cartella del gioco vera e non gira in CI (`#[ignore]`).
//!
//! Legge solo due variabili d'ambiente: `CURSEFORGE_API_KEY` (mai stampata) e
//! `WOWDON_IMPORT_LIST`, il percorso di un file con una stringa `WOWDON1`. Dalla radice del
//! repository:
//!
//! ```text
//! cd src-tauri
//! CURSEFORGE_API_KEY=<key> WOWDON_IMPORT_LIST=<file> \
//!   cargo test --locked --lib import_bench -- --ignored --nocapture
//! ```
//!
//! Stampa il tempo totale e quello delle fasi, ricavate dagli eventi di avanzamento: ogni
//! intervallo tra due eventi va alla fase dell'evento che lo apre. "network" sono `checking`,
//! `downloading` e `downloaded`; "disk" sono `extracting` e `installing`.

use std::fs;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::*;
use crate::progress::Phase;
use crate::test_support::TempDir;

/// Gli eventi di avanzamento con il momento in cui sono arrivati.
#[derive(Default)]
struct Timeline(Mutex<Vec<(Instant, Phase)>>);

impl Timeline {
    fn push(&self, progress: Progress) {
        self.0
            .lock()
            .unwrap()
            .push((Instant::now(), progress.phase));
    }

    /// (rete, disco): la somma degli intervalli tra eventi consecutivi, fino a `end`.
    fn split(&self, end: Instant) -> (Duration, Duration) {
        let events = self.0.lock().unwrap();
        let mut network = Duration::ZERO;
        let mut disk = Duration::ZERO;
        for (index, (at, phase)) in events.iter().enumerate() {
            let next = events.get(index + 1).map_or(end, |(next, _)| *next);
            let span = next.saturating_duration_since(*at);
            match phase {
                Phase::Extracting | Phase::Installing => disk += span,
                _ => network += span,
            }
        }
        (network, disk)
    }
}

#[test]
#[ignore = "real network and API key: run by hand, see the module documentation"]
fn import_bench() {
    let key = std::env::var("CURSEFORGE_API_KEY")
        .ok()
        .filter(|key| !key.trim().is_empty())
        .expect("CURSEFORGE_API_KEY is not set");
    let list = std::env::var("WOWDON_IMPORT_LIST").expect("WOWDON_IMPORT_LIST is not set");
    let text = fs::read_to_string(list).expect("the WOWDON_IMPORT_LIST file is not readable");
    let decoded = addon_list::decode(&text).expect("not a WOWDON1 string");

    let tmp = TempDir::new();
    let flavor = decoded.list.flavor.clone();
    let installation = Installation {
        id: "bench".to_owned(),
        path: tmp
            .path()
            .join("World of Warcraft")
            .join(format!("_{flavor}_")),
        name: flavor.clone(),
        flavor,
    };
    let addons_dir = installation.path.join("Interface").join("AddOns");
    fs::create_dir_all(&addons_dir).unwrap();
    let installed = Mutex::new(InstalledAddonsStore::load(
        tmp.path().join("installed-addons.json"),
    ));
    let settings = Mutex::new(AddonSettingsStore::load(
        tmp.path().join("addon-settings.json"),
    ));
    let cache = FingerprintCache::default();
    let timeline = Timeline::default();
    let progress = |event: Progress| timeline.push(event);
    let before = updates::Refresh {
        addons: Vec::new(),
        updates: Vec::new(),
        check_error: None,
    };

    let start = Instant::now();
    let transport = ReqwestTransport::new().unwrap();
    let client = CurseForgeClient::new(transport, Some(key));
    let tukui = tukui_source().unwrap();
    let github = github_source().unwrap();
    let (availability, lookup_error) =
        import_availability(&decoded.list.addons, &client, true, &tukui);
    let checked = Instant::now();
    let mut installer = CommandInstaller {
        installation: &installation,
        client: &client,
        tukui: &tukui,
        github: &github,
        availability: &availability,
        allow_beta: false,
        installed: &installed,
        settings: &settings,
        cache: &cache,
        progress: &progress,
    };
    let result = addon_import::run_import(
        &addons_dir,
        &decoded,
        addon_import::ImportMode::Keep,
        &[],
        &before,
        &SourceRecords::new(),
        &mut installer,
        &mut || panic!("keep does not read the disk again"),
    );
    let end = Instant::now();

    let (network, disk) = timeline.split(end);
    let count = |status| result.entries.iter().filter(|e| e.status == status).count();
    println!(
        "entries: {} ({} installed, {} skipped, {} errors){}",
        result.entries.len(),
        count(addon_import::EntryStatus::Installed),
        count(addon_import::EntryStatus::Skipped),
        count(addon_import::EntryStatus::Error),
        lookup_error.map_or(String::new(), |e| format!(", lookup error: {e}")),
    );
    println!("availability: {:.2}s", (checked - start).as_secs_f64());
    println!("installs:     {:.2}s", (end - checked).as_secs_f64());
    println!("  network:    {:.2}s", network.as_secs_f64());
    println!("  disk:       {:.2}s", disk.as_secs_f64());
    println!("total:        {:.2}s", (end - start).as_secs_f64());
}
