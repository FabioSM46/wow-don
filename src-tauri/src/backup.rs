//! Backup delle cartelle `WTF` e `Interface` di un'installazione in uno zip.
//!
//! Solo funzioni su percorsi: niente Tauri. La cartella dei backup la sceglie il chiamante
//! (`app_data_dir()/backups/<id-installazione>/`). Lo zip si scrive in streaming, file per file,
//! in un file temporaneo nascosto nella stessa cartella e si rinomina solo a scrittura finita:
//! un backup interrotto non lascia mai uno zip a metà con un nome valido.

use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, DateTime, ZipWriter};

use crate::installations::{Installation, strip_verbatim_prefix};

/// Le cartelle salvate, con il nome che hanno nel gioco.
const BACKUP_FOLDERS: [&str; 2] = ["WTF", "Interface"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupInfo {
    /// Nome del file, per esempio `retail-20261023-142501.zip`.
    pub name: String,
    /// Percorso completo dello zip, senza il prefisso `\\?\` di Windows.
    pub path: PathBuf,
    /// Dimensione dello zip in byte.
    pub size: u64,
    /// Data di modifica dello zip in millisecondi dall'epoca Unix.
    pub modified_ms: Option<u64>,
}

/// Crea lo zip di `WTF/` e `Interface/` di `installation` dentro `backups_dir` (creata se
/// manca). Le voci hanno percorsi relativi alla cartella `_<flavor>_`. Una delle due cartelle
/// può mancare; se mancano entrambe è un errore. I link simbolici non si seguono e non si
/// includono.
pub fn create_backup(
    installation: &Installation,
    backups_dir: &Path,
) -> Result<BackupInfo, String> {
    create_backup_at(installation, backups_dir, SystemTime::now())
}

fn create_backup_at(
    installation: &Installation,
    backups_dir: &Path,
    now: SystemTime,
) -> Result<BackupInfo, String> {
    let roots: Vec<(String, PathBuf)> = BACKUP_FOLDERS
        .iter()
        .filter_map(|name| find_child_dir(&installation.path, name))
        .collect();
    if roots.is_empty() {
        return Err(
            "Niente da salvare: l'installazione non ha né la cartella WTF né la cartella \
             Interface."
                .to_owned(),
        );
    }

    fs::create_dir_all(backups_dir).map_err(|error| {
        format!(
            "Impossibile creare la cartella dei backup ({}).",
            error.kind()
        )
    })?;

    let stem = format!("{}-{}", installation.flavor, timestamp(now));
    let final_path = free_name(backups_dir, &stem);
    let name = file_name(&final_path);
    let temp = TempFile(backups_dir.join(format!(".{name}.tmp")));

    write_zip(&temp.0, &roots)?;
    // Il nome finale compare solo a scrittura completa.
    fs::rename(&temp.0, &final_path)
        .map_err(|error| format!("Impossibile completare il backup ({}).", error.kind()))?;
    info_for(&final_path)
}

/// I backup in `backups_dir`, dal più recente. Una cartella mancante vuol dire zero backup.
/// Si elencano solo i file `.zip` non nascosti: i temporanei restano fuori.
pub fn list_backups(backups_dir: &Path) -> Result<Vec<BackupInfo>, String> {
    let entries = match fs::read_dir(backups_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(format!(
                "Impossibile leggere la cartella dei backup ({}).",
                error.kind()
            ));
        }
    };
    let mut backups = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let is_zip = Path::new(name)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("zip"));
        let is_file = entry.file_type().is_ok_and(|kind| kind.is_file());
        if name.starts_with('.') || !is_zip || !is_file {
            continue;
        }
        if let Ok(info) = info_for(&path) {
            backups.push(info);
        }
    }
    // Il nome contiene la data UTC: a parità di data di modifica decide lui.
    backups.sort_by(|a, b| {
        b.modified_ms
            .cmp(&a.modified_ms)
            .then_with(|| b.name.cmp(&a.name))
    });
    Ok(backups)
}

/// Il file temporaneo si cancella da solo se il backup non arriva al `rename`.
struct TempFile(PathBuf);

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// Cerca una sottocartella senza distinguere maiuscole e minuscole, come fa il gioco. Un link
/// simbolico non conta: non si segue. Restituisce il nome reale e il percorso.
fn find_child_dir(parent: &Path, wanted: &str) -> Option<(String, PathBuf)> {
    let mut found: Vec<(String, PathBuf)> = fs::read_dir(parent)
        .ok()?
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            name.eq_ignore_ascii_case(wanted)
                .then(|| (name, entry.path()))
        })
        .collect();
    // Su Linux possono esserci due cartelle che differiscono solo per le maiuscole: vince
    // quella scritta esattamente come la usa il gioco, poi l'ordine alfabetico.
    found.sort_by(|a, b| (a.0 != wanted).cmp(&(b.0 != wanted)).then(a.0.cmp(&b.0)));
    found.into_iter().next()
}

fn write_zip(target: &Path, roots: &[(String, PathBuf)]) -> Result<(), String> {
    let file = File::create(target)
        .map_err(|error| format!("Impossibile creare il file del backup ({}).", error.kind()))?;
    let mut writer = ZipWriter::new(BufWriter::new(file));
    for (name, path) in roots {
        add_dir(&mut writer, path, name)?;
    }
    let inner = writer
        .finish()
        .map_err(|error| format!("Impossibile chiudere lo zip del backup ({error})."))?;
    let file = inner
        .into_inner()
        .map_err(|error| format!("Impossibile scrivere il backup ({}).", error.error().kind()))?;
    file.sync_all()
        .map_err(|error| format!("Impossibile scrivere il backup ({}).", error.kind()))
}

/// Aggiunge `dir` come `entry_name/` e tutto il suo contenuto, ricorsivamente. Le voci usano
/// `/` come separatore, come richiede il formato zip.
fn add_dir<W: Write + io::Seek>(
    writer: &mut ZipWriter<W>,
    dir: &Path,
    entry_name: &str,
) -> Result<(), String> {
    let read_error =
        |error: io::Error| format!("Impossibile leggere {entry_name} ({}).", error.kind());
    let mut children: Vec<_> = fs::read_dir(dir)
        .map_err(read_error)?
        .collect::<Result<_, _>>()
        .map_err(read_error)?;
    // Ordine stabile: lo stesso albero dà sempre lo stesso zip.
    children.sort_by_key(|entry| entry.file_name());

    writer
        .add_directory(format!("{entry_name}/"), options_for(dir, 0))
        .map_err(|error| format!("Impossibile scrivere {entry_name} nel backup ({error})."))?;

    for child in children {
        let name = child.file_name().into_string().map_err(|_| {
            format!(
                "Un file in {entry_name} ha un nome con caratteri non validi: rinominalo e \
                 riprova."
            )
        })?;
        let child_entry = format!("{entry_name}/{name}");
        // `DirEntry::file_type` non segue i link simbolici: un link non è né file né cartella.
        let kind = child
            .file_type()
            .map_err(|error| format!("Impossibile leggere {child_entry} ({}).", error.kind()))?;
        let path = child.path();
        if kind.is_dir() {
            add_dir(writer, &path, &child_entry)?;
        } else if kind.is_file() {
            add_file(writer, &path, &child_entry)?;
        }
    }
    Ok(())
}

fn add_file<W: Write + io::Seek>(
    writer: &mut ZipWriter<W>,
    path: &Path,
    entry_name: &str,
) -> Result<(), String> {
    let mut source = File::open(path)
        .map_err(|error| format!("Impossibile leggere {entry_name} ({}).", error.kind()))?;
    let size = source.metadata().map(|meta| meta.len()).unwrap_or(0);
    writer
        .start_file(entry_name, options_for(path, size))
        .map_err(|error| format!("Impossibile scrivere {entry_name} nel backup ({error})."))?;
    // In streaming: un file di WTF da centinaia di MB non passa mai tutto dalla memoria.
    io::copy(&mut source, writer)
        .map_err(|error| format!("Impossibile copiare {entry_name} ({}).", error.kind()))?;
    Ok(())
}

fn options_for(path: &Path, size: u64) -> SimpleFileOptions {
    let mut options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .large_file(size >= u64::from(u32::MAX));
    let modified = fs::metadata(path).and_then(|meta| meta.modified()).ok();
    if let Some(time) = modified.and_then(zip_time) {
        options = options.last_modified_time(time);
    }
    options
}

/// Data e ora UTC di un istante. Senza nuove dipendenze: conversione dei giorni dall'epoca in
/// data civile (algoritmo `civil_from_days` di Howard Hinnant).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct UtcTime {
    year: i64,
    month: u8,
    day: u8,
    hour: u8,
    minute: u8,
    second: u8,
}

fn utc(time: SystemTime) -> UtcTime {
    // Un orologio prima del 1970 diventa il 1970: il nome resta comunque valido.
    let secs = time
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0);
    let days = (secs / 86_400) as i64;
    let rest = secs % 86_400;

    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u8;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u8;
    let year = yoe + era * 400 + i64::from(month <= 2);

    UtcTime {
        year,
        month,
        day,
        hour: (rest / 3_600) as u8,
        minute: (rest % 3_600 / 60) as u8,
        second: (rest % 60) as u8,
    }
}

/// `AAAAMMGG-HHMMSS` in UTC.
fn timestamp(time: SystemTime) -> String {
    let t = utc(time);
    format!(
        "{:04}{:02}{:02}-{:02}{:02}{:02}",
        t.year, t.month, t.day, t.hour, t.minute, t.second
    )
}

/// L'ora di modifica per la voce zip (in UTC: il formato non ha un fuso). Fuori dagli anni
/// che lo zip sa scrivere resta quella predefinita.
fn zip_time(time: SystemTime) -> Option<DateTime> {
    let t = utc(time);
    DateTime::from_date_and_time(
        u16::try_from(t.year).ok()?,
        t.month,
        t.day,
        t.hour,
        t.minute,
        t.second,
    )
    .ok()
}

/// `<stem>.zip`, oppure `<stem>-2.zip`, `<stem>-3.zip`… se due backup cadono nello stesso
/// secondo: un backup non ne sovrascrive mai un altro.
fn free_name(dir: &Path, stem: &str) -> PathBuf {
    let first = dir.join(format!("{stem}.zip"));
    if !first.exists() {
        return first;
    }
    (2u32..)
        .map(|n| dir.join(format!("{stem}-{n}.zip")))
        .find(|path| !path.exists())
        .expect("a free backup name always exists")
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn info_for(path: &Path) -> Result<BackupInfo, String> {
    let meta = fs::metadata(path)
        .map_err(|error| format!("Impossibile leggere il backup ({}).", error.kind()))?;
    let modified_ms = meta
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|elapsed| elapsed.as_millis() as u64);
    Ok(BackupInfo {
        name: file_name(path),
        path: without_verbatim_prefix(path),
        size: meta.len(),
        modified_ms,
    })
}

fn without_verbatim_prefix(path: &Path) -> PathBuf {
    path.to_str()
        .and_then(strip_verbatim_prefix)
        .map(PathBuf::from)
        .unwrap_or_else(|| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;
    use std::io::Read;
    use std::time::Duration;
    use zip::ZipArchive;

    fn installation(tmp: &TempDir) -> Installation {
        let dir = tmp.path().join("World of Warcraft").join("_classic_era_");
        fs::create_dir_all(&dir).unwrap();
        Installation::from_canonical(dir).unwrap()
    }

    fn write(path: &Path, data: &[u8]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, data).unwrap();
    }

    /// Nomi delle voci e contenuto dei file dello zip.
    fn read_zip(path: &Path) -> Vec<(String, Option<Vec<u8>>)> {
        let mut archive = ZipArchive::new(File::open(path).unwrap()).unwrap();
        let mut out = Vec::new();
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index).unwrap();
            let name = entry.name().unwrap().into_owned();
            if entry.is_dir() {
                out.push((name, None));
            } else {
                let mut data = Vec::new();
                entry.read_to_end(&mut data).unwrap();
                out.push((name, Some(data)));
            }
        }
        out
    }

    fn names(entries: &[(String, Option<Vec<u8>>)]) -> Vec<&str> {
        entries.iter().map(|(name, _)| name.as_str()).collect()
    }

    fn only_zip_files(dir: &Path) -> Vec<String> {
        let mut found: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        found.sort();
        found
    }

    #[test]
    fn nested_files_and_names_with_spaces_are_saved_relative_to_the_flavor_folder() {
        let tmp = TempDir::new();
        let inst = installation(&tmp);
        write(
            &inst
                .path
                .join("WTF/Account/MY ACCOUNT/SavedVariables/Details.lua"),
            b"Details = {}",
        );
        write(&inst.path.join("WTF/Config.wtf"), b"SET a 1");
        write(
            &inst.path.join("Interface/AddOns/My Addon/My Addon.toc"),
            b"## Title: My Addon",
        );
        fs::create_dir_all(inst.path.join("Interface/AddOns/Empty")).unwrap();
        let backups = tmp.path().join("backups").join(&inst.id);

        let info = create_backup(&inst, &backups).unwrap();

        let entries = read_zip(&info.path);
        assert_eq!(
            names(&entries),
            [
                "WTF/",
                "WTF/Account/",
                "WTF/Account/MY ACCOUNT/",
                "WTF/Account/MY ACCOUNT/SavedVariables/",
                "WTF/Account/MY ACCOUNT/SavedVariables/Details.lua",
                "WTF/Config.wtf",
                "Interface/",
                "Interface/AddOns/",
                "Interface/AddOns/Empty/",
                "Interface/AddOns/My Addon/",
                "Interface/AddOns/My Addon/My Addon.toc",
            ]
        );
        let details = entries
            .iter()
            .find(|(name, _)| name.ends_with("Details.lua"))
            .unwrap();
        assert_eq!(details.1.as_deref(), Some(&b"Details = {}"[..]));
        assert_eq!(info.size, fs::metadata(&info.path).unwrap().len());
        assert_eq!(info.path.parent().unwrap(), backups);
        assert!(info.name.starts_with("classic_era-") && info.name.ends_with(".zip"));
    }

    #[test]
    fn entries_are_deflated() {
        let tmp = TempDir::new();
        let inst = installation(&tmp);
        write(&inst.path.join("WTF/big.lua"), &[b'a'; 64 * 1024]);
        let info = create_backup(&inst, &tmp.path().join("backups")).unwrap();
        let mut archive = ZipArchive::new(File::open(&info.path).unwrap()).unwrap();
        let entry = archive.by_name("WTF/big.lua").unwrap();
        assert_eq!(entry.compression(), CompressionMethod::Deflated);
        assert!(entry.compressed_size() < entry.size());
    }

    #[test]
    fn only_interface_is_enough() {
        let tmp = TempDir::new();
        let inst = installation(&tmp);
        write(&inst.path.join("Interface/AddOns/A/A.toc"), b"x");
        let info = create_backup(&inst, &tmp.path().join("backups")).unwrap();
        assert_eq!(
            names(&read_zip(&info.path)),
            [
                "Interface/",
                "Interface/AddOns/",
                "Interface/AddOns/A/",
                "Interface/AddOns/A/A.toc"
            ]
        );
    }

    #[test]
    fn folder_names_are_matched_without_case() {
        let tmp = TempDir::new();
        let inst = installation(&tmp);
        write(&inst.path.join("wtf/Config.wtf"), b"x");
        let info = create_backup(&inst, &tmp.path().join("backups")).unwrap();
        assert_eq!(names(&read_zip(&info.path)), ["wtf/", "wtf/Config.wtf"]);
    }

    #[test]
    fn neither_folder_is_a_clear_error_and_writes_nothing() {
        let tmp = TempDir::new();
        let inst = installation(&tmp);
        write(&inst.path.join("Logs/x.log"), b"x");
        let backups = tmp.path().join("backups");
        let error = create_backup(&inst, &backups).unwrap_err();
        assert!(
            error.contains("WTF") && error.contains("Interface"),
            "{error}"
        );
        assert!(!backups.exists());
    }

    #[test]
    fn no_temporary_file_is_left_behind() {
        let tmp = TempDir::new();
        let inst = installation(&tmp);
        write(&inst.path.join("WTF/Config.wtf"), b"x");
        let backups = tmp.path().join("backups");
        let info = create_backup(&inst, &backups).unwrap();
        assert_eq!(only_zip_files(&backups), [info.name]);
    }

    #[test]
    fn a_failed_write_removes_the_temporary_file() {
        let tmp = TempDir::new();
        let backups = tmp.path().join("backups");
        fs::create_dir_all(&backups).unwrap();
        let target = backups.join(".x.zip.tmp");
        let missing = tmp.path().join("missing");
        let temp = TempFile(target.clone());
        assert!(write_zip(&temp.0, &[("WTF".to_owned(), missing)]).is_err());
        drop(temp);
        assert!(only_zip_files(&backups).is_empty());
    }

    #[test]
    fn the_file_name_has_the_flavor_and_the_utc_date() {
        let tmp = TempDir::new();
        let inst = installation(&tmp);
        write(&inst.path.join("WTF/Config.wtf"), b"x");
        let backups = tmp.path().join("backups");
        // 2024-02-29 23:59:58 UTC.
        let when = UNIX_EPOCH + Duration::from_secs(1_709_251_198);
        let first = create_backup_at(&inst, &backups, when).unwrap();
        let second = create_backup_at(&inst, &backups, when).unwrap();
        assert_eq!(first.name, "classic_era-20240229-235958.zip");
        // Stesso secondo: il primo non viene sovrascritto.
        assert_eq!(second.name, "classic_era-20240229-235958-2.zip");
    }

    #[test]
    fn utc_conversion_matches_known_dates() {
        let at = |secs| timestamp(UNIX_EPOCH + Duration::from_secs(secs));
        assert_eq!(at(0), "19700101-000000");
        assert_eq!(at(951_782_400), "20000229-000000");
        assert_eq!(at(1_735_689_599), "20241231-235959");
        assert_eq!(at(4_102_444_800), "21000101-000000");
    }

    #[test]
    fn list_is_sorted_from_the_newest_and_skips_other_files() {
        let tmp = TempDir::new();
        let backups = tmp.path().join("backups");
        write(&backups.join("retail-20240101-000000.zip"), b"old");
        write(&backups.join("retail-20250101-000000.zip"), b"newer");
        write(&backups.join(".retail-20260101-000000.zip.tmp"), b"tmp");
        write(&backups.join("notes.txt"), b"x");
        fs::create_dir_all(backups.join("folder.zip")).unwrap();
        let old = File::options()
            .write(true)
            .open(backups.join("retail-20240101-000000.zip"))
            .unwrap();
        old.set_modified(UNIX_EPOCH + Duration::from_secs(1_704_067_200))
            .unwrap();
        drop(old);

        let list = list_backups(&backups).unwrap();
        let names: Vec<&str> = list.iter().map(|info| info.name.as_str()).collect();
        assert_eq!(
            names,
            ["retail-20250101-000000.zip", "retail-20240101-000000.zip"]
        );
        assert_eq!(list[0].size, 5);
        assert_eq!(list[1].modified_ms, Some(1_704_067_200_000));
    }

    #[test]
    fn a_missing_backups_folder_lists_nothing() {
        let tmp = TempDir::new();
        assert!(
            list_backups(&tmp.path().join("missing"))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn returned_paths_have_no_verbatim_prefix() {
        assert_eq!(
            without_verbatim_prefix(Path::new(r"\\?\C:\data\backups\retail.zip")),
            PathBuf::from(r"C:\data\backups\retail.zip")
        );
        let plain = Path::new("data").join("retail.zip");
        assert_eq!(without_verbatim_prefix(&plain), plain);
    }

    #[cfg(unix)]
    #[test]
    fn symbolic_links_are_neither_followed_nor_included() {
        use std::os::unix::fs::symlink;
        let tmp = TempDir::new();
        let inst = installation(&tmp);
        let outside = tmp.path().join("outside");
        write(&outside.join("secret.txt"), b"secret");
        write(&inst.path.join("WTF/Config.wtf"), b"x");
        symlink(&outside, inst.path.join("WTF/linked-dir")).unwrap();
        symlink(
            outside.join("secret.txt"),
            inst.path.join("WTF/linked-file"),
        )
        .unwrap();
        // Una cartella Interface che è un link non si segue: resta solo WTF.
        symlink(&outside, inst.path.join("Interface")).unwrap();

        let info = create_backup(&inst, &tmp.path().join("backups")).unwrap();
        assert_eq!(names(&read_zip(&info.path)), ["WTF/", "WTF/Config.wtf"]);
    }
}
