# WoW-don

Gestore di addon per World of Warcraft per **Linux, Windows e macOS**. Su Linux gestisce
il gioco dentro un prefisso Wine (Lutris, Battle.net), su Windows e su macOS
l'installazione normale del client nativo. App desktop nativa scritta in Rust con
interfaccia Tauri, alternativa più leggera e più comoda a WowUp-CF. Si distribuisce come
pacchetto `.deb` su Linux, come installer su Windows e come `.dmg` universale (Apple
Silicon e Intel) su macOS.

## Stato

**Fase M2: CurseForge.** Si aggiungono a mano le installazioni di WoW (cartelle
`_<flavor>_`, anche dentro prefissi Wine) e si vede la lista degli addon installati,
letta dai file `.toc`, con ricerca, filtri e ordinamento. Gli addon vengono riconosciuti
su CurseForge tramite fingerprint e l'app indica quali hanno un aggiornamento (per ora
Retail e Classic Era). Download e installazione arrivano con M3. Il piano è in [docs/roadmap.md](docs/roadmap.md).

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

Su **macOS** servono gli strumenti da riga di comando di Xcode (`xcode-select --install`);
WebKit fa già parte del sistema.

Su **Linux** servono le librerie di sistema per Tauri. Su Ubuntu/Debian:

  ```bash
  sudo apt install libwebkit2gtk-4.1-dev build-essential curl wget file \
    libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev
  ```

### Chiave API

```bash
cp .env.example .env
chmod 600 .env
# poi inserisci la chiave in CURSEFORGE_API_KEY, tra apici singoli
```

La chiave va **tra apici singoli**: `CURSEFORGE_API_KEY='$2a$10$...'`. Le chiavi di
CurseForge contengono `$`, e senza apici (o con apici doppi) il lettore di `.env` li
tratta come variabili da sostituire: la chiave arriva rovinata e CurseForge la rifiuta.

`.env` è in `.gitignore` e non va mai committato. In debug l'app lo legge dalla
radice del repo; in alternativa puoi esportare la chiave nell'ambiente, sempre tra
apici singoli perché anche la shell sostituirebbe i `$`:
`export CURSEFORGE_API_KEY='$2a$10$...'`.

### Comandi

```bash
pnpm install          # dipendenze del frontend
pnpm tauri dev        # avvia l'app in sviluppo con hot reload
pnpm tauri build      # build di release e pacchetto della piattaforma corrente
```

`pnpm tauri build` produce il pacchetto della piattaforma su cui gira: su Linux il
`.deb` (`wow-don_<versione>_amd64.deb`) in `src-tauri/target/release/bundle/deb/`, su Windows l'installer
`*-setup.exe` in `src-tauri/target/release/bundle/nsis/`, su macOS il `.dmg` in
`src-tauri/target/release/bundle/dmg/`. Un pacchetto per Windows si costruisce su Windows
(o sul runner Windows della CI) e uno per macOS su macOS, non da Linux. Il `.dmg`
universale della release si ottiene con
`pnpm tauri build --target universal-apple-darwin --bundles dmg`, dopo aver aggiunto i
target `aarch64-apple-darwin` e `x86_64-apple-darwin` al toolchain con `rustup target add`;
finisce in `src-tauri/target/universal-apple-darwin/release/bundle/dmg/`.

Controlli rapidi:

```bash
pnpm build                              # type-check e build del frontend
pnpm test                               # test del frontend (node --test, Node 22.18 o successivo)
cargo check --manifest-path src-tauri/Cargo.toml
```

### Flusso di lavoro

Il branch di integrazione è `develop` (branch predefinito); `main` riceve solo i rilasci. Il
lavoro passa da issue → PR verso `develop` → CI e review automatica DeepSeek → merge. Le regole
per gli agenti AI e i dettagli della pipeline sono in [AGENTS.md](AGENTS.md).

## Rilascio

Un tag `v<versione>` su un commit di `main` avvia `.github/workflows/release.yml`, che costruisce il `.deb`,
l'installer per Windows e il `.dmg` universale per macOS e li allega a una release di GitHub
in bozza. La release si pubblica a mano dopo averla controllata.

### Installare su macOS

Apri il `.dmg` e trascina WoW-don in Applicazioni. L'app non è firmata né notarizzata,
quindi al primo avvio macOS la blocca dicendo che non può verificarne lo sviluppatore. Per
aprirla la prima volta:

- nel Finder, in Applicazioni, fai clic con il tasto destro (o Ctrl-clic) su WoW-don e
  scegli **Apri**, poi conferma con **Apri** nel dialogo;
- oppure, dopo il primo tentativo bloccato, vai in **Impostazioni di Sistema → Privacy e
  sicurezza** e, nella sezione Sicurezza, premi **Apri comunque** accanto al messaggio su
  WoW-don.

Basta farlo una volta: dagli avvii successivi l'app si apre normalmente.

## Documentazione

- [Contesto](docs/context.md)
- [Decisioni](docs/decisions.md)
- [Architettura](docs/architecture.md)
- [Roadmap](docs/roadmap.md)

## Note

World of Warcraft è un marchio di Blizzard Entertainment. WoW-don è un progetto
indipendente, non affiliato né approvato da Blizzard, CurseForge o Overwolf.
