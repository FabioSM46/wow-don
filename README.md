# WoW-don

Gestore di addon per World of Warcraft per **Linux e Windows**. Su Linux gestisce il
gioco dentro un prefisso Wine (Lutris, Battle.net), su Windows l'installazione
normale. App desktop nativa scritta in Rust con interfaccia Tauri, alternativa più
leggera e più comoda a WowUp-CF. Si distribuisce come pacchetto `.deb` su Linux e
come installer su Windows.

## Stato

**Fase M0: scaffolding.** C'è lo scheletro dell'app Tauri, che si avvia e mostra se
la chiave API di CurseForge è configurata. La gestione degli addon non è ancora
implementata. Il piano è in [docs/roadmap.md](docs/roadmap.md).

## Perché

WowUp-CF fa il suo lavoro, ma su Linux ha diversi limiti:

- **interfaccia e filtri scomodi**: cercare, filtrare e ordinare gli addon è lento e
  poco chiaro;
- **Electron in AppImage**: serve `--no-sandbox` e l'auto-update rompe il symlink;
- **prefissi Wine come ripiego**: il percorso del gioco va impostato a mano;
- **versioni beta coperte male**: per versioni come WoW Forever i metadati delle
  fonti spesso non indicano la compatibilità.

WoW-don punta su tre cose:

1. **Ricerca, filtri e ordinamento** come funzione principale, sia sugli addon
   installati sia sul catalogo.
2. **Installazioni di WoW nei prefissi Wine** gestite fin dall'inizio, comprese più
   versioni del gioco (Retail, Classic, beta) nella stessa cartella.
3. **App nativa e leggera**: un solo binario Rust con webview di sistema, niente
   Electron.

Il contesto completo è in [docs/context.md](docs/context.md).

## Funzionalità previste

- Aggiunta di installazioni di WoW (cartelle `_retail_`, `_classic_era_`,
  `_classic_beta_`, ...), anche dentro prefissi Wine.
- Lettura degli addon installati dai file `.toc`.
- Riconoscimento degli addon installati tramite fingerprint di CurseForge.
- Controllo degli aggiornamenti e aggiornamento singolo o di tutti gli addon.
- Ricerca nel catalogo CurseForge con filtri per categoria, versione del gioco,
  popolarità e data di aggiornamento.
- Versione del gioco forzata per singolo addon, utile per le beta.
- Backup di `WTF` e `Interface`.
- Più avanti: GitHub releases, WoWInterface e Wago come fonti aggiuntive.

## Fonti degli addon e API CurseForge

La fonte principale è l'[API di CurseForge](https://docs.curseforge.com/rest-api).
L'API richiede una chiave, anche per scaricare i file.

- **In sviluppo** si usa una chiave personale, generata dal console di CurseForge for
  Studios (`console.curseforge.com`).
- **Per distribuire l'app** servirà una chiave per applicazioni di terzi, da
  richiedere a CurseForge/Overwolf quando il progetto sarà abbastanza avanti.

Regole che il progetto rispetta:

- la chiave resta nel backend Rust: il frontend non la vede e non finisce nei log;
- niente scraping del sito CurseForge e niente riuso di chiavi di altre app;
- gli addon con `allowModDistribution = false` non vengono scaricati
  automaticamente.

## Stack

| Parte | Tecnologia |
| --- | --- |
| Backend | Rust (edition 2024) |
| App desktop | Tauri 2 |
| Frontend | React 19 + TypeScript, Vite |
| Package manager | pnpm |

Le decisioni e i motivi sono in [docs/decisions.md](docs/decisions.md).

## Struttura del repo

```
wow-don/
├── src/                    # frontend React
│   ├── App.tsx
│   ├── App.css
│   └── main.tsx
├── src-tauri/              # backend Rust e configurazione Tauri
│   ├── src/
│   │   ├── main.rs         # entry point
│   │   ├── lib.rs          # setup di Tauri e registrazione dei comandi
│   │   ├── config.rs       # lettura della chiave API da ambiente / .env
│   │   └── commands.rs     # comandi invocabili dal frontend
│   ├── capabilities/       # permessi Tauri della finestra
│   ├── icons/
│   ├── Cargo.toml
│   └── tauri.conf.json
├── docs/
│   ├── context.md          # idea, motivazioni, precedenti, fonti
│   ├── decisions.md        # registro delle decisioni
│   ├── architecture.md     # moduli, API usate, sicurezza
│   └── roadmap.md          # milestone
├── scripts/                # helper della pipeline e test dell'automazione
├── .github/                # CI, review DeepSeek, labeler, template delle issue
├── .claude/skills/         # skill per gli agenti (/dev-issue, /process-pr, /scrum-master)
├── AGENTS.md               # regole per gli agenti AI e pipeline
├── .env.example            # modello del file con la chiave API
├── index.html
├── package.json
└── vite.config.ts
```

La struttura prevista dei moduli del backend è in
[docs/architecture.md](docs/architecture.md).

## Sviluppo

### Requisiti

| Strumento | Versione | Serve per |
| ----- | ------------------------------- | ------------------ |
| Rust  | fissata in `src-tauri/rust-toolchain.toml` (rustfmt + clippy) | backend |
| Node  | 22                              | frontend |
| pnpm  | 10                              | frontend |
| gh    | 2.18.0 o successivo             | script della pipeline |
| jq    | qualunque recente               | script della pipeline, `scripts/test/` |
| python3 | 3.12 consigliato              | test del reviewer DeepSeek |

Su **Windows** servono Microsoft C++ Build Tools (carico di lavoro "Sviluppo di
applicazioni desktop con C++") e WebView2, già presente su Windows 10 e 11.

Su **Linux** servono le librerie di sistema per Tauri. Su Ubuntu/Debian:

  ```bash
  sudo apt install libwebkit2gtk-4.1-dev build-essential curl wget file \
    libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev
  ```

### Chiave API

```bash
cp .env.example .env
chmod 600 .env
# poi inserisci la chiave in CURSEFORGE_API_KEY
```

`.env` è in `.gitignore` e non va mai committato. In debug l'app lo legge dalla
radice del repo; in alternativa basta esportare `CURSEFORGE_API_KEY` nell'ambiente.

### Comandi

```bash
pnpm install          # dipendenze del frontend
pnpm tauri dev        # avvia l'app in sviluppo con hot reload
pnpm tauri build      # build di release e pacchetto della piattaforma corrente
```

`pnpm tauri build` produce il pacchetto della piattaforma su cui gira: su Linux il
`.deb` (`wow-don_<versione>_amd64.deb`) in `src-tauri/target/release/bundle/deb/`, su Windows l'installer
`*-setup.exe` in `src-tauri/target/release/bundle/nsis/`. Un pacchetto per Windows si
costruisce su Windows (o sul runner Windows della CI), non da Linux.

Controlli rapidi:

```bash
pnpm build                              # type-check e build del frontend
cargo check --manifest-path src-tauri/Cargo.toml
```

### Flusso di lavoro

Il branch di integrazione è `develop` (branch predefinito); `main` riceve solo i rilasci. Il
lavoro passa da issue → PR verso `develop` → CI e review automatica DeepSeek → merge. Le regole
per gli agenti AI e i dettagli della pipeline sono in [AGENTS.md](AGENTS.md).

## Documentazione

- [Contesto](docs/context.md)
- [Decisioni](docs/decisions.md)
- [Architettura](docs/architecture.md)
- [Roadmap](docs/roadmap.md)

## Note

World of Warcraft è un marchio di Blizzard Entertainment. WoW-don è un progetto
indipendente, non affiliato né approvato da Blizzard, CurseForge o Overwolf.
