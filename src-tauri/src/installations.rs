//! Installazioni di WoW registrate dall'utente.
//!
//! Un'installazione è una cartella `_<flavor>_` dentro la cartella del gioco, per esempio
//! `World of Warcraft/_retail_` o `World of Warcraft/_classic_beta_`. Ogni flavor ha i suoi
//! `Interface/AddOns` e `WTF`, quindi è un'installazione separata.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Installation {
    /// Stabile per lo stesso percorso canonico.
    pub id: String,
    /// Percorso canonico della cartella `_<flavor>_`.
    pub path: PathBuf,
    /// Il nome della cartella senza i trattini bassi esterni: `_classic_beta_` → `classic_beta`.
    pub flavor: String,
    /// Nome leggibile ricavato dal flavor: `classic_beta` → `Classic Beta`.
    pub name: String,
}

/// Ricava il flavor dal nome della cartella: `_<flavor>_`, con il flavor fatto di lettere
/// minuscole, cifre e `_`, che non inizia e non finisce con `_`.
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

/// Id stabile ricavato dal percorso canonico (FNV-1a a 64 bit). Non usa `DefaultHasher`, il cui
/// algoritmo non è garantito stabile tra versioni di Rust: l'id finisce nel file salvato.
pub fn installation_id(canonical: &Path) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in canonical.as_os_str().as_encoded_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// Controlla che `path` sia una cartella `_<flavor>_` esistente e costruisce l'installazione.
/// L'assenza di `Interface/AddOns` non è un errore: vuol dire zero addon.
pub fn validate(path: &Path) -> Result<Installation, String> {
    if path.as_os_str().is_empty() {
        return Err("Nessuna cartella indicata.".to_owned());
    }
    let canonical = path
        .canonicalize()
        .map_err(|_| "La cartella non esiste o non è leggibile.".to_owned())?;
    if !canonical.is_dir() {
        return Err("Il percorso indicato non è una cartella.".to_owned());
    }
    let dir_name = canonical
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let flavor = flavor_from_dir_name(dir_name).ok_or_else(|| {
        "La cartella deve essere una cartella `_<flavor>_` di World of Warcraft, \
         per esempio `_retail_` o `_classic_era_`."
            .to_owned()
    })?;
    Ok(Installation {
        id: installation_id(&canonical),
        name: display_name(&flavor),
        flavor,
        path: canonical,
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
        let a = installation_id(Path::new("/games/wow/_retail_"));
        assert_eq!(a, installation_id(Path::new("/games/wow/_retail_")));
        assert_ne!(a, installation_id(Path::new("/games/wow/_classic_era_")));
        assert_eq!(a.len(), 16);
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
        assert_eq!(installation.path, dir.canonicalize().unwrap());
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
                .contains("non esiste")
        );
        assert!(validate(&file).unwrap_err().contains("non è una cartella"));
        assert!(validate(&wrong_name).unwrap_err().contains("_<flavor>_"));
        assert!(validate(Path::new("")).is_err());
    }
}
