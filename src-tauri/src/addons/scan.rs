//! Scansione di `Interface/AddOns` di un'installazione. Solo lettura: non modifica niente.

use std::collections::HashMap;
use std::fs;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};
use std::thread;
use std::time::UNIX_EPOCH;

use serde::Serialize;

use super::cache::FingerprintCache;
use super::fingerprint;
use super::toc::{self, TocInfo};
use crate::store::InstalledRecords;

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
#[cfg(test)]
pub fn scan_addons(installation_dir: &Path, flavor: &str) -> Result<Vec<InstalledAddon>, String> {
    scan_addons_with(installation_dir, flavor, None)
}

/// Come [`scan_addons`], con i fingerprint presi da `cache` quando le cartelle non sono
/// cambiate (issue #125). Le cartelle che non ci sono più escono dalla cache.
pub fn scan_addons_with(
    installation_dir: &Path,
    flavor: &str,
    cache: Option<&FingerprintCache>,
) -> Result<Vec<InstalledAddon>, String> {
    if !installation_dir.is_dir() {
        return Err("The installation folder no longer exists or cannot be reached.".to_owned());
    }
    let addons_dir = installation_dir.join("Interface").join("AddOns");
    let entries = match fs::read_dir(&addons_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(format!(
                "Could not read the addons folder ({}).",
                error.kind()
            ));
        }
    };

    let mut addons = Vec::new();
    // Le cartelle da leggere: la lettura (e il fingerprint) si fa dopo, in parallelo.
    let mut folders = Vec::new();
    let mut unreadable = 0;
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                // Senza la voce non si conosce nemmeno il nome: resta un segnaposto visibile.
                unreadable += 1;
                addons.push(unreadable_entry(
                    format!("(unreadable entry {unreadable})"),
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
        folders.push((entry.path(), folder, modified_ms));
    }
    if let Some(cache) = cache {
        let present: Vec<String> = folders
            .iter()
            .map(|(_, folder, _)| folder.clone())
            .collect();
        cache.retain(&addons_dir, &present);
    }
    addons.extend(read_addons(folders, flavor, cache));
    addons.sort_by(|a, b| {
        a.folder
            .to_lowercase()
            .cmp(&b.folder.to_lowercase())
            .then_with(|| a.folder.cmp(&b.folder))
    });
    Ok(addons)
}

/// Legge le cartelle con più thread: il costo è quasi tutto nel fingerprint, che legge per
/// intero i file dell'addon. Ogni thread prende la prossima cartella libera, così una cartella
/// molto grande non blocca le altre dietro di sé; ogni risultato resta al posto della sua
/// cartella, quindi l'ordine non dipende da quale thread finisce prima.
fn read_addons(
    folders: Vec<Folder>,
    flavor: &str,
    cache: Option<&FingerprintCache>,
) -> Vec<InstalledAddon> {
    let threads = thread::available_parallelism()
        .map_or(1, NonZeroUsize::get)
        .min(MAX_SCAN_THREADS)
        .min(folders.len());
    let results: Vec<Mutex<Option<InstalledAddon>>> =
        folders.iter().map(|_| Mutex::new(None)).collect();
    let next = AtomicUsize::new(0);
    let work = || {
        loop {
            let index = next.fetch_add(1, Ordering::Relaxed);
            let Some((dir, folder, modified_ms)) = folders.get(index) else {
                return;
            };
            let addon = read_addon(dir, folder.clone(), flavor, *modified_ms, cache);
            *results[index]
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = Some(addon);
        }
    };
    if threads <= 1 {
        work();
    } else {
        thread::scope(|scope| {
            for _ in 0..threads {
                scope.spawn(work);
            }
        });
    }
    results
        .into_iter()
        .filter_map(|result| result.into_inner().unwrap_or_else(PoisonError::into_inner))
        .collect()
}

/// Thread della scansione al massimo. Il lavoro è soprattutto lettura dal disco, quindi oltre
/// questa soglia non si guadagna; e ogni thread tiene in memoria il file che sta calcolando
/// (fino a decine di MB per alcuni `.toc`), quindi la soglia limita anche il picco di memoria.
const MAX_SCAN_THREADS: usize = 8;

/// Una cartella da leggere: percorso, nome e data di modifica in millisecondi.
type Folder = (PathBuf, String, Option<u64>);

fn unreadable_entry(folder: String, error: &std::io::Error) -> InstalledAddon {
    InstalledAddon {
        folder,
        toc_file: None,
        toc: TocInfo::default(),
        modified_ms: None,
        fingerprint: None,
        error: Some(format!("Unreadable folder ({}).", error.kind())),
    }
}

fn read_addon(
    dir: &Path,
    folder: String,
    flavor: &str,
    modified_ms: Option<u64>,
    cache: Option<&FingerprintCache>,
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
            addon.error = Some(format!("Unreadable folder ({}).", error.kind()));
            return addon;
        }
    };
    // Il fingerprint serve anche quando il `.toc` per questo flavor manca: CurseForge
    // riconosce la cartella dai suoi file, non dal flavor.
    addon.fingerprint = match cache {
        Some(cache) => cache.fingerprint(dir),
        None => fingerprint::folder_fingerprint(dir),
    };
    let Some(toc_file) = toc::select_toc(&addon.folder, &file_names, flavor) else {
        addon.error = Some("No .toc file for this game version.".to_owned());
        return addon;
    };
    match fs::read(dir.join(toc_file)) {
        // I `.toc` non sono sempre UTF-8: quello che non lo è diventa un carattere sostitutivo.
        Ok(bytes) => addon.toc = toc::parse_toc(&String::from_utf8_lossy(&bytes)),
        Err(error) => {
            addon.error = Some(format!("Unreadable .toc file ({}).", error.kind()));
        }
    }
    addon.toc_file = Some(toc_file.to_owned());
    addon
}

/// Il progetto CurseForge che l'app ha installato in una cartella, dal registro
/// `installed-addons.json` (issue #62).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledProject {
    pub id: u32,
    /// Le cartelle del record che esistono ancora, con il nome che hanno sul disco.
    pub folders: Vec<String>,
}

/// Una cartella di `list_addons`: la scansione più il progetto registrato, se c'è.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListedAddon {
    #[serde(flatten)]
    pub addon: InstalledAddon,
    pub installed_project: Option<InstalledProject>,
}

/// Unisce la scansione ai record dell'app. Pura: non legge né disco né store. I nomi si
/// confrontano senza distinguere maiuscole e minuscole, come fa il gioco. Un record conta solo
/// per le sue cartelle ancora presenti; un record senza cartelle presenti non produce niente.
/// Una cartella che compare nei record di più progetti (una libreria distribuita da più addon)
/// non viene attribuita a nessuno: non è la prova di un solo progetto.
pub fn with_installed_projects(
    addons: Vec<InstalledAddon>,
    records: &InstalledRecords,
) -> Vec<ListedAddon> {
    let on_disk: HashMap<String, &str> = addons
        .iter()
        .map(|a| (a.folder.to_lowercase(), a.folder.as_str()))
        .collect();
    let mut owners: HashMap<String, Vec<InstalledProject>> = HashMap::new();
    for (mod_id, record) in records {
        let mut present: Vec<String> = Vec::new();
        for folder in &record.folders {
            if let Some(name) = on_disk.get(&folder.to_lowercase())
                && !present.iter().any(|p| p == name)
            {
                present.push((*name).to_owned());
            }
        }
        let project = InstalledProject {
            id: *mod_id,
            folders: present,
        };
        for folder in &project.folders {
            owners
                .entry(folder.to_lowercase())
                .or_default()
                .push(project.clone());
        }
    }
    addons
        .into_iter()
        .map(|addon| {
            let installed_project = match owners.remove(&addon.folder.to_lowercase()) {
                Some(mut projects) if projects.len() == 1 => projects.pop(),
                _ => None,
            };
            ListedAddon {
                addon,
                installed_project,
            }
        })
        .collect()
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
    fn folders_read_in_parallel_keep_their_own_results_and_order() {
        let tmp = TempDir::new();
        let names: Vec<String> = (0..40).map(|i| format!("Addon{i:02}")).collect();
        for name in &names {
            let toc = (
                format!("{name}.toc"),
                format!("## Title: {name}\n{name}.lua\n"),
            );
            let lua = (format!("{name}.lua"), format!("print('{name}')\n"));
            addon(
                tmp.path(),
                name,
                &[
                    (toc.0.as_str(), toc.1.as_str()),
                    (lua.0.as_str(), lua.1.as_str()),
                ],
            );
        }
        let addons = scan_addons(tmp.path(), "retail").unwrap();
        let folders: Vec<&str> = addons.iter().map(|a| a.folder.as_str()).collect();
        assert_eq!(
            folders,
            names.iter().map(String::as_str).collect::<Vec<_>>()
        );
        for addon in &addons {
            assert_eq!(addon.toc.title.as_deref(), Some(addon.folder.as_str()));
            let dir = tmp
                .path()
                .join("Interface")
                .join("AddOns")
                .join(&addon.folder);
            assert_eq!(addon.fingerprint, fingerprint::folder_fingerprint(&dir));
        }
    }

    /// Fingerprint della cartella AddOns vera indicata da `WOWDON_FINGERPRINT_DIR` (la cartella
    /// dell'installazione, `_<flavor>_`): ogni cartella deve dare lo stesso valore del calcolo
    /// di riferimento, e il tempo della scansione si stampa. `cargo test --release -- --ignored
    /// real_folder_fingerprints --nocapture`.
    #[test]
    #[ignore = "needs a real installation folder"]
    fn real_folder_fingerprints_match_the_reference() {
        let Ok(dir) = std::env::var("WOWDON_FINGERPRINT_DIR") else {
            panic!("set WOWDON_FINGERPRINT_DIR to an installation folder");
        };
        let dir = PathBuf::from(dir);
        let started = std::time::Instant::now();
        let addons = scan_addons(&dir, "retail").unwrap();
        eprintln!("scan: {} folders in {:?}", addons.len(), started.elapsed());
        let addons_dir = dir.join("Interface").join("AddOns");
        for addon in &addons {
            let expected = fingerprint::reference_fingerprint(&addons_dir.join(&addon.folder));
            assert_eq!(addon.fingerprint, expected, "{}", addon.folder);
            eprintln!("{} {:?}", addon.folder, addon.fingerprint);
        }
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
                .contains("no longer exists")
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
        assert!(addons[0].error.as_deref().unwrap().contains("Unreadable"));
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
            "group",
            "curseProjectId",
            "wagoId",
            "wowiId",
        ] {
            assert!(keys.contains(&key.to_owned()), "missing {key} in {keys:?}");
        }
        // `json["group"]` vale `Null` anche per una chiave assente: prima la presenza.
        assert!(json.as_object().unwrap().contains_key("group"));
        assert_eq!(json["group"], serde_json::Value::Null);
        let grouped = InstalledAddon {
            toc: toc::parse_toc("## Group: EllesmereUI\n"),
            ..addon
        };
        assert_eq!(
            serde_json::to_value(&grouped).unwrap()["group"],
            serde_json::Value::String("EllesmereUI".to_owned())
        );
        assert_eq!(keys.len(), 17, "unexpected keys: {keys:?}");
    }

    fn listed(folders: &[&str]) -> Vec<InstalledAddon> {
        folders
            .iter()
            .map(|folder| InstalledAddon {
                folder: (*folder).to_owned(),
                toc_file: None,
                toc: TocInfo::default(),
                modified_ms: None,
                fingerprint: None,
                error: None,
            })
            .collect()
    }

    fn records(entries: &[(u32, &[&str])]) -> InstalledRecords {
        entries
            .iter()
            .map(|(id, folders)| {
                (
                    *id,
                    crate::store::InstalledRecord {
                        file_id: id * 10,
                        folders: folders.iter().map(|f| (*f).to_owned()).collect(),
                    },
                )
            })
            .collect()
    }

    /// La cartella e, se c'è, il progetto registrato con le sue cartelle.
    type Attributed<'a> = (&'a str, Option<(u32, Vec<&'a str>)>);

    fn projects(listed: &[ListedAddon]) -> Vec<Attributed<'_>> {
        listed
            .iter()
            .map(|l| {
                (
                    l.addon.folder.as_str(),
                    l.installed_project
                        .as_ref()
                        .map(|p| (p.id, p.folders.iter().map(String::as_str).collect())),
                )
            })
            .collect()
    }

    #[test]
    fn recorded_folders_carry_their_project() {
        let addons = listed(&["Details", "details_compare2", "Other"]);
        // Il record usa i nomi dello zip; sul disco una cartella ha un'altra capitalizzazione,
        // e una cartella del record non c'è più.
        let records = records(&[(7, &["Details", "Details_Compare2", "Details_Gone"])]);
        let result = with_installed_projects(addons, &records);
        let both = Some((7, vec!["Details", "details_compare2"]));
        assert_eq!(
            projects(&result),
            [
                ("Details", both.clone()),
                ("details_compare2", both),
                ("Other", None),
            ]
        );
    }

    #[test]
    fn a_record_whose_folders_are_gone_produces_nothing() {
        let addons = listed(&["Other"]);
        let result = with_installed_projects(addons, &records(&[(7, &["Details"])]));
        assert_eq!(projects(&result), [("Other", None)]);
    }

    #[test]
    fn a_folder_in_several_records_is_not_attributed() {
        let addons = listed(&["Alpha", "Beta", "LibShared"]);
        let records = records(&[(1, &["Alpha", "LibShared"]), (2, &["Beta", "LibShared"])]);
        let result = with_installed_projects(addons, &records);
        assert_eq!(
            projects(&result),
            [
                ("Alpha", Some((1, vec!["Alpha", "LibShared"]))),
                ("Beta", Some((2, vec!["Beta", "LibShared"]))),
                ("LibShared", None),
            ]
        );
    }

    #[test]
    fn the_listed_payload_adds_installed_project() {
        let mut result =
            with_installed_projects(listed(&["Alpha", "Beta"]), &records(&[(1, &["Alpha"])]));
        let alpha = serde_json::to_value(&result[0]).unwrap();
        assert_eq!(alpha["folder"], "Alpha");
        assert_eq!(
            alpha["installedProject"],
            serde_json::json!({"id": 1, "folders": ["Alpha"]})
        );
        let beta = serde_json::to_value(result.pop().unwrap()).unwrap();
        assert!(beta.as_object().unwrap().contains_key("installedProject"));
        assert_eq!(beta["installedProject"], serde_json::Value::Null);
        assert_eq!(beta.as_object().unwrap().len(), 18);
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
    /// system le distingue: su Windows (NTFS) e su macOS (APFS, che di norma non distingue le
    /// maiuscole) sarebbero la stessa cartella. Per questo il test gira solo su Linux.
    #[cfg(target_os = "linux")]
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
