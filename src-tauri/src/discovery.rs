//! Ricerca automatica delle installazioni di WoW nei percorsi standard (issue #87).
//!
//! Due parti separate:
//!
//! - [`candidate_roots`] costruisce l'elenco delle cartelle `World of Warcraft` da guardare e
//!   dipende dalla piattaforma (Windows: dischi e registro; macOS: le due `Applications`;
//!   Linux: le cartelle del gioco dentro i prefissi Wine della home, issue #88);
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
        return Some(absolute_drive(trimmed));
    }
    if parent.is_empty() {
        return None;
    }
    Some(absolute_drive(parent))
}

/// `C:` da solo indica la cartella corrente del disco, non la sua radice: diventa `C:\`.
fn absolute_drive(path: &str) -> PathBuf {
    if path.ends_with(':') {
        PathBuf::from(format!("{path}\\"))
    } else {
        PathBuf::from(path)
    }
}

const GAME_DIR: &str = "World of Warcraft";

/// Le cartelle `World of Warcraft` da esaminare su Windows: prima quelle indicate dal registro,
/// poi i percorsi standard su ogni disco presente da C a Z. `home` non serve qui.
///
/// Limite noto: il controllo di ogni lettera tocca anche le unità rimovibili senza supporto e
/// le unità di rete mappate; una di rete disconnessa può far attendere il sistema qualche
/// secondo per lettera. Il comando gira in `spawn_blocking`, quindi la finestra non si blocca,
/// ma la risposta può tardare. Filtrare per tipo di unità (`GetDriveTypeW`) richiederebbe una
/// dipendenza diretta da `windows-sys`, non approvata per questa issue.
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

/// Su Linux il gioco gira dentro un prefisso Wine: le candidate sono le cartelle del gioco
/// nei prefissi trovati sotto la home chiesta a Tauri (vedi [`wine_candidates`]). Senza home
/// non c'è niente da cercare.
#[cfg(target_os = "linux")]
pub fn candidate_roots(home: Option<&Path>) -> Vec<Candidate> {
    home.map(wine_candidates).unwrap_or_default()
}

/// Le cartelle `Program Files` dove Battle.net installa il gioco dentro `drive_c`, nell'ordine
/// in cui si guardano.
const PROGRAM_FILES: [&str; 2] = ["Program Files (x86)", "Program Files"];

/// Le cartelle `World of Warcraft` dentro i prefissi Wine sotto `home`, due per prefisso
/// (`Program Files (x86)` e `Program Files`), nell'ordine di [`wine_prefixes`]. È comune a
/// tutte le piattaforme così i test girano ovunque; la chiama solo la parte Linux.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn wine_candidates(home: &Path) -> Vec<Candidate> {
    let mut candidates = Vec::new();
    for (drive_c, origin) in wine_prefixes(home) {
        for folder in PROGRAM_FILES {
            let program_files = child_or_join(&drive_c, folder);
            let game = child_or_join(&program_files, GAME_DIR);
            candidates.push(Candidate::new(game, &origin));
        }
    }
    candidates
}

/// I prefissi Wine sotto `home`, ciascuno come la sua cartella `drive_c` con la provenienza da
/// mostrare: `.wine`, i figli diretti di `Games` (Lutris), i bottle di Bottles nativo e Flatpak
/// e i `pfx` di Steam/Proton. Solo questi livelli fissi, nessuna visita ricorsiva; una cartella
/// è un prefisso solo se contiene `drive_c`. Lo stesso prefisso raggiunto due volte (per
/// esempio `.steam/steam`, che di norma è un link a `.local/share/Steam`) compare una volta
/// sola, con la prima provenienza: si confronta il percorso canonico, o quello così com'è se
/// non si riesce a canonicalizzarlo. La provenienza non contiene mai la home.
fn wine_prefixes(home: &Path) -> Vec<(PathBuf, String)> {
    let mut prefixes = vec![(home.join(".wine"), "Wine prefix (~/.wine)".to_owned())];
    for (name, path) in child_dirs(&home.join("Games")) {
        prefixes.push((path, format!("Wine prefix (Lutris: {name})")));
    }
    for (bottles, label) in [
        (
            join_all(home, &[".local", "share", "bottles", "bottles"]),
            "Bottles",
        ),
        (
            join_all(
                home,
                &[
                    ".var",
                    "app",
                    "com.usebottles.bottles",
                    "data",
                    "bottles",
                    "bottles",
                ],
            ),
            "Bottles (Flatpak)",
        ),
    ] {
        for (name, path) in child_dirs(&bottles) {
            prefixes.push((path, format!("{label}: {name}")));
        }
    }
    for steam in [&[".steam", "steam"][..], &[".local", "share", "Steam"][..]] {
        let compatdata = join_all(&join_all(home, steam), &["steamapps", "compatdata"]);
        for (appid, path) in child_dirs(&compatdata) {
            prefixes.push((path.join("pfx"), format!("Steam/Proton ({appid})")));
        }
    }

    let mut seen = HashSet::new();
    prefixes
        .into_iter()
        .filter_map(|(prefix, origin)| {
            let drive_c = child_ignoring_case(&prefix, "drive_c")?;
            let key = std::fs::canonicalize(&prefix).unwrap_or(prefix);
            seen.insert(key).then_some((drive_c, origin))
        })
        .collect()
}

/// `base` con i nomi aggiunti uno alla volta, senza separatori scritti a mano.
fn join_all(base: &Path, names: &[&str]) -> PathBuf {
    names
        .iter()
        .fold(base.to_path_buf(), |path, name| path.join(name))
}

/// Le cartelle figlie dirette di `dir`, con il nome, ordinate per nome. I link a cartelle
/// contano come cartelle. Una cartella che non si legge non ha figli.
fn child_dirs(dir: &Path) -> Vec<(String, PathBuf)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut children: Vec<(String, PathBuf)> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .filter_map(|path| {
            let name = path.file_name()?.to_string_lossy().into_owned();
            Some((name, path))
        })
        .collect();
    children.sort();
    children
}

/// La cartella figlia di `dir` che si chiama `name` senza distinguere maiuscole e minuscole,
/// come fa il gioco sotto Wine. Se ce ne sono più di una vince il nome identico, poi il primo in
/// ordine di nome.
fn child_ignoring_case(dir: &Path, name: &str) -> Option<PathBuf> {
    child_dirs(dir)
        .into_iter()
        .filter(|(child, _)| child.eq_ignore_ascii_case(name))
        .min_by_key(|(child, _)| child != name)
        .map(|(_, path)| path)
}

/// Come [`child_ignoring_case`], ma se la cartella non c'è restituisce `dir/name`: la candidata
/// resta nell'elenco e [`examine`] la salta.
fn child_or_join(dir: &Path, name: &str) -> PathBuf {
    child_ignoring_case(dir, name).unwrap_or_else(|| dir.join(name))
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
        // Un disco senza cartella diventa la sua radice, non la cartella corrente del disco.
        assert_eq!(
            game_dir_from_install_path("C:"),
            Some(PathBuf::from(r"C:\"))
        );
        assert_eq!(
            game_dir_from_install_path(r"C:\"),
            Some(PathBuf::from(r"C:\"))
        );
        assert_eq!(game_dir_from_install_path(""), None);
        assert_eq!(game_dir_from_install_path(r"  \ "), None);
        assert_eq!(game_dir_from_install_path("_retail_"), None);
    }

    /// Una home sintetica con un prefisso per ogni forma cercata, più cartelle che non sono
    /// prefissi. Restituisce la cartella `World of Warcraft` del prefisso Lutris.
    fn synthetic_home(home: &Path) -> PathBuf {
        let lutris = "Games/battlenet/drive_c/Program Files (x86)/World of Warcraft";
        make_dirs(
            home,
            &[
                &format!("{lutris}/_retail_"),
                &format!("{lutris}/_classic_beta_"),
                ".wine/drive_c/Program Files/World of Warcraft/_classic_era_",
                "Games/not-a-prefix/Program Files (x86)/World of Warcraft/_retail_",
                ".local/share/bottles/bottles/gaming/drive_c/program files (x86)/world of warcraft/_ptr_",
                ".var/app/com.usebottles.bottles/data/bottles/bottles/flat/drive_c/users",
                ".local/share/Steam/steamapps/compatdata/12345/pfx/drive_c/Program Files (x86)/World of Warcraft/_classic_",
                ".local/share/Steam/steamapps/compatdata/67890/no-pfx-here",
            ],
        );
        std::fs::write(home.join("Games/notes.txt"), b"").unwrap();
        home.join(lutris)
    }

    fn origins(candidates: &[Candidate]) -> Vec<&str> {
        candidates.iter().map(|c| c.origin.as_str()).collect()
    }

    #[test]
    fn wine_prefixes_give_two_candidates_each() {
        let tmp = TempDir::new();
        let home = tmp.path();
        let lutris_game = synthetic_home(home);

        let candidates = wine_candidates(home);
        assert_eq!(
            origins(&candidates),
            [
                "Wine prefix (~/.wine)",
                "Wine prefix (~/.wine)",
                "Wine prefix (Lutris: battlenet)",
                "Wine prefix (Lutris: battlenet)",
                "Bottles: gaming",
                "Bottles: gaming",
                "Bottles (Flatpak): flat",
                "Bottles (Flatpak): flat",
                "Steam/Proton (12345)",
                "Steam/Proton (12345)",
            ]
        );
        assert_eq!(candidates[2].path, lutris_game);
        assert_eq!(
            candidates[3].path,
            home.join("Games/battlenet/drive_c/Program Files/World of Warcraft")
        );
        // Le maiuscole diverse si seguono come sono sul disco.
        assert_eq!(
            candidates[4].path,
            home.join(
                ".local/share/bottles/bottles/gaming/drive_c/program files (x86)/world of warcraft"
            )
        );
        for candidate in &candidates {
            assert!(candidate.path.starts_with(home));
            assert!(!candidate.origin.contains(&*home.to_string_lossy()));
        }
    }

    #[test]
    fn the_games_in_wine_prefixes_are_found() {
        let tmp = TempDir::new();
        synthetic_home(tmp.path());

        let found = examine(&wine_candidates(tmp.path()), &[]);
        assert_eq!(
            flavors(&found),
            [
                ("classic_era", "Wine prefix (~/.wine)"),
                ("classic_beta", "Wine prefix (Lutris: battlenet)"),
                ("retail", "Wine prefix (Lutris: battlenet)"),
                ("ptr", "Bottles: gaming"),
                ("classic", "Steam/Proton (12345)"),
            ]
        );
    }

    #[test]
    fn an_empty_home_has_no_wine_candidates() {
        let tmp = TempDir::new();
        assert!(wine_candidates(tmp.path()).is_empty());
        assert!(wine_candidates(&tmp.path().join("missing")).is_empty());
    }

    #[test]
    fn the_exact_name_wins_over_other_cases() {
        let tmp = TempDir::new();
        make_dirs(
            tmp.path(),
            &["PROGRAM FILES", "Program Files", "program files"],
        );
        let found = child_ignoring_case(tmp.path(), "Program Files").unwrap();
        if child_dirs(tmp.path()).len() == 3 {
            assert_eq!(found, tmp.path().join("Program Files"));
        } else {
            // Su un file system che non distingue le maiuscole le tre cartelle sono una sola.
            assert!(
                found
                    .file_name()
                    .unwrap()
                    .eq_ignore_ascii_case("program files")
            );
        }
        assert!(child_ignoring_case(tmp.path(), "Program Files (x86)").is_none());
    }

    #[cfg(unix)]
    #[test]
    fn a_prefix_reached_through_a_link_is_listed_once() {
        let tmp = TempDir::new();
        let home = tmp.path();
        let steam = home.join(".local/share/Steam");
        make_dirs(
            &steam,
            &["steamapps/compatdata/42/pfx/drive_c/Program Files (x86)/World of Warcraft/_retail_"],
        );
        make_dirs(home, &[".steam"]);
        std::os::unix::fs::symlink(&steam, home.join(".steam/steam")).unwrap();

        let candidates = wine_candidates(home);
        assert_eq!(
            origins(&candidates),
            ["Steam/Proton (42)", "Steam/Proton (42)"]
        );
        let found = examine(&candidates, &[]);
        assert_eq!(flavors(&found), [("retail", "Steam/Proton (42)")]);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_candidates_are_the_wine_prefixes() {
        let tmp = TempDir::new();
        synthetic_home(tmp.path());
        assert_eq!(
            candidate_roots(Some(tmp.path())),
            wine_candidates(tmp.path())
        );
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
