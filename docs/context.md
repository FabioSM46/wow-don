# Context

Last updated: 2026-10-24.

## The idea

An addon manager for World of Warcraft on Linux, Windows and macOS (D29). On Linux the game
runs inside a Wine prefix (Lutris, Battle.net), and that is the case the project was born from;
on Windows and on macOS the native client is used. The starting point is WoW Forever
beta, played on Linux with Battle.net under Lutris and WowUp-CF as the addon manager.

## Why another manager

WowUp-CF works, but:

- the interface and the filters are awkward: searching, filtering and sorting addons is
  slow and unclear;
- on Linux it is an Electron AppImage: it needs `--no-sandbox`, auto-update breaks the
  symlink and the game path inside the Wine prefix has to be set by hand;
- beta versions (like Forever) are poorly covered by the sources' metadata.

WoW-don wants to solve these points with a lighter native app (Tauri + Rust),
a UI centred on search and filters, and handling of Wine prefixes as a first-class
feature.

## Prior art

- **WowUp / WowUp-CF**: Electron, cross-platform, uses the CurseForge APIs with an
  approved key, plus Wago, WoWInterface and GitHub.
- **Ajour**: a Rust manager with an `iced` GUI. Abandoned in 2022, largely
  because CurseForge closed its public APIs and started requiring a key.
- **Instawow**: a terminal manager in Python, with sources as plugins.

The lesson from Ajour: the main risk is not technical but access to the data.

## Addon sources

- **CurseForge**: the largest catalogue. Requires an API key, even to
  download files from the CDN. There are two kinds of key: personal (from the CurseForge for
  Studios console) and for third-party applications (a form approved by Overwolf).
- **Wago Addons**: an alternative backed by big authors; API key under agreement.
- **WoWInterface**: a long-standing catalogue with a more accessible API.
- **GitHub releases**: free; many important addons publish there.

Many authors use the BigWigs packager, which publishes the same release to all the
sources: popular addons can almost always be found in more than one place.

## References

- CurseForge API: <https://docs.curseforge.com/rest-api>
- Third-party API access: <https://support.curseforge.com/support/solutions/articles/9000208346>
- Key required for downloads: <https://blog.curseforge.com/introducing-api-key-authentication-for-curseforge-file-downloads/>
