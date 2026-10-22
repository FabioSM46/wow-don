//! Scansione di `Interface/AddOns` di un'installazione. Solo lettura: non modifica niente.

use std::fs;
use std::path::Path;
use std::time::UNIX_EPOCH;

use serde::Serialize;

use super::fingerprint;
use super::toc::{self, TocInfo};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledAddon {
    /// Nome della cartella in `Interface/AddOns`.
    pub folder: String,
    /// Il `.toc` scelto per il flavor, se ce n'è uno.
    pub toc_file: Option<String>,
    /// Campi del `.toc`; vuoti se il `.toc` manca o non si legge.
    #[serde(flatten)]
    pub toc: TocInfo,
    /// Data di modifica della cartella, in millisecondi Unix.
    pub modified_ms: Option<u64>,
    /// Fingerprint di CurseForge della cartella (vedi `addons::fingerprint`).
    pub fingerprint: Option<u32>,
    /// Perché l'addon non si è potuto leggere: l'addon resta in lista invece di sparire.
    pub error: Option<String>,
}

/// Legge gli addon di `<installazione>/Interface/AddOns`. Se manca `Interface/AddOns` non ci
/// sono addon; se manca la cartella dell'installazione è un errore, perché "zero addon"
/// nasconderebbe un gioco spostato o un disco non montato. Una voce che non si legge resta in
/// lista con un errore. L'ordine è stabile: per nome della cartella, senza distinguere
/// maiuscole e minuscole.
pub fn scan_addons(installation_dir: &Path, flavor: &str) -> Result<Vec<InstalledAddon>, String> {
    if !installation_dir.is_dir() {
        return Err(
            "La cartella dell'installazione non esiste più o non è raggiungibile.".to_owned(),
        );
    }
    let addons_dir = installation_dir.join("Interface").join("AddOns");
    let entries = match fs::read_dir(&addons_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(format!(
                "Impossibile leggere la cartella degli addon ({}).",
                error.kind()
            ));
        }
    };

    let mut addons = Vec::new();
    let mut unreadable = 0;
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                // Senza la voce non si conosce nemmeno il nome: resta un segnaposto visibile.
                unreadable += 1;
                addons.push(unreadable_entry(
                    format!("(voce non leggibile {unreadable})"),
                    &error,
                ));
                continue;
            }
        };
        let folder = entry.file_name().to_string_lossy().into_owned();
        if folder.starts_with('.') {
            continue;
        }
        // `metadata` segue i link simbolici: un addon collegato da un'altra cartella conta,
        // e un collegamento rotto resta in lista con il suo errore.
        let metadata = match fs::metadata(entry.path()) {
            Ok(metadata) => metadata,
            Err(error) => {
                addons.push(unreadable_entry(folder, &error));
                continue;
            }
        };
        if !metadata.is_dir() {
            continue;
        }
        let modified_ms = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .and_then(|duration| u64::try_from(duration.as_millis()).ok());
        addons.push(read_addon(&entry.path(), folder, flavor, modified_ms));
    }
    addons.sort_by(|a, b| {
        a.folder
            .to_lowercase()
            .cmp(&b.folder.to_lowercase())
            .then_with(|| a.folder.cmp(&b.folder))
    });
    Ok(addons)
}

fn unreadable_entry(folder: String, error: &std::io::Error) -> InstalledAddon {
    InstalledAddon {
        folder,
        toc_file: None,
        toc: TocInfo::default(),
        modified_ms: None,
        fingerprint: None,
        error: Some(format!("Cartella non leggibile ({}).", error.kind())),
    }
}

fn read_addon(
    dir: &Path,
    folder: String,
    flavor: &str,
    modified_ms: Option<u64>,
) -> InstalledAddon {
    let mut addon = InstalledAddon {
        folder,
        toc_file: None,
        toc: TocInfo::default(),
        modified_ms,
        fingerprint: None,
        error: None,
    };
    let file_names: Vec<String> = match fs::read_dir(dir) {
        Ok(entries) => entries
            .flatten()
            .filter_map(|entry| entry.file_name().into_string().ok())
            .collect(),
        Err(error) => {
            addon.error = Some(format!("Cartella non leggibile ({}).", error.kind()));
            return addon;
        }
    };
    // Il fingerprint serve anche quando il `.toc` per questo flavor manca: CurseForge
    // riconosce la cartella dai suoi file, non dal flavor.
    addon.fingerprint = fingerprint::folder_fingerprint(dir);
    let Some(toc_file) = toc::select_toc(&addon.folder, &file_names, flavor) else {
        addon.error = Some("Nessun file .toc per questa versione del gioco.".to_owned());
        return addon;
    };
    match fs::read(dir.join(toc_file)) {
        // I `.toc` non sono sempre UTF-8: quello che non lo è diventa un carattere sostitutivo.
        Ok(bytes) => addon.toc = toc::parse_toc(&String::from_utf8_lossy(&bytes)),
        Err(error) => {
            addon.error = Some(format!("File .toc non leggibile ({}).", error.kind()));
        }
    }
    addon.toc_file = Some(toc_file.to_owned());
    addon
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;

    fn addon(root: &Path, folder: &str, files: &[(&str, &str)]) {
        let dir = root.join("Interface/AddOns").join(folder);
        fs::create_dir_all(&dir).unwrap();
        for (name, content) in files {
            fs::write(dir.join(name), content).unwrap();
        }
    }

    /// Nomi, dimensioni e date di tutto quello che c'è sotto `root`, per verificare che la
    /// scansione non abbia cambiato niente.
    fn snapshot(root: &Path) -> Vec<(String, u64, Option<std::time::SystemTime>)> {
        let mut out = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            for entry in fs::read_dir(&dir).unwrap().flatten() {
                // `fs::metadata` e non `DirEntry::metadata`: su Windows la seconda restituisce
                // i dati memorizzati quando si è letta la cartella padre, che possono essere
                // vecchi, e il confronto fallirebbe senza che niente sia cambiato.
                let meta = fs::metadata(entry.path()).unwrap();
                out.push((
                    entry.path().to_string_lossy().into_owned(),
                    meta.len(),
                    meta.modified().ok(),
                ));
                if meta.is_dir() {
                    stack.push(entry.path());
                }
            }
        }
        out.sort();
        out
    }

    #[test]
    fn a_missing_addons_folder_means_no_addons() {
        let tmp = TempDir::new();
        assert_eq!(scan_addons(tmp.path(), "retail").unwrap(), Vec::new());
    }

    #[test]
    fn a_missing_installation_folder_is_an_error() {
        let tmp = TempDir::new();
        let gone = tmp.path().join("_retail_");
        assert!(
            scan_addons(&gone, "retail")
                .unwrap_err()
                .contains("non esiste")
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_broken_link_stays_in_the_list_with_an_error() {
        let tmp = TempDir::new();
        let addons_dir = tmp.path().join("Interface/AddOns");
        fs::create_dir_all(&addons_dir).unwrap();
        std::os::unix::fs::symlink(tmp.path().join("missing"), addons_dir.join("Linked")).unwrap();

        let addons = scan_addons(tmp.path(), "retail").unwrap();
        assert_eq!(addons.len(), 1);
        assert_eq!(addons[0].folder, "Linked");
        assert!(
            addons[0]
                .error
                .as_deref()
                .unwrap()
                .contains("non leggibile")
        );
    }

    #[test]
    fn the_payload_uses_camel_case_throughout() {
        let addon = InstalledAddon {
            folder: "Foo".to_owned(),
            toc_file: Some("Foo.toc".to_owned()),
            toc: toc::parse_toc("## Title: Foo\n## OptionalDeps: Bar\n## X-Wago-ID: x\n"),
            modified_ms: Some(1),
            fingerprint: Some(7),
            error: None,
        };
        let json = serde_json::to_value(&addon).unwrap();
        let keys: Vec<_> = json.as_object().unwrap().keys().cloned().collect();
        for key in [
            "folder",
            "tocFile",
            "modifiedMs",
            "fingerprint",
            "error",
            "title",
            "interface",
            "version",
            "author",
            "notes",
            "dependencies",
            "optionalDeps",
            "loadOnDemand",
            "curseProjectId",
            "wagoId",
            "wowiId",
        ] {
            assert!(keys.contains(&key.to_owned()), "missing {key} in {keys:?}");
        }
        assert_eq!(keys.len(), 16, "unexpected keys: {keys:?}");
    }

    #[test]
    fn addons_are_read_with_the_toc_for_the_flavor() {
        let tmp = TempDir::new();
        addon(
            tmp.path(),
            "Details",
            &[("Details.toc", "## Title: Details!\n## Author: Terciob\n")],
        );
        addon(
            tmp.path(),
            "Questie",
            &[
                ("Questie.toc", "## Title: Questie generic\n"),
                (
                    "Questie_Vanilla.toc",
                    "## Title: Questie\n## Interface: 11507\n",
                ),
            ],
        );

        let addons = scan_addons(tmp.path(), "classic_era").unwrap();
        assert_eq!(addons.len(), 2);
        assert_eq!(addons[0].folder, "Details");
        assert_eq!(addons[0].toc.title.as_deref(), Some("Details!"));
        assert_eq!(addons[0].toc.author.as_deref(), Some("Terciob"));
        assert_eq!(addons[1].toc_file.as_deref(), Some("Questie_Vanilla.toc"));
        assert_eq!(addons[1].toc.interface, [11507]);
        assert!(
            addons
                .iter()
                .all(|a| a.error.is_none() && a.modified_ms.is_some())
        );
        assert!(addons.iter().all(|a| a.fingerprint.is_some()));
        assert_ne!(addons[0].fingerprint, addons[1].fingerprint);
    }

    #[test]
    fn unreadable_addons_stay_in_the_list_with_an_error() {
        let tmp = TempDir::new();
        addon(tmp.path(), "NoToc", &[("core.lua", "-- code")]);
        addon(
            tmp.path(),
            "OtherName",
            &[("Wrong.toc", "## Title: Wrong\n")],
        );

        let addons = scan_addons(tmp.path(), "retail").unwrap();
        assert_eq!(addons.len(), 2);
        assert!(
            addons
                .iter()
                .all(|a| a.error.is_some() && a.toc_file.is_none())
        );
    }

    #[test]
    fn a_toc_that_is_not_utf8_is_still_read() {
        let tmp = TempDir::new();
        let dir = tmp.path().join("Interface/AddOns/Latin");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("Latin.toc"),
            b"## Title: Caf\xe9\n## Version: 1.0\n",
        )
        .unwrap();

        let addons = scan_addons(tmp.path(), "retail").unwrap();
        assert_eq!(addons[0].toc.version.as_deref(), Some("1.0"));
        assert!(addons[0].toc.title.as_deref().unwrap().starts_with("Caf"));
        assert!(addons[0].error.is_none());
    }

    #[test]
    fn hidden_folders_and_loose_files_are_ignored() {
        let tmp = TempDir::new();
        addon(tmp.path(), ".git", &[(".git.toc", "## Title: hidden\n")]);
        addon(tmp.path(), "Real", &[("Real.toc", "## Title: Real\n")]);
        fs::write(tmp.path().join("Interface/AddOns/readme.txt"), "x").unwrap();

        let folders: Vec<_> = scan_addons(tmp.path(), "retail")
            .unwrap()
            .into_iter()
            .map(|a| a.folder)
            .collect();
        assert_eq!(folders, ["Real"]);
    }

    fn folders(root: &Path) -> Vec<String> {
        scan_addons(root, "retail")
            .unwrap()
            .into_iter()
            .map(|a| a.folder)
            .collect()
    }

    #[test]
    fn the_order_ignores_case() {
        let tmp = TempDir::new();
        for folder in ["zeta", "Alpha", "beta", "Gamma"] {
            addon(tmp.path(), folder, &[]);
        }
        assert_eq!(folders(tmp.path()), ["Alpha", "beta", "Gamma", "zeta"]);
    }

    /// Due cartelle che differiscono solo per le maiuscole possono esistere solo dove il file
    /// system le distingue: su Windows (NTFS) sarebbero la stessa cartella.
    #[cfg(unix)]
    #[test]
    fn names_differing_only_by_case_have_a_stable_order() {
        let tmp = TempDir::new();
        for folder in ["alpha", "Alpha"] {
            addon(tmp.path(), folder, &[]);
        }
        assert_eq!(folders(tmp.path()), ["Alpha", "alpha"]);
    }

    #[test]
    fn scanning_changes_nothing_on_disk() {
        let tmp = TempDir::new();
        addon(
            tmp.path(),
            "Details",
            &[("Details.toc", "## Title: Details!\n")],
        );
        addon(tmp.path(), "NoToc", &[("core.lua", "-- code")]);
        let before = snapshot(tmp.path());
        scan_addons(tmp.path(), "retail").unwrap();
        assert_eq!(snapshot(tmp.path()), before);
    }
}
