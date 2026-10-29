//! WoW installations registered by the user.
//!
//! An installation is a `_<flavor>_` folder inside the game folder, for example
//! `World of Warcraft/_retail_` or `World of Warcraft/_classic_beta_`. Each flavor has its own
//! `Interface/AddOns` and `WTF`, so it is a separate installation.

use std::path::{Path, PathBuf};

use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Installation {
    /// Stable for the same canonical path.
    pub id: String,
    /// Canonical path of the `_<flavor>_` folder.
    pub path: PathBuf,
    /// The folder name without the outer underscores: `_classic_beta_` → `classic_beta`.
    pub flavor: String,
    /// Readable name derived from the flavor: `classic_beta` → `Classic Beta`.
    pub name: String,
}

/// Derives the flavor from the folder name: `_<flavor>_`, with the flavor made of lowercase
/// letters, digits and `_`, neither starting nor ending with `_`.
pub fn flavor_from_dir_name(name: &str) -> Option<String> {
    let inner = name.strip_prefix('_')?.strip_suffix('_')?;
    let valid = !inner.is_empty()
        && !inner.starts_with('_')
        && !inner.ends_with('_')
        && inner
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    valid.then(|| inner.to_owned())
}

/// `classic_beta` → `Classic Beta`.
pub fn display_name(flavor: &str) -> String {
    flavor
        .split('_')
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Stable id derived from the canonical path (64-bit FNV-1a). It does not use `DefaultHasher`,
/// whose algorithm is not guaranteed stable across Rust versions: the id ends up in the saved file.
pub fn installation_id(canonical: &Path) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in canonical.as_os_str().as_encoded_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

impl Installation {
    /// Derives id, flavor and name from an already canonical path, without touching the disk.
    /// It is also what reading the store does: only the path is saved on disk, so the derived
    /// fields always follow the current code.
    pub fn from_canonical(path: PathBuf) -> Option<Self> {
        let flavor = path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(flavor_from_dir_name)?;
        Some(Self {
            id: installation_id(&path),
            name: display_name(&flavor),
            flavor,
            path,
        })
    }
}

/// Removes the "verbatim" prefix that `canonicalize` adds on Windows: `\\?\C:\x` becomes
/// `C:\x` and `\\?\UNC\server\share` becomes `\\server\share`. Returns `None` if the
/// prefix is absent. It is a text function, so it can be tested on every platform.
pub fn strip_verbatim_prefix(path: &str) -> Option<String> {
    let rest = path.strip_prefix(r"\\?\")?;
    if let Some(unc) = rest.strip_prefix(r"UNC\") {
        return Some(format!(r"\\{unc}"));
    }
    let bytes = rest.as_bytes();
    let has_drive = bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
    has_drive.then(|| rest.to_owned())
}

fn canonicalize(path: &Path) -> std::io::Result<PathBuf> {
    let canonical = path.canonicalize()?;
    let simple = cfg!(windows)
        .then(|| canonical.to_str().and_then(strip_verbatim_prefix))
        .flatten();
    Ok(simple.map(PathBuf::from).unwrap_or(canonical))
}

/// Checks that `path` is an existing `_<flavor>_` folder and builds the installation.
/// A missing `Interface/AddOns` is not an error: it means zero addons.
pub fn validate(path: &Path) -> Result<Installation, String> {
    if path.as_os_str().is_empty() {
        return Err("No folder given.".to_owned());
    }
    let canonical = canonicalize(path)
        .map_err(|_| "The folder does not exist or cannot be read.".to_owned())?;
    if !canonical.is_dir() {
        return Err("The given path is not a folder.".to_owned());
    }
    // The path is saved as JSON text: a non-UTF-8 one could not be saved.
    if canonical.to_str().is_none() {
        return Err("The path contains invalid characters: rename or move the folder.".to_owned());
    }
    Installation::from_canonical(canonical).ok_or_else(|| {
        "The folder must be a World of Warcraft _<flavor>_ folder, \
         for example _retail_ or _classic_era_."
            .to_owned()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;

    #[test]
    fn flavor_is_read_from_valid_names() {
        assert_eq!(flavor_from_dir_name("_retail_").as_deref(), Some("retail"));
        assert_eq!(
            flavor_from_dir_name("_classic_era_").as_deref(),
            Some("classic_era")
        );
        assert_eq!(
            flavor_from_dir_name("_classic_beta_").as_deref(),
            Some("classic_beta")
        );
        assert_eq!(flavor_from_dir_name("_ptr2_").as_deref(), Some("ptr2"));
    }

    #[test]
    fn invalid_names_have_no_flavor() {
        for name in [
            "retail",
            "_retail",
            "retail_",
            "_Retail_",
            "__",
            "_",
            "",
            "___",
            "__retail_",
            "_retail__",
            "_re tail_",
            "_retail-x_",
        ] {
            assert_eq!(flavor_from_dir_name(name), None, "{name:?}");
        }
    }

    #[test]
    fn display_name_capitalizes_each_word() {
        assert_eq!(display_name("retail"), "Retail");
        assert_eq!(display_name("classic_beta"), "Classic Beta");
        assert_eq!(display_name("classic_era"), "Classic Era");
    }

    #[test]
    fn id_is_stable_and_depends_on_the_path() {
        let retail = PathBuf::from("games").join("wow").join("_retail_");
        let a = installation_id(&retail);
        assert_eq!(a, installation_id(&retail));
        assert_ne!(
            a,
            installation_id(&PathBuf::from("games").join("wow").join("_classic_era_"))
        );
        assert_eq!(a.len(), 16);
    }

    #[test]
    fn the_windows_verbatim_prefix_is_removed() {
        assert_eq!(
            strip_verbatim_prefix(r"\\?\C:\Games\World of Warcraft\_retail_").as_deref(),
            Some(r"C:\Games\World of Warcraft\_retail_")
        );
        assert_eq!(
            strip_verbatim_prefix(r"\\?\UNC\nas\games\_retail_").as_deref(),
            Some(r"\\nas\games\_retail_")
        );
        assert_eq!(strip_verbatim_prefix(r"C:\Games\_retail_"), None);
        assert_eq!(strip_verbatim_prefix(r"\\?\Volume{abc}\_retail_"), None);
        assert_eq!(strip_verbatim_prefix("/games/_retail_"), None);
    }

    #[test]
    fn derived_fields_come_from_the_path_alone() {
        let installation =
            Installation::from_canonical(PathBuf::from("games").join("_classic_era_")).unwrap();
        assert_eq!(installation.flavor, "classic_era");
        assert_eq!(installation.name, "Classic Era");
        assert!(Installation::from_canonical(PathBuf::from("games").join("Interface")).is_none());
    }

    /// Linux only: Windows has no names in arbitrary bytes, and APFS on macOS refuses to create
    /// a name that is not valid UTF-8, so such a path cannot exist there.
    #[cfg(target_os = "linux")]
    #[test]
    fn non_utf8_paths_are_rejected_with_a_reason() {
        use std::os::unix::ffi::OsStrExt;
        let tmp = TempDir::new();
        let odd = tmp
            .path()
            .join(std::ffi::OsStr::from_bytes(b"caf\xe9"))
            .join("_retail_");
        std::fs::create_dir_all(&odd).unwrap();
        assert!(validate(&odd).unwrap_err().contains("invalid characters"));
    }

    #[test]
    fn a_valid_folder_with_spaces_and_parentheses_is_accepted() {
        let tmp = TempDir::new();
        let dir = tmp
            .path()
            .join("drive_c/Program Files (x86)/World of Warcraft/_classic_beta_");
        std::fs::create_dir_all(&dir).unwrap();

        let installation = validate(&dir).unwrap();
        assert_eq!(installation.flavor, "classic_beta");
        assert_eq!(installation.name, "Classic Beta");
        assert_eq!(installation.path, canonicalize(&dir).unwrap());
    }

    #[test]
    fn missing_addons_folder_is_not_an_error() {
        let tmp = TempDir::new();
        let dir = tmp.path().join("_retail_");
        std::fs::create_dir(&dir).unwrap();
        assert!(validate(&dir).is_ok());
    }

    #[test]
    fn the_same_folder_written_two_ways_has_the_same_id() {
        let tmp = TempDir::new();
        let dir = tmp.path().join("wow/_retail_");
        std::fs::create_dir_all(&dir).unwrap();
        let roundabout = tmp.path().join("wow/../wow/_retail_");
        assert_eq!(
            validate(&dir).unwrap().id,
            validate(&roundabout).unwrap().id
        );
    }

    #[test]
    fn invalid_paths_are_rejected_with_a_reason() {
        let tmp = TempDir::new();
        let file = tmp.path().join("_retail_");
        std::fs::write(&file, b"").unwrap();
        let wrong_name = tmp.path().join("World of Warcraft");
        std::fs::create_dir(&wrong_name).unwrap();

        assert!(
            validate(&tmp.path().join("missing/_retail_"))
                .unwrap_err()
                .contains("does not exist")
        );
        assert!(validate(&file).unwrap_err().contains("is not a folder"));
        assert!(
            validate(&wrong_name)
                .unwrap_err()
                .contains("must be a World of Warcraft _<flavor>_ folder")
        );
        assert!(validate(Path::new("")).is_err());
    }
}
