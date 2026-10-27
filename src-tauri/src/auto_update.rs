//! Il giro automatico degli aggiornamenti (D41, issue #200): un thread del backend che controlla
//! (modalità `notify`) o controlla e installa (`install`) gli aggiornamenti di ogni installazione
//! che partecipa, poco dopo l'avvio e poi ogni `autoUpdateIntervalHours`.
//!
//! Il giro usa le stesse fonti, versioni forzate e opzione della Beta dei comandi manuali
//! (`commands::LiveSources`) e gli stessi `check_updates_with` e `update_addons_with`. Prende
//! [`InstallLock`] senza aspettare: se un comando lo tiene, il giro salta e riprova dopo
//! [`BUSY_RETRY`]. Un controllo fallito per la rete riprova dopo [`BACKOFF`], mai oltre
//! l'intervallo. La pianificazione ([`Schedule`]), il filtro dei progetti ([`plan`]) e la
//! classificazione dei fallimenti ([`needs_retry`]) sono funzioni pure.
//!
//! L'esito dell'ultimo giro resta in memoria in [`AutoUpdate`], si legge con
//! `get_auto_update_status` e parte come evento [`EVENT`] alla fine di ogni giro: entrambi
//! portano un [`AutoUpdateStatus`]. Nessun esito contiene la chiave CurseForge, URL o percorsi.

use std::cell::RefCell;
use std::collections::HashSet;
use std::sync::{Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::addons::cache::FingerprintCache;
use crate::addons::scan::InstalledAddon;
use crate::backup::iso_utc;
use crate::commands::{self, InstallLock, LiveSources};
use crate::installations::Installation;
use crate::sources::{ProjectKey, RecordWriter, Source, SourceKind, UpdateTarget};
use crate::store::{
    AUTO_UPDATE_INTERVALS_HOURS, AddonSettingsStore, AppSettingsStore, AutoUpdateMode,
    InstallationAutoUpdate, InstallationStore, InstalledAddonsStore, ProjectRecords, SourceRecords,
};
use crate::updates::{self, AddonUpdate, FileSummary, OutcomeStatus, UpdateOutcome, UpdateStatus};

/// Nome dell'evento Tauri emesso alla fine di ogni giro, con un [`AutoUpdateStatus`].
pub const EVENT: &str = "auto-update";

/// Quanto aspetta il primo giro dopo l'avvio: la finestra e la tray partono prima della rete.
pub const STARTUP_DELAY: Duration = Duration::from_secs(30);

/// Quanto aspetta un giro saltato perché un comando dell'utente teneva [`InstallLock`].
pub const BUSY_RETRY: Duration = Duration::from_secs(5 * 60);

/// Le attese dopo il primo, il secondo e il terzo (e ogni successivo) controllo fallito per la
/// rete, ognuna limitata dall'intervallo scelto.
pub const BACKOFF: [Duration; 3] = [
    Duration::from_secs(5 * 60),
    Duration::from_secs(15 * 60),
    Duration::from_secs(60 * 60),
];

/// Gli estremi dell'intervallo tra due giri, in ore: quelli di [`AUTO_UPDATE_INTERVALS_HOURS`].
const MIN_INTERVAL_HOURS: u32 = AUTO_UPDATE_INTERVALS_HOURS[0];
const MAX_INTERVAL_HOURS: u32 = AUTO_UPDATE_INTERVALS_HOURS[AUTO_UPDATE_INTERVALS_HOURS.len() - 1];

/// Il messaggio di un progetto con un aggiornamento che l'utente ha escluso (issue #199).
const EXCLUDED: &str = "Excluded from automatic updates.";

// ── Esito ───────────────────────────────────────────────────────────────────

/// Lo stato del giro automatico: la risposta di `get_auto_update_status` e il payload
/// dell'evento [`EVENT`]. In JSON è camelCase.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutoUpdateStatus {
    /// Un giro è in corso adesso.
    pub running: bool,
    /// Quando è previsto il prossimo giro, ISO 8601 UTC (`2026-10-27T10:00:00Z`); `null` con la
    /// modalità `off`, durante un giro e prima che il thread abbia pianificato il primo.
    pub next_round_at: Option<String>,
    /// L'ultimo giro finito; `null` prima del primo. Un giro saltato perché un comando teneva il
    /// lock non lo sostituisce.
    pub last_round: Option<RoundReport>,
}

/// Un giro finito.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoundReport {
    /// La modalità del giro: `notify` o `install` (con `off` non ci sono giri).
    pub mode: AutoUpdateMode,
    pub started_at: String,
    pub finished_at: String,
    /// Almeno un controllo è fallito per la rete: il prossimo giro è anticipato (backoff).
    pub retrying: bool,
    /// Una voce per installazione che partecipa, nell'ordine di `installations.json`; quelle
    /// escluse per intero non ci sono.
    pub installations: Vec<InstallationReport>,
}

/// L'esito di un giro su un'installazione.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallationReport {
    pub installation_id: String,
    pub installation_name: String,
    /// Quando è stato fatto il controllo, ISO 8601 UTC.
    pub checked_at: String,
    /// I progetti con un aggiornamento che partecipano (non esclusi), uno per progetto. In
    /// `notify` è l'elenco da segnalare; in `install` quello che il giro ha provato a installare.
    pub available: Vec<AvailableUpdate>,
    /// Solo `install`: i progetti aggiornati, con il file installato (`file`).
    pub updated: Vec<UpdateOutcome>,
    /// I progetti con un aggiornamento non installati e il motivo (`message`): esclusi
    /// dall'utente (`"Excluded from automatic updates."`) e, in `install`, quelli che la fonte
    /// salta (per esempio `allowModDistribution = false` senza mirror GitHub).
    pub skipped: Vec<UpdateOutcome>,
    /// Gli errori di singoli progetti o cartelle, con il messaggio di oggi: un aggiornamento
    /// fallito (su Windows un file aperto dal gioco blocca il `rename`) o una cartella che
    /// nessuna fonte ha potuto controllare. Si riprovano al giro successivo.
    pub errors: Vec<AutoUpdateError>,
    /// Il controllo dell'installazione è fallito per intero (cartella irraggiungibile, tutte le
    /// fonti senza risposta, impostazioni illeggibili): le liste sopra restano vuote, o con quello
    /// che si sapeva prima del fallimento.
    pub check_error: Option<String>,
}

/// Un progetto con un aggiornamento disponibile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AvailableUpdate {
    pub source: SourceKind,
    /// L'id del progetto nella fonte, come in `ProjectKey`.
    pub key: String,
    pub name: String,
    /// Il file che l'aggiornamento installerebbe.
    pub latest: FileSummary,
}

/// Un errore di un progetto (`source` e `key`) o di una cartella che nessuna fonte ha
/// riconosciuto (`source` e `key` nulli, `name` è la cartella).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutoUpdateError {
    pub source: Option<SourceKind>,
    pub key: Option<String>,
    pub name: String,
    pub message: String,
}

impl From<UpdateOutcome> for AutoUpdateError {
    fn from(outcome: UpdateOutcome) -> Self {
        Self {
            source: Some(outcome.source),
            key: Some(outcome.key),
            name: outcome.name,
            message: outcome.message.unwrap_or_default(),
        }
    }
}

// ── Pianificazione (pura) ───────────────────────────────────────────────────

/// Com'è finito un giro, per decidere il prossimo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoundResult {
    /// Fatto, anche con errori di singoli progetti: il prossimo dopo l'intervallo.
    Done,
    /// Saltato perché un comando teneva [`InstallLock`]: riprova dopo [`BUSY_RETRY`].
    Busy,
    /// Un controllo è fallito per la rete: riprova dopo [`BACKOFF`].
    Failed,
}

/// Quando fare il prossimo giro. I tempi sono durate dall'avvio del thread (un `Instant` di
/// base), così la pianificazione si prova senza orologio.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Schedule {
    /// La fine dell'ultimo tentativo, fatto o saltato.
    last_attempt: Option<Duration>,
    /// Com'è finito l'ultimo tentativo.
    busy: bool,
    /// I controlli falliti di fila; zero dopo un giro riuscito.
    failures: usize,
}

impl Schedule {
    /// Il prossimo giro, `None` con la modalità `off` (il thread aspetta di essere svegliato).
    /// L'intervallo si rilegge a ogni chiamata: un cambio vale subito, anche all'indietro (un
    /// giro già in ritardo parte subito). Si limita agli intervalli ammessi (da 1 a 24 ore): lo
    /// store rimette già il default su un valore sconosciuto, ma uno zero qui farebbe girare il
    /// thread senza pause e un valore enorme farebbe traboccare l'`Instant` del prossimo giro.
    pub fn next_due(&self, mode: AutoUpdateMode, interval_hours: u32) -> Option<Duration> {
        if mode == AutoUpdateMode::Off {
            return None;
        }
        let hours = interval_hours.clamp(MIN_INTERVAL_HOURS, MAX_INTERVAL_HOURS);
        let interval = Duration::from_secs(u64::from(hours) * 60 * 60);
        let Some(last) = self.last_attempt else {
            return Some(STARTUP_DELAY);
        };
        let wait = if self.busy {
            BUSY_RETRY
        } else if self.failures > 0 {
            BACKOFF[(self.failures - 1).min(BACKOFF.len() - 1)]
        } else {
            interval
        };
        Some(last + wait.min(interval))
    }

    /// Registra la fine di un tentativo, `at` dall'avvio del thread.
    pub fn finished(&mut self, at: Duration, result: RoundResult) {
        self.last_attempt = Some(at);
        self.busy = result == RoundResult::Busy;
        match result {
            RoundResult::Done => self.failures = 0,
            RoundResult::Failed => self.failures += 1,
            // Un giro saltato non dice niente sulla rete: il conteggio resta.
            RoundResult::Busy => {}
        }
    }
}

/// Se un giro va ripetuto presto: almeno una fonte ha fallito il controllo per intero, e non
/// è CurseForge senza chiave (la chiave mancante non è un errore di rete e non si risolve
/// riprovando).
pub fn needs_retry(failed: &[SourceKind], curseforge_has_key: bool) -> bool {
    failed
        .iter()
        .any(|kind| *kind != SourceKind::Curseforge || curseforge_has_key)
}

/// Quello che il controllo dice al giro di un'installazione.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Plan {
    /// I progetti da installare in `install`, nell'ordine delle cartelle e senza ripetizioni.
    pub projects: Vec<ProjectKey>,
    pub available: Vec<AvailableUpdate>,
    /// I progetti con un aggiornamento esclusi dall'utente.
    pub excluded: Vec<UpdateOutcome>,
    /// I progetti riconosciuti il cui controllo è fallito (per esempio la release GitHub di un
    /// progetto registrato che non si legge) e le cartelle che nessuna fonte ha potuto
    /// controllare.
    pub errors: Vec<AutoUpdateError>,
}

/// Filtra il controllo: un progetto con `updateAvailable` (non `betaOnly`: la Beta conta solo
/// con l'opzione, e allora il controllo dice già `updateAvailable`) partecipa se non è tra gli
/// esclusi di `auto`. Più cartelle dello stesso progetto danno una voce sola. Un'installazione
/// esclusa per intero non ha niente da fare (e il giro non la controlla nemmeno).
pub fn plan(updates: &[AddonUpdate], auto: &InstallationAutoUpdate) -> Plan {
    let mut plan = Plan::default();
    if auto.disabled {
        return plan;
    }
    let mut seen = HashSet::new();
    for update in updates {
        let (project, status) = match (&update.project, &update.status) {
            (Some(project), status) => (project, status),
            (None, UpdateStatus::Error { message }) => {
                plan.errors.push(AutoUpdateError {
                    source: None,
                    key: None,
                    name: update.folder.clone(),
                    message: message.clone(),
                });
                continue;
            }
            (None, _) => continue,
        };
        let key = ProjectKey {
            source: project.source,
            key: project.key.clone(),
        };
        let latest = match status {
            UpdateStatus::UpdateAvailable { latest } => latest,
            UpdateStatus::Error { message } => {
                if seen.insert(key.clone()) {
                    plan.errors.push(AutoUpdateError {
                        source: Some(key.source),
                        key: Some(key.key),
                        name: project.name.clone(),
                        message: message.clone(),
                    });
                }
                continue;
            }
            _ => continue,
        };
        if !seen.insert(key.clone()) {
            continue;
        }
        if auto.excluded.contains(&key) {
            plan.excluded.push(UpdateOutcome::skipped(
                key.source,
                &key.key,
                &project.name,
                EXCLUDED,
            ));
            continue;
        }
        plan.available.push(AvailableUpdate {
            source: key.source,
            key: key.key.clone(),
            name: project.name.clone(),
            latest: latest.clone(),
        });
        plan.projects.push(key);
    }
    plan
}

// ── Giro di un'installazione ────────────────────────────────────────────────

/// Una fonte che ricorda se il suo controllo è fallito per intero; per il resto passa tutto
/// alla fonte vera, senza cambiarne il risultato.
struct Watched<'a> {
    inner: &'a dyn Source,
    failed: &'a RefCell<Vec<SourceKind>>,
}

impl Source for Watched<'_> {
    fn kind(&self) -> SourceKind {
        self.inner.kind()
    }

    fn check(
        &self,
        installation: &Installation,
        offered: &[InstalledAddon],
        records: &ProjectRecords,
    ) -> Result<Vec<AddonUpdate>, String> {
        let result = self.inner.check(installation, offered, records);
        if result.is_err() {
            self.failed.borrow_mut().push(self.inner.kind());
        }
        result
    }

    fn update(&self, target: &UpdateTarget<'_>, record: &mut RecordWriter<'_>) -> UpdateOutcome {
        self.inner.update(target, record)
    }

    fn finish(&self, installation: &Installation, updates: &mut [AddonUpdate]) {
        self.inner.finish(installation, updates);
    }
}

/// Il giro su un'installazione che partecipa: controlla e, in `install`, aggiorna i soli
/// progetti di [`plan`] senza reinstallare, con `record` per il registro. Restituisce l'esito e
/// le fonti il cui controllo è fallito per intero (per [`needs_retry`]). Bloccante.
#[allow(clippy::too_many_arguments)]
pub fn run_installation(
    installation: &Installation,
    mode: AutoUpdateMode,
    auto: &InstallationAutoUpdate,
    cache: Option<&FingerprintCache>,
    sources: &[&dyn Source],
    records: &SourceRecords,
    record: &mut RecordWriter<'_>,
    checked_at: String,
) -> (InstallationReport, Vec<SourceKind>) {
    let failed = RefCell::new(Vec::new());
    let watched: Vec<Watched<'_>> = sources
        .iter()
        .map(|inner| Watched {
            inner: *inner,
            failed: &failed,
        })
        .collect();
    let watched: Vec<&dyn Source> = watched.iter().map(|s| s as &dyn Source).collect();
    let mut report = InstallationReport {
        installation_id: installation.id.clone(),
        installation_name: installation.name.clone(),
        checked_at,
        available: Vec::new(),
        updated: Vec::new(),
        skipped: Vec::new(),
        errors: Vec::new(),
        check_error: None,
    };
    match updates::check_updates_with(installation, cache, &watched, records) {
        Err(error) => report.check_error = Some(error),
        Ok(found) => {
            let plan = plan(&found, auto);
            report.available = plan.available;
            report.skipped = plan.excluded;
            report.errors = plan.errors;
            if mode == AutoUpdateMode::Install && !plan.projects.is_empty() {
                // `update_addons_with` rifà il controllo: un fallimento lì non tocca niente.
                match updates::update_addons_with(
                    installation,
                    cache,
                    &watched,
                    records,
                    &plan.projects,
                    false,
                    record,
                    &|_| {},
                ) {
                    Err(error) => report.check_error = Some(error),
                    Ok(outcomes) => {
                        for outcome in outcomes {
                            match outcome.status {
                                OutcomeStatus::Updated => report.updated.push(outcome),
                                OutcomeStatus::Skipped => report.skipped.push(outcome),
                                OutcomeStatus::Error => report.errors.push(outcome.into()),
                            }
                        }
                    }
                }
            }
        }
    }
    (report, failed.into_inner())
}

// ── Thread ──────────────────────────────────────────────────────────────────

/// Lo stato condiviso tra il thread del giro e i comandi (stato gestito da Tauri).
#[derive(Default)]
pub struct AutoUpdate {
    shared: Mutex<Shared>,
    wake: Condvar,
}

#[derive(Default)]
struct Shared {
    status: AutoUpdateStatus,
    /// Le impostazioni sono cambiate: il thread ripianifica.
    woken: bool,
}

impl AutoUpdate {
    /// Lo stato attuale (`get_auto_update_status`).
    pub fn status(&self) -> AutoUpdateStatus {
        self.lock().status.clone()
    }

    /// Fa rileggere modalità e intervallo al thread (`set_auto_update`): con la modalità che
    /// passa da `off` a un'altra il giro riparte senza aspettare il riavvio.
    pub fn wake(&self) {
        self.lock().woken = true;
        self.wake.notify_all();
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Shared> {
        // Lo stato è solo un esito da mostrare: un panic non lo lascia incoerente.
        self.shared.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn update(&self, change: impl FnOnce(&mut AutoUpdateStatus)) -> AutoUpdateStatus {
        let mut shared = self.lock();
        change(&mut shared.status);
        shared.status.clone()
    }

    /// Aspetta fino a `due` (per sempre con `None`). `true` se è arrivata l'ora, `false` se un
    /// comando l'ha svegliato prima.
    fn sleep_until(&self, due: Option<Instant>) -> bool {
        let mut shared = self.lock();
        loop {
            if shared.woken {
                shared.woken = false;
                return false;
            }
            shared = match due {
                None => self
                    .wake
                    .wait(shared)
                    .unwrap_or_else(PoisonError::into_inner),
                Some(due) => {
                    let now = Instant::now();
                    if now >= due {
                        return true;
                    }
                    self.wake
                        .wait_timeout(shared, due - now)
                        .unwrap_or_else(PoisonError::into_inner)
                        .0
                }
            };
        }
    }
}

/// Avvia il thread del giro; va chiamata in `setup`, dopo aver registrato gli stati.
pub fn start(app: AppHandle) {
    let spawned = std::thread::Builder::new()
        .name("auto-update".to_owned())
        .spawn(move || run(&app));
    if spawned.is_err() {
        eprintln!("Automatic updates are not available: the background thread did not start.");
    }
}

/// La modalità e l'intervallo salvati. Un `settings.json` illeggibile vale `off`: senza sapere
/// cosa ha scelto l'utente il giro non usa la rete e non tocca il disco.
fn read_settings(app: &AppHandle) -> (AutoUpdateMode, u32) {
    app.state::<Mutex<AppSettingsStore>>()
        .lock()
        .ok()
        .and_then(|store| store.get().ok())
        .map_or((AutoUpdateMode::Off, 0), |settings| {
            (settings.auto_update, settings.auto_update_interval_hours)
        })
}

fn run(app: &AppHandle) {
    let state = app.state::<AutoUpdate>();
    let base = Instant::now();
    let mut schedule = Schedule::default();
    loop {
        let (mode, interval_hours) = read_settings(app);
        let due = schedule.next_due(mode, interval_hours);
        state.update(|status| {
            status.next_round_at = due.map(|due| {
                let wait = due.saturating_sub(base.elapsed());
                iso_utc(SystemTime::now() + wait)
            });
        });
        if !state.sleep_until(due.map(|due| base + due)) {
            continue;
        }
        // Le impostazioni possono essere cambiate durante l'attesa senza svegliare il thread
        // (per esempio un file riscritto da fuori): si rileggono.
        let (mode, _) = read_settings(app);
        if mode == AutoUpdateMode::Off {
            continue;
        }
        let result = round(app, &state, mode);
        schedule.finished(base.elapsed(), result);
    }
}

/// Un giro su tutte le installazioni che partecipano, sotto [`InstallLock`].
fn round(app: &AppHandle, state: &AutoUpdate, mode: AutoUpdateMode) -> RoundResult {
    let lock = app.state::<InstallLock>();
    let Some(_held) = lock.try_hold() else {
        return RoundResult::Busy;
    };
    let started_at = iso_utc(SystemTime::now());
    state.update(|status| {
        status.running = true;
        status.next_round_at = None;
    });
    let (installations, retrying) = round_installations(app, mode);
    let status = state.update(|status| {
        status.running = false;
        status.last_round = Some(RoundReport {
            mode,
            started_at,
            finished_at: iso_utc(SystemTime::now()),
            retrying,
            installations,
        });
    });
    // Un evento che non parte non ferma il giro: l'esito resta leggibile con il comando.
    let _ = app.emit(EVENT, &status);
    if retrying {
        RoundResult::Failed
    } else {
        RoundResult::Done
    }
}

fn round_installations(app: &AppHandle, mode: AutoUpdateMode) -> (Vec<InstallationReport>, bool) {
    let installations = match app.state::<Mutex<InstallationStore>>().lock() {
        Ok(store) => store.list().unwrap_or_default(),
        Err(_) => Vec::new(),
    };
    if installations.is_empty() {
        return (Vec::new(), false);
    }
    let key = app
        .state::<crate::config::Config>()
        .curseforge_api_key
        .clone();
    let settings = app.state::<Mutex<AddonSettingsStore>>();
    let installed = app.state::<Mutex<InstalledAddonsStore>>();
    let cache = app.state::<FingerprintCache>();
    let allow_beta = commands::beta_when_no_release(&app.state::<Mutex<AppSettingsStore>>());
    let sources = match LiveSources::new(key) {
        Ok(sources) => sources,
        Err(error) => {
            // Senza fonti nessuna installazione si controlla: vale come un controllo fallito.
            let reports = installations
                .iter()
                .map(|installation| failed_report(installation, error.clone()))
                .collect();
            return (reports, true);
        }
    };
    let mut reports = Vec::new();
    let mut retrying = false;
    for installation in &installations {
        let auto = settings
            .lock()
            .map_err(|_| "Addon settings are not available: restart the app.".to_owned())
            .and_then(|store| store.auto_update_for(&installation.id));
        let auto = match auto {
            Ok(auto) if auto.disabled => continue,
            Ok(auto) => auto,
            // Senza le esclusioni non si sa cosa l'utente vuole lasciare com'è: niente giro.
            Err(error) => {
                reports.push(failed_report(installation, error));
                continue;
            }
        };
        let overrides = match commands::overrides_for(&settings, &installation.id) {
            Ok(overrides) => overrides,
            Err(error) => {
                reports.push(failed_report(installation, error));
                continue;
            }
        };
        let records = commands::records_for_matching(&installed, &installation.id);
        let mut record = commands::record_writer(&installed, &cache, installation);
        let (report, failed) = sources.with(&overrides, allow_beta, |sources| {
            run_installation(
                installation,
                mode,
                &auto,
                Some(&cache),
                sources,
                &records,
                &mut record,
                iso_utc(SystemTime::now()),
            )
        });
        retrying |= needs_retry(&failed, sources.has_curseforge_key());
        reports.push(report);
    }
    (reports, retrying)
}

fn failed_report(installation: &Installation, error: String) -> InstallationReport {
    InstallationReport {
        installation_id: installation.id.clone(),
        installation_name: installation.name.clone(),
        checked_at: iso_utc(SystemTime::now()),
        available: Vec::new(),
        updated: Vec::new(),
        skipped: Vec::new(),
        errors: Vec::new(),
        check_error: Some(error),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use super::*;
    use crate::store::SourceRecord;
    use crate::test_support::TempDir;
    use crate::updates::Project;

    const MIN: Duration = Duration::from_secs(60);
    const HOUR: Duration = Duration::from_secs(60 * 60);

    fn finished(results: &[RoundResult]) -> Schedule {
        let mut schedule = Schedule::default();
        for (i, result) in results.iter().enumerate() {
            schedule.finished(HOUR * (i as u32 + 1), *result);
        }
        schedule
    }

    #[test]
    fn the_first_round_comes_shortly_after_start_and_then_every_interval() {
        let schedule = Schedule::default();
        assert_eq!(
            schedule.next_due(AutoUpdateMode::Notify, 6),
            Some(STARTUP_DELAY)
        );
        let schedule = finished(&[RoundResult::Done]);
        assert_eq!(
            schedule.next_due(AutoUpdateMode::Install, 6),
            Some(HOUR * 7)
        );
        // L'intervallo si rilegge a ogni giro.
        assert_eq!(
            schedule.next_due(AutoUpdateMode::Install, 1),
            Some(HOUR * 2)
        );
    }

    #[test]
    fn an_interval_outside_the_allowed_ones_is_clamped() {
        let schedule = finished(&[RoundResult::Done]);
        assert_eq!(schedule.next_due(AutoUpdateMode::Notify, 0), Some(HOUR * 2));
        assert_eq!(
            schedule.next_due(AutoUpdateMode::Notify, u32::MAX),
            Some(HOUR * 25)
        );
        // Anche il backoff resta sopra zero con un intervallo nullo.
        let schedule = finished(&[RoundResult::Failed]);
        assert_eq!(
            schedule.next_due(AutoUpdateMode::Notify, 0),
            Some(HOUR + MIN * 5)
        );
    }

    #[test]
    fn with_the_mode_off_nothing_is_planned() {
        assert_eq!(Schedule::default().next_due(AutoUpdateMode::Off, 6), None);
        let schedule = finished(&[RoundResult::Failed]);
        assert_eq!(schedule.next_due(AutoUpdateMode::Off, 6), None);
    }

    #[test]
    fn failed_checks_back_off_and_a_good_round_resets_it() {
        use RoundResult::{Done, Failed};
        let due = |results: &[RoundResult], hours| {
            let schedule = finished(results);
            schedule.next_due(AutoUpdateMode::Notify, hours).unwrap() - HOUR * results.len() as u32
        };
        assert_eq!(due(&[Failed], 6), MIN * 5);
        assert_eq!(due(&[Failed, Failed], 6), MIN * 15);
        assert_eq!(due(&[Failed, Failed, Failed], 6), MIN * 60);
        assert_eq!(due(&[Failed, Failed, Failed, Failed], 6), MIN * 60);
        assert_eq!(due(&[Failed, Failed, Done], 6), HOUR * 6);
        assert_eq!(due(&[Failed, Done, Failed], 6), MIN * 5);
        // Mai oltre l'intervallo.
        assert_eq!(due(&[Failed, Failed, Failed], 1), HOUR);
    }

    #[test]
    fn a_busy_lock_retries_in_five_minutes_and_keeps_the_backoff() {
        use RoundResult::{Busy, Failed};
        let schedule = finished(&[Busy]);
        assert_eq!(
            schedule.next_due(AutoUpdateMode::Install, 6),
            Some(HOUR + BUSY_RETRY)
        );
        // Dopo il giro saltato, un nuovo fallimento è il secondo di fila.
        let schedule = finished(&[Failed, Busy, Failed]);
        assert_eq!(
            schedule.next_due(AutoUpdateMode::Install, 6),
            Some(HOUR * 3 + MIN * 15)
        );
    }

    #[test]
    fn a_missing_curseforge_key_is_not_a_network_failure() {
        assert!(!needs_retry(&[], true));
        assert!(!needs_retry(&[SourceKind::Curseforge], false));
        assert!(needs_retry(&[SourceKind::Curseforge], true));
        assert!(needs_retry(
            &[SourceKind::Curseforge, SourceKind::Tukui],
            false
        ));
    }

    fn summary(id: u32) -> FileSummary {
        FileSummary {
            id,
            name: format!("file {id}"),
            date: "2026-10-27T00:00:00Z".to_owned(),
        }
    }

    fn row(folder: &str, project: Option<(SourceKind, &str)>, status: UpdateStatus) -> AddonUpdate {
        AddonUpdate {
            folder: folder.to_owned(),
            source: project.map(|(source, _)| source),
            project: project.map(|(source, key)| Project {
                id: 0,
                source,
                key: key.to_owned(),
                name: format!("{key} name"),
                url: None,
                allow_mod_distribution: true,
            }),
            installed_file: None,
            status,
            version_override: None,
            project_folders: Vec::new(),
            installed_modules: Vec::new(),
            reinstall_file: None,
            mirror: Default::default(),
        }
    }

    fn available(id: u32) -> UpdateStatus {
        UpdateStatus::UpdateAvailable {
            latest: summary(id),
        }
    }

    fn key(source: SourceKind, key: &str) -> ProjectKey {
        ProjectKey {
            source,
            key: key.to_owned(),
        }
    }

    #[test]
    fn only_updates_that_are_not_excluded_are_planned_once_per_project() {
        const CF: SourceKind = SourceKind::Curseforge;
        const GH: SourceKind = SourceKind::Github;
        let rows = [
            row("DBM-Core", Some((CF, "3358")), available(2)),
            row("DBM-GUI", Some((CF, "3358")), available(2)),
            row("Details", Some((CF, "61284")), UpdateStatus::UpToDate),
            row(
                "Beta",
                Some((CF, "9")),
                UpdateStatus::BetaOnly { latest: summary(3) },
            ),
            row("BigWigs", Some((GH, "owner/repo")), available(4)),
            row("Plater", Some((CF, "100")), available(5)),
            row(
                "Lonely",
                None,
                UpdateStatus::Error {
                    message: "CurseForge check failed: offline".to_owned(),
                },
            ),
            row("Unknown", None, UpdateStatus::NotMatched),
            row(
                "Recorded",
                Some((GH, "other/repo")),
                UpdateStatus::Error {
                    message: "GitHub did not respond in time.".to_owned(),
                },
            ),
        ];
        let auto = InstallationAutoUpdate {
            disabled: false,
            excluded: [key(CF, "100")].into(),
        };
        let plan = plan(&rows, &auto);
        assert_eq!(plan.projects, [key(CF, "3358"), key(GH, "owner/repo")]);
        assert_eq!(
            plan.available
                .iter()
                .map(|a| (a.name.as_str(), a.latest.id))
                .collect::<Vec<_>>(),
            [("3358 name", 2), ("owner/repo name", 4)]
        );
        assert_eq!(plan.excluded.len(), 1);
        assert_eq!(plan.excluded[0].key, "100");
        assert_eq!(plan.excluded[0].status, OutcomeStatus::Skipped);
        assert_eq!(plan.excluded[0].message.as_deref(), Some(EXCLUDED));
        // Gli errori delle cartelle senza fonte e quelli dei progetti riconosciuti.
        assert_eq!(
            plan.errors,
            [
                AutoUpdateError {
                    source: None,
                    key: None,
                    name: "Lonely".to_owned(),
                    message: "CurseForge check failed: offline".to_owned(),
                },
                AutoUpdateError {
                    source: Some(GH),
                    key: Some("other/repo".to_owned()),
                    name: "other/repo name".to_owned(),
                    message: "GitHub did not respond in time.".to_owned(),
                },
            ]
        );

        // Senza aggiornamenti non c'è niente da fare.
        let nothing = plan_of(&[row("Details", Some((CF, "61284")), UpdateStatus::UpToDate)]);
        assert_eq!(nothing, Plan::default());

        // Un'installazione esclusa per intero non pianifica niente.
        let disabled = InstallationAutoUpdate {
            disabled: true,
            excluded: Default::default(),
        };
        assert_eq!(super::plan(&rows, &disabled), Plan::default());
    }

    fn plan_of(rows: &[AddonUpdate]) -> Plan {
        plan(rows, &InstallationAutoUpdate::default())
    }

    #[test]
    fn try_hold_does_not_wait_for_a_held_lock() {
        let lock = InstallLock::default();
        let held = lock.try_hold().expect("free lock");
        assert!(lock.try_hold().is_none());
        drop(held);
        assert!(lock.try_hold().is_some());
    }

    #[test]
    fn the_status_payload_is_camel_case() {
        let status = AutoUpdateStatus {
            running: false,
            next_round_at: Some("2026-10-27T10:00:00Z".to_owned()),
            last_round: Some(RoundReport {
                mode: AutoUpdateMode::Install,
                started_at: "a".to_owned(),
                finished_at: "b".to_owned(),
                retrying: true,
                installations: vec![InstallationReport {
                    installation_id: "id".to_owned(),
                    installation_name: "Retail".to_owned(),
                    checked_at: "c".to_owned(),
                    available: vec![AvailableUpdate {
                        source: SourceKind::Github,
                        key: "owner/repo".to_owned(),
                        name: "Addon".to_owned(),
                        latest: summary(1),
                    }],
                    updated: Vec::new(),
                    skipped: Vec::new(),
                    errors: Vec::new(),
                    check_error: None,
                }],
            }),
        };
        let json = serde_json::to_value(&status).unwrap();
        assert_eq!(json["nextRoundAt"], "2026-10-27T10:00:00Z");
        let round = &json["lastRound"];
        assert_eq!(round["mode"], "install");
        assert_eq!(round["retrying"], true);
        let installation = &round["installations"][0];
        assert_eq!(installation["installationId"], "id");
        assert_eq!(installation["checkError"], serde_json::Value::Null);
        assert_eq!(installation["available"][0]["source"], "github");
        assert_eq!(installation["available"][0]["latest"]["id"], 1);
    }

    /// Una fonte finta: reclama le cartelle di `claims` con un aggiornamento (progetto =
    /// cartella), fallisce il controllo con `fails`, e all'aggiornamento dà un errore per i
    /// progetti di `broken`. Ricorda i progetti che le chiedono di aggiornare.
    struct FakeSource {
        kind: SourceKind,
        claims: Vec<&'static str>,
        broken: Vec<&'static str>,
        fails: bool,
        asked: RefCell<Vec<String>>,
    }

    impl FakeSource {
        fn new(kind: SourceKind, claims: &[&'static str]) -> Self {
            Self {
                kind,
                claims: claims.to_vec(),
                broken: Vec::new(),
                fails: false,
                asked: RefCell::new(Vec::new()),
            }
        }
    }

    impl Source for FakeSource {
        fn kind(&self) -> SourceKind {
            self.kind
        }

        fn check(
            &self,
            _installation: &Installation,
            offered: &[InstalledAddon],
            _records: &ProjectRecords,
        ) -> Result<Vec<AddonUpdate>, String> {
            if self.fails {
                return Err("Could not reach the source: check the connection.".to_owned());
            }
            Ok(offered
                .iter()
                .filter(|a| self.claims.contains(&a.folder.as_str()))
                .map(|a| row(&a.folder, Some((self.kind, &a.folder)), available(2)))
                .collect())
        }

        fn update(
            &self,
            target: &UpdateTarget<'_>,
            record: &mut RecordWriter<'_>,
        ) -> UpdateOutcome {
            self.asked.borrow_mut().push(target.key.to_owned());
            if self.broken.contains(&target.key) {
                // Il messaggio di oggi per un file aperto dal gioco su Windows.
                return UpdateOutcome::error(
                    self.kind,
                    target.key,
                    target.key,
                    "Could not replace the addon folder: a file is in use.".to_owned(),
                );
            }
            record(
                self.kind,
                target.key,
                SourceRecord {
                    file: "2".to_owned(),
                    folders: vec![target.key.to_owned()],
                },
            )
            .unwrap();
            UpdateOutcome::updated(self.kind, target.key, target.key, summary(2), None)
        }
    }

    fn installation_with(tmp: &TempDir, folders: &[&str]) -> Installation {
        let path = tmp.path().join("World of Warcraft").join("_retail_");
        let addons = path.join("Interface").join("AddOns");
        for folder in folders {
            write_addon(&addons, folder);
        }
        Installation {
            id: "test".to_owned(),
            path,
            flavor: "retail".to_owned(),
            name: "Retail".to_owned(),
        }
    }

    fn write_addon(addons: &Path, folder: &str) {
        let dir = addons.join(folder);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join(format!("{folder}.toc")),
            format!("## Interface: 120000\n## Title: {folder}\n{folder}.lua\n"),
        )
        .unwrap();
        fs::write(dir.join(format!("{folder}.lua")), folder).unwrap();
    }

    fn run(
        installation: &Installation,
        mode: AutoUpdateMode,
        auto: &InstallationAutoUpdate,
        sources: &[&dyn Source],
    ) -> (InstallationReport, Vec<SourceKind>, Vec<String>) {
        let mut written = Vec::new();
        let (report, failed) = run_installation(
            installation,
            mode,
            auto,
            None,
            sources,
            &SourceRecords::new(),
            &mut |_, key, _| {
                written.push(key.to_owned());
                Ok(())
            },
            "2026-10-27T10:00:00Z".to_owned(),
        );
        (report, failed, written)
    }

    #[test]
    fn notify_only_checks() {
        let tmp = TempDir::new();
        let installation = installation_with(&tmp, &["Alpha", "Beta"]);
        let source = FakeSource::new(SourceKind::Tukui, &["Alpha", "Beta"]);
        let auto = InstallationAutoUpdate {
            disabled: false,
            excluded: [key(SourceKind::Tukui, "Beta")].into(),
        };
        let (report, failed, written) =
            run(&installation, AutoUpdateMode::Notify, &auto, &[&source]);
        assert_eq!(report.installation_name, "Retail");
        assert_eq!(report.checked_at, "2026-10-27T10:00:00Z");
        assert_eq!(report.available.len(), 1);
        assert_eq!(report.available[0].key, "Alpha");
        assert_eq!(report.skipped.len(), 1);
        assert_eq!(report.skipped[0].message.as_deref(), Some(EXCLUDED));
        assert!(report.updated.is_empty());
        assert!(report.check_error.is_none());
        assert!(failed.is_empty());
        assert!(source.asked.borrow().is_empty());
        assert!(written.is_empty());
    }

    #[test]
    fn install_updates_the_planned_projects_and_a_failure_does_not_stop_the_others() {
        let tmp = TempDir::new();
        let installation = installation_with(&tmp, &["Alpha", "Beta", "Gamma", "Delta"]);
        let mut source = FakeSource::new(SourceKind::Tukui, &["Alpha", "Beta", "Gamma"]);
        source.broken = vec!["Alpha"];
        let auto = InstallationAutoUpdate {
            disabled: false,
            excluded: [key(SourceKind::Tukui, "Gamma")].into(),
        };
        let (report, failed, written) =
            run(&installation, AutoUpdateMode::Install, &auto, &[&source]);
        // Solo i progetti con un aggiornamento e non esclusi; "Delta" nessuno lo riconosce.
        assert_eq!(*source.asked.borrow(), ["Alpha", "Beta"]);
        assert_eq!(written, ["Beta"]);
        assert_eq!(report.updated.len(), 1);
        assert_eq!(report.updated[0].key, "Beta");
        assert_eq!(report.updated[0].file.as_ref().map(|f| f.id), Some(2));
        assert_eq!(
            report.errors,
            [AutoUpdateError {
                source: Some(SourceKind::Tukui),
                key: Some("Alpha".to_owned()),
                name: "Alpha".to_owned(),
                message: "Could not replace the addon folder: a file is in use.".to_owned(),
            }]
        );
        assert_eq!(report.skipped.len(), 1);
        assert_eq!(report.skipped[0].key, "Gamma");
        assert!(failed.is_empty());
    }

    #[test]
    fn a_failed_source_is_reported_for_the_backoff() {
        let tmp = TempDir::new();
        let installation = installation_with(&tmp, &["Alpha"]);
        let mut offline = FakeSource::new(SourceKind::Curseforge, &[]);
        offline.fails = true;
        let tukui = FakeSource::new(SourceKind::Tukui, &["Alpha"]);
        let (report, failed, _) = run(
            &installation,
            AutoUpdateMode::Install,
            &InstallationAutoUpdate::default(),
            &[&offline, &tukui],
        );
        // Una fonte che fallisce non ferma le altre; il fallimento si conta una volta per
        // controllo (qui due: quello del giro e quello di `update_addons_with`).
        assert_eq!(report.updated.len(), 1);
        assert_eq!(failed, [SourceKind::Curseforge, SourceKind::Curseforge]);
        assert!(needs_retry(&failed, true));
        assert!(!needs_retry(&failed, false));

        // Se falliscono tutte, il controllo fallisce per intero e niente si installa.
        let (report, failed, written) = run(
            &installation,
            AutoUpdateMode::Install,
            &InstallationAutoUpdate::default(),
            &[&offline],
        );
        assert_eq!(
            report.check_error.as_deref(),
            Some("Could not reach the source: check the connection.")
        );
        assert!(report.available.is_empty());
        assert!(written.is_empty());
        assert_eq!(failed, [SourceKind::Curseforge]);
    }
}
