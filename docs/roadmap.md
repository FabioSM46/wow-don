# Roadmap

Ultimo aggiornamento: 2026-10-22.

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

## M3 — Installazione e aggiornamento

- Download, estrazione sicura e sostituzione delle cartelle.
- Aggiornamento singolo e "aggiorna tutto".
- Versione del gioco forzata per singolo addon.
- Backup delle cartelle `WTF` e `Interface`.

## M4 — Ricerca e catalogo

- Ricerca nel catalogo CurseForge con filtri (categoria, versione del gioco,
  popolarità, data di aggiornamento) e ordinamenti.
- Pagina di dettaglio dell'addon.

## M5 — Richiesta chiave per applicazioni di terzi

- Inviare a CurseForge/Overwolf il form per la chiave da applicazione.
- Spostare la chiave dalla `.env` alla config utente.

## Dopo

- Altre fonti: GitHub releases, WoWInterface, Wago.
- Scoperta automatica dei prefissi Lutris.
- Pacchetti di rilascio: `.deb` per Linux e installer NSIS per Windows (D24), costruiti
  dalla CI.
