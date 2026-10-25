import { invoke } from "@tauri-apps/api/core";

// Tipi che rispecchiano quelli serializzati dal backend (camelCase).

export type AppStatus = {
  version: string;
  curseforgeKeyConfigured: boolean;
};

/** Il tema dell'interfaccia; `system` segue il sistema operativo. */
export type Theme = "system" | "light" | "dark";

/** Le impostazioni dell'app salvate dal backend in `settings.json` (`get_settings`). */
export type AppSettings = { theme: Theme };

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
} & (
  | { status: "notMatched" }
  | { status: "upToDate" }
  | { status: "updateAvailable"; latest: FileSummary }
  | { status: "error"; message: string }
);

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
export type CatalogFile = { id: number; name: string; gameVersion: string };

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
};

/** Un file del progetto nel dettaglio; `releaseType` 1 Release, 2 Beta. */
export type DetailFile = { id: number; name: string; date: string; releaseType: number };

/** Una dipendenza obbligatoria; `name` è vuoto se CurseForge non restituisce il progetto. */
export type Dependency = { id: number; name: string };

/**
 * Il dettaglio di un addon (`addon_detail`): i campi di `CatalogAddon` più la versione del
 * gioco usata, la descrizione, i file recenti e le dipendenze obbligatorie.
 */
export type AddonDetail = CatalogAddon & {
  versionType: number;
  /** Testo semplice, che può contenere `<` e `>`: si mostra solo come testo, mai come HTML. */
  description: string;
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

export const api = {
  appStatus: () => invoke<AppStatus>("app_status"),
  listInstallations: () => invoke<Installation[]>("list_installations"),
  addInstallation: (path: string) => invoke<Installation>("add_installation", { path }),
  /** Le installazioni già aggiunte sono escluse dal backend; nessuna trovata è una lista vuota. */
  discoverInstallations: () => invoke<DiscoveredInstallation[]>("discover_installations"),
  removeInstallation: (id: string) => invoke<void>("remove_installation", { id }),
  listAddons: (installationId: string) =>
    invoke<InstalledAddon[]>("list_addons", { installationId }),
  checkUpdates: (installationId: string) =>
    invoke<AddonUpdate[]>("check_updates", { installationId }),
  updateAddons: (installationId: string, modIds: number[]) =>
    invoke<UpdateOutcome[]>("update_addons", { installationId, modIds }),
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
  listCategories: () => invoke<CatalogCategory[]>("list_categories"),
  getSettings: () => invoke<AppSettings>("get_settings"),
  /** Un valore sconosciuto è rifiutato dal backend. */
  setTheme: (theme: Theme) => invoke<AppSettings>("set_theme", { theme }),
};

/** I comandi rifiutano con il messaggio del backend; qualunque altra cosa diventa testo. */
export function errorMessage(error: unknown): string {
  return typeof error === "string" ? error : String(error);
}
