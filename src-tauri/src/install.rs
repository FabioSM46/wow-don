//! Installazione di uno zip di addon: estrazione sicura in una cartella di staging e
//! sostituzione delle cartelle in `Interface/AddOns` senza lasciarla a metà.
//!
//! Solo funzioni su percorsi: niente rete, niente Tauri. Staging e appoggio sono cartelle
//! nascoste nella cartella padre di `AddOns` ([`WorkDir`]), così ogni `rename` resta sullo stesso
//! disco; si cancellano da sole anche dopo un errore.
//!
//! Il chiamante (il comando di installazione, issue successiva) fa:
//! `WorkDir::staging(addons)` → [`extract`] → [`replace`].

// Il comando di installazione che usa questo modulo arriva con una issue successiva.
#![allow(dead_code)]

use std::collections::HashSet;
use std::fs;
use std::io::{self, Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use zip::ZipArchive;

/// Numero massimo di voci in uno zip.
pub const MAX_ENTRIES: usize = 20_000;
/// Dimensione massima estratta, sommando tutti i file: 1 GiB.
pub const MAX_TOTAL_BYTES: u64 = 1 << 30;

#[derive(Debug, Clone, Copy)]
struct Limits {
    entries: usize,
    total_bytes: u64,
}

const DEFAULT_LIMITS: Limits = Limits {
    entries: MAX_ENTRIES,
    total_bytes: MAX_TOTAL_BYTES,
};

/// Cartella di lavoro nascosta accanto ad `AddOns` (stesso padre, quindi stesso disco).
/// Si cancella quando esce di scope, anche dopo un errore.
#[derive(Debug)]
pub struct WorkDir(PathBuf);

impl WorkDir {
    /// Cartella in cui estrarre lo zip.
    pub fn staging(addons_dir: &Path) -> Result<Self, String> {
        Self::create(addons_dir, "staging")
    }

    /// Cartella in cui spostare da parte le cartelle sostituite o rimosse.
    fn backup(addons_dir: &Path) -> Result<Self, String> {
        Self::create(addons_dir, "backup")
    }

    fn create(addons_dir: &Path, kind: &str) -> Result<Self, String> {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let parent = addons_dir
            .parent()
            .ok_or_else(|| "La cartella AddOns non ha una cartella padre.".to_string())?;
        let name = format!(
            ".wow-don-{kind}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let path = parent.join(name);
        // Un avanzo di un'esecuzione interrotta con lo stesso pid non deve mescolarsi.
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).map_err(|error| {
            format!(
                "Impossibile creare la cartella temporanea di installazione ({}).",
                error.kind()
            )
        })?;
        Ok(Self(path))
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for WorkDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Estrae `zip_bytes` in `staging_dir`, che deve esistere ed essere vuota, e restituisce i nomi
/// delle cartelle di primo livello (gli addon), nell'ordine in cui compaiono.
///
/// Tutto lo zip viene controllato prima di scrivere il primo file: una voce con un percorso
/// assoluto, un componente `..`, un prefisso di unità, un link simbolico, un file sciolto alla
/// radice o un limite superato fanno rifiutare l'intero zip. Nessun file viene mai scritto fuori
/// da `staging_dir`.
pub fn extract(zip_bytes: &[u8], staging_dir: &Path) -> Result<Vec<String>, String> {
    extract_with_limits(zip_bytes, staging_dir, DEFAULT_LIMITS)
}

/// Una voce dello zip già validata.
struct Entry {
    index: usize,
    components: Vec<String>,
    is_dir: bool,
}

fn extract_with_limits(
    zip_bytes: &[u8],
    staging_dir: &Path,
    limits: Limits,
) -> Result<Vec<String>, String> {
    let not_empty = fs::read_dir(staging_dir)
        .map_err(|error| {
            format!(
                "Impossibile leggere la cartella di staging ({}).",
                error.kind()
            )
        })?
        .next()
        .is_some();
    if not_empty {
        return Err("La cartella di staging non è vuota.".to_string());
    }

    let mut archive = ZipArchive::new(Cursor::new(zip_bytes))
        .map_err(|_| "Il file scaricato non è uno zip valido.".to_string())?;
    if archive.len() > limits.entries {
        return Err(format!(
            "Lo zip ha troppe voci ({}, massimo {}).",
            archive.len(),
            limits.entries
        ));
    }

    // Primo passaggio: solo controlli, nessuna scrittura.
    let mut entries = Vec::with_capacity(archive.len());
    let mut folders: Vec<String> = Vec::new();
    let mut folder_keys: HashSet<String> = HashSet::new();
    let mut declared: u64 = 0;
    for index in 0..archive.len() {
        let file = archive
            .by_index_raw(index)
            .map_err(|_| "Lo zip è danneggiato.".to_string())?;
        let name = String::from_utf8_lossy(file.name_raw()).into_owned();
        if file.is_symlink() {
            return Err(format!("Lo zip contiene un link simbolico ({name})."));
        }
        if file.encrypted() {
            return Err("Lo zip contiene file cifrati.".to_string());
        }
        let components = entry_components(&name)?;
        if file.enclosed_name().is_none() {
            return Err(format!("Lo zip contiene un percorso non sicuro ({name})."));
        }
        let is_dir = file.is_dir();
        if components.len() == 1 && !is_dir {
            return Err(format!(
                "Lo zip contiene un file fuori da una cartella ({name}): ogni addon deve stare in una cartella."
            ));
        }
        declared = declared.saturating_add(file.size());
        if declared > limits.total_bytes {
            return Err(too_big(limits));
        }
        let folder = &components[0];
        if folder_keys.insert(folder.to_lowercase()) {
            folders.push(folder.clone());
        } else if !folders.contains(folder) {
            return Err(format!(
                "Lo zip contiene cartelle che differiscono solo per maiuscole e minuscole ({folder})."
            ));
        }
        entries.push(Entry {
            index,
            components,
            is_dir,
        });
    }
    if folders.is_empty() {
        return Err("Lo zip è vuoto.".to_string());
    }

    // Secondo passaggio: scrittura, contando i byte reali (la dimensione dichiarata può mentire).
    let mut remaining = limits.total_bytes;
    for entry in entries {
        let target = entry
            .components
            .iter()
            .fold(staging_dir.to_path_buf(), |path, part| path.join(part));
        if !target.starts_with(staging_dir) {
            return Err("Lo zip contiene un percorso non sicuro.".to_string());
        }
        if entry.is_dir {
            fs::create_dir_all(&target).map_err(write_error)?;
            continue;
        }
        if let Some(dir) = target.parent() {
            fs::create_dir_all(dir).map_err(write_error)?;
        }
        let file = archive
            .by_index(entry.index)
            .map_err(|_| "Lo zip è danneggiato.".to_string())?;
        let mut out = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target)
            .map_err(|error| match error.kind() {
                io::ErrorKind::AlreadyExists => format!(
                    "Lo zip contiene due volte lo stesso file ({}).",
                    entry.components.join("/")
                ),
                _ => write_error(error),
            })?;
        let written =
            io::copy(&mut file.take(remaining.saturating_add(1)), &mut out).map_err(|error| {
                match error.kind() {
                    io::ErrorKind::InvalidData => "Lo zip è danneggiato.".to_string(),
                    _ => write_error(error),
                }
            })?;
        if written > remaining {
            return Err(too_big(limits));
        }
        remaining -= written;
        out.flush().map_err(write_error)?;
    }
    Ok(folders)
}

fn too_big(limits: Limits) -> String {
    format!(
        "Lo zip estratto supera il limite di {} MiB.",
        limits.total_bytes / (1 << 20)
    )
}

fn write_error(error: io::Error) -> String {
    format!("Impossibile estrarre lo zip ({}).", error.kind())
}

/// Divide il nome di una voce in componenti sicuri. Accetta `/` e `\` come separatori (gli zip
/// creati su Windows a volte usano il secondo) e ignora i componenti vuoti e `.`.
fn entry_components(name: &str) -> Result<Vec<String>, String> {
    let unsafe_path = || Err(format!("Lo zip contiene un percorso non sicuro ({name})."));
    if name.contains('\0') || name.starts_with('/') || name.starts_with('\\') {
        return unsafe_path();
    }
    let mut components = Vec::new();
    for part in name.split(['/', '\\']) {
        match part {
            "" | "." => continue,
            ".." => return unsafe_path(),
            // `C:` è un prefisso di unità; altrove `:` apre uno stream alternativo su NTFS.
            _ if part.contains(':') => return unsafe_path(),
            _ => components.push(part.to_string()),
        }
    }
    if components.is_empty() {
        return unsafe_path();
    }
    Ok(components)
}

/// Sostituisce in `addons_dir` le cartelle estratte in `staging_dir`.
///
/// Sposta da parte, in una cartella di appoggio accanto ad `AddOns`, le cartelle esistenti con
/// lo stesso nome di quelle nuove (senza distinguere maiuscole e minuscole) e quelle elencate in
/// `remove`; poi sposta le nuove con `rename` e solo alla fine cancella l'appoggio. Se un passo
/// fallisce rimette tutto com'era e restituisce l'errore.
pub fn replace(addons_dir: &Path, staging_dir: &Path, remove: &[String]) -> Result<(), String> {
    replace_with(addons_dir, staging_dir, remove, |from, to| {
        fs::rename(from, to)
    })
}

fn replace_with(
    addons_dir: &Path,
    staging_dir: &Path,
    remove: &[String],
    mut rename: impl FnMut(&Path, &Path) -> io::Result<()>,
) -> Result<(), String> {
    for name in remove {
        if entry_components(name).map(|parts| parts.len()) != Ok(1) || name.contains(['/', '\\']) {
            return Err(format!(
                "Nome di cartella da rimuovere non valido ({name})."
            ));
        }
    }

    let new_folders = folder_names(staging_dir)?;
    if new_folders.is_empty() {
        return Err("Non c'è niente da installare.".to_string());
    }
    fs::create_dir_all(addons_dir)
        .map_err(|error| format!("Impossibile creare la cartella AddOns ({}).", error.kind()))?;

    let wanted: HashSet<String> = new_folders
        .iter()
        .chain(remove)
        .map(|name| name.to_lowercase())
        .collect();
    let set_aside: Vec<String> = folder_names(addons_dir)?
        .into_iter()
        .filter(|name| wanted.contains(&name.to_lowercase()))
        .collect();

    let backup = WorkDir::backup(addons_dir)?;
    // Ogni spostamento riuscito, in ordine, per poterlo annullare.
    let mut done: Vec<(PathBuf, PathBuf)> = Vec::new();

    let moves = set_aside
        .iter()
        .map(|name| (addons_dir.join(name), backup.path().join(name)))
        .chain(
            new_folders
                .iter()
                .map(|name| (staging_dir.join(name), addons_dir.join(name))),
        );
    for (from, to) in moves {
        if let Err(error) = rename(&from, &to) {
            // Si prova a ripristinare ogni spostamento, anche dopo un ripristino fallito.
            let restored = done
                .iter()
                .rev()
                .fold(true, |ok, (from, to)| rename(to, from).is_ok() && ok);
            let mut message = rename_error(&error);
            if !restored {
                // L'appoggio resta dov'è: contiene le cartelle originali non ripristinate.
                let kept = backup.path().to_path_buf();
                std::mem::forget(backup);
                message.push_str(&format!(
                    " Non è stato possibile ripristinare tutte le cartelle originali: si trovano in {}.",
                    kept.display()
                ));
            }
            return Err(message);
        }
        done.push((from, to));
    }
    drop(backup);
    Ok(())
}

/// Nomi delle voci di una cartella (cartelle e file), in ordine alfabetico.
fn folder_names(dir: &Path) -> Result<Vec<String>, String> {
    let read_error =
        |error: io::Error| format!("Impossibile leggere {} ({}).", dir.display(), error.kind());
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(read_error(error)),
    };
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry.map_err(read_error)?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| format!("Nome non leggibile in {}.", dir.display()))?;
        names.push(name);
    }
    names.sort();
    Ok(names)
}

/// Messaggio per un `rename` fallito. Su Windows un file aperto dal gioco blocca lo spostamento:
/// in quel caso si chiede di chiudere WoW.
fn rename_error(error: &io::Error) -> String {
    if is_locked(error) {
        "Alcuni file dell'addon sono in uso: chiudi World of Warcraft e riprova.".to_string()
    } else {
        format!(
            "Impossibile spostare le cartelle degli addon ({}).",
            error.kind()
        )
    }
}

#[cfg(target_os = "windows")]
fn is_locked(error: &io::Error) -> bool {
    // ERROR_ACCESS_DENIED, ERROR_SHARING_VIOLATION, ERROR_LOCK_VIOLATION.
    matches!(error.raw_os_error(), Some(5 | 32 | 33))
}

/// Su Linux il gioco gira sotto Wine, che non blocca i file aperti: un `rename` non fallisce
/// perché WoW è aperto.
#[cfg(not(target_os = "windows"))]
fn is_locked(_error: &io::Error) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;
    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;

    enum Item<'a> {
        File(&'a str, &'a [u8]),
        Dir(&'a str),
        Symlink(&'a str, &'a str),
    }
    use Item::{Dir, File, Symlink};

    fn zip_of(items: &[Item]) -> Vec<u8> {
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        let options = SimpleFileOptions::default();
        for item in items {
            match item {
                File(name, data) => {
                    writer.start_file(*name, options).unwrap();
                    writer.write_all(data).unwrap();
                }
                Dir(name) => writer.add_directory(*name, options).unwrap(),
                Symlink(name, target) => writer.add_symlink(*name, *target, options).unwrap(),
            }
        }
        writer.finish().unwrap().into_inner()
    }

    /// `Interface/AddOns` dentro una cartella temporanea, con spazi nel percorso come nei
    /// prefissi Wine.
    fn addons_dir(tmp: &TempDir) -> PathBuf {
        let dir = tmp
            .path()
            .join("Program Files (x86)")
            .join("World of Warcraft")
            .join("_retail_")
            .join("Interface")
            .join("AddOns");
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn read(path: &Path) -> String {
        fs::read_to_string(path).unwrap()
    }

    fn listing(dir: &Path) -> Vec<String> {
        folder_names(dir).unwrap()
    }

    /// Le uniche voci accanto ad `AddOns`: nessuna cartella di lavoro rimasta.
    fn interface_listing(addons: &Path) -> Vec<String> {
        listing(addons.parent().unwrap())
    }

    fn extract_err(bytes: &[u8]) -> (String, TempDir) {
        let tmp = TempDir::new();
        let addons = addons_dir(&tmp);
        let staging = WorkDir::staging(&addons).unwrap();
        let error = extract(bytes, staging.path()).unwrap_err();
        assert!(
            listing(staging.path()).is_empty(),
            "nothing may be written for a rejected zip"
        );
        drop(staging);
        (error, tmp)
    }

    #[test]
    fn extracts_a_single_addon() {
        let tmp = TempDir::new();
        let addons = addons_dir(&tmp);
        let staging = WorkDir::staging(&addons).unwrap();
        assert_eq!(staging.path().parent(), addons.parent());
        let bytes = zip_of(&[
            Dir("Questie/"),
            File("Questie/Questie.toc", b"## Title: Questie\n"),
            File("Questie/Modules/Core.lua", b"local x = 1\n"),
        ]);
        let folders = extract(&bytes, staging.path()).unwrap();
        assert_eq!(folders, ["Questie"]);
        assert_eq!(
            read(&staging.path().join("Questie/Modules/Core.lua")),
            "local x = 1\n"
        );
    }

    #[test]
    fn extracts_several_folders_without_directory_entries() {
        let tmp = TempDir::new();
        let addons = addons_dir(&tmp);
        let staging = WorkDir::staging(&addons).unwrap();
        let bytes = zip_of(&[
            File("DBM-Core/DBM-Core.toc", b"core"),
            File("DBM-StatusBarTimers/DBM-StatusBarTimers.toc", b"bars"),
            File("DBM-Core/sounds/a.ogg", b"ogg"),
        ]);
        let folders = extract(&bytes, staging.path()).unwrap();
        assert_eq!(folders, ["DBM-Core", "DBM-StatusBarTimers"]);
        assert_eq!(read(&staging.path().join("DBM-Core/sounds/a.ogg")), "ogg");
    }

    #[test]
    fn accepts_backslash_separators() {
        let tmp = TempDir::new();
        let addons = addons_dir(&tmp);
        let staging = WorkDir::staging(&addons).unwrap();
        let bytes = zip_of(&[File("Bagnon\\Bagnon.toc", b"toc")]);
        assert_eq!(extract(&bytes, staging.path()).unwrap(), ["Bagnon"]);
        assert_eq!(
            read(&staging.path().join("Bagnon").join("Bagnon.toc")),
            "toc"
        );
    }

    #[test]
    fn rejects_parent_components() {
        for name in [
            "../evil/x.lua",
            "Addon/../../evil.lua",
            "Addon/../Other/x.lua",
            "..\\evil\\x.lua",
        ] {
            let bytes = zip_of(&[File("Good/Good.toc", b"ok"), File(name, b"bad")]);
            let (error, tmp) = extract_err(&bytes);
            assert!(error.contains("non sicuro"), "{name}: {error}");
            assert!(!tmp.path().join("evil").exists());
            assert!(
                !tmp.path()
                    .join("Program Files (x86)/World of Warcraft/_retail_/evil")
                    .exists()
            );
        }
    }

    #[test]
    fn rejects_absolute_and_drive_paths() {
        for name in [
            "/evil/x.lua",
            "\\evil\\x.lua",
            "C:/evil/x.lua",
            "C:evil/x.lua",
            "Addon/file:stream",
        ] {
            let bytes = zip_of(&[File(name, b"bad")]);
            let (error, _tmp) = extract_err(&bytes);
            assert!(error.contains("non sicuro"), "{name}: {error}");
        }
    }

    #[test]
    fn rejects_symlinks() {
        let bytes = zip_of(&[
            File("Addon/Addon.toc", b"ok"),
            Symlink("Addon/link", "../../../../etc"),
        ]);
        let (error, _tmp) = extract_err(&bytes);
        assert!(error.contains("link simbolico"), "{error}");
    }

    #[test]
    fn rejects_a_loose_file_at_the_root() {
        let bytes = zip_of(&[File("Addon/Addon.toc", b"ok"), File("README.txt", b"hi")]);
        let (error, _tmp) = extract_err(&bytes);
        assert!(error.contains("fuori da una cartella"), "{error}");
    }

    #[test]
    fn rejects_an_empty_zip_and_garbage() {
        let (error, _tmp) = extract_err(&zip_of(&[]));
        assert!(error.contains("vuoto"), "{error}");
        let (error, _tmp) = extract_err(b"not a zip at all");
        assert!(error.contains("non è uno zip"), "{error}");
    }

    #[test]
    fn rejects_folders_that_differ_only_by_case() {
        let bytes = zip_of(&[File("Addon/a.lua", b"a"), File("addon/b.lua", b"b")]);
        let (error, _tmp) = extract_err(&bytes);
        assert!(error.contains("maiuscole"), "{error}");
    }

    #[test]
    fn enforces_the_entry_limit() {
        let tmp = TempDir::new();
        let addons = addons_dir(&tmp);
        let staging = WorkDir::staging(&addons).unwrap();
        let bytes = zip_of(&[
            File("A/1.lua", b"1"),
            File("A/2.lua", b"2"),
            File("A/3.lua", b"3"),
        ]);
        let limits = Limits {
            entries: 2,
            total_bytes: MAX_TOTAL_BYTES,
        };
        let error = extract_with_limits(&bytes, staging.path(), limits).unwrap_err();
        assert!(error.contains("troppe voci"), "{error}");
        assert!(listing(staging.path()).is_empty());
    }

    #[test]
    fn enforces_the_size_limit() {
        let tmp = TempDir::new();
        let addons = addons_dir(&tmp);
        let staging = WorkDir::staging(&addons).unwrap();
        let bytes = zip_of(&[File("A/1.lua", &[b'x'; 600]), File("A/2.lua", &[b'y'; 600])]);
        let limits = Limits {
            entries: MAX_ENTRIES,
            total_bytes: 1000,
        };
        let error = extract_with_limits(&bytes, staging.path(), limits).unwrap_err();
        assert!(error.contains("supera il limite"), "{error}");
        assert!(listing(staging.path()).is_empty());
    }

    #[test]
    fn refuses_a_staging_dir_that_is_not_empty() {
        let tmp = TempDir::new();
        let addons = addons_dir(&tmp);
        let staging = WorkDir::staging(&addons).unwrap();
        write(&staging.path().join("Old/x.lua"), "x");
        let error = extract(&zip_of(&[File("A/a.lua", b"a")]), staging.path()).unwrap_err();
        assert!(error.contains("non è vuota"), "{error}");
    }

    #[test]
    fn work_dirs_are_removed_when_dropped() {
        let tmp = TempDir::new();
        let addons = addons_dir(&tmp);
        let staging = WorkDir::staging(&addons).unwrap();
        extract(&zip_of(&[File("A/a.lua", b"a")]), staging.path()).unwrap();
        assert_eq!(interface_listing(&addons).len(), 2);
        drop(staging);
        assert_eq!(interface_listing(&addons), ["AddOns"]);
    }

    /// Estrae `items` in uno staging nuovo accanto ad `addons`.
    fn staged(addons: &Path, items: &[Item]) -> WorkDir {
        let staging = WorkDir::staging(addons).unwrap();
        extract(&zip_of(items), staging.path()).unwrap();
        staging
    }

    #[test]
    fn replaces_existing_folders_and_removes_obsolete_ones() {
        let tmp = TempDir::new();
        let addons = addons_dir(&tmp);
        write(&addons.join("DBM-Core/old.lua"), "old");
        write(&addons.join("DBM-Obsolete/x.lua"), "x");
        write(&addons.join("Other/keep.lua"), "keep");
        let staging = staged(
            &addons,
            &[
                File("DBM-Core/new.lua", b"new"),
                File("DBM-StatusBarTimers/b.lua", b"b"),
            ],
        );

        replace(&addons, staging.path(), &["DBM-Obsolete".to_string()]).unwrap();
        drop(staging);

        assert_eq!(
            listing(&addons),
            ["DBM-Core", "DBM-StatusBarTimers", "Other"]
        );
        assert_eq!(listing(&addons.join("DBM-Core")), ["new.lua"]);
        assert_eq!(read(&addons.join("Other/keep.lua")), "keep");
        assert_eq!(interface_listing(&addons), ["AddOns"]);
    }

    #[test]
    fn replaces_folders_regardless_of_case() {
        let tmp = TempDir::new();
        let addons = addons_dir(&tmp);
        write(&addons.join("dbm-core/old.lua"), "old");
        write(&addons.join("bagnon/x.lua"), "x");
        let staging = staged(&addons, &[File("DBM-Core/new.lua", b"new")]);

        replace(&addons, staging.path(), &["BAGNON".to_string()]).unwrap();

        assert_eq!(listing(&addons), ["DBM-Core"]);
        assert_eq!(read(&addons.join("DBM-Core/new.lua")), "new");
    }

    #[test]
    fn creates_addons_when_missing() {
        let tmp = TempDir::new();
        let addons = addons_dir(&tmp);
        fs::remove_dir(&addons).unwrap();
        let staging = staged(&addons, &[File("Questie/q.lua", b"q")]);
        replace(&addons, staging.path(), &[]).unwrap();
        assert_eq!(listing(&addons), ["Questie"]);
    }

    #[test]
    fn restores_the_original_folders_when_a_move_fails() {
        let tmp = TempDir::new();
        let addons = addons_dir(&tmp);
        write(&addons.join("DBM-Core/old.lua"), "old");
        write(&addons.join("DBM-Obsolete/x.lua"), "x");
        write(&addons.join("Other/keep.lua"), "keep");
        let staging = staged(
            &addons,
            &[
                File("DBM-Core/new.lua", b"new"),
                File("DBM-StatusBarTimers/b.lua", b"b"),
            ],
        );

        // Spostamenti: 2 da parte, poi DBM-Core installato; il quarto (DBM-StatusBarTimers)
        // fallisce.
        for fail_at in 0..4 {
            let mut calls = 0;
            let error = replace_with(
                &addons,
                staging.path(),
                &["DBM-Obsolete".to_string()],
                |from, to| {
                    calls += 1;
                    if calls == fail_at + 1 {
                        Err(io::Error::from(io::ErrorKind::PermissionDenied))
                    } else {
                        fs::rename(from, to)
                    }
                },
            )
            .unwrap_err();
            assert!(error.contains("Impossibile spostare"), "{error}");
            assert_eq!(listing(&addons), ["DBM-Core", "DBM-Obsolete", "Other"]);
            assert_eq!(read(&addons.join("DBM-Core/old.lua")), "old");
            assert_eq!(read(&addons.join("DBM-Obsolete/x.lua")), "x");
            assert_eq!(listing(staging.path()), ["DBM-Core", "DBM-StatusBarTimers"]);
            // Solo AddOns e lo staging: l'appoggio è stato cancellato.
            assert_eq!(interface_listing(&addons).len(), 2, "attempt {fail_at}");
        }
    }

    #[test]
    fn keeps_the_backup_when_restoring_fails() {
        let tmp = TempDir::new();
        let addons = addons_dir(&tmp);
        write(&addons.join("A/old.lua"), "old");
        let staging = staged(&addons, &[File("A/new.lua", b"new"), File("B/b.lua", b"b")]);
        let mut calls = 0;
        let error = replace_with(&addons, staging.path(), &[], |from, to| {
            calls += 1;
            if calls >= 3 {
                Err(io::Error::from(io::ErrorKind::PermissionDenied))
            } else {
                fs::rename(from, to)
            }
        })
        .unwrap_err();
        assert!(error.contains("ripristinare"), "{error}");
        let backup = interface_listing(&addons)
            .into_iter()
            .find(|name| name.starts_with(".wow-don-backup-"))
            .expect("backup kept");
        let backup = addons.parent().unwrap().join(backup);
        assert_eq!(read(&backup.join("A/old.lua")), "old");
        fs::remove_dir_all(backup).unwrap();
    }

    #[test]
    fn rejects_invalid_names_to_remove() {
        let tmp = TempDir::new();
        let addons = addons_dir(&tmp);
        write(&addons.join("Keep/x.lua"), "x");
        let staging = staged(&addons, &[File("A/a.lua", b"a")]);
        for name in ["..", "", "../Keep", "Keep/x.lua", "Keep\\x.lua", "C:", "."] {
            let error = replace(&addons, staging.path(), &[name.to_string()]).unwrap_err();
            assert!(error.contains("non valido"), "{name}: {error}");
        }
        assert_eq!(listing(&addons), ["Keep"]);
    }

    #[test]
    fn an_empty_staging_is_an_error() {
        let tmp = TempDir::new();
        let addons = addons_dir(&tmp);
        let staging = WorkDir::staging(&addons).unwrap();
        let error = replace(&addons, staging.path(), &[]).unwrap_err();
        assert!(error.contains("niente da installare"), "{error}");
    }

    #[test]
    fn a_generic_rename_error_does_not_blame_the_game() {
        let error = rename_error(&io::Error::from(io::ErrorKind::NotFound));
        assert!(!error.contains("World of Warcraft"), "{error}");
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn a_locked_file_asks_to_close_the_game() {
        for code in [5, 32, 33] {
            let error = rename_error(&io::Error::from_raw_os_error(code));
            assert!(
                error.contains("chiudi World of Warcraft"),
                "{code}: {error}"
            );
        }
    }
}
