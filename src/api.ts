import { invoke } from "@tauri-apps/api/core";

// Tipi che rispecchiano quelli serializzati dal backend (camelCase).

export type AppStatus = {
  version: string;
  curseforgeKeyConfigured: boolean;
};

/** Il tema dell'interfaccia; `system` segue il sistema operativo. */
export type Theme = "system" | "light" | "dark";

/**
 * Le impostazioni dell'app come le restituiscono `get_settings` e i comandi `set_*` (il
 * `SettingsView` del backend): quelle salvate in `settings.json` più `trayAvailable`.
 */
export type AppSettings = {
  theme: Theme;
  /** Installare e aggiornare alla Beta quando la versione non ha una Release (issue #141). */
  betaWhenNoRelease: boolean;
  /** Chiudere la finestra la nasconde nella tray invece di chiudere l'app (issue #158). */
  closeToTray: boolean;
  /**
   * Non salvato: se adesso l'icona nella tray c'è e qualcuno la mostra. Può cambiare mentre
   * l'app gira (per esempio un'estensione di GNOME attivata o disattivata).
   */
  trayAvailable: boolean;
  /** L'aggiornamento automatico degli addon (D41, issue #199): niente, solo avviso o installa. */
  autoUpdate: AutoUpdateMode;
  /** Le ore tra due giri dell'aggiornamento automatico: una di 1, 3, 6, 12 e 24. */
  autoUpdateIntervalHours: number;
};

/** Cosa fa l'aggiornamento automatico (`AutoUpdateMode` del backend, in minuscolo). */
export type AutoUpdateMode = "off" | "notify" | "install";

/**
 * L'aggiornamento automatico di un'installazione (`get_auto_update`, issue #199). Un escluso è
 * `{ source, key }`: si abbina a una riga con `project.source` e `project.key` del suo
 * `AddonUpdate`, mai con `project.id`, che vale `0` fuori da CurseForge.
 */
export type InstallationAutoUpdate = {
  /** L'installazione non partecipa per intero. */
  disabled: boolean;
  excluded: { source: SourceKind; key: string }[];
};

/** Un progetto con un aggiornamento trovato da un giro automatico (`AvailableUpdate`). */
export type AvailableUpdate = {
  source: SourceKind;
  key: string;
  name: string;
  latest: FileSummary;
};

/**
 * Un errore di un giro automatico. Con `source` e `key` nulli `name` è una cartella che nessuna
 * fonte ha riconosciuto; con `name` vuoto è la versione precedente che non si è potuta tenere
 * per l'Undo (gli aggiornamenti restano installati).
 */
export type AutoUpdateError = {
  source: SourceKind | null;
  key: string | null;
  name: string;
  message: string;
};

/** L'esito di un giro automatico su un'installazione (`InstallationReport`, issue #200). */
export type InstallationReport = {
  installationId: string;
  installationName: string;
  /** ISO 8601 UTC. */
  checkedAt: string;
  available: AvailableUpdate[];
  /** Solo in `install`. */
  updated: UpdateOutcome[];
  /** I progetti esclusi dall'utente: una scelta, non un problema. */
  excluded: UpdateOutcome[];
  /** Solo in `install`: i progetti che la fonte salta, con il motivo. */
  skipped: UpdateOutcome[];
  errors: AutoUpdateError[];
  /** Il controllo dell'installazione è fallito per intero. */
  checkError: string | null;
};

/** Un giro automatico finito (`RoundReport`): `mode` è `notify` o `install`. */
export type RoundReport = {
  mode: Exclude<AutoUpdateMode, "off">;
  startedAt: string;
  finishedAt: string;
  /** Un controllo è fallito per la rete: il prossimo giro è anticipato. */
  retrying: boolean;
  /** Solo le installazioni che partecipano. */
  installations: InstallationReport[];
};

/** Un giro annullabile con `undo_last_auto_update` (issue #201). */
export type UndoableRound = {
  installationId: string;
  /** ISO 8601 UTC. */
  roundAt: string;
  /** Quanti addon l'Undo rimetterebbe. */
  addons: number;
};

/**
 * Lo stato dell'aggiornamento automatico: la risposta di `get_auto_update_status` e il payload
 * dell'evento `auto-update` (emesso alla fine di ogni giro e dopo un Undo).
 */
export type AutoUpdateStatus = {
  running: boolean;
  /** ISO 8601 UTC, o `null` (modalità `off`, giro in corso, primo giro non ancora pianificato). */
  nextRoundAt: string | null;
  /** `null` prima del primo giro. */
  lastRound: RoundReport | null;
  undoable: UndoableRound[];
};

/** L'esito dell'Undo di un progetto; `message` spiega `skipped` ed `error`. */
export type UndoOutcome = {
  source: SourceKind;
  key: string;
  name: string;
  status: "restored" | "skipped" | "error";
  message?: string;
};

export type Installation = {
  id: string;
  path: string;
  flavor: string;
  name: string;
};

/**
 * Un'installazione trovata sul computer e non ancora aggiunta (`discover_installations`): gli
 * stessi `path`, `flavor` e `name` di `Installation`, più da dove viene (testo in inglese).
 */
export type DiscoveredInstallation = {
  path: string;
  flavor: string;
  name: string;
  origin: string;
};

export type InstalledAddon = {
  folder: string;
  tocFile: string | null;
  title: string | null;
  interface: number[];
  version: string | null;
  author: string | null;
  notes: string | null;
  dependencies: string[];
  optionalDeps: string[];
  loadOnDemand: boolean;
  /** `## Group` del `.toc`: la cartella sotto cui il gioco raggruppa questo addon. */
  group: string | null;
  curseProjectId: string | null;
  wagoId: string | null;
  wowiId: string | null;
  modifiedMs: number | null;
  fingerprint: number | null;
  error: string | null;
  /** Il progetto che l'app ha installato in questa cartella (`installed-addons.json`), se c'è. */
  installedProject: InstalledProject | null;
};

/**
 * Un progetto CurseForge installato dall'app: l'id e le sue cartelle ancora presenti, con il
 * nome che hanno sul disco. Una cartella che compare nei record di più progetti non ne ha.
 */
export type InstalledProject = { id: number; folders: string[] };

/**
 * Esito di `install_zip` (D32): le cartelle dello zip ora in `AddOns` e quelle che esistevano
 * già con lo stesso nome e sono state sostituite, con il nome che avevano sul disco.
 */
export type LocalInstall = {
  installed: string[];
  replaced: string[];
  /** Un avviso su un'installazione riuscita (per esempio record dell'app non aggiornati). */
  warning?: string;
};

export type FileSummary = { id: number; name: string; date: string };

/** Le fonti degli addon (D12, D31); su disco e nei tipi del backend sono in minuscolo. */
export type SourceKind = "curseforge" | "github" | "wowinterface" | "tukui";

export type Project = {
  /** L'id CurseForge; `0` per le altre fonti, dove valgono `source` e `key`. */
  id: number;
  source: SourceKind;
  /** L'id del progetto nella fonte, come stringa (`61284`, `owner/repo`, `-2`). */
  key: string;
  name: string;
  url: string | null;
  allowModDistribution: boolean;
};

/** Risultato di `check_updates` per una cartella; `status` dice quale variante è. */
export type AddonUpdate = {
  folder: string;
  /** La fonte che ha reclamato la cartella, se c'è. */
  source: SourceKind | null;
  project: Project | null;
  installedFile: FileSummary | null;
  /** Versione del gioco forzata per il progetto (`gameVersionTypeId`), se c'è. */
  versionOverride: number | null;
  /** Le cartelle del file riconosciuto per questa cartella; vuoto se non riconosciuta. */
  projectFolders: string[];
  /**
   * Il mirror GitHub di un progetto CurseForge bloccato con un aggiornamento (D34, issue #163):
   * con un mirror Update e Reinstall scaricano da GitHub. `null` quando non è stato calcolato
   * (progetto non bloccato, aggiornato o di un'altra fonte) o non si può usare.
   */
  githubMirror: GithubMirror | null;
  /** Perché il mirror non si può usare (in inglese); `null` con un mirror o senza calcolo. */
  githubMirrorReason: string | null;
} & (
  | { status: "notMatched" }
  | { status: "upToDate" }
  | { status: "updateAvailable"; latest: FileSummary }
  /** Nessuna Release per la versione, ma una Beta più recente sì, con l'opzione spenta. */
  | { status: "betaOnly"; latest: FileSummary }
  | { status: "error"; message: string }
);

/** Il mirror GitHub di un progetto CurseForge bloccato: `owner/repo` e il tag della release. */
export type GithubMirror = { repo: string; tag: string };

/**
 * Esito di `github_mirror` (issue #164) per un progetto della Search. `available`: `install_addon`
 * lo installa da GitHub (`repo`, `tag`). Altrimenti `reason` dice perché; `reason` è `null` per
 * un progetto che CurseForge lascia scaricare (nessuna richiesta a GitHub).
 */
export type MirrorAvailability = {
  available: boolean;
  repo: string | null;
  tag: string | null;
  reason: string | null;
};

/** Esito di `update_addons` per un progetto; `message` spiega `skipped` ed `error`. */
export type UpdateOutcome = {
  /** L'id CurseForge; `0` per le altre fonti. */
  modId: number;
  source: SourceKind;
  key: string;
  name: string;
  status: "updated" | "skipped" | "error";
  message?: string;
  /** Il file installato, solo per `updated`. */
  file?: FileSummary;
};

/**
 * Anteprima di `resolve_link` (issue #109): il progetto di GitHub, WoWInterface o Tukui indicato
 * da un link. `id` va passato a `install_from_link`; `message` spiega un progetto non
 * scaricabile o avvisa.
 */
export type LinkPreview = {
  source: SourceKind;
  id: string;
  name: string;
  version: string;
  /** WoW-don lo può scaricare per il gioco dell'installazione. */
  downloadable: boolean;
  message?: string;
  /** La pagina del progetto sul sito della fonte. */
  url: string;
  /** L'app l'ha già installato. */
  installed: boolean;
};

/**
 * Esito di `install_addon` per un progetto: il primo è quello scelto, poi le dipendenze
 * obbligatorie in ampiezza. `name` può essere vuoto; `message` spiega `skipped` ed `error` o
 * avvisa su un'installazione riuscita.
 */
export type InstallOutcome = {
  modId: number;
  name: string;
  status: "installed" | "skipped" | "error";
  message?: string;
  /** Il file installato, solo per `installed`. */
  file?: FileSummary;
  /** Per una dipendenza, l'id del progetto che la richiede. */
  dependencyOf?: number;
};

/** Un tipo di versione del gioco che si può forzare per un addon (`list_version_types`). */
export type VersionType = { id: number; name: string };

/** Uno zip di backup di `WTF` e `Interface`. */
export type BackupInfo = {
  name: string;
  path: string;
  size: number;
  modifiedMs: number | null;
};

/** Una categoria degli addon di WoW su CurseForge (`list_categories`). */
export type CatalogCategory = { id: number; name: string; parentCategoryId: number | null };

/** L'ultimo file Release di un progetto per la versione del gioco della ricerca. */
/** `releaseType` 1 Release, 2 Beta (solo con l'opzione della Beta, issue #141). */
export type CatalogFile = { id: number; name: string; gameVersion: string; releaseType: number };

/** Un addon del catalogo di CurseForge, come lo restituisce `search_catalog`. */
export type CatalogAddon = {
  id: number;
  name: string;
  summary: string;
  authors: string[];
  downloadCount: number;
  /** Data ISO 8601 dell'ultima modifica del progetto. */
  dateModified: string;
  categories: CatalogCategory[];
  thumbnailUrl: string | null;
  url: string | null;
  /** Falso: CurseForge non permette di scaricarlo da un'app di terzi. */
  allowModDistribution: boolean;
  latestFile: CatalogFile | null;
  /** Il progetto è già nell'installazione. */
  installed: boolean;
};

/** Una pagina di `search_catalog`: `totalCount` conta tutti i risultati della ricerca. */
export type CatalogPage = {
  addons: CatalogAddon[];
  index: number;
  pageSize: number;
  totalCount: number;
  /** Risultati letti dall'API in questa pagina, nascosti compresi: 0 vuol dire che è finita. */
  fetched: number;
  /** Risultati tolti perché senza un file Release per la versione del gioco (issue #140). */
  hidden: number;
};

/** Un file del progetto nel dettaglio; `releaseType` 1 Release, 2 Beta. */
export type DetailFile = { id: number; name: string; date: string; releaseType: number };

/** Una dipendenza obbligatoria; `name` è vuoto se CurseForge non restituisce il progetto. */
export type Dependency = { id: number; name: string };

/** Un'immagine della gallery: URL pubblici del CDN di CurseForge, da caricare con `<img src>`.
 *  Titolo e descrizione sono testo semplice (vuoti se mancano): si mostrano solo come testo. */
export type Screenshot = {
  title: string;
  description: string;
  thumbnailUrl: string;
  url: string;
};

/**
 * Il dettaglio di un addon (`addon_detail`): i campi di `CatalogAddon` più la versione del
 * gioco usata, la descrizione, la gallery, i file recenti e le dipendenze obbligatorie.
 */
export type AddonDetail = CatalogAddon & {
  versionType: number;
  /** Testo semplice, che può contenere `<` e `>`: si mostra solo come testo, mai come HTML. */
  description: string;
  /** Le immagini della gallery, nell'ordine dell'API. */
  screenshots: Screenshot[];
  /** I file Release e Beta per la versione, dal più recente, al massimo 20. */
  files: DetailFile[];
  dependencies: Dependency[];
};

export type CatalogSort = "popularity" | "lastUpdated" | "name" | "totalDownloads";
export type SortOrder = "asc" | "desc";

/** I parametri di `search_catalog`; `null` lascia decidere al backend (tutte le categorie,
 *  la versione del gioco dell'installazione). */
export type CatalogSearch = {
  query: string;
  categoryId: number | null;
  versionType: number | null;
  sort: CatalogSort;
  sortOrder: SortOrder;
};

/** Il risultato di `refresh_addons` (issue #126). */
export type Refresh = {
  addons: InstalledAddon[];
  updates: AddonUpdate[];
  checkError: string | null;
};

/** Un progetto della stringa di import/export (issue #133). */
export type ListEntry = {
  source: SourceKind;
  key: string;
  name: string;
  /** La versione del gioco forzata per il progetto (`gameVersionTypeId`). */
  versionType?: number;
};

/** Il risultato di `export_addons`: la stringa, i progetti esportati e le cartelle rimaste fuori. */
export type ExportResult = {
  text: string;
  exported: ListEntry[];
  skipped: string[];
  /** Il controllo non è riuscito: i progetti vengono solo dai record dell'app. */
  checkError: string | null;
};

/** Una voce della stringa che non si è potuta leggere: posizione (da 1) e motivo. */
export type InvalidEntry = { position: number; reason: string };

export type PreviewState = "new" | "installed" | "notDownloadable";

export type PreviewEntry = {
  source: SourceKind;
  key: string;
  name: string;
  state: PreviewState;
  message?: string;
  /** La pagina dell'addon, per quelli che non si possono scaricare. */
  url?: string;
};

/** L'anteprima di `preview_import` (issue #134): non scrive niente. */
export type ImportPreview = {
  flavor: string;
  entries: PreviewEntry[];
  invalid: InvalidEntry[];
  /** Le cartelle che "Replace all" toglierebbe ora. */
  removals: string[];
  checkError?: string;
  lookupError?: string;
};

export type ImportMode = "keep" | "replace";

export type ImportEntryStatus = "installed" | "alreadyInstalled" | "skipped" | "error";

export type ImportEntryOutcome = {
  source: SourceKind;
  key: string;
  name: string;
  status: ImportEntryStatus;
  message?: string;
};

/** Il risultato di `import_addons`. */
export type ImportResult = {
  entries: ImportEntryOutcome[];
  invalid: InvalidEntry[];
  removed: string[];
  /** Cartelle comparse dopo l'anteprima: non confermate, restano. */
  notConfirmed: string[];
  removalErrors: { folder: string; message: string }[];
  /** Perché "Replace all" non ha tolto niente. */
  removalSkipped?: string;
  lookupError?: string;
};

/** L'esito di `remove_addons` per una cartella; `message` spiega `error`. */
export type FolderRemoval = {
  folder: string;
  status: "removed" | "error";
  message: string | null;
};

/** Il risultato di `remove_addons` (issue #147): un esito per cartella, nell'ordine chiesto. */
export type RemoveResult = {
  folders: FolderRemoval[];
  /** Le cartelle sono tolte ma il registro non si è aggiornato. */
  warning: string | null;
};

export const api = {
  appStatus: () => invoke<AppStatus>("app_status"),
  listInstallations: () => invoke<Installation[]>("list_installations"),
  addInstallation: (path: string) => invoke<Installation>("add_installation", { path }),
  /** Le installazioni già aggiunte sono escluse dal backend; nessuna trovata è una lista vuota. */
  discoverInstallations: () => invoke<DiscoveredInstallation[]>("discover_installations"),
  removeInstallation: (id: string) => invoke<void>("remove_installation", { id }),
  /** `force` ricalcola tutti i fingerprint invece di prenderli dalla cache del backend. */
  listAddons: (installationId: string, force = false) =>
    invoke<InstalledAddon[]>("list_addons", { installationId, force }),
  checkUpdates: (installationId: string) =>
    invoke<AddonUpdate[]>("check_updates", { installationId }),
  /** Lista e stati da una sola scansione; un controllo fallito arriva in `checkError`. */
  refreshAddons: (installationId: string, force = false) =>
    invoke<Refresh>("refresh_addons", { installationId, force }),
  /**
   * `modIds` sono progetti CurseForge, `projects` quelli delle altre fonti. Con `reinstall`
   * (issue #148) riscarica anche i progetti già aggiornati.
   */
  updateAddons: (
    installationId: string,
    modIds: number[],
    projects: { source: SourceKind; key: string }[] = [],
    reinstall = false,
  ) => invoke<UpdateOutcome[]>("update_addons", { installationId, modIds, projects, reinstall }),
  /** Toglie le cartelle da `Interface/AddOns`, senza backup; `WTF` non si tocca (issue #147). */
  removeAddons: (installationId: string, folders: string[]) =>
    invoke<RemoveResult>("remove_addons", { installationId, folders }),
  listVersionTypes: () => invoke<VersionType[]>("list_version_types"),
  setVersionOverride: (installationId: string, modId: number, versionType: number) =>
    invoke<void>("set_version_override", { installationId, modId, versionType }),
  clearVersionOverride: (installationId: string, modId: number) =>
    invoke<void>("clear_version_override", { installationId, modId }),
  createBackup: (installationId: string) =>
    invoke<BackupInfo>("create_backup", { installationId }),
  listBackups: (installationId: string) =>
    invoke<BackupInfo[]>("list_backups", { installationId }),
  searchCatalog: (installationId: string, search: CatalogSearch, index: number) =>
    invoke<CatalogPage>("search_catalog", {
      installationId,
      query: search.query,
      categoryId: search.categoryId,
      versionType: search.versionType,
      sort: search.sort,
      sortOrder: search.sortOrder,
      index,
    }),
  /** `versionType` `null`: la versione forzata per il progetto, altrimenti quella
   *  dell'installazione. */
  addonDetail: (installationId: string, modId: number, versionType: number | null) =>
    invoke<AddonDetail>("addon_detail", { installationId, modId, versionType }),
  /** `fileId` assente: l'ultimo Release per la versione del gioco del progetto. */
  installAddon: (installationId: string, modId: number, fileId?: number) =>
    invoke<InstallOutcome[]>("install_addon", { installationId, modId, fileId: fileId ?? null }),
  /**
   * Il mirror GitHub di un progetto CurseForge bloccato (issue #164), per lo stesso `fileId` di
   * `installAddon`. Fa richieste a GitHub (60 l'ora): si chiama solo dal dettaglio o da Install,
   * mai per tutti i risultati della ricerca.
   */
  githubMirror: (installationId: string, modId: number, fileId?: number) =>
    invoke<MirrorAvailability>("github_mirror", { installationId, modId, fileId: fileId ?? null }),
  /** Un link non riconosciuto o malformato è rifiutato con un messaggio che dice cosa incollare. */
  resolveLink: (installationId: string, url: string) =>
    invoke<LinkPreview>("resolve_link", { installationId, url }),
  /** L'esito ha la forma di `UpdateOutcome`: `updated` vuol dire installato. */
  installFromLink: (installationId: string, source: SourceKind, id: string) =>
    invoke<UpdateOutcome>("install_from_link", { installationId, source, id }),
  /** `path` è il percorso dello zip trascinato o scelto: il file lo legge il backend. */
  installZip: (installationId: string, path: string) =>
    invoke<LocalInstall>("install_zip", { installationId, path }),
  listCategories: () => invoke<CatalogCategory[]>("list_categories"),
  getSettings: () => invoke<AppSettings>("get_settings"),
  /** Un valore sconosciuto è rifiutato dal backend. */
  setTheme: (theme: Theme) => invoke<AppSettings>("set_theme", { theme }),
  setBetaWhenNoRelease: (enabled: boolean) =>
    invoke<AppSettings>("set_beta_when_no_release", { enabled }),
  setCloseToTray: (enabled: boolean) => invoke<AppSettings>("set_close_to_tray", { enabled }),
  /** Lo stato reale, riletto dal sistema operativo: non sta in `settings.json` (issue #159). */
  getAutostart: () => invoke<boolean>("get_autostart"),
  /** Restituisce lo stato riletto dal sistema operativo dopo la modifica. */
  setAutostart: (enabled: boolean) => invoke<boolean>("set_autostart", { enabled }),
  /** Un valore sconosciuto (modalità o intervallo) è rifiutato dal backend. */
  setAutoUpdate: (mode: AutoUpdateMode, intervalHours: number) =>
    invoke<AppSettings>("set_auto_update", { mode, intervalHours }),
  getAutoUpdate: (installationId: string) =>
    invoke<InstallationAutoUpdate>("get_auto_update", { installationId }),
  /** Esclude il progetto (`excluded`) o lo riammette; restituisce lo stato salvato. */
  setAutoUpdateExcluded: (
    installationId: string,
    project: { source: SourceKind; key: string },
    excluded: boolean,
  ) =>
    invoke<InstallationAutoUpdate>("set_auto_update_excluded", {
      installationId,
      project,
      excluded,
    }),
  setInstallationAutoUpdate: (installationId: string, enabled: boolean) =>
    invoke<InstallationAutoUpdate>("set_installation_auto_update", { installationId, enabled }),
  /** Memoria del backend e il manifesto della versione precedente, nessuna rete. */
  getAutoUpdateStatus: () => invoke<AutoUpdateStatus>("get_auto_update_status"),
  /** Rifiuta se non c'è niente da annullare; emette `auto-update` con lo stato nuovo. */
  undoLastAutoUpdate: (installationId: string) =>
    invoke<UndoOutcome[]>("undo_last_auto_update", { installationId }),
  exportAddons: (installationId: string) =>
    invoke<ExportResult>("export_addons", { installationId }),
  previewImport: (installationId: string, text: string) =>
    invoke<ImportPreview>("preview_import", { installationId, text }),
  /** `confirmedRemovals`: le cartelle che l'utente ha visto e confermato per "Replace all". */
  importAddons: (
    installationId: string,
    text: string,
    mode: ImportMode,
    confirmedRemovals: string[] = [],
  ) => invoke<ImportResult>("import_addons", { installationId, text, mode, confirmedRemovals }),
};

/** I comandi rifiutano con il messaggio del backend; qualunque altra cosa diventa testo. */
export function errorMessage(error: unknown): string {
  return typeof error === "string" ? error : String(error);
}
