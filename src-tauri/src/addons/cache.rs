//! Cache in memoria dei fingerprint delle cartelle degli addon (issue #125).
//!
//! Il fingerprint legge per intero i file di un addon; la scansione gira a ogni apertura della
//! tab, a ogni controllo e a ogni aggiornamento. Una cartella i cui file hanno ancora gli stessi
//! percorsi, dimensioni e date di modifica dà lo stesso fingerprint: si riusa senza rileggerli.
//! La sola data della cartella non basta, perché non cambia quando cambia un file dentro una
//! sottocartella.
//!
//! Un file sostituito con uno della stessa dimensione e della stessa data non si vede: per
//! questo le scritture dell'app tolgono le loro cartelle ([`FingerprintCache::forget`]) e
//! "Reload addons" ricalcola tutto ([`FingerprintCache::forget_all`]). Solo memoria: alla
//! chiusura dell'app la cache si perde.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

use super::fingerprint::{self, FileStamp};

struct Entry {
    stamps: Vec<FileStamp>,
    fingerprint: Option<u32>,
}

/// Per cartella di addon (il percorso in `AddOns`), i file visti all'ultimo calcolo e il
/// fingerprint che ne è uscito.
#[derive(Default)]
pub struct FingerprintCache {
    entries: Mutex<HashMap<PathBuf, Entry>>,
    /// Quanti fingerprint sono stati calcolati davvero: i test verificano il riuso così, senza
    /// dipendere dai tempi.
    #[cfg(test)]
    computed: std::sync::atomic::AtomicUsize,
}

impl FingerprintCache {
    /// Il fingerprint di `addon_dir`, dalla cache se i file non sono cambiati. Il lock non resta
    /// preso durante la lettura dei file: si confronta, si calcola fuori e si scrive dopo. Un file
    /// cambiato durante il calcolo ha una data più recente di quella salvata, quindi la
    /// scansione successiva lo ricalcola.
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

    /// Toglie le cartelle `folders` di `addons_dir`, senza distinguere maiuscole e minuscole come
    /// il gioco: l'app le ha appena scritte o tolte.
    pub fn forget(&self, addons_dir: &Path, folders: &[String]) {
        let names: HashSet<String> = folders.iter().map(|f| f.to_lowercase()).collect();
        self.lock().retain(|dir, _| {
            !(dir.parent() == Some(addons_dir) && name_of(dir).is_some_and(|n| names.contains(&n)))
        });
    }

    /// Toglie tutte le cartelle di `addons_dir`: la prossima scansione le ricalcola.
    pub fn forget_all(&self, addons_dir: &Path) {
        self.lock()
            .retain(|dir, _| dir.parent() != Some(addons_dir));
    }

    /// Tiene, di `addons_dir`, solo le cartelle `present`: quelle sparite dal disco escono, così
    /// la cache non cresce senza limite.
    pub fn retain(&self, addons_dir: &Path, present: &[String]) {
        let names: HashSet<String> = present.iter().map(|f| f.to_lowercase()).collect();
        self.lock().retain(|dir, _| {
            dir.parent() != Some(addons_dir) || name_of(dir).is_some_and(|n| names.contains(&n))
        });
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<PathBuf, Entry>> {
        // Una scansione andata in panic non rende inutilizzabile la cache: al peggio si ricalcola.
        self.entries.lock().unwrap_or_else(PoisonError::into_inner)
    }

    #[cfg(test)]
    fn computed(&self) -> usize {
        self.computed.load(std::sync::atomic::Ordering::Relaxed)
    }

    #[cfg(test)]
    fn len(&self) -> usize {
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

        // Dimensione diversa, in una sottocartella: la data della cartella non cambia.
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

        // Il nome arriva da un record, con le maiuscole che vuole.
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
        // Restano Bar e la cartella dell'altra installazione.
        assert_eq!(cache.len(), 2);
        cache.fingerprint(&bar);
        cache.fingerprint(&elsewhere);
        assert_eq!(cache.computed(), 3);
    }

    /// Prima e seconda scansione della cartella vera indicata da `WOWDON_FINGERPRINT_DIR` (la
    /// cartella dell'installazione): stessi fingerprint, e la seconda non ne calcola nessuno.
    /// `cargo test -- --ignored second_scan --nocapture` stampa i tempi.
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
