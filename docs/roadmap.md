# Roadmap

Ultimo aggiornamento: 2026-10-27.

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

## M4.6 — Fonti libere (fatto)

Fonti senza accordi pubblicitari né chiavi (D31), dietro l'interfaccia comune (D12):

- Installazione di uno zip scaricato a mano, trascinandolo nella finestra o con "Install from
  zip…" (D32): così si installano anche gli addon con `allowModDistribution = false`.
- `installed-addons.json` v2 con i record per fonte e l'ordine di abbinamento tra le fonti (D33).
- Tukui (ElvUI, Tukui), WoWInterface e GitHub Releases, con l'installazione da link nella tab
  Search e la fonte di ogni addon nella tab Installed.

Iteration 3: #102, #103, #104, #105, #106, #111, #117. Iteration 5: #107, #108, #109, #110.

## M4.7 — Velocità, import/export e gestione degli addon (fatto)

- Scansione veloce, cache dei fingerprint, `refresh_addons` e barra di avanzamento
  (Iteration 4: #123–#128).
- Ricerca che nasconde gli addon senza un file Release per la versione del gioco, opzione
  per ripiegare sulle Beta (#140, #141).
- Export e import della lista degli addon come stringa `WOWDON1:`, con "Keep my addons" e
  "Replace all" (#133–#135).
- Tab Installed: menu contestuale, selezione multipla, reinstallazione e rimozione
  (Iteration 5: #147, #148, #149).

## M4.8 — Mirror GitHub, tray e import veloce (fatto)

Iteration 6:

- Mirror GitHub per i progetti CurseForge bloccati: stessa versione da `sourceUrl`,
  aggiornamento, reinstallazione e installazione dalla Search, "via GitHub" nella tab
  Installed (#162–#165).
- Release GitHub lette senza API, dal redirect di `releases/latest` e dal `release.json`
  (D37, #179).
- Icona nella tray, "Close to tray" e avvio con il sistema (D35, D36, #158–#160).
- Tab Description e Gallery nel dettaglio di un addon (#167, #168).
- Import più veloce: download in parallelo, installazione in fila (#180); mediana su 12 addon
  CurseForge da 10,6 s a 4,5 s.

## M5 — Chiave per applicazioni di terzi e primo rilascio

- Inviare a CurseForge/Overwolf il form per la chiave da applicazione.
- Chiedere a Wago un accesso in lettura all'API (D39), in parallelo.
- Decidere come la chiave arriva nell'app installata: oggi il backend la legge solo da
  `CURSEFORGE_API_KEY` o dalla `.env` del checkout, quindi un pacchetto installato parte senza
  chiave.
- Primo rilascio provato: PR `develop → main` (la prima a far girare `backend-macos` sul
  backend completo), poi `.deb`, installer NSIS e `.dmg` costruiti dalla CI (D24) al primo tag.

## Dopo

- Token GitHub personale facoltativo, per superare il limite di 60 richieste l'ora.
- Ricerca nel catalogo WoWInterface.

- Wago Addons (D39): richiesta di accesso all'API inviata dall'utente; la fonte si specifica
  solo se Wago concede una chiave da usare in WoW-don.
