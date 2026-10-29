//! In-memory cache of addon folder fingerprints (issue #125).
//!
//! The fingerprint reads an addon's files in full; the scan runs every time the tab opens, on
//! every check and on every update. A folder whose files still have the same paths, sizes and
//! modification times gives the same fingerprint: it is reused without reading them again.
//! The folder's own timestamp is not enough, because it does not change when a file inside a
//! subfolder changes.
//!
//! A file replaced by one with the same size and the same timestamp goes unnoticed: that is
//! why the app's writes drop their folders ([`FingerprintCache::forget`]) and "Reload addons"
//! recomputes everything ([`FingerprintCache::forget_all`]). Memory only: the cache is lost
//! when the app closes.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

use super::fingerprint::{self, FileStamp};

struct Entry {
    stamps: Vec<FileStamp>,
    fingerprint: Option<u32>,
}

/// Per addon folder (the path in `AddOns`), the files seen at the last computation and the
/// fingerprint that came out of it.
#[derive(Default)]
pub struct FingerprintCache {
    entries: Mutex<HashMap<PathBuf, Entry>>,
    /// How many fingerprints were actually computed: the tests check reuse this way, without
    /// depending on timing.
    #[cfg(test)]
    computed: std::sync::atomic::AtomicUsize,
}

impl FingerprintCache {
    /// The fingerprint of `addon_dir`, from the cache if the files have not changed. The lock is
    /// not held while the files are read: compare, compute outside it and write afterwards. A
    /// file changed during the computation has a newer timestamp than the stored one, so the
    /// next scan recomputes it.
    pub fn fingerprint(&self, addon_dir: &Path) -> Option<u32> {
        let files = fingerprint::folder_files(addon_dir)?;
        if let Some(entry) = self.lock().get(addon_dir)
            && entry.stamps == files.stamps
        {
            return entry.fingerprint;
        }
        let value = fingerprint::fingerprint_of(addon_dir, &files);
        #[cfg(test)]
        self.computed
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.lock().insert(
            addon_dir.to_path_buf(),
            Entry {
                stamps: files.stamps,
                fingerprint: value,
            },
        );
        value
    }

    /// Drops the `folders` of `addons_dir`, ignoring case like the game does: the app has just
    /// written or removed them.
    pub fn forget(&self, addons_dir: &Path, folders: &[String]) {
        let names: HashSet<String> = folders.iter().map(|f| f.to_lowercase()).collect();
        self.lock().retain(|dir, _| {
            !(dir.parent() == Some(addons_dir) && name_of(dir).is_some_and(|n| names.contains(&n)))
        });
    }

    /// Drops every folder of `addons_dir`: the next scan recomputes them.
    pub fn forget_all(&self, addons_dir: &Path) {
        self.lock()
            .retain(|dir, _| dir.parent() != Some(addons_dir));
    }

    /// Keeps, of `addons_dir`, only the `present` folders: those gone from the disk leave, so
    /// the cache does not grow without bound.
    pub fn retain(&self, addons_dir: &Path, present: &[String]) {
        let names: HashSet<String> = present.iter().map(|f| f.to_lowercase()).collect();
        self.lock().retain(|dir, _| {
            dir.parent() != Some(addons_dir) || name_of(dir).is_some_and(|n| names.contains(&n))
        });
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<PathBuf, Entry>> {
        // A scan that panicked does not make the cache unusable: at worst it is recomputed.
        self.entries.lock().unwrap_or_else(PoisonError::into_inner)
    }

    #[cfg(test)]
    fn computed(&self) -> usize {
        self.computed.load(std::sync::atomic::Ordering::Relaxed)
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.lock().len()
    }
}

fn name_of(dir: &Path) -> Option<String> {
    dir.file_name().map(|n| n.to_string_lossy().to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;
    use std::fs;

    fn addon(addons: &Path, folder: &str, lua: &str) -> PathBuf {
        let dir = addons.join(folder);
        fs::create_dir_all(dir.join("sub")).unwrap();
        fs::write(dir.join(format!("{folder}.toc")), "sub/core.lua\n").unwrap();
        fs::write(dir.join("sub").join("core.lua"), lua).unwrap();
        dir
    }

    #[test]
    fn an_unchanged_folder_is_not_computed_again() {
        let tmp = TempDir::new();
        let dir = addon(tmp.path(), "Foo", "print(1)");
        let cache = FingerprintCache::default();
        let first = cache.fingerprint(&dir);
        assert!(first.is_some());
        assert_eq!(cache.fingerprint(&dir), first);
        assert_eq!(cache.computed(), 1);
        assert_eq!(first, fingerprint::folder_fingerprint(&dir));
    }

    #[test]
    fn a_changed_added_or_removed_file_is_computed_again() {
        let tmp = TempDir::new();
        let dir = addon(tmp.path(), "Foo", "print(1)");
        let cache = FingerprintCache::default();
        cache.fingerprint(&dir);

        // Different size, in a subfolder: the folder's timestamp does not change.
        fs::write(dir.join("sub").join("core.lua"), "print(12)").unwrap();
        assert_eq!(
            cache.fingerprint(&dir),
            fingerprint::folder_fingerprint(&dir)
        );
        assert_eq!(cache.computed(), 2);

        fs::write(dir.join("extra.txt"), "x").unwrap();
        cache.fingerprint(&dir);
        assert_eq!(cache.computed(), 3);

        fs::remove_file(dir.join("extra.txt")).unwrap();
        cache.fingerprint(&dir);
        assert_eq!(cache.computed(), 4);
    }

    #[test]
    fn forgotten_folders_are_computed_again_and_others_are_kept() {
        let tmp = TempDir::new();
        let foo = addon(tmp.path(), "Foo", "print(1)");
        let bar = addon(tmp.path(), "Bar", "print(2)");
        let cache = FingerprintCache::default();
        cache.fingerprint(&foo);
        cache.fingerprint(&bar);

        // The name comes from a record, with whatever case it has.
        cache.forget(tmp.path(), &["FOO".to_owned()]);
        cache.fingerprint(&foo);
        cache.fingerprint(&bar);
        assert_eq!(cache.computed(), 3);

        cache.forget_all(tmp.path());
        cache.fingerprint(&foo);
        cache.fingerprint(&bar);
        assert_eq!(cache.computed(), 5);
    }

    #[test]
    fn folders_no_longer_present_leave_the_cache() {
        let tmp = TempDir::new();
        let foo = addon(tmp.path(), "Foo", "print(1)");
        let bar = addon(tmp.path(), "Bar", "print(2)");
        let other = TempDir::new();
        let elsewhere = addon(other.path(), "Foo", "print(3)");
        let cache = FingerprintCache::default();
        cache.fingerprint(&foo);
        cache.fingerprint(&bar);
        cache.fingerprint(&elsewhere);

        cache.retain(tmp.path(), &["bar".to_owned()]);
        // Bar and the other installation's folder remain.
        assert_eq!(cache.len(), 2);
        cache.fingerprint(&bar);
        cache.fingerprint(&elsewhere);
        assert_eq!(cache.computed(), 3);
    }

    /// First and second scan of the real folder named by `WOWDON_FINGERPRINT_DIR` (the
    /// installation folder): same fingerprints, and the second computes none of them.
    /// `cargo test -- --ignored second_scan --nocapture` prints the timings.
    #[test]
    #[ignore = "needs a real installation folder"]
    fn second_scan_of_a_real_folder_reuses_every_fingerprint() {
        let Ok(dir) = std::env::var("WOWDON_FINGERPRINT_DIR") else {
            panic!("set WOWDON_FINGERPRINT_DIR to an installation folder");
        };
        let dir = PathBuf::from(dir);
        let cache = FingerprintCache::default();
        let started = std::time::Instant::now();
        let first = crate::addons::scan::scan_addons_with(&dir, "retail", Some(&cache)).unwrap();
        let cold = started.elapsed();
        let computed = cache.computed();
        let started = std::time::Instant::now();
        let second = crate::addons::scan::scan_addons_with(&dir, "retail", Some(&cache)).unwrap();
        let warm = started.elapsed();
        eprintln!(
            "{} folders: first scan {cold:?}, second {warm:?}",
            first.len()
        );
        assert_eq!(first, second);
        assert_eq!(cache.computed(), computed);
    }
}
