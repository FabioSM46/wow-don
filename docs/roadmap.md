# Roadmap

Last updated: 2026-10-27.

## M0 — Scaffolding (done)

- Tauri 2 + React + TypeScript project.
- Reading the CurseForge key from `.env`.
- Documentation: context, decisions, architecture, roadmap.

## M1 — Installed addons (done)

- Add a WoW installation by hand (path of the `_<flavor>_` folder).
- Read `Interface/AddOns` and the `.toc` files.
- Show the list of installed addons with local search and filters.

## M2 — CurseForge (done)

- API client with a personal key.
- Recognition of installed addons through fingerprints.
- Update check for the installation's game version.

## M3 — Installation and update (done)

- Download, safe extraction and folder replacement.
- Single update and "update all".
- Game version forced per addon.
- Backup of the `WTF` and `Interface` folders.
- Addons with `allowModDistribution = false` (for example Details!) are recognised and
  report updates, but are updated by hand from the site (D13).

## M3.5 — English interface and tabs (done)

- Interface in English only, including the backend messages (D27).
- App split into tabs: for now Installed and Options, with the installation selector
  in the header; installation management and backups live in Options. The Search tab
  arrived with M4.

## M4 — Search and catalogue (done)

- Search tab, the first of three (Search, Installed, Options); at startup Installed stays open.
- Search in the CurseForge catalogue with filters (category, game version) and sorting
  (popularity, update date, name, downloads), in pages of 50.
- Addon detail: description converted to text by the backend, Release and Beta files for
  the game version, required dependencies.
- Installation of a new addon from the search or from the detail, with required
  dependencies installed automatically as in WowUp; addons with
  `allowModDistribution = false` send you to the site (D13).

## M4.5 — macOS, installation discovery and theme (done)

- The app also works on macOS with the native client (D29); `backend-macos` job in CI on
  PRs targeting `main` and on those that touch `ci.yml`.
- Universal `.dmg` package (Apple Silicon and Intel), unsigned, in the release (D24).
- Automatic discovery of installations: standard paths and the Battle.net registry on
  Windows (D30), `Applications` on macOS, Wine prefixes on Linux (`~/.wine`, Lutris, Bottles,
  Steam/Proton). Found installations are proposed in Options with "Add", never added on
  their own.
- Options: theme choice (system, light, dark) saved by the backend.

## M4.6 — Free sources (done)

Sources without advertising agreements or keys (D31), behind the common interface (D12):

- Installation of a zip downloaded by hand, by dragging it into the window or with "Install from
  zip…" (D32): this way addons with `allowModDistribution = false` can be installed too.
- `installed-addons.json` v2 with records per source and the matching order across sources (D33).
- Tukui (ElvUI, Tukui), WoWInterface and GitHub Releases, with installation from a link in the
  Search tab and each addon's source in the Installed tab.

Iteration 3: #102, #103, #104, #105, #106, #111, #117. Iteration 5: #107, #108, #109, #110.

## M4.7 — Speed, import/export and addon management (done)

- Fast scan, fingerprint cache, `refresh_addons` and progress bar
  (Iteration 4: #123–#128).
- Search that hides addons without a Release file for the game version, with an option
  to fall back to Betas (#140, #141).
- Export and import of the addon list as a `WOWDON1:` string, with "Keep my addons" and
  "Replace all" (#133–#135).
- Installed tab: context menu, multiple selection, reinstallation and removal
  (Iteration 5: #147, #148, #149).

## M4.8 — GitHub mirror, tray and fast import (done)

Iteration 6:

- GitHub mirror for blocked CurseForge projects: same version from `sourceUrl`,
  update, reinstallation and installation from Search, "via GitHub" in the Installed
  tab (#162–#165).
- GitHub releases read without the API, from the `releases/latest` redirect and the `release.json`
  (D37, #179).
- Tray icon, "Close to tray" and start with the system (D35, D36, #158–#160).
- Description and Gallery tabs in an addon's detail (#167, #168).
- Faster import: parallel downloads, sequential installation (#180); median over 12 CurseForge
  addons from 10.6 s to 4.5 s.

## M4.9 — Automatic addon updates

D41, issues #199–#204:

- Modes `off`, `notify` (default) and `install`, with an interval and exclusions per addon and per
  installation (#199).
- Automatic round at startup and at intervals in the backend (#200), count in the tray (#202).
- Previous version kept by automatic rounds and "Undo last auto-update" (#201).
- Options, "Don't auto-update" entry and status line in the Installed tab (#203, #204).

## M5 — Third-party application key and first release

- Send CurseForge/Overwolf the form for the application key.
- Ask Wago for read access to the API (D39), in parallel.
- Decide how the key reaches the installed app: today the backend reads it only from
  `CURSEFORGE_API_KEY` or from the checkout's `.env`, so an installed package starts without a
  key.
- First tested release: PR `develop → main` (the first to run `backend-macos` on the
  complete backend), then the `.deb`, NSIS installer and `.dmg` built by CI (D24) at the first tag.

## Later

- Optional personal GitHub token, to get past the limit of 60 requests an hour.
- Search in the WoWInterface catalogue.
- System notifications for automatic updates (D41): they require
  `tauri-plugin-notification`, a dependency to be approved.

- Wago Addons (D39): API access request sent by the user; the source is specified
  only if Wago grants a key to use in WoW-don.
