//! Installation of an addon zip: safe extraction into a staging folder and replacement of the
//! folders in `Interface/AddOns` without leaving it half-done.
//!
//! Path functions only: no network, no Tauri. Staging and scratch are hidden folders in the
//! parent folder of `AddOns` ([`WorkDir`]), so every `rename` stays on the same disk; they
//! delete themselves even after an error.
//!
//! The callers (`updates::update_addons` and `install_new::install_addon`) do:
//! `WorkDir::staging(addons)` → [`extract`] → [`replace`].
//!
//! The automatic round (D41, issue #201) keeps the previous version: inside
//! [`keeping_replaced`] the folders that [`replace`] moves aside end up in a generation folder
//! instead of being deleted, and [`take_kept`] says which ones. Outside that scope (every
//! manual command) nothing changes.

use std::cell::RefCell;
use std::collections::HashSet;
use std::fs;
use std::io::{self, Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use zip::ZipArchive;

/// Maximum number of entries in a zip.
pub const MAX_ENTRIES: usize = 20_000;
/// Maximum extracted size, summing all files: 1 GiB.
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

/// Hidden work folder next to `AddOns` (same parent, hence same disk).
/// Deletes itself when it goes out of scope, even after an error.
#[derive(Debug)]
pub struct WorkDir(PathBuf);

impl WorkDir {
    /// Folder to extract the zip into.
    pub fn staging(addons_dir: &Path) -> Result<Self, String> {
        Self::create(addons_dir, "staging")
    }

    /// Folder to move replaced or removed folders aside into.
    fn backup(addons_dir: &Path) -> Result<Self, String> {
        Self::create(addons_dir, "backup")
    }

    /// A `.wow-don-<kind>-*` work folder next to `AddOns`, for callers that move folders on
    /// the same disk outside this module (the automatic round's generation,
    /// `previous`).
    pub(crate) fn create(addons_dir: &Path, kind: &str) -> Result<Self, String> {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let parent = addons_dir
            .parent()
            .ok_or_else(|| "The AddOns folder has no parent folder.".to_string())?;
        let name = format!(
            ".wow-don-{kind}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let path = parent.join(name);
        // A leftover from an interrupted run with the same pid must not get mixed in.
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).map_err(|error| {
            format!(
                "Could not create the temporary install folder ({}).",
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

/// Extracts `zip_bytes` into `staging_dir`, which must exist and be empty, and returns the names
/// of the top-level folders (the addons), in the order they appear.
///
/// The whole zip is checked before the first file is written: an entry with an absolute path,
/// a `..` component, a drive prefix, a symbolic link, a loose file at the root or an exceeded
/// limit makes the whole zip be rejected. No file is ever written outside
/// `staging_dir`.
pub fn extract(zip_bytes: &[u8], staging_dir: &Path) -> Result<Vec<String>, String> {
    extract_with_limits(zip_bytes, staging_dir, DEFAULT_LIMITS)
}

/// A zip entry that has already been validated.
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
        .map_err(|error| format!("Could not read the staging folder ({}).", error.kind()))?
        .next()
        .is_some();
    if not_empty {
        return Err("The staging folder is not empty.".to_string());
    }

    let mut archive = ZipArchive::new(Cursor::new(zip_bytes))
        .map_err(|_| "The downloaded file is not a valid zip.".to_string())?;
    if archive.len() > limits.entries {
        return Err(format!(
            "The zip has too many entries ({}, maximum {}).",
            archive.len(),
            limits.entries
        ));
    }

    // First pass: checks only, no writing.
    let mut entries = Vec::with_capacity(archive.len());
    let mut folders: Vec<String> = Vec::new();
    let mut folder_keys: HashSet<String> = HashSet::new();
    let mut declared: u64 = 0;
    for index in 0..archive.len() {
        let file = archive
            .by_index_raw(index)
            .map_err(|_| "The zip is damaged.".to_string())?;
        // `name()` decodes according to the entry's flag (UTF-8 or CP437 for old zips), so
        // accented names stay those of the archive.
        let name = file
            .name()
            .map_err(|_| "The zip is damaged.".to_string())?
            .into_owned();
        if file.is_symlink() {
            return Err(format!("The zip contains a symbolic link ({name})."));
        }
        if file.encrypted() {
            return Err("The zip contains encrypted files.".to_string());
        }
        let components = entry_components(&name)?;
        if file.enclosed_name().is_none() {
            return Err(format!("The zip contains an unsafe path ({name})."));
        }
        let is_dir = file.is_dir();
        if components.len() == 1 && !is_dir {
            return Err(format!(
                "The zip contains a file outside a folder ({name}): every addon must be in a folder."
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
                "The zip contains folders that differ only in letter case ({folder})."
            ));
        }
        entries.push(Entry {
            index,
            components,
            is_dir,
        });
    }
    if folders.is_empty() {
        return Err("The zip is empty.".to_string());
    }

    // Second pass: writing, counting the real bytes (the declared size can lie).
    let mut remaining = limits.total_bytes;
    for entry in entries {
        let target = entry
            .components
            .iter()
            .fold(staging_dir.to_path_buf(), |path, part| path.join(part));
        if !target.starts_with(staging_dir) {
            return Err("The zip contains an unsafe path.".to_string());
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
            .map_err(|_| "The zip is damaged.".to_string())?;
        let mut out = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target)
            .map_err(|error| match error.kind() {
                io::ErrorKind::AlreadyExists => format!(
                    "The zip contains the same file twice ({}).",
                    entry.components.join("/")
                ),
                _ => write_error(error),
            })?;
        let written =
            io::copy(&mut file.take(remaining.saturating_add(1)), &mut out).map_err(|error| {
                match error.kind() {
                    io::ErrorKind::InvalidData => "The zip is damaged.".to_string(),
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
        "The extracted zip exceeds the {} MiB limit.",
        limits.total_bytes / (1 << 20)
    )
}

fn write_error(error: io::Error) -> String {
    format!("Could not extract the zip ({}).", error.kind())
}

/// Splits an entry name into safe components. Accepts `/` and `\` as separators (zips created
/// on Windows sometimes use the latter) and ignores empty and `.` components. Rejects `..`,
/// components with a trailing dot or space, and those with `:`.
fn entry_components(name: &str) -> Result<Vec<String>, String> {
    let unsafe_path = || Err(format!("The zip contains an unsafe path ({name})."));
    if name.contains('\0') || name.starts_with('/') || name.starts_with('\\') {
        return unsafe_path();
    }
    let mut components = Vec::new();
    for part in name.split(['/', '\\']) {
        match part {
            "" | "." => continue,
            ".." => return unsafe_path(),
            // Windows strips trailing dots and spaces from every component: `.. ` would become
            // `..` and climb out of the staging folder.
            _ if part.ends_with(['.', ' ']) => return unsafe_path(),
            // `C:` is a drive prefix; elsewhere `:` opens an alternate data stream on NTFS.
            _ if part.contains(':') => return unsafe_path(),
            _ => components.push(part.to_string()),
        }
    }
    if components.is_empty() {
        return unsafe_path();
    }
    Ok(components)
}

/// Replaces in `addons_dir` the folders extracted into `staging_dir`.
///
/// Moves aside, into a scratch folder next to `AddOns`, the existing folders with the same name
/// as the new ones (case-insensitively) and those listed in `remove`; then moves the new ones
/// in with `rename` and only at the end deletes the scratch folder. If a step fails it puts
/// everything back as it was and returns the error.
pub fn replace(addons_dir: &Path, staging_dir: &Path, remove: &[String]) -> Result<(), String> {
    replace_with(addons_dir, staging_dir, remove, |from, to| {
        fs::rename(from, to)
    })
}

pub(crate) fn replace_with(
    addons_dir: &Path,
    staging_dir: &Path,
    remove: &[String],
    mut rename: impl FnMut(&Path, &Path) -> io::Result<()>,
) -> Result<(), String> {
    for name in remove {
        if entry_components(name).map(|parts| parts.len()) != Ok(1) || name.contains(['/', '\\']) {
            return Err(format!("Invalid name for a folder to remove ({name})."));
        }
    }

    let new_folders = folder_names(staging_dir)?;
    if new_folders.is_empty() {
        return Err("There is nothing to install.".to_string());
    }
    fs::create_dir_all(addons_dir)
        .map_err(|error| format!("Could not create the AddOns folder ({}).", error.kind()))?;

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
    // Every successful move, in order, so it can be undone.
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
            // Try to restore every move, even after a failed restore.
            let restored = done
                .iter()
                .rev()
                .fold(true, |ok, (from, to)| rename(to, from).is_ok() && ok);
            let mut message = rename_error(&error);
            if !restored {
                // The scratch folder stays where it is: it holds the original folders not restored.
                let kept = backup.path().to_path_buf();
                std::mem::forget(backup);
                message.push_str(&format!(
                    " Not all original folders could be restored: they are in {}.",
                    kept.display()
                ));
            }
            return Err(message);
        }
        done.push((from, to));
    }
    keep_aside(backup.path(), &set_aside, &mut rename);
    drop(backup);
    Ok(())
}

// ── Previous version from the automatic round (issue #201) ──────────────────

thread_local! {
    /// Active only inside [`keeping_replaced`], on the thread that calls it: the automatic round
    /// updates projects one after the other on that thread, and no manual command sees it.
    static KEEPER: RefCell<Option<Keeper>> = const { RefCell::new(None) };
}

struct Keeper {
    /// The folder of the generation being prepared.
    root: PathBuf,
    next_slot: usize,
    /// What the `replace` calls kept since the last [`take_kept`].
    current: Option<Kept>,
}

/// The replaced folders that a project's [`replace`] calls kept, in
/// `<generation>/<slot>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Kept {
    /// The name of the generation subfolder, a number (`0`, `1`, …).
    pub slot: String,
    /// The kept folders, with the name they had in `AddOns`.
    pub folders: Vec<String>,
    /// False if a folder could not be kept (and was deleted as today): the previous version is
    /// not complete and cannot be put back.
    pub complete: bool,
}

/// Runs `run` with this thread's [`replace`] calls keeping the replaced folders in a subfolder
/// of `root` instead of deleting them. `root` must exist, on the same disk as `AddOns`. At the
/// end (even after a panic) the `replace` calls go back to deleting.
pub fn keeping_replaced<R>(root: &Path, run: impl FnOnce() -> R) -> R {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            KEEPER.with(|keeper| keeper.borrow_mut().take());
        }
    }
    KEEPER.with(|keeper| {
        *keeper.borrow_mut() = Some(Keeper {
            root: root.to_path_buf(),
            next_slot: 0,
            current: None,
        });
    });
    let _reset = Reset;
    run()
}

/// What the [`replace`] calls kept since the last call, and starts over: the next `replace`
/// call uses a new slot. `None` outside [`keeping_replaced`] or if no `replace`
/// succeeded.
pub fn take_kept() -> Option<Kept> {
    KEEPER.with(|keeper| keeper.borrow_mut().as_mut().and_then(|k| k.current.take()))
}

/// After a successful [`replace`], moves the `names` folders from `backup` to the project's
/// slot, if [`keeping_replaced`] is active. With the slot still to be created a `rename` of the
/// whole scratch folder is enough; a second `replace` of the same project adds its folders,
/// without overwriting those already kept (the oldest version stays).
fn keep_aside(
    backup: &Path,
    names: &[String],
    rename: &mut impl FnMut(&Path, &Path) -> io::Result<()>,
) {
    KEEPER.with(|keeper| {
        let mut keeper = keeper.borrow_mut();
        let Some(Keeper {
            root,
            next_slot,
            current,
        }) = keeper.as_mut()
        else {
            return;
        };
        let kept = current.get_or_insert_with(|| {
            let slot = next_slot.to_string();
            *next_slot += 1;
            Kept {
                slot,
                folders: Vec::new(),
                complete: true,
            }
        });
        if names.is_empty() {
            return;
        }
        let slot = root.join(&kept.slot);
        if fs::symlink_metadata(&slot).is_err() {
            match rename(backup, &slot) {
                Ok(()) => kept.folders.extend(names.iter().cloned()),
                Err(_) => kept.complete = false,
            }
            return;
        }
        for name in names {
            let lower = name.to_lowercase();
            if kept.folders.iter().any(|f| f.to_lowercase() == lower) {
                continue;
            }
            match rename(&backup.join(name), &slot.join(name)) {
                Ok(()) => kept.folders.push(name.clone()),
                Err(_) => kept.complete = false,
            }
        }
    });
}

/// Deletes the folder `name` of `addons_dir` (the import with "Replace all", issue #134). `name`
/// must be a single valid component, with the same checks as [`replace`]: no separators, `..`,
/// empty or reserved names. A symbolic link is removed without following it, so nothing
/// outside `AddOns` is touched; a file that is not a folder stays where it is.
pub fn remove_folder(addons_dir: &Path, name: &str) -> Result<(), String> {
    if entry_components(name).map(|parts| parts.len()) != Ok(1) || name.contains(['/', '\\']) {
        return Err(format!("Invalid name for a folder to remove ({name})."));
    }
    let path = addons_dir.join(name);
    let error = |error: io::Error| format!("Could not remove {name} ({}).", error.kind());
    let meta = fs::symlink_metadata(&path).map_err(error)?;
    if meta.file_type().is_symlink() {
        // On Windows a link to a folder is removed as a folder, elsewhere as a file.
        return fs::remove_file(&path)
            .or_else(|_| fs::remove_dir(&path))
            .map_err(error);
    }
    if !meta.is_dir() {
        return Err(format!("{name} is not a folder: it was left in place."));
    }
    fs::remove_dir_all(&path).map_err(|e| {
        if is_locked(&e) {
            format!("Could not remove {name}: close the game and try again.")
        } else {
            error(e)
        }
    })
}

/// Names of a folder's entries (folders and files), in alphabetical order.
pub(crate) fn folder_names(dir: &Path) -> Result<Vec<String>, String> {
    let read_error =
        |error: io::Error| format!("Could not read {} ({}).", dir.display(), error.kind());
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
            .map_err(|_| format!("Unreadable name in {}.", dir.display()))?;
        names.push(name);
    }
    names.sort();
    Ok(names)
}

/// Message for a failed `rename`. On Windows a file opened by the game blocks the move: in that
/// case the user is asked to close WoW. On Linux and macOS it does not happen (see `is_locked`).
fn rename_error(error: &io::Error) -> String {
    if is_locked(error) {
        "Some addon files are in use: close World of Warcraft and try again.".to_string()
    } else {
        format!("Could not move the addon folders ({}).", error.kind())
    }
}

#[cfg(target_os = "windows")]
fn is_locked(error: &io::Error) -> bool {
    // ERROR_ACCESS_DENIED, ERROR_SHARING_VIOLATION, ERROR_LOCK_VIOLATION.
    matches!(error.raw_os_error(), Some(5 | 32 | 33))
}

/// On Linux the game runs under Wine and on macOS it is the native client (D29): in both cases
/// Unix rules apply, where an open file does not block the `rename` of the folder that
/// contains it. A `rename` does not fail because WoW is open, so no error is attributed to
/// the game.
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

    /// `Interface/AddOns` inside a temporary folder, with spaces in the path as in
    /// Wine prefixes.
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

    /// The only entries next to `AddOns`: no leftover work folder.
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
            assert!(error.contains("unsafe path"), "{name}: {error}");
            assert!(!tmp.path().join("evil").exists());
            assert!(
                !tmp.path()
                    .join("Program Files (x86)/World of Warcraft/_retail_/evil")
                    .exists()
            );
        }
    }

    #[test]
    fn rejects_absolute_drive_and_windows_trimmed_paths() {
        for name in [
            "/evil/x.lua",
            "\\evil\\x.lua",
            "C:/evil/x.lua",
            "C:evil/x.lua",
            "Addon/.. /evil.lua",
            "Addon/.. /.. /evil.lua",
            "Addon/... /evil.lua",
            "Addon/x.lua.",
            "Addon /x.lua",
            "Addon/file:stream",
        ] {
            let bytes = zip_of(&[File(name, b"bad")]);
            let (error, _tmp) = extract_err(&bytes);
            assert!(error.contains("unsafe path"), "{name}: {error}");
        }
    }

    #[test]
    fn decodes_legacy_cp437_names() {
        let mut bytes = zip_of(&[File("Addon/caf_.lua", b"x")]);
        // Without the UTF-8 flag the name is CP437: 0x82 is `é`. The name appears in the local
        // header and in the central directory; the CRC covers only the data.
        let mut patched = 0;
        for i in 0..bytes.len() - 3 {
            if &bytes[i..i + 4] == b"caf_" {
                bytes[i + 3] = 0x82;
                patched += 1;
            }
        }
        assert_eq!(patched, 2);
        let tmp = TempDir::new();
        let addons = addons_dir(&tmp);
        let staging = WorkDir::staging(&addons).unwrap();
        extract(&bytes, staging.path()).unwrap();
        assert_eq!(listing(&staging.path().join("Addon")), ["café.lua"]);
    }

    #[test]
    fn rejects_symlinks() {
        let bytes = zip_of(&[
            File("Addon/Addon.toc", b"ok"),
            Symlink("Addon/link", "../../../../etc"),
        ]);
        let (error, _tmp) = extract_err(&bytes);
        assert!(error.contains("symbolic link"), "{error}");
    }

    #[test]
    fn rejects_a_loose_file_at_the_root() {
        let bytes = zip_of(&[File("Addon/Addon.toc", b"ok"), File("README.txt", b"hi")]);
        let (error, _tmp) = extract_err(&bytes);
        assert!(error.contains("outside a folder"), "{error}");
    }

    #[test]
    fn rejects_an_empty_zip_and_garbage() {
        let (error, _tmp) = extract_err(&zip_of(&[]));
        assert!(error.contains("empty"), "{error}");
        let (error, _tmp) = extract_err(b"not a zip at all");
        assert!(error.contains("not a valid zip"), "{error}");
    }

    #[test]
    fn rejects_folders_that_differ_only_by_case() {
        let bytes = zip_of(&[File("Addon/a.lua", b"a"), File("addon/b.lua", b"b")]);
        let (error, _tmp) = extract_err(&bytes);
        assert!(error.contains("letter case"), "{error}");
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
        assert!(error.contains("too many entries"), "{error}");
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
        assert!(error.contains("exceeds the"), "{error}");
        assert!(listing(staging.path()).is_empty());
    }

    #[test]
    fn refuses_a_staging_dir_that_is_not_empty() {
        let tmp = TempDir::new();
        let addons = addons_dir(&tmp);
        let staging = WorkDir::staging(&addons).unwrap();
        write(&staging.path().join("Old/x.lua"), "x");
        let error = extract(&zip_of(&[File("A/a.lua", b"a")]), staging.path()).unwrap_err();
        assert!(error.contains("not empty"), "{error}");
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

    /// Extracts `items` into a new staging folder next to `addons`.
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

        // Moves: 2 aside, then DBM-Core installed; the fourth (DBM-StatusBarTimers)
        // fails.
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
            assert!(error.contains("Could not move"), "{error}");
            assert_eq!(listing(&addons), ["DBM-Core", "DBM-Obsolete", "Other"]);
            assert_eq!(read(&addons.join("DBM-Core/old.lua")), "old");
            assert_eq!(read(&addons.join("DBM-Obsolete/x.lua")), "x");
            assert_eq!(listing(staging.path()), ["DBM-Core", "DBM-StatusBarTimers"]);
            // Only AddOns and the staging folder: the scratch folder was deleted.
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
        assert!(error.contains("could be restored"), "{error}");
        let backup = interface_listing(&addons)
            .into_iter()
            .find(|name| name.starts_with(".wow-don-backup-"))
            .expect("backup kept");
        let backup = addons.parent().unwrap().join(backup);
        assert_eq!(read(&backup.join("A/old.lua")), "old");
        fs::remove_dir_all(backup).unwrap();
    }

    #[test]
    fn inside_keeping_replaced_the_replaced_folders_go_to_a_slot() {
        let tmp = TempDir::new();
        let addons = addons_dir(&tmp);
        let root = addons.parent().unwrap().join("generation");
        fs::create_dir_all(&root).unwrap();
        write(&addons.join("A/old.lua"), "old a");
        write(&addons.join("Obsolete/x.lua"), "x");
        write(&addons.join("B/old.lua"), "old b");
        let (first, second, none) = keeping_replaced(&root, || {
            let staging = staged(&addons, &[File("a/new.lua", b"new")]);
            replace(&addons, staging.path(), &["Obsolete".to_string()]).unwrap();
            // A second `replace` of the same project adds to the same slot, without
            // overwriting the version already kept.
            let staging = staged(&addons, &[File("A/newer.lua", b"newer")]);
            replace(&addons, staging.path(), &[]).unwrap();
            let first = take_kept();
            let staging = staged(&addons, &[File("B/new.lua", b"new")]);
            replace(&addons, staging.path(), &[]).unwrap();
            (first, take_kept(), take_kept())
        });
        let first = first.expect("kept");
        assert_eq!(first.slot, "0");
        assert_eq!(first.folders, ["A", "Obsolete"]);
        assert!(first.complete);
        assert_eq!(read(&root.join("0").join("A").join("old.lua")), "old a");
        assert_eq!(read(&root.join("0").join("Obsolete").join("x.lua")), "x");
        assert_eq!(listing(&root.join("0")), ["A", "Obsolete"]);
        let second = second.expect("kept");
        assert_eq!(
            (second.slot.as_str(), second.folders),
            ("1", vec!["B".to_string()])
        );
        assert_eq!(none, None);
        assert_eq!(listing(&addons), ["A", "B"]);
        // Outside the scope `replace` goes back to deleting.
        assert_eq!(take_kept(), None);
        let staging = staged(&addons, &[File("B/newest.lua", b"newest")]);
        replace(&addons, staging.path(), &[]).unwrap();
        drop(staging);
        assert_eq!(listing(&root), ["0", "1"]);
        assert_eq!(interface_listing(&addons), ["AddOns", "generation"]);
    }

    #[test]
    fn a_folder_that_cannot_be_kept_marks_the_slot_incomplete() {
        let tmp = TempDir::new();
        let addons = addons_dir(&tmp);
        let root = addons.parent().unwrap().join("generation");
        fs::create_dir_all(&root).unwrap();
        write(&addons.join("A/old.lua"), "old");
        let kept = keeping_replaced(&root, || {
            let staging = staged(&addons, &[File("A/new.lua", b"new")]);
            let mut calls = 0;
            replace_with(&addons, staging.path(), &[], |from, to| {
                calls += 1;
                // The third `rename` is the one into the slot.
                if calls == 3 {
                    Err(io::Error::from(io::ErrorKind::PermissionDenied))
                } else {
                    fs::rename(from, to)
                }
            })
            .unwrap();
            take_kept().unwrap()
        });
        assert!(!kept.complete);
        assert!(kept.folders.is_empty());
        assert_eq!(read(&addons.join("A/new.lua")), "new");
        // The scratch folder was deleted as in a manual update.
        assert_eq!(interface_listing(&addons), ["AddOns", "generation"]);
    }

    #[test]
    fn rejects_invalid_names_to_remove() {
        let tmp = TempDir::new();
        let addons = addons_dir(&tmp);
        write(&addons.join("Keep/x.lua"), "x");
        let staging = staged(&addons, &[File("A/a.lua", b"a")]);
        for name in ["..", "", "../Keep", "Keep/x.lua", "Keep\\x.lua", "C:", "."] {
            let error = replace(&addons, staging.path(), &[name.to_string()]).unwrap_err();
            assert!(error.contains("Invalid name"), "{name}: {error}");
        }
        assert_eq!(listing(&addons), ["Keep"]);
    }

    #[test]
    fn an_empty_staging_is_an_error() {
        let tmp = TempDir::new();
        let addons = addons_dir(&tmp);
        let staging = WorkDir::staging(&addons).unwrap();
        let error = replace(&addons, staging.path(), &[]).unwrap_err();
        assert!(error.contains("nothing to install"), "{error}");
    }

    #[test]
    fn remove_folder_deletes_only_a_valid_folder_inside_addons() {
        let tmp = TempDir::new();
        let addons = addons_dir(&tmp);
        write(&addons.join("Foo").join("sub").join("a.lua"), "x");
        write(&addons.join("loose.txt"), "x");
        remove_folder(&addons, "Foo").unwrap();
        assert!(!addons.join("Foo").exists());
        for name in ["", ".", "..", "../Foo", "a/b", "a\\b", "C:"] {
            assert!(remove_folder(&addons, name).is_err(), "{name:?}");
        }
        assert!(remove_folder(&addons, "loose.txt").is_err());
        assert!(addons.join("loose.txt").exists());
        assert!(remove_folder(&addons, "Missing").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn remove_folder_drops_a_link_without_following_it() {
        let tmp = TempDir::new();
        let addons = addons_dir(&tmp);
        let outside = tmp.path().join("outside");
        write(&outside.join("keep.lua"), "x");
        fs::create_dir_all(&addons).unwrap();
        std::os::unix::fs::symlink(&outside, addons.join("Linked")).unwrap();
        remove_folder(&addons, "Linked").unwrap();
        assert!(!addons.join("Linked").exists());
        assert!(outside.join("keep.lua").exists());
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
            assert!(error.contains("close World of Warcraft"), "{code}: {error}");
        }
    }
}
