//! Automatic discovery of WoW installations in the standard paths (issue #87).
//!
//! Two separate parts:
//!
//! - [`candidate_roots`] builds the list of `World of Warcraft` folders to look at and
//!   depends on the platform (Windows: drives and registry; macOS: the two `Applications`;
//!   Linux: the game folders inside the Wine prefixes in the home, issue #88);
//! - [`examine`] is common to all platforms: it reads only the candidate folders and their
//!   direct children, and keeps the `_<flavor>_` folders that are valid according to
//!   [`installations::validate`], the same rule as manual adding, and that still contain
//!   the game ([`has_game_files`]).
//!
//! The module only suggests: it never writes `installations.json`. It writes no logs, so
//! no machine path ends up in a message.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::installations::{self, Installation};

/// A `World of Warcraft` folder to examine, with the text that says where it comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub path: PathBuf,
    /// Short English text shown to the user, for example `"Program Files (x86)"`.
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

/// An installation found and not yet added: the same `path`, `flavor` and `name` as
/// [`Installation`], plus the origin.
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

/// Examines the candidates in the given order and returns the valid direct-child `_<flavor>_`
/// folders, without duplicates (same canonical path, i.e. same id: the first candidate that
/// found it wins) and without those already in `known`. Within a candidate the order is that of
/// the folder names. A candidate that does not exist or cannot be read is skipped without error,
/// and so is every child that cannot be read.
pub fn examine(candidates: &[Candidate], known: &[Installation]) -> Vec<DiscoveredInstallation> {
    let mut seen: HashSet<String> = known.iter().map(|k| k.id.clone()).collect();
    let mut found = Vec::new();
    for candidate in candidates {
        for child in flavor_children(&candidate.path) {
            if !has_game_files(&child) {
                continue;
            }
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

/// Whether the `_<flavor>_` folder still contains the game: the `.flavor.info` file the launcher
/// writes in every installed folder, a `Wow*.exe` executable (Windows and Wine: `Wow.exe`,
/// `WowClassic.exe`, `WowB.exe`, `WowT.exe`) or a `*.app` bundle (macOS: the bundle sits inside
/// the folder, for example `_retail_/World of Warcraft.app`, and is a folder, not a file
/// that a leftover could imitate). When uninstalling a
/// version, the launcher leaves the folder with a few leftover files (libraries, `D3D12`) but
/// with none of these: it is not an installation to suggest. Names are case-insensitive,
/// like the game.
fn has_game_files(dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries.filter_map(Result::ok).any(|entry| {
        let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
        name == ".flavor.info"
            || (name.starts_with("wow") && name.ends_with(".exe"))
            || (name.ends_with(".app") && entry.file_type().is_ok_and(|kind| kind.is_dir()))
    })
}

/// The direct children of `root` with a `_<flavor>_` name, sorted by name. A single read
/// of the folder, no recursive walk; `validate` then checks that they are folders.
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

/// Derives the game folder from the `InstallPath` value of the Windows registry. If the value
/// already points to a `_<flavor>_` folder (for example `...\World of Warcraft\_retail_\`) the
/// folder above it is returned. It is a text function, so it is tested on all
/// platforms: it accepts both `\` and `/` as separator. An empty value yields nothing.
// Only the Windows registry read uses it, but the tests run on all platforms.
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

/// `C:` alone means the drive's current folder, not its root: it becomes `C:\`.
fn absolute_drive(path: &str) -> PathBuf {
    if path.ends_with(':') {
        PathBuf::from(format!("{path}\\"))
    } else {
        PathBuf::from(path)
    }
}

const GAME_DIR: &str = "World of Warcraft";

/// The `World of Warcraft` folders to examine on Windows: first those named by the registry,
/// then the standard paths on every drive present from C to Z. `home` is not needed here.
///
/// Known limit: checking each letter also touches removable drives with no media and
/// mapped network drives; a disconnected network drive can make the system wait a few
/// seconds per letter. The command runs in `spawn_blocking`, so the window does not freeze,
/// but the answer may be late. Filtering by drive type (`GetDriveTypeW`) would require a
/// direct dependency on `windows-sys`, not approved for this issue.
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

/// The `InstallPath` values of the Blizzard key, in the 32-bit view (`WOW6432Node`) and in
/// the native one. A missing key or value is not an error: it simply is not there.
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

/// The `World of Warcraft` folders to examine on macOS: the system one and the user's
/// one, with the home asked of Tauri by the caller.
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

/// On Linux the game runs inside a Wine prefix: the candidates are the game folders
/// in the prefixes found under the home asked of Tauri (see [`wine_candidates`]). Without a home
/// there is nothing to search.
#[cfg(target_os = "linux")]
pub fn candidate_roots(home: Option<&Path>) -> Vec<Candidate> {
    home.map(wine_candidates).unwrap_or_default()
}

/// The `Program Files` folders where Battle.net installs the game inside `drive_c`, in the order
/// in which they are checked.
const PROGRAM_FILES: [&str; 2] = ["Program Files (x86)", "Program Files"];

/// The `World of Warcraft` folders inside the Wine prefixes under `home`, two per prefix
/// (`Program Files (x86)` and `Program Files`), in the order of [`wine_prefixes`]. It is common to
/// all platforms so the tests run everywhere; only the Linux part calls it.
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

/// The Wine prefixes under `home`, each as its `drive_c` folder with the origin to
/// show: `.wine`, the direct children of `Games` (Lutris), the bottles of native and Flatpak
/// Bottles and the Steam/Proton `pfx`. Only these fixed levels, no recursive walk; a folder
/// is a prefix only if it contains `drive_c`. The same prefix reached twice (for
/// example `.steam/steam`, which is usually a link to `.local/share/Steam`) appears only
/// once, with the first origin: the canonical path is compared, or the path as it is if
/// it cannot be canonicalized. The origin never contains the home.
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

/// `base` with the names appended one at a time, without hand-written separators.
fn join_all(base: &Path, names: &[&str]) -> PathBuf {
    names
        .iter()
        .fold(base.to_path_buf(), |path, name| path.join(name))
}

/// The direct child folders of `dir`, with their name, sorted by name. Links to folders
/// count as folders. A folder that cannot be read has no children.
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

/// The child folder of `dir` named `name`, case-insensitively,
/// as the game does under Wine. If there is more than one, the identical name wins, then the
/// first in name order.
fn child_ignoring_case(dir: &Path, name: &str) -> Option<PathBuf> {
    child_dirs(dir)
        .into_iter()
        .filter(|(child, _)| child.eq_ignore_ascii_case(name))
        .min_by_key(|(child, _)| child != name)
        .map(|(_, path)| path)
}

/// Like [`child_ignoring_case`], but if the folder is not there it returns `dir/name`: the
/// candidate stays in the list and [`examine`] skips it.
fn child_or_join(dir: &Path, name: &str) -> PathBuf {
    child_ignoring_case(dir, name).unwrap_or_else(|| dir.join(name))
}

/// Other platforms are not supported: no candidates.
#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
pub fn candidate_roots(_home: Option<&Path>) -> Vec<Candidate> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;

    /// Creates the folders; those with a `_<flavor>_` name also get the `.flavor.info` of
    /// a real installation.
    fn make_dirs(root: &Path, dirs: &[&str]) {
        for dir in dirs {
            let path = root.join(dir);
            std::fs::create_dir_all(&path).unwrap();
            let name = path.file_name().unwrap().to_str().unwrap();
            if installations::flavor_from_dir_name(name).is_some() {
                std::fs::write(path.join(".flavor.info"), b"").unwrap();
            }
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
    fn a_folder_left_by_an_uninstall_is_not_suggested() {
        let tmp = TempDir::new();
        let game = tmp.path().join(GAME_DIR);
        // The real leftovers of an uninstalled `_retail_`.
        let leftover = game.join("_retail_");
        std::fs::create_dir_all(leftover.join("D3D12")).unwrap();
        for file in ["icudt78l.dat", "libxell.dll", "wow_loader.dll"] {
            std::fs::write(leftover.join(file), b"").unwrap();
        }
        // Each sign of the game is enough on its own.
        for (dir, marker) in [
            ("_classic_era_", "WowClassic.exe"),
            ("_forever_", "Wow.exe"),
            ("_classic_beta_", "WOWB.EXE"),
        ] {
            std::fs::create_dir_all(game.join(dir)).unwrap();
            std::fs::write(game.join(dir).join(marker), b"").unwrap();
        }
        std::fs::create_dir_all(game.join("_classic_").join("World of Warcraft Classic.app"))
            .unwrap();
        // A `.app` file is not a bundle.
        std::fs::create_dir_all(game.join("_ptr_")).unwrap();
        std::fs::write(game.join("_ptr_").join("World of Warcraft.app"), b"").unwrap();

        let found = examine(&[Candidate::new(game, "Program Files")], &[]);
        assert_eq!(
            flavors(&found),
            [
                ("classic", "Program Files"),
                ("classic_beta", "Program Files"),
                ("classic_era", "Program Files"),
                ("forever", "Program Files"),
            ]
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
        // A name that is not `_<flavor>_` stays the game folder.
        assert_eq!(
            game_dir_from_install_path(r"C:\WoW\_Retail_"),
            Some(PathBuf::from(r"C:\WoW\_Retail_"))
        );
        // A drive without a folder becomes its root, not the drive's current folder.
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

    /// A synthetic home with one prefix for each searched form, plus folders that are not
    /// prefixes. Returns the `World of Warcraft` folder of the Lutris prefix.
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
        // Different cases are followed as they are on disk.
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
            // On a case-insensitive file system the three folders are a single one.
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
