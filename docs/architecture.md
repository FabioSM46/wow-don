# Architettura

Ultimo aggiornamento: 2026-10-24. Descrive la struttura prevista; quello che esiste
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
| `commands` | Comandi Tauri esposti al frontend | Esiste (`app_status`, `list_installations`, `add_installation`, `remove_installation`, `discover_installations`, `list_addons`, `check_updates`, `update_addons`, `install_addon`, `search_catalog`, `list_categories`, `addon_detail`, `list_version_types`, `set_version_override`, `clear_version_override`, `create_backup`, `list_backups`, `install_zip`) |
| `installations` | Trova e gestisce le installazioni di WoW, anche dentro prefissi Wine | Esiste (aggiunta manuale e validazione; la scoperta automatica è in `discovery`) |
| `discovery` | Ricerca automatica delle installazioni (`discover_installations`): una funzione per piattaforma costruisce le cartelle `World of Warcraft` candidate (Windows: `InstallPath` del registro, D30, poi `World of Warcraft`, `Program Files (x86)`, `Program Files` e `Games` su ogni disco da C a Z; macOS: `/Applications` e `Applications` nella home chiesta a Tauri; Linux: `World of Warcraft` sotto `drive_c/Program Files (x86)` e `drive_c/Program Files` di ogni prefisso Wine della home, cioè `.wine`, i figli di `Games` (Lutris), i bottle di Bottles nativo e Flatpak e i `pfx` di `steamapps/compatdata` (Steam/Proton), a livelli fissi, con i nomi letti senza distinguere maiuscole e minuscole e senza leggere le configurazioni di Lutris, Bottles o Steam); una funzione comune legge solo le candidate e i loro figli diretti, tiene le cartelle `_<flavor>_` valide per `installations::validate`, senza duplicati (stesso id) e senza quelle già in `installations.json`. Restituisce `path`, `flavor`, `name` e `origin`; suggerisce soltanto, non scrive niente, e non trovare niente non è un errore | Esiste (backend; interfaccia da fare, issue #89) |
| `addons::toc` | Legge i file `.toc` in `Interface/AddOns` (titolo, versione, `## Interface`) e sceglie il `.toc` giusto per il flavor dell'installazione | Esiste |
| `addons::scan` | Legge le cartelle di `Interface/AddOns` e il loro `.toc`, senza modificare niente | Esiste |
| `addons::fingerprint` | Calcola i fingerprint di CurseForge (MurmurHash2 seme 1 senza spaziatura, sui file inclusi da `.toc` e `.xml`) per riconoscere gli addon installati | Esiste |
| `sources` | Trait comune per le fonti (`search`, `versions`, `download`) | Da fare (oggi solo il modulo `curseforge`) |
| `sources::curseforge` | Client per `https://api.curseforge.com/v1` con header `x-api-key`: fingerprint, mod, tipi di versione del gioco, ricerca (`/v1/mods/search`), categorie (`/v1/categories`), descrizione (`/v1/mods/{modId}/description`) e file di un mod con le loro dipendenze (`/v1/mods/{modId}/files`) | Esiste |
| `catalog` | Ricerca nel catalogo CurseForge per un'installazione (`search_catalog`): 50 risultati per pagina, versione del gioco dell'installazione o una nota all'app, ordinamenti `popularity`/`lastUpdated`/`name`/`totalDownloads` tradotti in `sortField` 2/3/4/6, oltre i 10.000 risultati dell'API una pagina vuota con il conteggio reale; categorie per nome senza le classi (`list_categories`). Il flag `installed` (record di `installed-addons.json` con una cartella ancora presente, oppure `## X-Curse-Project-ID` nel `.toc`) non chiama l'API ed è la regola di "già installato" anche per l'installazione di un addon nuovo. Dettaglio di un addon (`addon_detail`): i campi della ricerca, la descrizione convertita in testo semplice nel backend (niente HTML dell'autore nella webview; il frontend la mostra **solo come testo**, mai con `innerHTML` o `dangerouslySetInnerHTML`, perché le entità decodificate possono contenere `<` e `>`), fino a 20 file Release e Beta dal più recente (prima e, oltre i 50 file, ultima pagina di `/files`, ordinate per id: l'ordine della risposta non conta) e le dipendenze obbligatorie (`relationType = 3`) dell'ultimo Release, quello di `latestFilesIndexes` per la versione (letto con `get_file` se non è tra i file letti) o altrimenti il più recente tra tutti i file letti, con i nomi letti da una sola `get_mods`; senza versione esplicita vale quella forzata del progetto, poi quella dell'installazione | Esiste (backend; la tab Search è da fare) |
| `updates` | Abbina gli addon ai progetti CurseForge e trova il file Release più recente per la versione del gioco (`check_updates`); aggiorna i progetti scelti rifacendo il controllo, scaricando il file e sostituendo le cartelle (`update_addons`); una cartella del file installato assente dal nuovo zip si toglie solo se è ancora sul disco con il fingerprint del modulo installato, non è un modulo di un altro progetto riconosciuto e nessun altro addon la dichiara tra le dipendenze del `.toc` | Esiste (Retail, Classic Era, Classic, Anniversary, WoW Forever; id verificati sull'API) |
| `install` | Estrazione sicura dello zip in una cartella di staging accanto ad `AddOns` (niente percorsi assoluti, `..`, unità o link simbolici; limiti di voci e dimensione) e sostituzione delle cartelle con ripristino in caso di errore (D26) | Esiste (usato da `update_addons` e `install_addon`) |
| `install_local` | Installazione di uno zip scaricato a mano dall'utente (`install_zip`, D32): accetta solo un file `.zip` entro il limite di dimensione dei download, lo estrae con le regole di `install`, rifiuta lo zip intero se una cartella di primo livello non ha un `.toc` con il suo nome (`<Cartella>.toc`, `<Cartella>_<suffisso>.toc` o `<Cartella>-<suffisso>.toc`, senza distinguere maiuscole e minuscole) e sostituisce senza conferma le cartelle con lo stesso nome; restituisce le cartelle installate e quelle sostituite e tiene `InstallLock`. Non scrive record: l'addon si riconosce poi dal fingerprint | Esiste (interfaccia: trascinamento di un file sulla finestra con lo strato di `ZipDrop` e pulsante "Install from zip…" nella tab Installed) |
| `install_new` | Installazione di un addon nuovo dal catalogo (`install_addon`): il file indicato, purché sia del progetto, oppure l'ultimo Release per la versione forzata o dell'installazione; un progetto già installato (stessa regola del flag `installed` della ricerca) si salta senza rete; `allowModDistribution` falso o assente e file senza `downloadUrl` si saltano senza scaricare (D13); una cartella dello zip che esiste già e appartiene a un altro progetto (nei record o, senza record, per il `## X-Curse-Project-ID` del `.toc`) fa fallire l'installazione senza toccare il disco, una cartella senza record né id si sostituisce; installazioni e aggiornamenti sono serializzati da un lock dell'app (`InstallLock`); registra file e cartelle in `installed-addons.json`. Come in WowUp, dopo un'installazione riuscita installa le dipendenze del file con `relationType = 3` (RequiredDependency), un livello alla volta e ricorsivamente fino a 5 livelli (oltre, un esito `skipped` lo dice), ogni progetto una volta sola (cicli e dipendenze condivise), con l'ultimo Release per la stessa versione del gioco del progetto scelto e le stesse regole; le dipendenze di un progetto già installato o non installato non si seguono; ogni livello si legge con una sola `get_mods` (che dà il nome anche alle dipendenze già installate o troppo profonde), il disco si legge una volta per sapere cosa è installato, e i record locali e l'insieme degli installati si aggiornano dopo ogni installazione. Restituisce un elenco di esiti `installed`/`skipped`/`error` con la forma di `UpdateOutcome`: prima il progetto scelto, poi una dipendenza per esito con `dependencyOf` (il progetto che la richiede); un errore su una dipendenza non tocca le altre né il progetto scelto | Esiste (backend; interfaccia da fare) |
| `backup` | Zip (deflate, in streaming) di `WTF` e `Interface` di un'installazione in `app_data_dir()/backups/<id-installazione>/<flavor>-AAAAMMGG-HHMMSS.zip` (ora UTC), scritto come file temporaneo e rinominato a fine scrittura; i link simbolici non si seguono; elenco dei backup dal più recente | Esiste (ripristino, backup automatico e rotazione da fare) |
| `store` | Stato locale: addon installati, fonte, versione, versione del gioco forzata | Esiste per le installazioni (`installations.json`, D22) e per la versione del gioco forzata per addon (`addon-settings.json`, D6); `check_updates` e `update_addons` la usano al posto di quella dell'installazione; un `addon-settings.json` illeggibile blocca anche il controllo e l'aggiornamento, invece di ignorare le versioni forzate, perché l'aggiornamento scriverebbe file per la versione sbagliata. `installed-addons.json` (issue #62) ricorda, per installazione e progetto, il file installato dall'app e le cartelle del suo zip: `update_addons` e `install_addon` lo scrivono dopo ogni installazione riuscita, `list_addons` lo espone come `installedProject` per le cartelle ancora presenti, la rimozione di un'installazione toglie i suoi record. `settings.json` (issue #92) tiene le impostazioni dell'app, per ora il tema (`system`, `light` o `dark`): `get_settings` lo legge all'avvio e `set_theme` lo salva |

## API CurseForge usate

- `GET /v1/mods/search` (con `gameId = 1` per WoW): ricerca e filtri.
- `GET /v1/mods/{modId}` e `GET /v1/mods/{modId}/files` (con `gameVersionTypeId`, `index`
  e `pageSize`, al massimo 50; `pagination.totalCount` dice se servono altre pagine, e
  `index + pageSize` resta entro 10.000): dettagli, versioni e, per ogni file, `dependencies` (`modId`,
  `relationType`: 1 EmbeddedLibrary, 2 OptionalDependency, 3 RequiredDependency, 4 Tool,
  5 Incompatible, 6 Include).
- `GET /v1/mods/{modId}/description`: descrizione completa in HTML (`data` è una stringa),
  convertita in testo prima di arrivare al frontend. Il testo non è HTML sicuro: si mostra
  con il binding di testo di React (che fa l'escape), mai come HTML.
- `POST /v1/fingerprints/1`: riconosce gli addon installati dai fingerprint delle cartelle;
  ogni cartella si abbina prima a `exactMatches` e, solo se lì non c'è, al file di
  `partialMatches` i cui `modules` contengono il suo fingerprint (come WowUp).
- `GET /v1/games/1/version-types`: tipi di versione del gioco (Retail, Classic, …).
- Le versioni del gioco (Retail, Classic, beta) si leggono dall'API, non si scrivono
  a mano.

Dettagli da verificare sulla documentazione ufficiale prima di implementarli: le
normalizzazioni dei file per il fingerprint e gli ID delle versioni del gioco. Il
codice di WowUp (open source) è un buon riferimento.

## Installazioni di WoW

L'app gira su Linux, Windows e macOS (D23, D29). Su macOS il client è nativo, senza Wine:
le cartelle `_<flavor>_` hanno la stessa struttura e il file system (APFS) di norma non
distingue maiuscole e minuscole, come Windows. Su Windows il gioco sta nella cartella scelta
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
installazioni a mano e cercarle nei prefissi Wine della home (`discovery`, issue #88);
i prefissi in percorsi personalizzati restano l'aggiunta manuale.

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
forzando X11/XWayland): è una scelta del protocollo. Su X11, su Windows e su macOS si
ripristina anche la posizione.

## Sicurezza

- La chiave non è mai inviata al frontend né scritta nei log.
- In produzione la chiave verrà dalla config utente (formato e posizione di D22).
- Su Windows `canonicalize` restituisce percorsi con prefisso `\\?\`, da togliere
  prima di mostrarli o salvarli.
- Gli zip scaricati si estraggono in una cartella temporanea e si controlla che i
  percorsi non escano dalla cartella `AddOns` prima di spostarli.
