# Architettura

Ultimo aggiornamento: 2026-10-23. Descrive la struttura prevista; quello che esiste
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
| `commands` | Comandi Tauri esposti al frontend | Esiste (`app_status`, `list_installations`, `add_installation`, `remove_installation`, `list_addons`, `check_updates`, `update_addons`, `list_version_types`, `set_version_override`, `clear_version_override`, `create_backup`, `list_backups`) |
| `installations` | Trova e gestisce le installazioni di WoW, anche dentro prefissi Wine | Esiste (aggiunta manuale e validazione; scoperta automatica da fare) |
| `addons::toc` | Legge i file `.toc` in `Interface/AddOns` (titolo, versione, `## Interface`) e sceglie il `.toc` giusto per il flavor dell'installazione | Esiste |
| `addons::scan` | Legge le cartelle di `Interface/AddOns` e il loro `.toc`, senza modificare niente | Esiste |
| `addons::fingerprint` | Calcola i fingerprint di CurseForge (MurmurHash2 seme 1 senza spaziatura, sui file inclusi da `.toc` e `.xml`) per riconoscere gli addon installati | Esiste |
| `sources` | Trait comune per le fonti (`search`, `versions`, `download`) | Da fare (oggi solo il modulo `curseforge`) |
| `sources::curseforge` | Client per `https://api.curseforge.com/v1` con header `x-api-key`: fingerprint, mod, tipi di versione del gioco | Esiste |
| `updates` | Abbina gli addon ai progetti CurseForge e trova il file Release più recente per la versione del gioco (`check_updates`); aggiorna i progetti scelti rifacendo il controllo, scaricando il file e sostituendo le cartelle (`update_addons`); una cartella del file installato assente dal nuovo zip si toglie solo se è ancora sul disco con il fingerprint del modulo installato, non è un modulo di un altro progetto riconosciuto e nessun altro addon la dichiara tra le dipendenze del `.toc` | Esiste (Retail, Classic Era, Classic, Anniversary, WoW Forever; id verificati sull'API) |
| `install` | Estrazione sicura dello zip in una cartella di staging accanto ad `AddOns` (niente percorsi assoluti, `..`, unità o link simbolici; limiti di voci e dimensione) e sostituzione delle cartelle con ripristino in caso di errore (D26) | Esiste (usato da `update_addons`; l'installazione di un addon nuovo è da fare) |
| `backup` | Zip (deflate, in streaming) di `WTF` e `Interface` di un'installazione in `app_data_dir()/backups/<id-installazione>/<flavor>-AAAAMMGG-HHMMSS.zip` (ora UTC), scritto come file temporaneo e rinominato a fine scrittura; i link simbolici non si seguono; elenco dei backup dal più recente | Esiste (ripristino, backup automatico e rotazione da fare) |
| `store` | Stato locale: addon installati, fonte, versione, versione del gioco forzata | Esiste per le installazioni (`installations.json`, D22) e per la versione del gioco forzata per addon (`addon-settings.json`, D6); `check_updates` e `update_addons` la usano al posto di quella dell'installazione; un `addon-settings.json` illeggibile blocca anche il controllo e l'aggiornamento, invece di ignorare le versioni forzate, perché l'aggiornamento scriverebbe file per la versione sbagliata |

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

## Finestra

La finestra principale (`main`, definita in `tauri.conf.json` senza `center` né posizione
fissa e senza `label`, quindi con l'etichetta predefinita `main`) ricorda dimensione,
posizione e stato massimizzato con `tauri-plugin-window-state` (D28), registrato in `lib.rs`
con i soli flag `SIZE`, `POSITION` e `MAXIMIZED` e con un filtro sull'etichetta `main`: una
finestra aggiunta in futuro non eredita il comportamento senza una scelta esplicita;
visibilità, decorazioni e schermo intero non si salvano. Il plugin ripristina lo stato quando
la finestra è pronta, lo aggiorna a ogni spostamento e ridimensionamento e lo scrive all'uscita
in `.window-state.json` dentro `app_config_dir()` (D22). La dimensione salvata è quella della
finestra non massimizzata: nella 2.5.0 sia il gestore di `Resized` sia `update_state` saltano
dimensione e posizione quando `is_maximized()` è vero, così togliere la massimizzazione riporta
la finestra com'era. Se il monitor su cui stava la finestra non c'è più, la posizione salvata
si ignora (si imposta solo se il rettangolo salvato interseca uno di `available_monitors()`) e
la finestra si apre dove la mette il sistema, su un monitor presente. Il frontend non ha
permessi per i comandi del plugin: lo usa solo il backend.

**Limite su Linux con Wayland**: il compositor non permette alle applicazioni di leggere né
di impostare la posizione delle proprie finestre, quindi lì si ripristinano solo la dimensione
e lo stato massimizzato; la posizione la sceglie il compositor. Non si aggira (per esempio
forzando X11/XWayland): è una scelta del protocollo. Su X11 e su Windows si ripristina anche
la posizione.

## Sicurezza

- La chiave non è mai inviata al frontend né scritta nei log.
- In produzione la chiave verrà dalla config utente (formato e posizione di D22).
- Su Windows `canonicalize` restituisce percorsi con prefisso `\\?\`, da togliere
  prima di mostrarli o salvarli.
- Gli zip scaricati si estraggono in una cartella temporanea e si controlla che i
  percorsi non escano dalla cartella `AddOns` prima di spostarli.
