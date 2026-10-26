# Registro delle decisioni

Ultimo aggiornamento: 2026-10-26.

Ogni decisione ha uno stato:

- **Confermata**: richiesta o approvata esplicitamente da Fabio.
- **Proposta**: scritta da Claude, in attesa di revisione. Vale finché non viene
  contestata, ma non è stata discussa.
- **Aperta**: da decidere.

Per cambiare una decisione non si cancella la riga: se ne aggiunge una nuova che la
sostituisce e si annota quella vecchia come superata.

## Prodotto

| # | Decisione | Motivo | Stato |
| --- | --- | --- | --- |
| D1 | Gestore di addon per WoW, alternativo a WowUp-CF | UI e filtri di WowUp-CF giudicati scadenti | Confermata |
| D2 | Linux è la piattaforma principale, con il gioco in un prefisso Wine | È l'ambiente d'uso reale (Lutris + Battle.net) | Confermata |
| D3 | Nome del progetto: WoW-don; repo e crate `wow-don` | Richiesta esplicita | Confermata |
| D4 | Il punto di forza dell'app sono ricerca, filtri e ordinamento | È il difetto principale di WowUp-CF | Proposta |
| D5 | Supporto a più installazioni di WoW e a più prefissi Wine | Retail, Classic e beta possono convivere | Proposta |
| D6 | Si può forzare la versione del gioco per un addon (per installazione e progetto CurseForge, in `addon-settings.json`, scelta tra i tipi noti all'app) | Le beta come Forever sono coperte male dai metadati. Approvato nella specifica di M3 | Confermata |
| D20 | UI minimale e moderna: poche superfici, spazio e tipografia al posto della decorazione, un solo colore d'accento, chiaro e scuro (regole in `AGENTS.md`, sezione "Interfaccia") | Richiesta esplicita; l'interfaccia scomoda è il difetto principale di WowUp-CF | Confermata |
| D23 | L'app funziona su Linux e su Windows. Su Linux resta centrale il gioco dentro un prefisso Wine (D2); su Windows si gestisce l'installazione nativa. Estesa a macOS da D29 | Richiesta esplicita | Confermata |
| D24 | Distribuzione: pacchetto `.deb` su Linux, installer NSIS (`*-setup.exe`) su Windows, installato per l'utente corrente (`installMode: currentUser`, senza diritti di amministratore, con voce nel menu Start e disinstallazione da Impostazioni); su macOS (D29) un `.dmg` **universale** per Apple Silicon e Intel (target `universal-apple-darwin`), **non firmato e non notarizzato** finché non si decide la firma, con dentro il solo `.app`; nessuna versione portable. Chiude A3 | Richiesta esplicita; tutti generati da Tauri senza strumenti esterni (il `.dmg` aggiunto con la issue #91) | Confermata |
| D27 | L'interfaccia dell'app è solo in inglese, compresi i messaggi d'errore e di esito restituiti dal backend ai comandi Tauri; la documentazione resta in italiano, codice e commit in inglese (D19 invariata). Nessuna libreria di traduzione: una sola lingua | Richiesta esplicita; una sola lingua nell'interfaccia | Confermata |
| D29 | L'app funziona su Linux, Windows e macOS (estende D23). Su macOS il client di WoW è nativo, senza Wine; le regole delle piattaforme in `AGENTS.md` valgono per tutte e tre. In CI il job `backend-macos` esegue clippy, build e test su `macos-latest`, ma solo sulle PR verso `main` e su quelle che modificano `ci.yml` | Richiesta esplicita; i minuti macOS costano dieci volte quelli Linux in un repository privato, quindi il job non gira su ogni PR | Confermata |

## Fonti e API

| # | Decisione | Motivo | Stato |
| --- | --- | --- | --- |
| D7 | Fonte principale: API di CurseForge | Catalogo più grande | Confermata |
| D8 | In sviluppo si usa la chiave API personale di CurseForge | Disponibile subito dal console | Confermata |
| D9 | La richiesta di chiave per applicazioni di terzi si manda quando il progetto sta in piedi | Serve un progetto da mostrare; la chiave personale basta per sviluppare | Confermata |
| D10 | La chiave sta in `.env` (gitignored) come `CURSEFORGE_API_KEY` e non lascia mai il backend Rust | Sicurezza; i termini CurseForge vietano di esporla | Confermata |
| D11 | Niente scraping del sito CurseForge e niente riuso della chiave di WowUp | Violano i termini d'uso | Confermata |
| D12 | Le fonti sono plugin dietro un'interfaccia comune | Permette di aggiungere Wago, WoWInterface e GitHub senza toccare il resto. Confermata con la specifica "Fonti libere" (issue #105) | Confermata |
| D13 | I download rispettano `allowModDistribution = false` | Termini CurseForge | Proposta |
| D31 | Fonti libere, senza accordi pubblicitari né chiavi: GitHub Releases (zip scelto con il `release.json` del packager di BigWigs, senza token), WoWInterface (API pubblica MMOUI, `filedetails`) e Tukui (`api.tukui.org/v1/addons`, ElvUI e Tukui). L'API di Wago resta esclusa. Il token GitHub facoltativo e la ricerca nel catalogo WoWInterface sono rimandati | Un solo sviluppatore non può ottenere accordi con Wago o Overwolf; queste API sono pubbliche (verificate il 2026-10-25). Approvato nella specifica "Fonti libere" (issue #102–#111) | Confermata |
| D32 | Un addon che non si può scaricare (per esempio `allowModDistribution = false`) si installa trascinando nella finestra lo zip scaricato a mano dal sito, o scegliendolo con "Install from zip…": il backend lo estrae con le regole di D26, accetta solo cartelle di primo livello con un `.toc` dello stesso nome e sostituisce senza conferma le cartelle con lo stesso nome. Non scrive un record nuovo in `installed-addons.json`; se sostituisce cartelle di un progetto che l'app aveva installato, il record di quel progetto si toglie (issue #117), così l'abbinamento di D33 non descrive un file che non c'è più. Dopo, l'addon si riconosce dal fingerprint se è su CurseForge, oppure dai segni delle fonti libere (`## X-WoWI-ID`, cartelle di Tukui); un addon che sta solo su GitHub resta senza fonte finché non si aggiunge dal link, perché GitHub si abbina solo dai record | Completa D13 senza aggirarla: il download lo fa l'utente dal sito dell'autore. Approvato nella specifica "Fonti libere" (issue #102, #103) | Confermata |
| D33 | Ordine di abbinamento delle cartelle installate tra le fonti: prima il record dell'app in `installed-addons.json` (v2, per fonte e id testuale), poi il fingerprint di CurseForge, poi le fonti libere (`## X-WoWI-ID` nel `.toc`, cartelle di `directories` per Tukui; GitHub solo dai record). Una cartella abbinata da una fonte non si rilegge con le altre | Evita doppioni quando lo stesso addon è su più fonti; la fonte da cui l'app l'ha installato vince. Approvato nella specifica "Fonti libere" (issue #104, #105) | Confermata |

## Stack

| # | Decisione | Motivo | Stato |
| --- | --- | --- | --- |
| D14 | Backend in Rust | Binario unico, niente Electron | Confermata |
| D15 | UI con Tauri 2 | Libertà nella UI (tabelle, filtri) con backend Rust | Confermata |
| D16 | Frontend React + TypeScript con Vite e pnpm | Template ufficiale Tauri; React già usato negli altri progetti | Proposta |
| D17 | Rust edition 2024 | Default del template | Proposta |
| D21 | Selettore di cartelle con il plugin ufficiale `tauri-plugin-dialog` | Percorsi lunghi dentro i prefissi Wine; approvato nella specifica di M1 (issue #5) | Confermata |
| D22 | Lo stato locale è in JSON nella cartella di configurazione dell'app risolta da Tauri (`app_config_dir()`, su Linux `~/.config/com.fabiosm46.wowdon/`), con scrittura atomica; le installazioni stanno in `installations.json` | Nessuna nuova dipendenza (`serde_json` c'è già); percorso standard della piattaforma. Chiude A1 | Confermata |
| D25 | Client HTTP: `reqwest` (versione bloccante, dentro `spawn_blocking`) con TLS `rustls` e certificati del sistema | Già nel lockfile tramite Tauri; TLS in puro Rust uguale su Linux e Windows, senza OpenSSL. Approvato nella specifica di M2 | Confermata |
| D26 | Estrazione degli zip con la crate `zip` (9.x), senza feature di default e con il solo deflate (`deflate-flate2-zlib-rs`, decompressione in puro Rust già presente nel lockfile) | Gli addon si distribuiscono come zip; in puro Rust, uguale su Linux e Windows. Servirà anche al backup di `WTF` e `Interface`. Approvato nella specifica di M3 | Confermata |
| D28 | Dimensione, posizione e stato massimizzato della finestra principale si ricordano tra un avvio e l'altro con il plugin ufficiale `tauri-plugin-window-state` (2.x), usato solo dal backend e limitato a `SIZE`, `POSITION` e `MAXIMIZED`; nessun permesso nuovo al frontend | Plugin ufficiale come `dialog` (D21) e `opener`; gestisce la finestra massimizzata e i monitor scollegati e salva nella cartella di configurazione dell'app (D22). Approvato esplicitamente per la issue #63 | Confermata |
| D30 | Su Windows la ricerca automatica delle installazioni legge il valore `InstallPath` della chiave di Blizzard nel registro con la crate `winreg` (0.56, già nel lockfile tramite Tauri), dichiarata solo per Windows in `[target.'cfg(windows)'.dependencies]`; il resto del modulo `discovery` non dipende da `winreg` | Il registro è il modo affidabile per trovare un'installazione fuori dai percorsi standard; la versione del lockfile evita un duplicato. Approvato esplicitamente nella specifica di Iterazione 2 (issue #87) | Confermata |
| D34 | Icona nella tray con la feature `tray-icon` della crate `tauri` (nessuna crate nuova; su Linux la libreria `libayatana-appindicator3-1`, caricata a runtime e dichiarata come dipendenza del `.deb`; verificato sull'artefatto che la CLI di Tauri 2.12 aggiunge la voce configurata a `libwebkit2gtk-4.1-0` e `libgtk-3-0` invece di sostituirle, da ricontrollare con `dpkg-deb -f <deb> Depends` quando si aggiorna la CLI). Il menu ha solo "Show WoW-don" (mostra, de-minimizza e mette a fuoco la finestra principale) e "Quit" (esce sempre, e il plugin di D28 salva lo stato della finestra); il clic sinistro fa come "Show WoW-don" dove la piattaforma lo segnala (Windows, macOS), su Linux AppIndicator apre il menu. L'opzione `closeToTray` in `settings.json` (default falso) fa nascondere la finestra principale alla chiusura (X, Alt+F4, Cmd+W) invece di chiudere l'app; su macOS il clic sull'icona nel Dock la riapre. Se l'icona non si crea (su Linux senza la libreria, verificata con `dlopen` prima di tray-icon) l'errore va su stderr, `trayAvailable` in `get_settings` è falso e la X chiude comunque; su Linux `trayAvailable` è falso anche quando sul bus di sessione non c'è `org.kde.StatusNotifierWatcher` con un host registrato (GNOME senza l'estensione AppIndicator), letto a ogni chiusura; anche un'impostazione illeggibile chiude: mai una finestra che non si chiude | Tenere l'app a portata di clic senza occupare la barra delle applicazioni. Approvato esplicitamente per la issue #158 (feature `tray-icon` e dipendenza del `.deb`) | Confermata |

## Processo

| # | Decisione | Motivo | Stato |
| --- | --- | --- | --- |
| D18 | Repo in `~/repo/hub/wow-don`, privata su GitHub | Richiesta esplicita | Confermata |
| D19 | Documentazione in italiano, codice e commit in inglese | Come negli altri progetti | Proposta |

## Aperte

| # | Domanda |
| --- | --- |
| A1 | ~~Formato della config utente (posizione in `~/.config/wow-don/`, TOML o JSON)~~ Risolta da D22 |
| A2 | Libreria per la tabella degli addon nel frontend |
| A3 | ~~Formato di distribuzione su Linux (AppImage, .deb, Flatpak)~~ Risolta da D24 |
| A4 | Licenza, se il progetto diventa pubblico |
