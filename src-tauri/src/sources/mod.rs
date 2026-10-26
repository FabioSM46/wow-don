//! Fonti degli addon dietro un'interfaccia comune (D12). CurseForge è la prima implementazione
//! (`updates::CurseForgeSource`); tra le fonti libere (D31) c'è Tukui ([`tukui`]), WoWInterface e
//! GitHub arriveranno con le issue #107 e #108.
//!
//! Il controllo degli aggiornamenti (`updates::check_updates`) offre le cartelle installate alle
//! fonti nell'ordine di D33: una cartella registrata in `installed-addons.json` va solo alla fonte
//! del record, le altre a tutte, nell'ordine dell'elenco (CurseForge prima delle fonti libere);
//! una cartella reclamata da una fonte non si offre alle successive.

use std::path::Path;

use serde::Deserialize;

use crate::addons::scan::InstalledAddon;
use crate::installations::Installation;
use crate::progress::ProgressSink;
use crate::store::{ProjectRecords, SourceRecord};
use crate::updates::{AddonUpdate, UpdateOutcome};

pub mod curseforge;
pub mod http;
pub mod tukui;

pub use crate::store::SourceKind;

/// Un progetto di una fonte: la fonte e l'id del progetto in quella fonte, come stringa
/// (`61284` per CurseForge, `owner/repo` per GitHub, `-2` per ElvUI su Tukui).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Deserialize)]
pub struct ProjectKey {
    pub source: SourceKind,
    pub key: String,
}

/// Quello che serve a una fonte per aggiornare un progetto: il controllo appena rifatto su
/// tutta l'installazione.
pub struct UpdateTarget<'a> {
    /// L'id del progetto nella fonte.
    pub key: &'a str,
    pub addons_dir: &'a Path,
    /// La scansione di `Interface/AddOns`.
    pub addons: &'a [InstalledAddon],
    /// Il controllo di tutte le fonti, una voce per cartella.
    pub updates: &'a [AddonUpdate],
    /// Dove riportare download, estrazione e installazione del progetto (issue #127).
    pub progress: &'a ProgressSink<'a>,
    /// Reinstallare anche un progetto già aggiornato (issue #148): la fonte scarica il file che
    /// sceglierebbe per un aggiornamento, anche se è la versione installata.
    pub reinstall: bool,
}

impl UpdateTarget<'_> {
    /// L'esito di un progetto che il controllo non ha trovato tra le cartelle installate. Per una
    /// reinstallazione il motivo è che nessuna cartella è legata a una fonte con quel progetto.
    pub fn not_found(&self, source: SourceKind) -> UpdateOutcome {
        let message = if self.reinstall {
            "Not linked to a source: cannot reinstall."
        } else {
            "Not found among the installed addons."
        };
        UpdateOutcome::skipped(source, self.key, "", message)
    }
}

/// Scrive in `installed-addons.json` quello che una fonte ha appena installato.
pub type RecordWriter<'a> = dyn FnMut(SourceKind, &str, SourceRecord) -> Result<(), String> + 'a;

/// Una fonte di addon.
pub trait Source {
    fn kind(&self) -> SourceKind;

    /// Abbina le cartelle `offered` ai progetti della fonte e valuta gli aggiornamenti. Solo
    /// una voce con un progetto reclama la cartella. Una voce con stato `error` e senza progetto
    /// non la reclama: la cartella resta alle fonti successive e l'errore si mostra solo se
    /// nessuna la riconosce. Le cartelle che la fonte non riconosce si possono omettere o
    /// restituire come `notMatched`. `records` sono i record
    /// della fonte per l'installazione. Un `Err` vale per tutte le cartelle offerte.
    fn check(
        &self,
        installation: &Installation,
        offered: &[InstalledAddon],
        records: &ProjectRecords,
    ) -> Result<Vec<AddonUpdate>, String>;

    /// Aggiorna il progetto `target.key` all'ultima versione trovata dal controllo: scarica,
    /// sostituisce le cartelle e registra con `record`. Con `target.reinstall` lo fa anche per un
    /// progetto già aggiornato. Le regole di distribuzione della fonte
    /// (per CurseForge `allowModDistribution`, D13) restano qui.
    fn update(&self, target: &UpdateTarget<'_>, record: &mut RecordWriter<'_>) -> UpdateOutcome;
}
