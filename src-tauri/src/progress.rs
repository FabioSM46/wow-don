//! Avanzamento di installazioni e aggiornamenti (issue #127): le fasi di un progetto e i byte
//! scaricati, consegnati a un callback. I moduli che installano non conoscono Tauri: il comando
//! collega il callback all'evento `install-progress` della finestra.

use std::cell::Cell;
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::sources::SourceKind;

/// Nome dell'evento Tauri che porta un [`Progress`] al frontend.
pub const EVENT: &str = "install-progress";

/// Tempo minimo tra due avanzamenti del download dello stesso progetto: al massimo circa dieci
/// eventi al secondo, più quello finale.
pub const MIN_INTERVAL: Duration = Duration::from_millis(100);

/// La fase di un progetto.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Phase {
    /// Lettura dell'installazione e delle informazioni del progetto, prima del download.
    Checking,
    /// Download dello zip: `received` e `total` valgono solo qui.
    Downloading,
    /// Estrazione e controllo dello zip in una cartella temporanea.
    Extracting,
    /// Sostituzione delle cartelle in `AddOns`.
    Installing,
}

/// Un passo dell'avanzamento. Contiene solo l'identità del progetto e i conteggi: niente URL,
/// chiavi, header o percorsi del disco.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    /// La fonte del progetto; `None` per uno zip scelto dall'utente, che non ha fonte.
    pub source: Option<SourceKind>,
    /// L'id del progetto nella fonte (vuoto per uno zip scelto dall'utente).
    pub key: String,
    /// Il nome del progetto, vuoto finché non è noto.
    pub name: String,
    pub phase: Phase,
    /// Byte ricevuti, solo in [`Phase::Downloading`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub received: Option<u64>,
    /// Byte attesi, solo in [`Phase::Downloading`] e solo se il server li dichiara.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<u64>,
}

/// Il payload dell'evento [`EVENT`]: l'avanzamento con l'installazione a cui appartiene.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgressEvent<'a> {
    pub installation_id: &'a str,
    #[serde(flatten)]
    pub progress: Progress,
}

/// Dove vanno gli avanzamenti.
pub type ProgressSink<'a> = dyn Fn(Progress) + 'a;

/// Un callback che non fa niente, per i test che non guardano l'avanzamento.
#[cfg(test)]
pub fn ignore(_: Progress) {}

/// L'avanzamento di un progetto: chiama `sink` con la sua identità e limita la frequenza degli
/// avanzamenti del download.
pub struct Reporter<'a> {
    sink: &'a ProgressSink<'a>,
    source: Option<SourceKind>,
    key: &'a str,
    name: &'a str,
    throttle: Throttle,
    /// L'ultimo `total` ricevuto dal download: quello che il server ha dichiarato, se l'ha fatto.
    declared_total: Cell<Option<u64>>,
}

impl<'a> Reporter<'a> {
    pub fn new(
        sink: &'a ProgressSink<'a>,
        source: Option<SourceKind>,
        key: &'a str,
        name: &'a str,
    ) -> Self {
        Self {
            sink,
            source,
            key,
            name,
            throttle: Throttle::new(MIN_INTERVAL),
            declared_total: Cell::new(None),
        }
    }

    /// Una fase senza conteggi.
    pub fn phase(&self, phase: Phase) {
        self.send(phase, None, None);
    }

    /// Byte ricevuti finora, se è passato abbastanza tempo dall'ultimo avanzamento.
    pub fn download(&self, received: u64, total: Option<u64>) {
        self.download_at(Instant::now(), received, total);
    }

    /// Come [`download`](Self::download), con l'orologio passato dal chiamante.
    pub fn download_at(&self, now: Instant, received: u64, total: Option<u64>) {
        self.declared_total.set(total);
        if self.throttle.ready(now) {
            self.send(Phase::Downloading, Some(received), total);
        }
    }

    /// Download finito: sempre consegnato, con il `total` dichiarato dal server se c'era. Senza
    /// `Content-Length` resta assente anche qui: la lunghezza non era nota.
    pub fn downloaded(&self, len: u64) {
        self.send(Phase::Downloading, Some(len), self.declared_total.get());
    }

    fn send(&self, phase: Phase, received: Option<u64>, total: Option<u64>) {
        (self.sink)(Progress {
            source: self.source,
            key: self.key.to_owned(),
            name: self.name.to_owned(),
            phase,
            received,
            total,
        });
    }
}

/// Lascia passare un avanzamento al primo colpo e poi solo quando è passato `interval`
/// dall'ultimo che ha lasciato passare. L'orologio lo passa il chiamante, così i test non
/// dipendono dal tempo.
pub struct Throttle {
    interval: Duration,
    last: Cell<Option<Instant>>,
}

impl Throttle {
    pub fn new(interval: Duration) -> Self {
        Self {
            interval,
            last: Cell::new(None),
        }
    }

    pub fn ready(&self, now: Instant) -> bool {
        let ready = self
            .last
            .get()
            .is_none_or(|last| now.saturating_duration_since(last) >= self.interval);
        if ready {
            self.last.set(Some(now));
        }
        ready
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::cell::RefCell;

    /// Raccoglie gli avanzamenti consegnati, per i test degli altri moduli.
    #[derive(Default)]
    pub struct Collected(pub RefCell<Vec<Progress>>);

    impl Collected {
        pub fn sink(&self) -> impl Fn(Progress) + '_ {
            |progress| self.0.borrow_mut().push(progress)
        }

        /// (key, fase) degli avanzamenti nell'ordine, con i passi consecutivi uguali uniti: quanti
        /// passi del download passano dipende dal tempo.
        pub fn phases(&self) -> Vec<(String, Phase)> {
            let mut phases: Vec<(String, Phase)> = Vec::new();
            for p in self.0.borrow().iter() {
                let step = (p.key.clone(), p.phase);
                if phases.last() != Some(&step) {
                    phases.push(step);
                }
            }
            phases
        }

        /// L'ultimo passo del download di `key`: (ricevuti, attesi).
        pub fn last_download(&self, key: &str) -> Option<(Option<u64>, Option<u64>)> {
            self.0
                .borrow()
                .iter()
                .rev()
                .find(|p| p.key == key && p.phase == Phase::Downloading)
                .map(|p| (p.received, p.total))
        }
    }

    #[test]
    fn the_throttle_lets_the_first_through_then_waits_the_interval() {
        let throttle = Throttle::new(Duration::from_millis(100));
        let start = Instant::now();
        assert!(throttle.ready(start));
        assert!(!throttle.ready(start + Duration::from_millis(50)));
        assert!(!throttle.ready(start + Duration::from_millis(99)));
        assert!(throttle.ready(start + Duration::from_millis(100)));
        assert!(!throttle.ready(start + Duration::from_millis(150)));
        assert!(throttle.ready(start + Duration::from_millis(250)));
    }

    #[test]
    fn the_last_download_step_is_always_delivered() {
        let collected = Collected::default();
        let sink = collected.sink();
        let reporter = Reporter::new(&sink, Some(SourceKind::Curseforge), "61284", "Bagnon");
        let start = Instant::now();
        for (ms, received) in [(0, 10), (40, 20), (80, 30)] {
            reporter.download_at(start + Duration::from_millis(ms), received, Some(100));
        }
        reporter.downloaded(100);
        let steps = collected.0.borrow();
        // Il primo passa, i successivi entro 100 ms no, quello finale sempre.
        assert_eq!(steps.len(), 2);
        assert_eq!((steps[0].received, steps[0].total), (Some(10), Some(100)));
        assert_eq!((steps[1].received, steps[1].total), (Some(100), Some(100)));
    }

    #[test]
    fn without_a_declared_length_the_last_step_has_no_total() {
        let collected = Collected::default();
        let sink = collected.sink();
        let reporter = Reporter::new(&sink, Some(SourceKind::Tukui), "-2", "ElvUI");
        reporter.download_at(Instant::now(), 0, None);
        reporter.downloaded(4096);
        let last = collected.0.borrow().last().cloned().unwrap();
        assert_eq!((last.received, last.total), (Some(4096), None));
    }

    #[test]
    fn the_payload_has_only_identity_phase_and_counts() {
        let progress = Progress {
            source: Some(SourceKind::Tukui),
            key: "-2".to_owned(),
            name: "ElvUI".to_owned(),
            phase: Phase::Downloading,
            received: Some(5),
            total: None,
        };
        let event = ProgressEvent {
            installation_id: "abc",
            progress,
        };
        let json = serde_json::to_value(&event).unwrap();
        let mut keys: Vec<&str> = json
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "installationId",
                "key",
                "name",
                "phase",
                "received",
                "source"
            ]
        );
        assert_eq!(json["phase"], "downloading");
        assert_eq!(json["source"], "tukui");
    }
}
