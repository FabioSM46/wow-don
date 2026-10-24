//! Ricerca automatica delle installazioni di WoW nei percorsi standard (issue #87).
//!
//! Due parti separate:
//!
//! - [`candidate_roots`] costruisce l'elenco delle cartelle `World of Warcraft` da guardare e
//!   dipende dalla piattaforma (Windows: dischi e registro; macOS: le due `Applications`;
//!   Linux: per ora niente, i prefissi Wine arrivano con la issue #88);
//! - [`examine`] è comune a tutte le piattaforme: legge solo le cartelle candidate e i loro
//!   figli diretti, e tiene le cartelle `_<flavor>_` valide secondo
//!   [`installations::validate`], la stessa regola dell'aggiunta manuale.
//!
//! Il modulo suggerisce soltanto: non scrive mai `installations.json`. Non scrive log, così
//! nessun percorso della macchina finisce in un messaggio.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::installations::{self, Installation};

/// Una cartella `World of Warcraft` da esaminare, con il testo che dice da dove viene.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub path: PathBuf,
    /// Testo breve in inglese mostrato all'utente, per esempio `"Program Files (x86)"`.
    pub origin: String,
}

impl Candidate {
    // Su Linux nessuna candidata ancora (issue #88 la userà per i prefissi Wine).
    #[cfg_attr(not(any(target_os = "windows", target_os = "macos")), allow(dead_code))]
    pub fn new(path: PathBuf, origin: &str) -> Self {
        Self {
            path,
            origin: origin.to_owned(),
        }
    }
}

/// Un'installazione trovata e non ancora aggiunta: gli stessi `path`, `flavor` e `name` di
/// [`Installation`], più la provenienza.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredInstallation {
    pub path: PathBuf,
    pub flavor: String,
    pub name: String,
    pub origin: String,
}

impl DiscoveredInstallation {
    fn new(installation: Installation, origin: &str) -> Self {
        Self {
            path: installation.path,
            flavor: installation.flavor,
            name: installation.name,
            origin: origin.to_owned(),
        }
    }
}

/// Esamina le candidate nell'ordine dato e restituisce le cartelle `_<flavor>_` figlie dirette
/// valide, senza duplicati (stesso percorso canonico, cioè stesso id: vale la prima candidata
/// che l'ha trovata) e senza quelle già in `known`. Dentro una candidata l'ordine è quello dei
/// nomi delle cartelle. Una candidata che non esiste o non si legge si salta senza errore, e
/// così ogni figlio che non si riesce a leggere.
pub fn examine(candidates: &[Candidate], known: &[Installation]) -> Vec<DiscoveredInstallation> {
    let mut seen: HashSet<String> = known.iter().map(|k| k.id.clone()).collect();
    let mut found = Vec::new();
    for candidate in candidates {
        for child in flavor_children(&candidate.path) {
            let Ok(installation) = installations::validate(&child) else {
                continue;
            };
            if seen.insert(installation.id.clone()) {
                found.push(DiscoveredInstallation::new(installation, &candidate.origin));
            }
        }
    }
    found
}

/// I figli diretti di `root` con un nome `_<flavor>_`, ordinati per nome. Una sola lettura
/// della cartella, nessuna visita ricorsiva; `validate` controlla poi che siano cartelle.
fn flavor_children(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut children: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .and_then(installations::flavor_from_dir_name)
                .is_some()
        })
        .map(|entry| entry.path())
        .collect();
    children.sort();
    children
}

/// Ricava la cartella del gioco dal valore `InstallPath` del registro di Windows. Se il valore
/// punta già a una cartella `_<flavor>_` (per esempio `...\World of Warcraft\_retail_\`) si
/// restituisce la cartella sopra di essa. È una funzione di testo, così si prova su tutte le
/// piattaforme: accetta sia `\` sia `/` come separatore. Un valore vuoto non dà niente.
// La usa solo la lettura del registro di Windows, ma i test girano su tutte le piattaforme.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub fn game_dir_from_install_path(value: &str) -> Option<PathBuf> {
    let trimmed = value.trim().trim_end_matches(['\\', '/']);
    if trimmed.is_empty() {
        return None;
    }
    let (parent, last) = match trimmed.rfind(['\\', '/']) {
        Some(index) => (&trimmed[..index], &trimmed[index + 1..]),
        None => ("", trimmed),
    };
    if installations::flavor_from_dir_name(last).is_none() {
        return Some(PathBuf::from(trimmed));
    }
    if parent.is_empty() {
        return None;
    }
    // `C:` da solo indica la cartella corrente del disco, non la sua radice.
    if parent.ends_with(':') {
        return Some(PathBuf::from(format!("{parent}\\")));
    }
    Some(PathBuf::from(parent))
}

// Su Linux nessuna candidata ancora (issue #88).
#[cfg_attr(not(any(target_os = "windows", target_os = "macos")), allow(dead_code))]
const GAME_DIR: &str = "World of Warcraft";

/// Le cartelle `World of Warcraft` da esaminare su Windows: prima quelle indicate dal registro,
/// poi i percorsi standard su ogni disco presente da C a Z. `home` non serve qui.
#[cfg(target_os = "windows")]
pub fn candidate_roots(_home: Option<&Path>) -> Vec<Candidate> {
    let mut candidates: Vec<Candidate> = registry_install_paths()
        .into_iter()
        .filter_map(|value| game_dir_from_install_path(&value))
        .map(|path| Candidate::new(path, "Windows registry"))
        .collect();
    for letter in 'C'..='Z' {
        let drive = PathBuf::from(format!("{letter}:\\"));
        if !drive.is_dir() {
            continue;
        }
        candidates.push(Candidate::new(drive.join(GAME_DIR), "Drive root"));
        for (folder, origin) in [
            ("Program Files (x86)", "Program Files (x86)"),
            ("Program Files", "Program Files"),
            ("Games", "Games folder"),
        ] {
            candidates.push(Candidate::new(drive.join(folder).join(GAME_DIR), origin));
        }
    }
    candidates
}

/// I valori `InstallPath` della chiave di Blizzard, nella vista a 32 bit (`WOW6432Node`) e in
/// quella nativa. Una chiave o un valore mancante non è un errore: semplicemente non c'è.
#[cfg(target_os = "windows")]
fn registry_install_paths() -> Vec<String> {
    use winreg::RegKey;
    use winreg::enums::HKEY_LOCAL_MACHINE;

    let machine = RegKey::predef(HKEY_LOCAL_MACHINE);
    [
        r"SOFTWARE\WOW6432Node\Blizzard Entertainment\World of Warcraft",
        r"SOFTWARE\Blizzard Entertainment\World of Warcraft",
    ]
    .iter()
    .filter_map(|key| {
        machine
            .open_subkey(key)
            .ok()?
            .get_value::<String, _>("InstallPath")
            .ok()
    })
    .collect()
}

/// Le cartelle `World of Warcraft` da esaminare su macOS: quella di sistema e quella
/// dell'utente, con la home chiesta a Tauri dal chiamante.
#[cfg(target_os = "macos")]
pub fn candidate_roots(home: Option<&Path>) -> Vec<Candidate> {
    let mut candidates = vec![Candidate::new(
        Path::new("/Applications").join(GAME_DIR),
        "Applications",
    )];
    if let Some(home) = home {
        candidates.push(Candidate::new(
            home.join("Applications").join(GAME_DIR),
            "User Applications",
        ));
    }
    candidates
}

/// Su Linux il gioco gira dentro un prefisso Wine: le candidate arriveranno con la issue #88.
/// Per ora nessuna candidata, quindi la ricerca restituisce una lista vuota.
#[cfg(target_os = "linux")]
pub fn candidate_roots(_home: Option<&Path>) -> Vec<Candidate> {
    Vec::new()
}

/// Le altre piattaforme non sono supportate: nessuna candidata.
#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
pub fn candidate_roots(_home: Option<&Path>) -> Vec<Candidate> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;

    fn make_dirs(root: &Path, dirs: &[&str]) {
        for dir in dirs {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
    }

    fn flavors(found: &[DiscoveredInstallation]) -> Vec<(&str, &str)> {
        found
            .iter()
            .map(|f| (f.flavor.as_str(), f.origin.as_str()))
            .collect()
    }

    #[test]
    fn flavor_folders_are_found_in_name_order() {
        let tmp = TempDir::new();
        let game = tmp.path().join(GAME_DIR);
        make_dirs(&game, &["_retail_", "_classic_era_"]);

        let found = examine(&[Candidate::new(game.clone(), "Program Files")], &[]);
        assert_eq!(
            flavors(&found),
            [
                ("classic_era", "Program Files"),
                ("retail", "Program Files")
            ]
        );
        assert_eq!(found[1].name, "Retail");
        assert_eq!(
            found[1].path,
            installations::validate(&game.join("_retail_"))
                .unwrap()
                .path
        );
    }

    #[test]
    fn a_missing_candidate_is_skipped_without_error() {
        let tmp = TempDir::new();
        let game = tmp.path().join("present");
        make_dirs(&game, &["_retail_"]);

        let found = examine(
            &[
                Candidate::new(tmp.path().join("missing"), "Drive root"),
                Candidate::new(game, "Games folder"),
            ],
            &[],
        );
        assert_eq!(flavors(&found), [("retail", "Games folder")]);
    }

    #[test]
    fn other_children_and_deeper_folders_are_ignored() {
        let tmp = TempDir::new();
        let game = tmp.path().join(GAME_DIR);
        make_dirs(
            &game,
            &["Interface", "_Retail_", "retail", "sub/_classic_", "_ptr_"],
        );
        std::fs::write(game.join("_beta_"), b"").unwrap();

        let found = examine(&[Candidate::new(game, "Drive root")], &[]);
        assert_eq!(flavors(&found), [("ptr", "Drive root")]);
    }

    #[test]
    fn the_same_folder_reached_twice_is_listed_once_with_the_first_origin() {
        let tmp = TempDir::new();
        let game = tmp.path().join("wow");
        make_dirs(&game, &["_retail_"]);
        let roundabout = tmp.path().join("wow").join("..").join("wow");

        let found = examine(
            &[
                Candidate::new(game.clone(), "Windows registry"),
                Candidate::new(roundabout, "Program Files"),
                Candidate::new(game, "Program Files"),
            ],
            &[],
        );
        assert_eq!(flavors(&found), [("retail", "Windows registry")]);
    }

    #[test]
    fn installations_already_added_are_excluded() {
        let tmp = TempDir::new();
        let game = tmp.path().join(GAME_DIR);
        make_dirs(&game, &["_retail_", "_classic_era_"]);
        let known = [installations::validate(&game.join("_retail_")).unwrap()];

        let found = examine(&[Candidate::new(game, "Applications")], &known);
        assert_eq!(flavors(&found), [("classic_era", "Applications")]);
    }

    #[test]
    fn the_fields_are_serialized_in_camel_case() {
        let tmp = TempDir::new();
        let game = tmp.path().join(GAME_DIR);
        make_dirs(&game, &["_retail_"]);
        let found = examine(&[Candidate::new(game, "Applications")], &[]);

        let json = serde_json::to_value(&found[0]).unwrap();
        let mut keys: Vec<_> = json.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(keys, ["flavor", "name", "origin", "path"]);
    }

    #[test]
    fn the_registry_value_gives_the_game_folder() {
        assert_eq!(
            game_dir_from_install_path(r"C:\Program Files (x86)\World of Warcraft\_retail_\"),
            Some(PathBuf::from(r"C:\Program Files (x86)\World of Warcraft"))
        );
        assert_eq!(
            game_dir_from_install_path(r"D:\Games\World of Warcraft\_classic_era_"),
            Some(PathBuf::from(r"D:\Games\World of Warcraft"))
        );
        assert_eq!(
            game_dir_from_install_path(r" D:\Games\World of Warcraft\ "),
            Some(PathBuf::from(r"D:\Games\World of Warcraft"))
        );
        assert_eq!(
            game_dir_from_install_path(r"E:\_retail_\"),
            Some(PathBuf::from(r"E:\"))
        );
        assert_eq!(
            game_dir_from_install_path("games/World of Warcraft/_retail_/"),
            Some(PathBuf::from("games/World of Warcraft"))
        );
        // Un nome che non è `_<flavor>_` resta la cartella del gioco.
        assert_eq!(
            game_dir_from_install_path(r"C:\WoW\_Retail_"),
            Some(PathBuf::from(r"C:\WoW\_Retail_"))
        );
        assert_eq!(game_dir_from_install_path(""), None);
        assert_eq!(game_dir_from_install_path(r"  \ "), None);
        assert_eq!(game_dir_from_install_path("_retail_"), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_has_no_candidates_yet() {
        let tmp = TempDir::new();
        assert!(candidate_roots(Some(tmp.path())).is_empty());
        assert!(candidate_roots(None).is_empty());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_looks_in_both_applications_folders() {
        let tmp = TempDir::new();
        let candidates = candidate_roots(Some(tmp.path()));
        assert_eq!(
            candidates,
            [
                Candidate::new(Path::new("/Applications").join(GAME_DIR), "Applications"),
                Candidate::new(
                    tmp.path().join("Applications").join(GAME_DIR),
                    "User Applications"
                ),
            ]
        );
        assert_eq!(candidate_roots(None).len(), 1);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_candidates_cover_the_standard_folders() {
        let candidates = candidate_roots(None);
        let system_drive = PathBuf::from(r"C:\");
        if system_drive.is_dir() {
            for folder in ["Program Files (x86)", "Program Files", "Games"] {
                let expected = system_drive.join(folder).join(GAME_DIR);
                assert!(candidates.iter().any(|c| c.path == expected), "{folder}");
            }
            assert!(
                candidates
                    .iter()
                    .any(|c| c.path == system_drive.join(GAME_DIR))
            );
        }
    }
}
