//! La versione precedente dell'ultimo giro automatico e il suo Undo (D41, issue #201).
//!
//! Solo nei giri automatici in modalità `install` le cartelle che un aggiornamento sostituisce
//! non si cancellano: [`NextGeneration`] le raccoglie (con `install::keeping_replaced`) in una
//! cartella di lavoro accanto ad `AddOns` e, se il giro ha installato almeno un progetto, la
//! rinomina in `Interface/.wow-don-previous/` al posto della generazione precedente. C'è una
//! sola generazione per installazione: dentro, una sottocartella numerata (lo "slot") per
//! progetto. Gli aggiornamenti manuali non passano di qui e cancellano come prima.
//!
//! Il manifesto [`PREVIOUS_FILE`] nella cartella di config dell'app (D22, scrittura atomica)
//! registra per installazione l'ora del giro e, per progetto, fonte, chiave, record precedente,
//! record scritto dal giro, slot e cartelle salvate. [`undo`] lo usa per rimettere le cartelle e
//! i record; un progetto il cui record non è più quello scritto dal giro si salta.
//!
//! Disco e manifesto si scrivono solo sotto `InstallLock` (il giro e `undo_last_auto_update`):
//! il manifesto si rilegge a ogni uso e non ha uno stato in memoria.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::addons::cache::FingerprintCache;
use crate::install::{self, WorkDir};
use crate::installations::Installation;
use crate::sources::RecordWriter;
use crate::store::{self, InstalledAddonsStore, SourceKind, SourceRecord, SourceRecords};
use crate::updates::UpdateOutcome;

/// Il manifesto delle generazioni, nella cartella di config dell'app.
pub const PREVIOUS_FILE: &str = "auto-update-previous.json";

/// La cartella della generazione, in `Interface` (stesso disco di `AddOns`).
pub const GENERATION_DIR: &str = ".wow-don-previous";

const VERSION: u32 = 1;

/// Un progetto aggiornato da un giro automatico, con quello che serve a rimetterlo com'era.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviousProject {
    pub source: SourceKind,
    pub key: String,
    pub name: String,
    /// Il record prima del giro; `null` se il progetto non ne aveva (riconosciuto solo dal
    /// fingerprint): l'Undo allora toglie il record.
    pub previous: Option<SourceRecord>,
    /// Il record scritto dal giro: l'Undo procede solo se è ancora quello attuale.
    pub current: SourceRecord,
    /// La sottocartella della generazione con le cartelle salvate.
    pub slot: String,
    /// Le cartelle salvate, con il nome che avevano in `AddOns`.
    pub saved: Vec<String>,
}

/// L'ultimo giro automatico annullabile di un'installazione.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviousRound {
    /// L'ora del giro, ISO 8601 UTC.
    pub round_at: String,
    /// I progetti, nell'ordine in cui il giro li ha aggiornati.
    pub projects: Vec<PreviousProject>,
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredPrevious {
    version: u32,
    installations: BTreeMap<String, PreviousRound>,
}

/// Il manifesto [`PREVIOUS_FILE`]. Si rilegge a ogni uso; un file illeggibile blocca l'Undo
/// invece di essere sovrascritto, e vale "niente da annullare" per lo stato.
#[derive(Debug)]
pub struct PreviousManifest {
    file: PathBuf,
}

impl PreviousManifest {
    pub fn new(file: PathBuf) -> Self {
        Self { file }
    }

    /// Le generazioni per installazione; vuoto senza file.
    pub fn read(&self) -> Result<BTreeMap<String, PreviousRound>, String> {
        let damaged = || {
            format!(
                "{PREVIOUS_FILE} is damaged: move it away to forget the automatic update that could be undone."
            )
        };
        let text = match store::read_text(&self.file) {
            Ok(Some(text)) => text,
            Ok(None) => return Ok(BTreeMap::new()),
            Err(error) => {
                return Err(format!(
                    "Could not read {PREVIOUS_FILE} ({}).",
                    error.kind()
                ));
            }
        };
        let stored: StoredPrevious = serde_json::from_str(&text).map_err(|_| damaged())?;
        if stored.version != VERSION {
            return Err(format!(
                "{PREVIOUS_FILE} has an unsupported version ({}): update the app or move the file away.",
                stored.version
            ));
        }
        Ok(stored.installations)
    }

    /// Sostituisce (`Some`) o toglie (`None`) la generazione di un'installazione.
    pub fn set(&self, installation_id: &str, round: Option<PreviousRound>) -> Result<(), String> {
        let mut installations = self.read()?;
        match round {
            Some(round) => {
                installations.insert(installation_id.to_owned(), round);
            }
            None => {
                if installations.remove(installation_id).is_none() {
                    return Ok(());
                }
            }
        }
        let stored = StoredPrevious {
            version: VERSION,
            installations,
        };
        store::write_atomically(&self.file, &stored).map_err(|error| {
            format!(
                "Could not save the automatic update that can be undone ({}).",
                error.kind()
            )
        })
    }
}

/// Un giro annullabile, per `get_auto_update_status` (`undoable`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UndoableRound {
    pub installation_id: String,
    /// L'ora del giro, ISO 8601 UTC.
    pub round_at: String,
    /// Quanti progetti (addon) l'Undo rimetterebbe.
    pub addons: usize,
}

/// I giri annullabili delle installazioni `installation_ids`, nel loro ordine. Un manifesto
/// illeggibile vale "niente da annullare": l'errore lo dà l'Undo.
pub fn undoable(manifest: &PreviousManifest, installation_ids: &[String]) -> Vec<UndoableRound> {
    let Ok(rounds) = manifest.read() else {
        return Vec::new();
    };
    installation_ids
        .iter()
        .filter_map(|id| {
            let round = rounds.get(id).filter(|round| !round.projects.is_empty())?;
            Some(UndoableRound {
                installation_id: id.clone(),
                round_at: round.round_at.clone(),
                addons: round.projects.len(),
            })
        })
        .collect()
}

fn interface_dir(installation: &Installation) -> PathBuf {
    installation.path.join("Interface")
}

fn addons_dir(installation: &Installation) -> PathBuf {
    interface_dir(installation).join("AddOns")
}

// ── Giro: la generazione nuova ──────────────────────────────────────────────

/// La generazione che un giro automatico prepara per un'installazione.
pub struct NextGeneration {
    addons_dir: PathBuf,
    generation: PathBuf,
    /// Creata al primo [`run`](Self::run); si cancella da sola se il giro non installa niente.
    dir: Option<WorkDir>,
    error: Option<String>,
    projects: Vec<PreviousProject>,
    /// I progetti aggiornati la cui versione precedente non è stata tenuta per intero (o non è
    /// arrivata a questo thread): non si possono annullare, e [`commit`](Self::commit) lo dice.
    not_kept: Vec<(SourceKind, String)>,
}

impl NextGeneration {
    pub fn new(installation: &Installation) -> Self {
        Self {
            addons_dir: addons_dir(installation),
            generation: interface_dir(installation).join(GENERATION_DIR),
            dir: None,
            error: None,
            projects: Vec::new(),
            not_kept: Vec::new(),
        }
    }

    /// Esegue `run` (l'aggiornamento del giro) tenendo le cartelle sostituite. `record` è il
    /// registro vero; `before` sono i record dell'installazione prima del giro. Un progetto entra
    /// nella generazione solo se il suo record si è salvato e tutte le sue cartelle sostituite
    /// sono state tenute. Se la cartella di lavoro non si crea l'aggiornamento procede come un
    /// aggiornamento manuale, e [`commit`](Self::commit) restituisce l'errore.
    pub fn run<R>(
        &mut self,
        before: &SourceRecords,
        record: &mut RecordWriter<'_>,
        run: impl FnOnce(&mut RecordWriter<'_>) -> R,
    ) -> R {
        if self.dir.is_none() && self.error.is_none() {
            match WorkDir::create(&self.addons_dir, "next") {
                Ok(dir) => self.dir = Some(dir),
                Err(error) => {
                    self.error = Some(format!("Could not keep the previous versions: {error}"))
                }
            }
        }
        let Some(dir) = &self.dir else {
            return run(record);
        };
        let projects = &mut self.projects;
        let not_kept = &mut self.not_kept;
        install::keeping_replaced(dir.path(), || {
            let mut keeping =
                |source: SourceKind, key: &str, new: SourceRecord| -> Result<(), String> {
                    let kept = install::take_kept();
                    let previous = before.get(&source).and_then(|p| p.get(key)).cloned();
                    record(source, key, new.clone())?;
                    match kept.filter(|kept| kept.complete) {
                        Some(kept) => projects.push(PreviousProject {
                            source,
                            key: key.to_owned(),
                            name: String::new(),
                            previous,
                            current: new,
                            slot: kept.slot,
                            saved: kept.folders,
                        }),
                        None => not_kept.push((source, key.to_owned())),
                    }
                    Ok(())
                };
            run(&mut keeping)
        })
    }

    /// Alla fine del giro: se almeno un progetto è entrato nella generazione, questa sostituisce
    /// quella precedente sul disco e nel manifesto, con l'ora `round_at` e i nomi presi da
    /// `updated`. Altrimenti la generazione precedente resta com'è.
    pub fn commit(
        self,
        manifest: &PreviousManifest,
        installation_id: &str,
        round_at: &str,
        updated: &[UpdateOutcome],
    ) -> Result<(), String> {
        let name_of = |source: SourceKind, key: &str| {
            updated
                .iter()
                .find(|o| o.source == source && o.key == key)
                .map_or_else(|| key.to_owned(), |o| o.name.clone())
        };
        // Un progetto aggiornato senza la versione precedente non si perde in silenzio.
        let not_kept = (!self.not_kept.is_empty()).then(|| {
            let names: Vec<String> = self
                .not_kept
                .iter()
                .map(|(source, key)| name_of(*source, key))
                .collect();
            format!(
                "Could not keep the previous version of {}: undo will not restore it.",
                names.join(", ")
            )
        });
        let Some(dir) = self.dir else {
            return self.error.map_or(Ok(()), Err);
        };
        if self.projects.is_empty() {
            return not_kept.map_or(Ok(()), Err);
        }
        let projects = self
            .projects
            .into_iter()
            .map(|mut project| {
                if let Some(outcome) = updated
                    .iter()
                    .find(|o| o.source == project.source && o.key == project.key)
                {
                    project.name = outcome.name.clone();
                }
                project
            })
            .collect();
        let failed = |error: io::Error| {
            format!(
                "Could not keep the previous versions for Undo ({}).",
                error.kind()
            )
        };
        if fs::symlink_metadata(&self.generation).is_ok() {
            // La generazione vecchia passa in un appoggio che si cancella da solo: il nome resta
            // libero per la nuova anche se la cancellazione non finisce.
            let trash = WorkDir::create(&self.addons_dir, "old").map_err(|error| {
                format!("Could not keep the previous versions for Undo: {error}")
            })?;
            fs::rename(&self.generation, trash.path().join("generation")).map_err(failed)?;
            // Le cartelle non ci sono più: la voce del manifesto non vale più.
            manifest.set(installation_id, None)?;
        }
        // Dopo il `rename` la cartella di lavoro non c'è più e il suo `Drop` non tocca niente.
        fs::rename(dir.path(), &self.generation).map_err(failed)?;
        let round = PreviousRound {
            round_at: round_at.to_owned(),
            projects,
        };
        if let Err(error) = manifest.set(installation_id, Some(round)) {
            let _ = fs::remove_dir_all(&self.generation);
            return Err(error);
        }
        not_kept.map_or(Ok(()), Err)
    }
}

// ── Undo ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum UndoStatus {
    /// Cartelle e record sono tornati alla versione di prima del giro.
    Restored,
    /// Il progetto è cambiato dopo il giro: niente è stato toccato.
    Skipped,
    /// Le cartelle non si sono potute rimettere (se la versione precedente è ancora sul disco
    /// l'Undo si può riprovare), oppure sono tornate ma il record no: allora cartelle e record
    /// non combaciano e il messaggio lo dice.
    Error,
}

/// L'esito dell'Undo di un progetto. I messaggi sono in inglese (D27).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UndoOutcome {
    pub source: SourceKind,
    pub key: String,
    pub name: String,
    pub status: UndoStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

const CHANGED: &str = "Changed after the automatic update: left as it is.";
const MISSING: &str = "The previous version is no longer on disk.";
const NOTHING: &str = "There is no automatic update to undo for this installation.";

/// Annulla l'ultimo giro automatico di `installation`: per ogni progetto toglie le cartelle
/// nuove, rimette quelle salvate e ripristina il record precedente, poi cancella generazione e
/// voce del manifesto. Va chiamata sotto `InstallLock`. Un progetto che non si rimette per un
/// `rename` fallito resta nella generazione e nel manifesto, e l'Undo si può riprovare; gli
/// altri ne escono comunque. `Err` solo se il manifesto o i record non si leggono, o se non c'è
/// niente da annullare.
pub fn undo(
    installation: &Installation,
    manifest: &PreviousManifest,
    installed: &Mutex<InstalledAddonsStore>,
    cache: Option<&FingerprintCache>,
) -> Result<Vec<UndoOutcome>, String> {
    undo_with(installation, manifest, installed, cache, |from, to| {
        fs::rename(from, to)
    })
}

fn locked(
    installed: &Mutex<InstalledAddonsStore>,
) -> Result<std::sync::MutexGuard<'_, InstalledAddonsStore>, String> {
    installed
        .lock()
        .map_err(|_| "The installed addons list is not available: restart the app.".to_owned())
}

pub(crate) fn undo_with(
    installation: &Installation,
    manifest: &PreviousManifest,
    installed: &Mutex<InstalledAddonsStore>,
    cache: Option<&FingerprintCache>,
    mut rename: impl FnMut(&Path, &Path) -> io::Result<()>,
) -> Result<Vec<UndoOutcome>, String> {
    let round = manifest
        .read()?
        .remove(&installation.id)
        .filter(|round| !round.projects.is_empty())
        .ok_or_else(|| NOTHING.to_owned())?;
    let records = locked(installed)?.source_records_for(&installation.id)?;
    let addons = addons_dir(installation);
    let generation = interface_dir(installation).join(GENERATION_DIR);

    let mut outcomes = Vec::new();
    let mut left = Vec::new();
    // Dall'ultimo al primo: se due progetti del giro hanno toccato la stessa cartella, ognuno
    // rimette quella che c'era prima di lui.
    for project in round.projects.iter().rev() {
        let outcome = |status, message: Option<String>| UndoOutcome {
            source: project.source,
            key: project.key.clone(),
            name: project.name.clone(),
            status,
            message,
        };
        let now = records
            .get(&project.source)
            .and_then(|p| p.get(&project.key));
        if now != Some(&project.current) {
            outcomes.push(outcome(UndoStatus::Skipped, Some(CHANGED.to_owned())));
            continue;
        }
        let slot = generation.join(&project.slot);
        let present = project
            .saved
            .iter()
            .all(|folder| fs::symlink_metadata(slot.join(folder)).is_ok());
        if !present {
            outcomes.push(outcome(UndoStatus::Error, Some(MISSING.to_owned())));
            continue;
        }
        if let Err(message) = put_back(&addons, &slot, project, &mut rename) {
            outcomes.push(outcome(UndoStatus::Error, Some(message)));
            left.push(project.clone());
            continue;
        }
        if let Some(cache) = cache {
            let touched: Vec<String> = project
                .current
                .folders
                .iter()
                .chain(&project.saved)
                .cloned()
                .collect();
            cache.forget(&addons, &touched);
        }
        let restored = locked(installed).and_then(|mut store| match &project.previous {
            Some(previous) => store.record_source(
                &installation.id,
                project.source,
                &project.key,
                previous.clone(),
            ),
            None => store.forget_project(&installation.id, project.source, &project.key),
        });
        // Le cartelle sono tornate ma il record no: non è un Undo riuscito, e non si può
        // ripetere (lo slot è vuoto). Lo dice lo stato, non solo il messaggio.
        outcomes.push(match restored {
            Ok(()) => outcome(UndoStatus::Restored, None),
            Err(error) => outcome(
                UndoStatus::Error,
                Some(format!(
                    "Restored the previous folders, but the app could not update its records: {error}"
                )),
            ),
        });
    }
    outcomes.reverse();
    left.reverse();

    if left.is_empty() {
        manifest.set(&installation.id, None)?;
        let _ = fs::remove_dir_all(&generation);
    } else {
        let keep: HashSet<&str> = left.iter().map(|p| p.slot.as_str()).collect();
        for project in &round.projects {
            if !keep.contains(project.slot.as_str()) {
                let _ = fs::remove_dir_all(generation.join(&project.slot));
            }
        }
        manifest.set(
            &installation.id,
            Some(PreviousRound {
                round_at: round.round_at,
                projects: left,
            }),
        )?;
    }
    Ok(outcomes)
}

/// Rimette in `addons` le cartelle salvate in `slot` al posto di quelle del record del giro.
/// Con `replace_with`: le cartelle nuove (senza distinguere maiuscole e minuscole) si spostano da
/// parte e si cancellano solo dopo che tutte le salvate sono tornate; se un `rename` fallisce
/// tutto torna com'era, e la versione precedente resta nello slot.
fn put_back(
    addons: &Path,
    slot: &Path,
    project: &PreviousProject,
    rename: &mut impl FnMut(&Path, &Path) -> io::Result<()>,
) -> Result<(), String> {
    if !project.saved.is_empty() {
        return install::replace_with(addons, slot, &project.current.folders, rename);
    }
    // Il giro non ha sostituito niente (le cartelle nuove non c'erano): basta toglierle.
    let wanted: HashSet<String> = project
        .current
        .folders
        .iter()
        .map(|f| f.to_lowercase())
        .collect();
    for name in install::folder_names(addons)? {
        if wanted.contains(&name.to_lowercase()) {
            install::remove_folder(addons, &name)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;

    const CF: SourceKind = SourceKind::Curseforge;
    const GH: SourceKind = SourceKind::Github;

    struct Fixture {
        _tmp: TempDir,
        installation: Installation,
        addons: PathBuf,
        manifest: PreviousManifest,
        installed: Mutex<InstalledAddonsStore>,
    }

    fn fixture() -> Fixture {
        let tmp = TempDir::new();
        let path = tmp.path().join("World of Warcraft").join("_retail_");
        let addons = path.join("Interface").join("AddOns");
        fs::create_dir_all(&addons).unwrap();
        let config = tmp.path().join("config");
        Fixture {
            installation: Installation {
                id: "test".to_owned(),
                path,
                flavor: "retail".to_owned(),
                name: "Retail".to_owned(),
            },
            addons,
            manifest: PreviousManifest::new(config.join(PREVIOUS_FILE)),
            installed: Mutex::new(InstalledAddonsStore::load(
                config.join(store::INSTALLED_ADDONS_FILE),
            )),
            _tmp: tmp,
        }
    }

    impl Fixture {
        fn write(&self, folder: &str, text: &str) {
            let dir = self.addons.join(folder);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("file.lua"), text).unwrap();
        }

        fn read(&self, folder: &str) -> String {
            fs::read_to_string(self.addons.join(folder).join("file.lua")).unwrap()
        }

        fn listing(&self) -> Vec<String> {
            install::folder_names(&self.addons).unwrap()
        }

        fn generation(&self) -> PathBuf {
            self.installation
                .path
                .join("Interface")
                .join(GENERATION_DIR)
        }

        /// Le cartelle di `Interface`, senza `AddOns`.
        fn interface_extras(&self) -> Vec<String> {
            install::folder_names(&self.installation.path.join("Interface"))
                .unwrap()
                .into_iter()
                .filter(|name| name != "AddOns")
                .collect()
        }

        fn records(&self) -> SourceRecords {
            self.installed
                .lock()
                .unwrap()
                .source_records_for("test")
                .unwrap()
        }

        fn record(&self, source: SourceKind, key: &str) -> Option<SourceRecord> {
            self.records()
                .get(&source)
                .and_then(|p| p.get(key))
                .cloned()
        }

        fn set_record(&self, source: SourceKind, key: &str, record: SourceRecord) {
            self.installed
                .lock()
                .unwrap()
                .record_source("test", source, key, record)
                .unwrap();
        }

        /// Un giro automatico: per ogni `(fonte, chiave, file, cartelle)` sostituisce le cartelle
        /// con un contenuto `new <file>` (come fa una fonte) e scrive il record. Restituisce gli
        /// esiti `updated` e quello di `commit`.
        fn round(
            &self,
            updates: &[(SourceKind, &str, &str, &[&str])],
        ) -> (Vec<UpdateOutcome>, Result<(), String>) {
            let before = self.records();
            let mut next = NextGeneration::new(&self.installation);
            let mut writer = |source, key: &str, record| {
                self.installed
                    .lock()
                    .unwrap()
                    .record_source("test", source, key, record)
            };
            let updated = next.run(&before, &mut writer, |record| {
                let mut updated = Vec::new();
                for (source, key, file, folders) in updates {
                    let staging = WorkDir::staging(&self.addons).unwrap();
                    for folder in *folders {
                        let dir = staging.path().join(folder);
                        fs::create_dir_all(&dir).unwrap();
                        fs::write(dir.join("file.lua"), format!("new {file}")).unwrap();
                    }
                    install::replace(&self.addons, staging.path(), &[]).unwrap();
                    record(
                        *source,
                        key,
                        SourceRecord {
                            file: (*file).to_owned(),
                            folders: folders.iter().map(|f| (*f).to_owned()).collect(),
                        },
                    )
                    .unwrap();
                    updated.push(UpdateOutcome::updated(
                        *source,
                        key,
                        &format!("{key} name"),
                        crate::updates::FileSummary {
                            id: 0,
                            name: (*file).to_owned(),
                            date: String::new(),
                        },
                        None,
                    ));
                }
                updated
            });
            let committed = next.commit(&self.manifest, "test", "2026-10-27T10:00:00Z", &updated);
            (updated, committed)
        }

        fn undo(&self) -> Result<Vec<UndoOutcome>, String> {
            undo(&self.installation, &self.manifest, &self.installed, None)
        }
    }

    fn record(file: &str, folders: &[&str]) -> SourceRecord {
        SourceRecord {
            file: file.to_owned(),
            folders: folders.iter().map(|f| (*f).to_owned()).collect(),
        }
    }

    fn statuses(outcomes: &[UndoOutcome]) -> Vec<(&str, UndoStatus)> {
        outcomes
            .iter()
            .map(|o| (o.key.as_str(), o.status))
            .collect()
    }

    /// Due progetti: DBM (CurseForge, con un record) e BigWigs (GitHub, riconosciuto senza).
    fn two_addons() -> Fixture {
        let fx = fixture();
        fx.write("DBM-Core", "old dbm core");
        fx.write("DBM-GUI", "old dbm gui");
        fx.write("BigWigs", "old bigwigs");
        fx.write("Other", "other");
        fx.set_record(CF, "3358", record("1", &["DBM-Core", "DBM-GUI"]));
        fx
    }

    #[test]
    fn an_automatic_round_keeps_the_replaced_folders_and_undo_puts_them_back() {
        let fx = two_addons();
        let (_, committed) = fx.round(&[
            (CF, "3358", "2", &["DBM-Core", "DBM-GUI"]),
            (GH, "owner/bigwigs", "v2", &["BigWigs"]),
        ]);
        committed.unwrap();
        assert_eq!(fx.read("DBM-Core"), "new 2");
        assert_eq!(fx.read("BigWigs"), "new v2");
        // Solo la generazione accanto ad AddOns: staging, appoggi e cartella di lavoro sono
        // spariti.
        assert_eq!(fx.interface_extras(), [GENERATION_DIR]);
        let rounds = fx.manifest.read().unwrap();
        let round = &rounds["test"];
        assert_eq!(round.round_at, "2026-10-27T10:00:00Z");
        assert_eq!(round.projects.len(), 2);
        let dbm = &round.projects[0];
        assert_eq!(dbm.name, "3358 name");
        assert_eq!(dbm.previous, Some(record("1", &["DBM-Core", "DBM-GUI"])));
        assert_eq!(dbm.current, record("2", &["DBM-Core", "DBM-GUI"]));
        assert_eq!(dbm.saved, ["DBM-Core", "DBM-GUI"]);
        assert_eq!(round.projects[1].previous, None);
        assert_eq!(
            undoable(&fx.manifest, &["test".to_owned(), "other".to_owned()]),
            [UndoableRound {
                installation_id: "test".to_owned(),
                round_at: "2026-10-27T10:00:00Z".to_owned(),
                addons: 2,
            }]
        );

        let outcomes = fx.undo().unwrap();
        assert_eq!(
            statuses(&outcomes),
            [
                ("3358", UndoStatus::Restored),
                ("owner/bigwigs", UndoStatus::Restored)
            ]
        );
        assert!(outcomes.iter().all(|o| o.message.is_none()));
        assert_eq!(fx.read("DBM-Core"), "old dbm core");
        assert_eq!(fx.read("DBM-GUI"), "old dbm gui");
        assert_eq!(fx.read("BigWigs"), "old bigwigs");
        assert_eq!(fx.read("Other"), "other");
        assert_eq!(fx.listing(), ["BigWigs", "DBM-Core", "DBM-GUI", "Other"]);
        // Il record precedente torna; quello che prima non c'era sparisce.
        assert_eq!(
            fx.record(CF, "3358"),
            Some(record("1", &["DBM-Core", "DBM-GUI"]))
        );
        assert_eq!(fx.record(GH, "owner/bigwigs"), None);
        assert!(fx.interface_extras().is_empty());
        assert!(fx.manifest.read().unwrap().is_empty());
        assert!(undoable(&fx.manifest, &["test".to_owned()]).is_empty());
        assert_eq!(fx.undo().unwrap_err(), NOTHING);
    }

    #[test]
    fn obsolete_folders_removed_by_the_round_come_back() {
        let fx = fixture();
        fx.write("Addon", "old");
        fx.write("Addon-Obsolete", "obsolete");
        fx.set_record(CF, "1", record("1", &["Addon", "Addon-Obsolete"]));
        let before = fx.records();
        let mut next = NextGeneration::new(&fx.installation);
        let mut writer = |source, key: &str, record| {
            fx.installed
                .lock()
                .unwrap()
                .record_source("test", source, key, record)
        };
        next.run(&before, &mut writer, |record| {
            let staging = WorkDir::staging(&fx.addons).unwrap();
            fs::create_dir_all(staging.path().join("Addon")).unwrap();
            fs::write(staging.path().join("Addon").join("file.lua"), "new").unwrap();
            install::replace(&fx.addons, staging.path(), &["Addon-Obsolete".to_owned()]).unwrap();
            record(CF, "1", record_of("2", &["Addon"])).unwrap();
        });
        next.commit(&fx.manifest, "test", "t", &[]).unwrap();
        assert_eq!(fx.listing(), ["Addon"]);

        fx.undo().unwrap();
        assert_eq!(fx.listing(), ["Addon", "Addon-Obsolete"]);
        assert_eq!(fx.read("Addon"), "old");
        assert_eq!(fx.read("Addon-Obsolete"), "obsolete");
    }

    fn record_of(file: &str, folders: &[&str]) -> SourceRecord {
        record(file, folders)
    }

    #[test]
    fn a_project_changed_after_the_round_is_skipped_without_touching_it() {
        let fx = two_addons();
        fx.round(&[
            (CF, "3358", "2", &["DBM-Core", "DBM-GUI"]),
            (GH, "owner/bigwigs", "v2", &["BigWigs"]),
        ])
        .1
        .unwrap();
        // DBM aggiornato a mano dopo il giro.
        fx.write("DBM-Core", "manual 3");
        fx.set_record(CF, "3358", record("3", &["DBM-Core", "DBM-GUI"]));

        let outcomes = fx.undo().unwrap();
        assert_eq!(
            statuses(&outcomes),
            [
                ("3358", UndoStatus::Skipped),
                ("owner/bigwigs", UndoStatus::Restored)
            ]
        );
        assert_eq!(outcomes[0].message.as_deref(), Some(CHANGED));
        assert_eq!(fx.read("DBM-Core"), "manual 3");
        assert_eq!(fx.read("DBM-GUI"), "new 2");
        assert_eq!(
            fx.record(CF, "3358"),
            Some(record("3", &["DBM-Core", "DBM-GUI"]))
        );
        assert_eq!(fx.read("BigWigs"), "old bigwigs");
        // Il progetto saltato non si potrà mai annullare: la generazione sparisce comunque.
        assert!(fx.interface_extras().is_empty());
        assert!(fx.manifest.read().unwrap().is_empty());
    }

    #[test]
    fn a_removed_project_is_skipped_too() {
        let fx = two_addons();
        fx.round(&[(GH, "owner/bigwigs", "v2", &["BigWigs"])])
            .1
            .unwrap();
        fx.installed
            .lock()
            .unwrap()
            .forget_project("test", GH, "owner/bigwigs")
            .unwrap();
        fs::remove_dir_all(fx.addons.join("BigWigs")).unwrap();
        let outcomes = fx.undo().unwrap();
        assert_eq!(
            statuses(&outcomes),
            [("owner/bigwigs", UndoStatus::Skipped)]
        );
        assert!(!fx.addons.join("BigWigs").exists());
    }

    #[test]
    fn a_second_round_replaces_the_generation_and_an_empty_one_leaves_it() {
        let fx = two_addons();
        fx.round(&[(CF, "3358", "2", &["DBM-Core", "DBM-GUI"])])
            .1
            .unwrap();
        fx.round(&[(GH, "owner/bigwigs", "v2", &["BigWigs"])])
            .1
            .unwrap();
        let rounds = fx.manifest.read().unwrap();
        assert_eq!(rounds["test"].projects.len(), 1);
        assert_eq!(rounds["test"].projects[0].key, "owner/bigwigs");
        assert_eq!(fx.interface_extras(), [GENERATION_DIR]);
        assert_eq!(
            install::folder_names(&fx.generation()).unwrap(),
            [rounds["test"].projects[0].slot.clone()]
        );

        // Un giro che non installa niente lascia la generazione e il manifesto.
        let (_, committed) = fx.round(&[]);
        committed.unwrap();
        assert_eq!(fx.manifest.read().unwrap(), rounds);
        assert_eq!(fx.interface_extras(), [GENERATION_DIR]);

        // L'Undo rimette solo l'ultimo giro: DBM resta alla versione del primo.
        fx.undo().unwrap();
        assert_eq!(fx.read("BigWigs"), "old bigwigs");
        assert_eq!(fx.read("DBM-Core"), "new 2");
        assert_eq!(
            fx.record(CF, "3358"),
            Some(record("2", &["DBM-Core", "DBM-GUI"]))
        );
    }

    #[test]
    fn a_failed_rename_keeps_the_rest_of_the_generation_for_another_try() {
        let fx = two_addons();
        fx.round(&[
            (CF, "3358", "2", &["DBM-Core", "DBM-GUI"]),
            (GH, "owner/bigwigs", "v2", &["BigWigs"]),
        ])
        .1
        .unwrap();
        // L'Undo va dall'ultimo al primo: BigWigs (2 spostamenti) riesce, poi il terzo
        // spostamento di DBM (la prima cartella salvata che torna) fallisce.
        let mut calls = 0;
        let outcomes = undo_with(
            &fx.installation,
            &fx.manifest,
            &fx.installed,
            None,
            |from, to| {
                calls += 1;
                if calls == 5 {
                    Err(io::Error::from(io::ErrorKind::PermissionDenied))
                } else {
                    fs::rename(from, to)
                }
            },
        )
        .unwrap();
        assert_eq!(
            statuses(&outcomes),
            [
                ("3358", UndoStatus::Error),
                ("owner/bigwigs", UndoStatus::Restored)
            ]
        );
        assert!(
            outcomes[0]
                .message
                .as_deref()
                .unwrap()
                .contains("Could not move")
        );
        // BigWigs è tornato con il suo record; DBM è ancora quello del giro, coerente con il
        // suo record.
        assert_eq!(fx.read("BigWigs"), "old bigwigs");
        assert_eq!(fx.record(GH, "owner/bigwigs"), None);
        assert_eq!(fx.read("DBM-Core"), "new 2");
        assert_eq!(fx.read("DBM-GUI"), "new 2");
        assert_eq!(
            fx.record(CF, "3358"),
            Some(record("2", &["DBM-Core", "DBM-GUI"]))
        );
        let rounds = fx.manifest.read().unwrap();
        assert_eq!(rounds["test"].projects.len(), 1);
        let dbm = rounds["test"].projects[0].clone();
        assert_eq!(dbm.key, "3358");
        assert_eq!(
            install::folder_names(&fx.generation()).unwrap(),
            [dbm.slot.as_str()]
        );

        // Il secondo tentativo finisce il lavoro.
        let outcomes = fx.undo().unwrap();
        assert_eq!(statuses(&outcomes), [("3358", UndoStatus::Restored)]);
        assert_eq!(fx.read("DBM-Core"), "old dbm core");
        assert!(fx.interface_extras().is_empty());
    }

    #[test]
    fn folder_names_are_matched_without_case() {
        // Il record e la generazione scrivono "dbm-core", il disco ha "DBM-Core": nessun nome
        // diverso solo per le maiuscole esiste mai insieme sul disco.
        let fx = fixture();
        fx.write("DBM-Core", "old");
        fx.round(&[(CF, "3358", "2", &["dbm-core"])]).1.unwrap();
        assert_eq!(fx.listing(), ["dbm-core"]);
        let rounds = fx.manifest.read().unwrap();
        assert_eq!(rounds["test"].projects[0].saved, ["DBM-Core"]);
        // Il record attuale usa ancora il nome del giro, il disco un'altra grafia.
        fs::rename(fx.addons.join("dbm-core"), fx.addons.join("tmp")).unwrap();
        fs::rename(fx.addons.join("tmp"), fx.addons.join("Dbm-Core")).unwrap();

        let outcomes = fx.undo().unwrap();
        assert_eq!(statuses(&outcomes), [("3358", UndoStatus::Restored)]);
        assert_eq!(fx.listing(), ["DBM-Core"]);
        assert_eq!(fx.read("DBM-Core"), "old");
    }

    #[test]
    fn a_project_without_replaced_folders_is_undone_by_removing_the_new_ones() {
        let fx = fixture();
        fx.write("Other", "other");
        fx.round(&[(GH, "owner/new", "v1", &["NewAddon"])])
            .1
            .unwrap();
        assert_eq!(
            fx.manifest.read().unwrap()["test"].projects[0].saved,
            Vec::<String>::new()
        );
        let outcomes = fx.undo().unwrap();
        assert_eq!(statuses(&outcomes), [("owner/new", UndoStatus::Restored)]);
        assert_eq!(fx.listing(), ["Other"]);
    }

    #[test]
    fn an_update_whose_previous_version_was_not_kept_is_reported() {
        let fx = two_addons();
        let before = fx.records();
        let mut next = NextGeneration::new(&fx.installation);
        let mut writer = |_, _: &str, _| Ok(());
        // Un record senza un `replace` tenuto prima (per esempio su un altro thread).
        next.run(&before, &mut writer, |record| {
            record(GH, "owner/bigwigs", record_of("v2", &["BigWigs"])).unwrap();
        });
        let updated = [UpdateOutcome::updated(
            GH,
            "owner/bigwigs",
            "BigWigs",
            crate::updates::FileSummary {
                id: 0,
                name: "v2".to_owned(),
                date: String::new(),
            },
            None,
        )];
        let error = next
            .commit(&fx.manifest, "test", "t", &updated)
            .unwrap_err();
        assert_eq!(
            error,
            "Could not keep the previous version of BigWigs: undo will not restore it."
        );
        assert!(fx.manifest.read().unwrap().is_empty());
    }

    #[test]
    fn a_damaged_manifest_blocks_undo_and_hides_the_status() {
        let fx = fixture();
        fs::create_dir_all(fx.manifest.file.parent().unwrap()).unwrap();
        fs::write(&fx.manifest.file, "{").unwrap();
        assert!(fx.undo().unwrap_err().contains("damaged"));
        assert!(undoable(&fx.manifest, &["test".to_owned()]).is_empty());
        assert!(fx.manifest.set("test", None).is_err());
    }

    #[test]
    fn the_undo_payloads_are_camel_case() {
        let outcome = UndoOutcome {
            source: GH,
            key: "owner/repo".to_owned(),
            name: "Addon".to_owned(),
            status: UndoStatus::Restored,
            message: None,
        };
        let json = serde_json::to_value(&outcome).unwrap();
        assert_eq!(json["status"], "restored");
        assert_eq!(json["source"], "github");
        assert!(json.get("message").is_none());
        let round = UndoableRound {
            installation_id: "id".to_owned(),
            round_at: "t".to_owned(),
            addons: 3,
        };
        let json = serde_json::to_value(&round).unwrap();
        assert_eq!(json["installationId"], "id");
        assert_eq!(json["roundAt"], "t");
        assert_eq!(json["addons"], 3);
    }
}
