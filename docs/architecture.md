# Architettura

Ultimo aggiornamento: 2026-10-22. Descrive la struttura prevista; quello che esiste
già nel codice è indicato come tale.

## Panoramica

```
┌──────────────────────────────┐      invoke()       ┌─────────────────────────────┐
│ Frontend (React + TS, Vite)  │ ──────────────────▶ │ Backend Rust (Tauri 2)      │
│ ricerca, filtri, liste       │ ◀────────────────── │ comandi, config, fonti, I/O │
└──────────────────────────────┘     JSON (serde)    └──────────────┬──────────────┘
                                                                     │ HTTPS
                                                     ┌───────────────┴──────────────┐
                                                     │ CurseForge API (poi Wago,    │
                                                     │ WoWInterface, GitHub)        │
                                                     └──────────────────────────────┘
```

Regola: tutto quello che tocca rete, disco e chiavi sta nel backend. Il frontend
mostra dati e chiama comandi, non conosce la chiave API.

## Moduli del backend (`src-tauri/src/`)

| Modulo | Responsabilità | Stato |
| --- | --- | --- |
| `config` | Legge `CURSEFORGE_API_KEY` dall'ambiente / `.env` | Esiste |
| `commands` | Comandi Tauri esposti al frontend | Esiste (`app_status`, `list_installations`, `add_installation`, `remove_installation`, `list_addons`) |
| `installations` | Trova e gestisce le installazioni di WoW, anche dentro prefissi Wine | Esiste (aggiunta manuale e validazione; scoperta automatica da fare) |
| `addons::toc` | Legge i file `.toc` in `Interface/AddOns` (titolo, versione, `## Interface`) e sceglie il `.toc` giusto per il flavor dell'installazione | Esiste |
| `addons::scan` | Legge le cartelle di `Interface/AddOns` e il loro `.toc`, senza modificare niente | Esiste |
| `addons::fingerprint` | Calcola i fingerprint (MurmurHash2) per riconoscere gli addon installati | Da fare |
| `sources` | Trait comune per le fonti (`search`, `versions`, `download`) | Da fare |
| `sources::curseforge` | Client per `https://api.curseforge.com/v1` con header `x-api-key` | Da fare |
| `install` | Download, estrazione zip, sostituzione atomica delle cartelle | Da fare |
| `store` | Stato locale: addon installati, fonte, versione, versione del gioco forzata | Esiste per le installazioni (`installations.json`, D22) |

## API CurseForge usate

- `GET /v1/mods/search` (con `gameId = 1` per WoW): ricerca e filtri.
- `GET /v1/mods/{modId}` e `GET /v1/mods/{modId}/files`: dettagli e versioni.
- `POST /v1/fingerprints`: riconosce gli addon installati dagli hash dei file.
- Le versioni del gioco (Retail, Classic, beta) si leggono dall'API, non si scrivono
  a mano.

Dettagli da verificare sulla documentazione ufficiale prima di implementarli: le
normalizzazioni dei file per il fingerprint e gli ID delle versioni del gioco. Il
codice di WowUp (open source) è un buon riferimento.

## Installazioni di WoW su Linux

Esempio reale: Battle.net su Lutris con prefisso in `~/Games/battlenet`:

```
~/Games/battlenet/drive_c/Program Files (x86)/World of Warcraft/
├── _retail_/            # se installato
├── _classic_era_/       # se installato
├── _classic_beta_/      # WoW Forever beta
│   ├── Interface/AddOns/
│   └── WTF/
└── Data/                # in comune tra le versioni
```

Ogni cartella `_<flavor>_` è un'installazione separata. L'app deve poter aggiungere
installazioni a mano e, più avanti, cercarle nei prefissi Lutris noti.

## Sicurezza

- La chiave non è mai inviata al frontend né scritta nei log.
- In produzione la chiave verrà dalla config utente (formato e posizione di D22).
- Gli zip scaricati si estraggono in una cartella temporanea e si controlla che i
  percorsi non escano dalla cartella `AddOns` prima di spostarli.
