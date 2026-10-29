//! Import benchmark (issue #180): measures a real import, over the real network, along the same
//! path as the `import_addons` command (`import_availability`, `CommandInstaller`,
//! `addon_import::run_import`), in a fake installation under a temporary folder. It never touches
//! a real game folder and does not run in CI (`#[ignore]`).
//!
//! It reaches CurseForge through the proxy, as the app does (D44), and reads one environment
//! variable: `WOWDON_IMPORT_LIST`, the path of a file holding a `WOWDON1` string. From the
//! repository root:
//!
//! ```text
//! cd src-tauri
//! WOWDON_IMPORT_LIST=<file> cargo test --locked --lib import_bench -- --ignored --nocapture
//! ```
//!
//! It prints the total time and the time of each phase, derived from the progress events: every
//! interval between two events goes to the phase of the event that opens it. "network" is
//! `checking`, `downloading` and `downloaded`, plus the time before the first event; "disk" is
//! `extracting` and `installing`.

use std::fs;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::*;
use crate::progress::Phase;
use crate::test_support::TempDir;

/// The progress events with the moment they arrived.
#[derive(Default)]
struct Timeline(Mutex<Vec<(Instant, Phase)>>);

impl Timeline {
    fn push(&self, progress: Progress) {
        self.0
            .lock()
            .unwrap()
            .push((Instant::now(), progress.phase));
    }

    /// (network, disk): the sum of the intervals between consecutive events from `start` to
    /// `end`. The time before the first event (the bulk requests) is network.
    fn split(&self, start: Instant, end: Instant) -> (Duration, Duration) {
        let events = self.0.lock().unwrap();
        let mut network = events
            .first()
            .map_or(end, |(first, _)| *first)
            .saturating_duration_since(start);
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
#[ignore = "real network through the proxy: run by hand, see the module documentation"]
fn import_bench() {
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
    let client = CurseForgeClient::new(
        transport,
        Some(crate::config::Config::load().curseforge_url),
    );
    let tukui = tukui_source().unwrap();
    let github = github_source().unwrap();
    let (availability, lookup_error) = import_availability(&decoded.list.addons, &client, &tukui);
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
        prepared: Prepared::default(),
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

    let (network, disk) = timeline.split(checked, end);
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
