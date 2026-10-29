# WoW-don

Addon manager for World of Warcraft on **Linux, Windows and macOS**. On Linux it manages
the game inside a Wine prefix (Lutris, Battle.net), on Windows and on macOS
the normal installation of the native client. A native desktop app written in Rust with a
Tauri interface, a lighter and more convenient alternative to WowUp-CF. It is distributed as a
`.deb` package on Linux, as an installer on Windows and as a universal `.dmg` (Apple
Silicon and Intel) on macOS.

## Status

**Phase M2: CurseForge.** WoW installations are added by hand (`_<flavor>_` folders, also
inside Wine prefixes) and you see the list of installed addons, read from the `.toc` files,
with search, filters and sorting. Addons are recognised on CurseForge through fingerprints and
the app shows which ones have an update (for now Retail and Classic Era). Download and
installation arrive with M3. The plan is in [docs/roadmap.md](docs/roadmap.md).

## Why

WowUp-CF does its job, but on Linux it has several limits:

- **awkward interface and filters**: searching, filtering and sorting addons is slow and
  unclear;
- **Electron in an AppImage**: it needs `--no-sandbox` and auto-update breaks the symlink;
- **Wine prefixes as a fallback**: the game path has to be set by hand;
- **beta versions poorly covered**: for versions like WoW Forever the sources' metadata
  often does not state compatibility.

WoW-don focuses on three things:

1. **Search, filters and sorting** as the main feature, both on installed addons and on the
   catalogue.
2. **WoW installations in Wine prefixes** handled from the start, including several
   versions of the game (Retail, Classic, beta) in the same folder.
3. **A native, lightweight app**: a single Rust binary with the system webview, no
   Electron.

The full context is in [docs/context.md](docs/context.md).

## Planned features

- Adding WoW installations (`_retail_`, `_classic_era_`, `_forever_`, `_classic_beta_`, ...
  folders), also inside Wine prefixes.
- Reading installed addons from the `.toc` files.
- Recognising installed addons through CurseForge fingerprints.
- Checking for updates and updating a single addon or all of them.
- Searching the CurseForge catalogue with filters for category, game version,
  popularity and update date.
- Game version forced per addon, useful for betas.
- Backup of `WTF` and `Interface`.
- Later on: GitHub releases, WoWInterface and Wago as additional sources.

## Addon sources and the CurseForge API

The main source is the [CurseForge API](https://docs.curseforge.com/rest-api).
The API requires a key. WoW-don has a third-party application key from CurseForge, and it
never ships in the app: it lives only in the secrets of the
[wow-don-proxy](https://github.com/FabioSM46/wow-don-proxy) Cloudflare Worker, which answers a
closed list of read-only, World of Warcraft-only calls. The backend calls the proxy; zips are
downloaded straight from CurseForge's CDN (D44).

Rules the project follows:

- the key is never in the app, the repository, a build or a log;
- no scraping of the CurseForge site and no reuse of other apps' keys;
- addons with `allowModDistribution = false` are not downloaded
  automatically.

## Stack

| Part | Technology |
| --- | --- |
| Backend | Rust (edition 2024) |
| Desktop app | Tauri 2 |
| Frontend | React 19 + TypeScript, Vite |
| Package manager | pnpm |

The decisions and their reasons are in [docs/decisions.md](docs/decisions.md).

## Repository structure

```
wow-don/
├── src/                    # React frontend
│   ├── App.tsx
│   ├── App.css
│   └── main.tsx
├── src-tauri/              # Rust backend and Tauri configuration
│   ├── src/
│   │   ├── main.rs         # entry point
│   │   ├── lib.rs          # Tauri setup and command registration
│   │   ├── config.rs       # the CurseForge proxy address (D44)
│   │   └── commands.rs     # commands the frontend can invoke
│   ├── capabilities/       # Tauri window permissions
│   ├── icons/
│   ├── Cargo.toml
│   └── tauri.conf.json
├── docs/
│   ├── context.md          # idea, motivation, prior art, sources
│   ├── decisions.md        # decision log
│   ├── architecture.md     # modules, APIs used, security
│   └── roadmap.md          # milestones
├── scripts/                # pipeline helpers and automation tests
├── .github/                # CI, DeepSeek review, labeler, issue templates
├── .claude/skills/         # agent skills (/dev-issue, /process-pr, /scrum-master)
├── AGENTS.md               # rules for AI agents and the pipeline
├── index.html
├── package.json
└── vite.config.ts
```

The planned structure of the backend modules is in
[docs/architecture.md](docs/architecture.md).

## Development

### Requirements

| Tool | Version | Needed for |
| ----- | ------------------------------- | ------------------ |
| Rust  | pinned in `src-tauri/rust-toolchain.toml` (rustfmt + clippy) | backend |
| Node  | 22                              | frontend |
| pnpm  | 10                              | frontend |
| gh    | 2.18.0 or later                 | pipeline scripts |
| jq    | any recent one                  | pipeline scripts, `scripts/test/` |
| python3 | 3.12 recommended              | DeepSeek reviewer tests |

On **Windows** you need the Microsoft C++ Build Tools (the "Desktop development with C++"
workload) and WebView2, already present on Windows 10 and 11.

On **macOS** you need the Xcode command line tools (`xcode-select --install`);
WebKit is already part of the system.

On **Linux** you need the system libraries for Tauri. On Ubuntu/Debian:

  ```bash
  sudo apt install libwebkit2gtk-4.1-dev build-essential curl wget file \
    libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev
  ```

### CurseForge

No key is needed, in development or in a release: the app reaches CurseForge through the
[wow-don-proxy](https://github.com/FabioSM46/wow-don-proxy) Worker, which holds the key (D44).
To test against a proxy run locally with `wrangler dev`, start a debug build with
`WOWDON_CURSEFORGE_PROXY=http://127.0.0.1:8787`; release builds accept only `https` there.

### Commands

```bash
pnpm install          # frontend dependencies
pnpm tauri dev        # starts the app in development with hot reload
pnpm tauri build      # release build and package for the current platform
```

`pnpm tauri build` produces the package for the platform it runs on: on Linux the
`.deb` (`wow-don_<version>_amd64.deb`) in `src-tauri/target/release/bundle/deb/`, on Windows the
`*-setup.exe` installer in `src-tauri/target/release/bundle/nsis/`, on macOS the `.dmg` in
`src-tauri/target/release/bundle/dmg/`. A Windows package is built on Windows
(or on the CI Windows runner) and a macOS one on macOS, not from Linux. The release's universal
`.dmg` is obtained with
`pnpm tauri build --target universal-apple-darwin --bundles dmg`, after adding the
`aarch64-apple-darwin` and `x86_64-apple-darwin` targets to the toolchain with `rustup target add`;
it ends up in `src-tauri/target/universal-apple-darwin/release/bundle/dmg/`.

Quick checks:

```bash
pnpm build                              # frontend type-check and build
pnpm test                               # frontend tests (node --test, Node 22.18 or later)
cargo check --manifest-path src-tauri/Cargo.toml
```

### Workflow

The integration branch is `develop` (the default branch); `main` receives only releases. Work
goes issue → PR against `develop` → CI and automated DeepSeek review → merge. The rules
for AI agents and the pipeline details are in [AGENTS.md](AGENTS.md).

## Release

A `v<version>` tag on a `main` commit starts `.github/workflows/release.yml`, which builds the `.deb`,
the Windows installer and the universal macOS `.dmg` and attaches them to a draft GitHub
release. The release is published by hand after checking it.

Every package contains `THIRD-PARTY-LICENSES`, with the licences of the crates and npm packages
compiled into the app. `scripts/third-party-licenses.py` writes it before every build, and the
`src-tauri/tauri.release*.conf.json` configuration puts it in the package: on Linux in
`/usr/share/doc/wow-don/` next to `copyright`, on Windows and macOS among the app's resources, together with `LICENSE`.

### Installing on macOS

Open the `.dmg` and drag WoW-don into Applications. The app is neither signed nor notarised,
so on the first launch macOS blocks it, saying it cannot verify the developer. To
open it the first time:

- in the Finder, in Applications, right-click (or Control-click) WoW-don and
  choose **Open**, then confirm with **Open** in the dialog;
- or, after the first blocked attempt, go to **System Settings → Privacy &
  Security** and, in the Security section, press **Open Anyway** next to the message about
  WoW-don.

You only need to do it once: from the following launches the app opens normally.

## Documentation

- [Context](docs/context.md)
- [Decisions](docs/decisions.md)
- [Architecture](docs/architecture.md)
- [Roadmap](docs/roadmap.md)

## Notes

World of Warcraft is a trademark of Blizzard Entertainment. WoW-don is an independent
project, neither affiliated with nor endorsed by Blizzard, CurseForge or Overwolf.

## Licence

WoW-don is free software, distributed under the [GNU General Public License v3.0 or
later](LICENSE) (`GPL-3.0-or-later`).
