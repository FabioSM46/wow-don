# Roadmap

Ultimo aggiornamento: 2026-10-24.

## M0 — Scaffolding (fatto)

- Progetto Tauri 2 + React + TypeScript.
- Lettura della chiave CurseForge da `.env`.
- Documentazione: contesto, decisioni, architettura, roadmap.

## M1 — Addon installati (fatto)

- Aggiungere un'installazione di WoW a mano (percorso della cartella `_<flavor>_`).
- Leggere `Interface/AddOns` e i file `.toc`.
- Mostrare la lista degli addon installati con ricerca e filtri locali.

## M2 — CurseForge (fatto)

- Client API con chiave personale.
- Riconoscimento degli addon installati tramite fingerprint.
- Controllo aggiornamenti per la versione del gioco dell'installazione.

## M3 — Installazione e aggiornamento (fatto)

- Download, estrazione sicura e sostituzione delle cartelle.
- Aggiornamento singolo e "aggiorna tutto".
- Versione del gioco forzata per singolo addon.
- Backup delle cartelle `WTF` e `Interface`.
- Gli addon con `allowModDistribution = false` (per esempio Details!) si riconoscono e
  segnalano gli aggiornamenti, ma si aggiornano a mano dal sito (D13).

## M3.5 — Interfaccia in inglese e tab (fatto)

- Interfaccia solo in inglese, compresi i messaggi del backend (D27).
- App divisa in tab: per ora Installed e Options, con il selettore dell'installazione
  nell'header; la gestione delle installazioni e i backup stanno in Options. La tab Search
  è arrivata con M4.

## M4 — Ricerca e catalogo (fatto)

- Tab Search, la prima delle tre (Search, Installed, Options); all'avvio resta aperta Installed.
- Ricerca nel catalogo CurseForge con filtri (categoria, versione del gioco) e ordinamenti
  (popolarità, data di aggiornamento, nome, download), a pagine da 50.
- Dettaglio dell'addon: descrizione convertita in testo dal backend, file Release e Beta per
  la versione del gioco, dipendenze obbligatorie.
- Installazione di un addon nuovo dalla ricerca o dal dettaglio, con le dipendenze
  obbligatorie installate in automatico come in WowUp; gli addon con
  `allowModDistribution = false` rimandano al sito (D13).

## M4.5 — macOS, ricerca delle installazioni e tema (fatto)

- L'app funziona anche su macOS con il client nativo (D29); job `backend-macos` in CI sulle
  PR verso `main` e su quelle che toccano `ci.yml`.
- Pacchetto `.dmg` universale (Apple Silicon e Intel), non firmato, nella release (D24).
- Ricerca automatica delle installazioni: percorsi standard e registro di Battle.net su
  Windows (D30), `Applications` su macOS, prefissi Wine su Linux (`~/.wine`, Lutris, Bottles,
  Steam/Proton). Le installazioni trovate si propongono in Options con "Add", mai aggiunte da
  sole.
- Options: scelta del tema (sistema, chiaro, scuro) salvata dal backend.

## M5 — Richiesta chiave per applicazioni di terzi

- Inviare a CurseForge/Overwolf il form per la chiave da applicazione.
- Spostare la chiave dalla `.env` alla config utente.

## Dopo

- Altre fonti: GitHub releases, WoWInterface, Wago. Wago richiede la "Wago Access Key" da abbonato
  Patreon dell'utente (facoltativa, mai inclusa nell'app; l'API usata da WowUp è
  `addons.wago.io/api/external`, non documentata) e il permesso di usarla in un gestore
  diverso da WowUp: in attesa della chiave. GitHub releases non aiuta per Details (nessuno
  zip dal 2021).
- Primo rilascio provato: `.deb`, installer NSIS e `.dmg` costruiti dalla CI (D24) al primo
  tag su `main`.
