# Architettura

Ultimo aggiornamento: 2026-10-25. Descrive la struttura prevista; quello che esiste
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
| `commands` | Comandi Tauri esposti al frontend | Esiste (`app_status`, `list_installations`, `add_installation`, `remove_installation`, `discover_installations`, `list_addons`, `refresh_addons`, `export_addons`, `preview_import`, `import_addons`, `check_updates`, `update_addons`, `install_addon`, `search_catalog`, `list_categories`, `addon_detail`, `list_version_types`, `set_version_override`, `clear_version_override`, `create_backup`, `list_backups`, `install_zip`) |
| `installations` | Trova e gestisce le installazioni di WoW, anche dentro prefissi Wine | Esiste (aggiunta manuale e validazione; la scoperta automatica è in `discovery`) |
| `discovery` | Ricerca automatica delle installazioni (`discover_installations`): una funzione per piattaforma costruisce le cartelle `World of Warcraft` candidate (Windows: `InstallPath` del registro, D30, poi `World of Warcraft`, `Program Files (x86)`, `Program Files` e `Games` su ogni disco da C a Z; macOS: `/Applications` e `Applications` nella home chiesta a Tauri; Linux: `World of Warcraft` sotto `drive_c/Program Files (x86)` e `drive_c/Program Files` di ogni prefisso Wine della home, cioè `.wine`, i figli di `Games` (Lutris), i bottle di Bottles nativo e Flatpak e i `pfx` di `steamapps/compatdata` (Steam/Proton), a livelli fissi, con i nomi letti senza distinguere maiuscole e minuscole e senza leggere le configurazioni di Lutris, Bottles o Steam); una funzione comune legge solo le candidate e i loro figli diretti, tiene le cartelle `_<flavor>_` valide per `installations::validate`, senza duplicati (stesso id) e senza quelle già in `installations.json`. Restituisce `path`, `flavor`, `name` e `origin`; suggerisce soltanto, non scrive niente, e non trovare niente non è un errore | Esiste (backend; interfaccia da fare, issue #89) |
| `addons::toc` | Legge i file `.toc` in `Interface/AddOns` (titolo, versione, `## Interface`) e sceglie il `.toc` giusto per il flavor dell'installazione | Esiste |
| `addons::scan` | Legge le cartelle di `Interface/AddOns` e il loro `.toc`, senza modificare niente | Esiste |
| `addon_import` | Import di una stringa (issue #134). `preview_import` riporta per ogni progetto `new`, `installed` o `notDownloadable` (CurseForge con `allowModDistribution` falso o senza chiave, progetto Tukui non elencato, fonti non ancora supportate), le voci non valide e le cartelle che *Replace all* toglierebbe. `import_addons` (sotto `InstallLock`) installa i mancanti con il flusso esistente (CurseForge con le dipendenze e la versione forzata della voce salvata come override; Tukui con `TukuiSource::install_new`), poi con `replace`, se almeno un'installazione è riuscita o era già tutto presente, rilegge il disco e toglie solo le cartelle confermate (`confirmedRemovals`) che nessun progetto importato reclama, né direttamente né come dipendenza dichiarata nel `.toc` di una cartella che resta. Niente backup (scelta dell'utente); la cancellazione è `install::remove_folder`, un solo componente valido dentro `AddOns`, i link si tolgono senza seguirli; `WTF` e SavedVariables non si toccano. Le cartelle tolte escono dai record e dalla cache dei fingerprint | Esiste (interfaccia: sezione "Import / export" di Options, issue #135) |
| `addon_list` | Stringa di import/export degli addon di un'installazione (issue #133): `WOWDON1:` seguito da base64url senza padding (scritta nel modulo) di un JSON `{v, flavor, addons: [{source, key, name, versionType?}]}`. `decode` rifiuta prefisso o versione sconosciuti, base64 o JSON malformati, testi oltre 64 KiB o con più di 1.000 voci, e salta le voci non valide riportandole; `export_list` ricava la lista dai risultati di `refresh_addons` (un progetto per voce, con la versione forzata) più i progetti registrati che il controllo non riconosce, e nomina le cartelle senza progetto. Comando `export_addons` | Esiste |
| `addons::cache` | Cache in memoria dei fingerprint per cartella (stato gestito da Tauri, issue #125): un fingerprint si riusa se percorsi, dimensioni e date di modifica di tutti i file della cartella sono uguali all'ultimo calcolo; le cartelle scritte dall'app (aggiornamento, installazione, zip) escono dalla cache, "Reload addons" (`refresh_addons` con `force`) ricalcola tutta l'installazione, le cartelle sparite escono alla scansione successiva. Niente su disco | Esiste |
| `addons::fingerprint` | Calcola i fingerprint di CurseForge (MurmurHash2 seme 1 senza spaziatura, sui file inclusi da `.toc` e `.xml`) per riconoscere gli addon installati; l'hash si calcola in streaming e i file letti non si copiano. `addons::scan` legge le cartelle in parallelo con i thread della libreria standard (issue #124) | Esiste |
| `sources` | Trait comune `Source` per le fonti (D12): `check` abbina le cartelle offerte e valuta gli aggiornamenti, `update` scarica, sostituisce le cartelle e registra il record con le regole di distribuzione della fonte. I progetti si identificano con `ProjectKey` (`source` + `key`, l'id nella fonte come stringa); `Project`, `AddonUpdate` e `UpdateOutcome` portano `source` e `key`, e `id`/`modId` restano l'id CurseForge (`0` per le altre fonti). Fonti previste: `curseforge`, `tukui`, `wowinterface`, `github` (D31) | Esiste (CurseForge come `updates::CurseForgeSource`, poi Tukui; WoWInterface e GitHub con le issue #107 e #108) |
| `sources::curseforge` | Client per `https://api.curseforge.com/v1` con header `x-api-key`: fingerprint, mod, tipi di versione del gioco, ricerca (`/v1/mods/search`), categorie (`/v1/categories`), descrizione (`/v1/mods/{modId}/description`) e file di un mod con le loro dipendenze (`/v1/mods/{modId}/files`) | Esiste |
| `sources::http` | Lo strato HTTP comune alle fonti: risposte, `TransportError` e lettura con un limite (`read_limited`), usati anche dal client di CurseForge. Per le fonti libere, il trasporto senza chiave `PublicTransport`: `GET` delle risposte JSON (al massimo `MAX_JSON_BYTES`, 4 MiB) e download con il limite di `MAX_DOWNLOAD_BYTES`, redirect seguiti solo verso i domini della fonte (`is_allowed_url`: `https`, senza credenziali né porta, il dominio o un suo sottodominio; la stessa regola vale per i download di CurseForge) | Esiste |
| `sources::tukui` | Fonte Tukui (D31) su `https://api.tukui.org/v1/addons` (ElvUI `-2`, Tukui `-1`), letto una volta per controllo e riusato dall'aggiornamento: un addon è installato se tutte le sue `directories` sono in `AddOns` (senza distinguere maiuscole e minuscole) o se ha un record; la versione installata è quella del record, altrimenti `## Version` del `.toc` della prima cartella, e una versione diversa da `version` (senza contare una `v` iniziale: il `.toc` di ElvUI scrive `v16.00`) è un aggiornamento. Vale per l'installazione se `patch` contiene una versione del suo gioco o se il `.toc` installato la dichiara (`patch` non elenca WoW Forever, che lo zip supporta), altrimenti la cartella riporta un errore; lo zip si scarica solo da `tukui.org` (l'URL contiene un token, mai nei log), le cartelle del progetto che il nuovo zip non ha più si tolgono, e si registra con la versione come `file`; una voce dell'elenco che non si legge si salta senza togliere le altre | Esiste |
| `catalog` | Ricerca nel catalogo CurseForge per un'installazione (`search_catalog`): 50 risultati per pagina, versione del gioco dell'installazione o una nota all'app, ordinamenti `popularity`/`lastUpdated`/`name`/`totalDownloads` tradotti in `sortField` 2/3/4/6, oltre i 10.000 risultati dell'API una pagina vuota con il conteggio reale; gli addon senza un file Release per la versione cercata si tolgono dalla pagina (l'installazione non li potrebbe installare: `gameVersionTypeId` include anche i progetti con sola Beta), e la pagina riporta quanti risultati ha letto (`fetched`) e quanti ne ha nascosti (`hidden`), così la paginazione non si ferma su una pagina svuotata dal filtro (issue #140); con l'opzione "Use beta files when there is no release" (`betaWhenNoRelease` in `settings.json`, issue #141) un addon con sola Beta per la versione resta, con il file Beta (`releaseType` 2) e l'azione "Install beta". La regola di scelta del file è una sola, `sources::curseforge::pick_file`, per ricerca, dettaglio, installazione (con le dipendenze) e aggiornamento: il Release più recente, altrimenti la Beta più recente solo con l'opzione, mai Alpha; con l'opzione spenta un aggiornamento che sarebbe solo Beta ha lo stato `betaOnly`; categorie per nome senza le classi (`list_categories`). Il flag `installed` (record di `installed-addons.json` con una cartella ancora presente, oppure `## X-Curse-Project-ID` nel `.toc`) non chiama l'API ed è la regola di "già installato" anche per l'installazione di un addon nuovo. Dettaglio di un addon (`addon_detail`): i campi della ricerca, la descrizione convertita in testo semplice nel backend (niente HTML dell'autore nella webview; il frontend la mostra **solo come testo**, mai con `innerHTML` o `dangerouslySetInnerHTML`, perché le entità decodificate possono contenere `<` e `>`), fino a 20 file Release e Beta dal più recente (prima e, oltre i 50 file, ultima pagina di `/files`, ordinate per id: l'ordine della risposta non conta) e le dipendenze obbligatorie (`relationType = 3`) dell'ultimo Release, quello di `latestFilesIndexes` per la versione (letto con `get_file` se non è tra i file letti) o altrimenti il più recente tra tutti i file letti, con i nomi letti da una sola `get_mods`; senza versione esplicita vale quella forzata del progetto, poi quella dell'installazione | Esiste (backend; la tab Search è da fare) |
| `updates` | Scansiona l'installazione e offre le cartelle alle fonti nell'ordine di D33 (`check_updates`): una cartella nominata da un record di `installed-addons.json` va solo alla fonte del record (una cartella nei record di più fonti a nessuna in particolare), le altre a ogni fonte che non le ha ancora reclamate, CurseForge per prima; reclama solo una fonte che dà un progetto alla cartella (un errore su una cartella resta in serbo e si mostra se nessuna la riconosce); una fonte che fallisce non ferma le altre e le cartelle che le erano offerte riportano il suo errore, mentre se falliscono tutte fallisce il controllo; un `installed-addons.json` illeggibile non blocca il controllo. Senza `CURSEFORGE_API_KEY` controllo e aggiornamento non falliscono subito (issue #123): CurseForge fallisce come una fonte qualsiasi, le fonti libere controllano e aggiornano le loro cartelle, le altre riportano la chiave mancante e un progetto CurseForge chiesto in aggiornamento ha un esito di errore con lo stesso motivo. `update_addons` riceve `modIds` (CurseForge) e `projects` (`{source, key}` di qualsiasi fonte), rifà il controllo e passa ogni progetto alla sua fonte. Per CurseForge abbina gli addon ai progetti con i fingerprint, trova il file Release più recente per la versione del gioco, poi scarica il file e sostituisce le cartelle; una cartella del file installato assente dal nuovo zip si toglie solo se è ancora sul disco con il fingerprint del modulo installato, non è un modulo di un altro progetto riconosciuto e nessun altro addon la dichiara tra le dipendenze del `.toc` | Esiste (Retail, Classic Era, Classic, Anniversary, WoW Forever; id verificati sull'API) |
| `install` | Estrazione sicura dello zip in una cartella di staging accanto ad `AddOns` (niente percorsi assoluti, `..`, unità o link simbolici; limiti di voci e dimensione) e sostituzione delle cartelle con ripristino in caso di errore (D26) | Esiste (usato da `update_addons` e `install_addon`) |
| `updates::refresh_addons` | Lista degli addon e stati di aggiornamento da una sola scansione (comando `refresh_addons`, issue #126): "Reload addons" e le riletture dopo un aggiornamento, un'installazione dalla Search o da zip lo usano al posto di `list_addons` più `check_updates`; un controllo fallito, o connessioni che non si preparano, lasciano la lista con `checkError`. Il primo caricamento della tab resta `list_addons` seguito dal controllo, per mostrare la lista prima della rete | Esiste |
| `install_local` | Installazione di uno zip scaricato a mano dall'utente (`install_zip`, D32): accetta solo un file `.zip` entro il limite di dimensione dei download, lo estrae con le regole di `install`, rifiuta lo zip intero se una cartella di primo livello non ha un `.toc` con il suo nome (`<Cartella>.toc`, `<Cartella>_<suffisso>.toc` o `<Cartella>-<suffisso>.toc`, senza distinguere maiuscole e minuscole) e sostituisce senza conferma le cartelle con lo stesso nome; restituisce le cartelle installate e quelle sostituite e tiene `InstallLock`. Non scrive record: l'addon si riconosce poi dal fingerprint; il comando toglie però i record interi (di qualsiasi fonte) che nominano una cartella scritta dallo zip, sostituita o ricreata (`InstalledAddonsStore::forget_folders`, issue #117), e se non ci riesce l'esito porta un avviso (`warning`) invece di fallire | Esiste (interfaccia: trascinamento di un file sulla finestra con lo strato di `ZipDrop` e pulsante "Install from zip…" nella tab Installed) |
| `progress` | Avanzamento di installazioni e aggiornamenti (issue #127): le fasi `checking`, `downloading` (con `received` e, se il server la dichiara, `total` in byte), `extracting` e `installing` di un progetto, identificato da `source` + `key` con il suo `name`. `updates`, `install_new` e `install_local` ricevono un callback e non conoscono Tauri; i comandi `update_addons`, `install_addon` e `install_zip` lo collegano all'evento `install-progress`, con `installationId`. I byte del download arrivano dal ciclo di lettura di `sources::http::read_limited_with`; `Reporter` ne lascia passare al massimo circa dieci al secondo per progetto più quello finale. Il payload non contiene URL, chiavi, header o percorsi. Uno zip scelto dall'utente non ha fonte (`source` nullo, `key` vuoto). Gli eventi non hanno una fase finale: un progetto saltato o fallito può fermarsi a `checking`, quindi chi ascolta chiude l'avanzamento di tutti i progetti quando il comando restituisce gli esiti, che restano la risposta definitiva. Senza `Content-Length` anche l'ultimo passo del download è senza `total` | Esiste (backend; la barra nel frontend è la issue #128) |
| `install_new` | Installazione di un addon nuovo dal catalogo (`install_addon`): il file indicato, purché sia del progetto, oppure l'ultimo Release per la versione forzata o dell'installazione; un progetto già installato (stessa regola del flag `installed` della ricerca) si salta senza rete; `allowModDistribution` falso o assente e file senza `downloadUrl` si saltano senza scaricare (D13); una cartella dello zip che esiste già e appartiene a un altro progetto (nei record o, senza record, per il `## X-Curse-Project-ID` del `.toc`) fa fallire l'installazione senza toccare il disco, una cartella senza record né id si sostituisce; installazioni e aggiornamenti sono serializzati da un lock dell'app (`InstallLock`); registra file e cartelle in `installed-addons.json`. Come in WowUp, dopo un'installazione riuscita installa le dipendenze del file con `relationType = 3` (RequiredDependency), un livello alla volta e ricorsivamente fino a 5 livelli (oltre, un esito `skipped` lo dice), ogni progetto una volta sola (cicli e dipendenze condivise), con l'ultimo Release per la stessa versione del gioco del progetto scelto e le stesse regole; le dipendenze di un progetto già installato o non installato non si seguono; ogni livello si legge con una sola `get_mods` (che dà il nome anche alle dipendenze già installate o troppo profonde), il disco si legge una volta per sapere cosa è installato, e i record locali e l'insieme degli installati si aggiornano dopo ogni installazione. Restituisce un elenco di esiti `installed`/`skipped`/`error` con la forma di `UpdateOutcome`: prima il progetto scelto, poi una dipendenza per esito con `dependencyOf` (il progetto che la richiede); un errore su una dipendenza non tocca le altre né il progetto scelto | Esiste (backend; interfaccia da fare) |
| `backup` | Zip (deflate, in streaming) di `WTF` e `Interface` di un'installazione in `app_data_dir()/backups/<id-installazione>/<flavor>-AAAAMMGG-HHMMSS.zip` (ora UTC), scritto come file temporaneo e rinominato a fine scrittura; i link simbolici non si seguono; elenco dei backup dal più recente | Esiste (ripristino, backup automatico e rotazione da fare) |
| `store` | Stato locale: addon installati, fonte, versione, versione del gioco forzata | Esiste per le installazioni (`installations.json`, D22) e per la versione del gioco forzata per addon (`addon-settings.json`, D6); `check_updates` e `update_addons` la usano al posto di quella dell'installazione; un `addon-settings.json` illeggibile blocca anche il controllo e l'aggiornamento, invece di ignorare le versioni forzate, perché l'aggiornamento scriverebbe file per la versione sbagliata. `installed-addons.json` (issue #62) ricorda, per installazione e progetto, il file installato dall'app e le cartelle del suo zip. Dalla versione 2 (issue #104) i record sono per fonte (`curseforge`, `github`, `wowinterface`, `tukui`) e id testuale del progetto, con il file come stringa (id del file CurseForge, tag GitHub, versione WoWInterface/Tukui); un file v1, solo CurseForge con id numerici, si legge ancora e si riscrive come v2 alla prima modifica; una fonte sconosciuta o un record CurseForge con id non numerici bloccano le modifiche come un file danneggiato. Controllo e installazione leggono per ora solo la vista CurseForge (`records_for`): `update_addons` e `install_addon` lo scrivono dopo ogni installazione riuscita, `list_addons` lo espone come `installedProject` per le cartelle ancora presenti, la rimozione di un'installazione toglie i suoi record. `settings.json` (issue #92) tiene le impostazioni dell'app: il tema (`system`, `light` o `dark`), che `get_settings` legge all'avvio e `set_theme` salva, e l'opzione della Beta (`betaWhenNoRelease`, issue #141, `set_beta_when_no_release`; un file senza il campo vale falso, e se `settings.json` non si legge vale falso senza bloccare ricerca e aggiornamenti) |

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
