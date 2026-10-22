# Contesto

Ultimo aggiornamento: 2026-10-22.

## L'idea

Un gestore di addon per World of Warcraft per Linux e Windows. Su Linux il gioco gira
dentro un prefisso Wine (Lutris, Battle.net), ed è il caso da cui nasce il progetto. Lo spunto è WoW Forever
beta, giocato su Linux con Battle.net sotto Lutris e WowUp-CF come gestore addon.

## Perché un altro gestore

WowUp-CF funziona, ma:

- l'interfaccia e i filtri sono scomodi: cercare, filtrare e ordinare gli addon è
  lento e poco chiaro;
- su Linux è un AppImage Electron: serve `--no-sandbox`, l'auto-update rompe il
  symlink e il percorso del gioco nel prefisso Wine va impostato a mano;
- le versioni beta (come Forever) sono coperte male dai metadati delle fonti.

WoW-don vuole risolvere questi punti con un'app nativa più leggera (Tauri + Rust),
una UI centrata su ricerca e filtri, e la gestione dei prefissi Wine come funzione di
primo piano.

## Precedenti

- **WowUp / WowUp-CF**: Electron, multipiattaforma, usa le API di CurseForge con una
  chiave approvata, più Wago, WoWInterface e GitHub.
- **Ajour**: gestore in Rust con GUI `iced`. Abbandonato nel 2022, in buona parte
  perché CurseForge ha chiuso le API pubbliche e ha iniziato a richiedere una chiave.
- **Instawow**: gestore da terminale in Python, con le fonti come plugin.

La lezione di Ajour: il rischio principale non è tecnico ma di accesso ai dati.

## Fonti degli addon

- **CurseForge**: il catalogo più grande. Richiede una chiave API, anche per
  scaricare i file dalla CDN. Esistono due tipi di chiave: personale (dal console di
  CurseForge for Studios) e per applicazioni di terzi (form approvato da Overwolf).
- **Wago Addons**: alternativa sostenuta da autori grossi; chiave API con accordo.
- **WoWInterface**: catalogo storico con API più accessibile.
- **GitHub releases**: libere; molti addon importanti pubblicano lì.

Molti autori usano il BigWigs packager, che pubblica la stessa release su tutte le
fonti: gli addon popolari si trovano quasi sempre in più di un posto.

## Riferimenti

- API CurseForge: <https://docs.curseforge.com/rest-api>
- Accesso alle API per terzi: <https://support.curseforge.com/support/solutions/articles/9000208346>
- Chiave obbligatoria per i download: <https://blog.curseforge.com/introducing-api-key-authentication-for-curseforge-file-downloads/>
