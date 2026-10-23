# Registro delle decisioni

Ultimo aggiornamento: 2026-10-23.

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
| D23 | L'app funziona su Linux e su Windows. Su Linux resta centrale il gioco dentro un prefisso Wine (D2); su Windows si gestisce l'installazione nativa | Richiesta esplicita | Confermata |
| D24 | Distribuzione: pacchetto `.deb` su Linux, installer NSIS (`*-setup.exe`) su Windows, installato per l'utente corrente (`installMode: currentUser`, senza diritti di amministratore, con voce nel menu Start e disinstallazione da Impostazioni); nessuna versione portable. Chiude A3 | Richiesta esplicita; entrambi generati da Tauri senza strumenti esterni | Confermata |

## Fonti e API

| # | Decisione | Motivo | Stato |
| --- | --- | --- | --- |
| D7 | Fonte principale: API di CurseForge | Catalogo più grande | Confermata |
| D8 | In sviluppo si usa la chiave API personale di CurseForge | Disponibile subito dal console | Confermata |
| D9 | La richiesta di chiave per applicazioni di terzi si manda quando il progetto sta in piedi | Serve un progetto da mostrare; la chiave personale basta per sviluppare | Confermata |
| D10 | La chiave sta in `.env` (gitignored) come `CURSEFORGE_API_KEY` e non lascia mai il backend Rust | Sicurezza; i termini CurseForge vietano di esporla | Confermata |
| D11 | Niente scraping del sito CurseForge e niente riuso della chiave di WowUp | Violano i termini d'uso | Confermata |
| D12 | Le fonti sono plugin dietro un'interfaccia comune | Permette di aggiungere Wago, WoWInterface e GitHub senza toccare il resto | Proposta |
| D13 | I download rispettano `allowModDistribution = false` | Termini CurseForge | Proposta |

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
