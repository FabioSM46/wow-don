//! Installazione di uno zip che l'utente ha scaricato a mano dal sito (D32), per gli addon che
//! l'app non può scaricare da sola (per esempio `allowModDistribution = false`, D13).
//!
//! Solo funzioni su percorsi: niente rete, niente Tauri. Il flusso è quello degli altri
//! chiamanti di [`install`]: `WorkDir::staging` → `extract` → controllo dei `.toc` → `replace`.
//! Non scrive record in `installed-addons.json`: l'addon si riconosce poi dal fingerprint, come
//! uno installato a mano.

use std::collections::HashSet;
use std::fs;
use std::io::{self, Read};
use std::path::Path;

use serde::Serialize;

use crate::install;
use crate::sources::curseforge::MAX_DOWNLOAD_BYTES;

/// Dimensione massima dello zip scelto dall'utente: la stessa di uno zip scaricato.
pub const MAX_ZIP_BYTES: u64 = MAX_DOWNLOAD_BYTES;

/// Esito di un'installazione da zip locale.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalInstall {
    /// Le cartelle dello zip, ora in `AddOns`, nell'ordine dello zip.
    pub installed: Vec<String>,
    /// Le cartelle che esistevano già con lo stesso nome (senza distinguere maiuscole e
    /// minuscole) e sono state sostituite, con il nome che avevano sul disco.
    pub replaced: Vec<String>,
}

/// Installa in `addons_dir` lo zip in `zip_path`. Lo zip intero si rifiuta, senza toccare
/// `AddOns`, se non è un file `.zip`, supera [`MAX_ZIP_BYTES`], non supera i controlli di
/// [`install::extract`] o ha una cartella di primo livello senza un `.toc` con il suo nome.
/// Le cartelle con lo stesso nome già presenti si sostituiscono senza conferma (D32).
pub fn install_zip(addons_dir: &Path, zip_path: &Path) -> Result<LocalInstall, String> {
    let bytes = read_zip(zip_path)?;
    let staging = install::WorkDir::staging(addons_dir)?;
    let folders = install::extract(&bytes, staging.path())?;
    for folder in &folders {
        if !has_own_toc(&staging.path().join(folder), folder)? {
            return Err(format!(
                "The zip is not a valid addon: the folder {folder} has no {folder}.toc file."
            ));
        }
    }
    let replaced = existing_folders(addons_dir, &folders)?;
    install::replace(addons_dir, staging.path(), &[])?;
    Ok(LocalInstall {
        installed: folders,
        replaced,
    })
}

/// Legge lo zip, con i controlli su tipo, estensione e dimensione. I messaggi non contengono
/// il percorso: l'utente sa quale file ha scelto.
fn read_zip(zip_path: &Path) -> Result<Vec<u8>, String> {
    let is_zip = zip_path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("zip"));
    if !is_zip {
        return Err("Only .zip files can be installed.".to_string());
    }
    let metadata = fs::metadata(zip_path).map_err(open_error)?;
    if !metadata.is_file() {
        return Err("The chosen item is not a file.".to_string());
    }
    if metadata.len() > MAX_ZIP_BYTES {
        return Err(too_big());
    }
    let file = fs::File::open(zip_path).map_err(open_error)?;
    let mut bytes = Vec::new();
    // Il file può crescere tra il controllo e la lettura: non si legge mai oltre il limite.
    file.take(MAX_ZIP_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(open_error)?;
    if bytes.len() as u64 > MAX_ZIP_BYTES {
        return Err(too_big());
    }
    Ok(bytes)
}

fn too_big() -> String {
    format!(
        "The zip is larger than the {} MiB limit.",
        MAX_ZIP_BYTES / (1024 * 1024)
    )
}

fn open_error(error: io::Error) -> String {
    match error.kind() {
        io::ErrorKind::NotFound => "The zip file was not found.".to_string(),
        kind => format!("Could not read the zip file ({kind})."),
    }
}

/// Se `dir` contiene un file `.toc` che il gioco caricherebbe per la cartella `folder`:
/// `<folder>.toc`, oppure `<folder>_<suffisso>.toc` o `<folder>-<suffisso>.toc` (vedi
/// `addons::toc::select_toc`), senza distinguere maiuscole e minuscole. Non serve che sia il
/// `.toc` del flavor dell'installazione.
fn has_own_toc(dir: &Path, folder: &str) -> Result<bool, String> {
    let read_error = |error: io::Error| {
        format!(
            "Could not read the extracted folder {folder} ({}).",
            error.kind()
        )
    };
    let folder = folder.to_lowercase();
    for entry in fs::read_dir(dir).map_err(read_error)? {
        let entry = entry.map_err(read_error)?;
        if !entry.file_type().map_err(read_error)?.is_file() {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_lowercase) else {
            continue;
        };
        let Some(stem) = name.strip_suffix(".toc") else {
            continue;
        };
        let own = stem == folder
            || stem
                .strip_prefix(folder.as_str())
                .is_some_and(|rest| rest.len() > 1 && rest.starts_with(['_', '-']));
        if own {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Le voci di `addons_dir` che `replace` sostituirà, con il nome che hanno sul disco.
fn existing_folders(addons_dir: &Path, folders: &[String]) -> Result<Vec<String>, String> {
    let wanted: HashSet<String> = folders.iter().map(|name| name.to_lowercase()).collect();
    let entries = match fs::read_dir(addons_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(format!(
                "Could not read the AddOns folder ({}).",
                error.kind()
            ));
        }
    };
    let mut replaced = Vec::new();
    for entry in entries {
        let entry = entry
            .map_err(|error| format!("Could not read the AddOns folder ({}).", error.kind()))?;
        if let Ok(name) = entry.file_name().into_string()
            && wanted.contains(&name.to_lowercase())
        {
            replaced.push(name);
        }
    }
    replaced.sort();
    Ok(replaced)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;
    use std::io::{Cursor, Write};
    use std::path::PathBuf;
    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;

    fn zip_of(files: &[(&str, &str)]) -> Vec<u8> {
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        for (name, text) in files {
            writer
                .start_file(*name, SimpleFileOptions::default())
                .unwrap();
            writer.write_all(text.as_bytes()).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    /// `Interface/AddOns` con spazi nel percorso, come nei prefissi Wine, e la cartella dei
    /// download dell'utente.
    struct Fixture {
        _tmp: TempDir,
        addons: PathBuf,
        downloads: PathBuf,
    }

    fn fixture() -> Fixture {
        let tmp = TempDir::new();
        let addons = tmp
            .path()
            .join("Program Files (x86)")
            .join("World of Warcraft")
            .join("_retail_")
            .join("Interface")
            .join("AddOns");
        let downloads = tmp.path().join("My Downloads");
        fs::create_dir_all(&addons).unwrap();
        fs::create_dir_all(&downloads).unwrap();
        Fixture {
            _tmp: tmp,
            addons,
            downloads,
        }
    }

    impl Fixture {
        fn zip(&self, name: &str, files: &[(&str, &str)]) -> PathBuf {
            let path = self.downloads.join(name);
            fs::write(&path, zip_of(files)).unwrap();
            path
        }
    }

    fn listing(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }

    /// Le uniche voci accanto ad `AddOns`: nessuna cartella di lavoro rimasta.
    fn assert_no_leftovers(fx: &Fixture) {
        assert_eq!(listing(fx.addons.parent().unwrap()), ["AddOns"]);
    }

    #[test]
    fn installs_a_single_addon() {
        let fx = fixture();
        let zip = fx.zip(
            "Details.zip",
            &[
                ("Details/Details.toc", "## Title: Details\n"),
                ("Details/core.lua", "-- code\n"),
            ],
        );
        let result = install_zip(&fx.addons, &zip).unwrap();
        assert_eq!(
            result,
            LocalInstall {
                installed: vec!["Details".to_string()],
                replaced: vec![],
            }
        );
        assert_eq!(
            fs::read_to_string(fx.addons.join("Details").join("core.lua")).unwrap(),
            "-- code\n"
        );
        assert_no_leftovers(&fx);
    }

    #[test]
    fn installs_several_folders_with_flavor_tocs() {
        let fx = fixture();
        let zip = fx.zip(
            "Pack.ZIP",
            &[
                ("Pack/Pack_Mainline.toc", ""),
                ("Pack_Options/Pack_Options-Classic.toc", ""),
                ("Pack_Config/pack_config.TOC", ""),
            ],
        );
        let result = install_zip(&fx.addons, &zip).unwrap();
        assert_eq!(result.installed, ["Pack", "Pack_Options", "Pack_Config"]);
        assert_eq!(listing(&fx.addons), ["Pack", "Pack_Config", "Pack_Options"]);
    }

    #[test]
    fn replaces_existing_folders_ignoring_letter_case() {
        let fx = fixture();
        fs::create_dir_all(fx.addons.join("details")).unwrap();
        fs::write(fx.addons.join("details").join("old.lua"), "old").unwrap();
        fs::create_dir_all(fx.addons.join("Other")).unwrap();
        let zip = fx.zip(
            "Details.zip",
            &[("Details/Details.toc", ""), ("Details/new.lua", "new")],
        );
        let result = install_zip(&fx.addons, &zip).unwrap();
        assert_eq!(result.replaced, ["details"]);
        assert_eq!(listing(&fx.addons), ["Details", "Other"]);
        assert_eq!(
            listing(&fx.addons.join("Details")),
            ["Details.toc", "new.lua"]
        );
        assert_no_leftovers(&fx);
    }

    #[test]
    fn rejects_a_folder_without_its_own_toc() {
        let fx = fixture();
        fs::create_dir_all(fx.addons.join("Good")).unwrap();
        let cases: &[&[(&str, &str)]] = &[
            // Una cartella valida e una senza `.toc`.
            &[("Good/Good.toc", ""), ("Libs/lib.lua", "")],
            // Il `.toc` ha il nome di un'altra cartella.
            &[("Good/Other.toc", "")],
            // Un prefisso che non è seguito da `_` o `-`.
            &[("Good/Goodies.toc", "")],
            // Un suffisso vuoto.
            &[("Good/Good_.toc", "")],
            // Il `.toc` è in una sottocartella.
            &[("Good/sub/Good.toc", "")],
            // Una cartella contenitore in più.
            &[("Good-v1/Good/Good.toc", "")],
        ];
        for files in cases {
            let zip = fx.zip("bad.zip", files);
            let error = install_zip(&fx.addons, &zip).unwrap_err();
            assert!(error.contains("not a valid addon"), "{files:?}: {error}");
            assert_eq!(listing(&fx.addons), ["Good"], "{files:?}");
            assert!(listing(&fx.addons.join("Good")).is_empty(), "{files:?}");
            assert_no_leftovers(&fx);
        }
    }

    #[test]
    fn rejects_what_extract_rejects() {
        let fx = fixture();
        let zip = fx.zip("loose.zip", &[("readme.txt", "hi")]);
        let error = install_zip(&fx.addons, &zip).unwrap_err();
        assert!(error.contains("outside a folder"), "{error}");
        assert!(listing(&fx.addons).is_empty());
        assert_no_leftovers(&fx);

        // Uno zip valido senza voci non installa niente: `extract` lo rifiuta, quindi il controllo
        // dei `.toc` non può passare a vuoto.
        let empty = fx.zip("empty.zip", &[]);
        assert_eq!(
            install_zip(&fx.addons, &empty).unwrap_err(),
            "The zip is empty."
        );
        assert!(listing(&fx.addons).is_empty());
        assert_no_leftovers(&fx);
    }

    #[test]
    fn rejects_files_that_are_not_zips() {
        let fx = fixture();
        let rar = fx.downloads.join("Details.rar");
        fs::write(&rar, zip_of(&[("Details/Details.toc", "")])).unwrap();
        assert_eq!(
            install_zip(&fx.addons, &rar).unwrap_err(),
            "Only .zip files can be installed."
        );

        let fake = fx.downloads.join("fake.zip");
        fs::write(&fake, "not a zip").unwrap();
        assert!(
            install_zip(&fx.addons, &fake)
                .unwrap_err()
                .contains("not a valid zip")
        );

        let dir = fx.downloads.join("folder.zip");
        fs::create_dir_all(&dir).unwrap();
        assert_eq!(
            install_zip(&fx.addons, &dir).unwrap_err(),
            "The chosen item is not a file."
        );

        assert_eq!(
            install_zip(&fx.addons, &fx.downloads.join("missing.zip")).unwrap_err(),
            "The zip file was not found."
        );
        assert!(listing(&fx.addons).is_empty());
        assert_no_leftovers(&fx);
    }

    #[test]
    fn rejects_a_zip_over_the_limit() {
        let fx = fixture();
        let big = fx.downloads.join("big.zip");
        let file = fs::File::create(&big).unwrap();
        // Un file sparso: nessun byte scritto davvero.
        file.set_len(MAX_ZIP_BYTES + 1).unwrap();
        let error = install_zip(&fx.addons, &big).unwrap_err();
        assert!(error.contains("larger than"), "{error}");
        assert!(listing(&fx.addons).is_empty());
    }
}
