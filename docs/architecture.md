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
| `commands` | Comandi Tauri esposti al frontend | Esiste (`app_status`, `list_installations`, `add_installation`, `remove_installation`, `list_addons`, `check_updates`) |
| `installations` | Trova e gestisce le installazioni di WoW, anche dentro prefissi Wine | Esiste (aggiunta manuale e validazione; scoperta automatica da fare) |
| `addons::toc` | Legge i file `.toc` in `Interface/AddOns` (titolo, versione, `## Interface`) e sceglie il `.toc` giusto per il flavor dell'installazione | Esiste |
| `addons::scan` | Legge le cartelle di `Interface/AddOns` e il loro `.toc`, senza modificare niente | Esiste |
| `addons::fingerprint` | Calcola i fingerprint di CurseForge (MurmurHash2 seme 1 senza spaziatura, sui file inclusi da `.toc` e `.xml`) per riconoscere gli addon installati | Esiste |
| `sources` | Trait comune per le fonti (`search`, `versions`, `download`) | Da fare (oggi solo il modulo `curseforge`) |
| `sources::curseforge` | Client per `https://api.curseforge.com/v1` con header `x-api-key`: fingerprint, mod, tipi di versione del gioco | Esiste |
| `updates` | Abbina gli addon ai progetti CurseForge e trova il file Release più recente per la versione del gioco (`check_updates`) | Esiste (Retail, Classic Era, Classic, Anniversary, WoW Forever; id verificati sull'API) |
| `install` | Estrazione sicura dello zip in una cartella di staging accanto ad `AddOns` (niente percorsi assoluti, `..`, unità o link simbolici; limiti di voci e dimensione) e sostituzione delle cartelle con ripristino in caso di errore (D26) | Esiste (il download e il comando di installazione sono da fare) |
| `store` | Stato locale: addon installati, fonte, versione, versione del gioco forzata | Esiste per le installazioni (`installations.json`, D22) |

## API CurseForge usate

- `GET /v1/mods/search` (con `gameId = 1` per WoW): ricerca e filtri.
- `GET /v1/mods/{modId}` e `GET /v1/mods/{modId}/files`: dettagli e versioni.
- `POST /v1/fingerprints/1`: riconosce gli addon installati dai fingerprint delle cartelle.
- `GET /v1/games/1/version-types`: tipi di versione del gioco (Retail, Classic, …).
- Le versioni del gioco (Retail, Classic, beta) si leggono dall'API, non si scrivono
  a mano.

Dettagli da verificare sulla documentazione ufficiale prima di implementarli: le
normalizzazioni dei file per il fingerprint e gli ID delle versioni del gioco. Il
codice di WowUp (open source) è un buon riferimento.

## Installazioni di WoW

L'app gira su Linux e su Windows (D23). Su Windows il gioco sta nella cartella scelta
da Battle.net, di solito:

```
C:\Program Files (x86)\World of Warcraft\
├── _retail_\
├── _classic_era_\
└── Data\
```

Su Linux il gioco gira dentro un prefisso Wine. Esempio reale: Battle.net su Lutris
con prefisso in `~/Games/battlenet`:

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
- Su Windows `canonicalize` restituisce percorsi con prefisso `\\?\`, da togliere
  prima di mostrarli o salvarli.
- Gli zip scaricati si estraggono in una cartella temporanea e si controlla che i
  percorsi non escano dalla cartella `AddOns` prima di spostarli.
