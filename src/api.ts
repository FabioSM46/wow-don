import { invoke } from "@tauri-apps/api/core";

// Types that mirror the ones serialized by the backend (camelCase).

export type AppStatus = {
  version: string;
};

/** The interface theme; `system` follows the operating system. */
export type Theme = "system" | "light" | "dark";

/**
 * The app settings as returned by `get_settings` and the `set_*` commands (the backend's
 * `SettingsView`): those saved in `settings.json` plus `trayAvailable`.
 */
export type AppSettings = {
  theme: Theme;
  /** Install and update to the Beta when the version has no Release (issue #141). */
  betaWhenNoRelease: boolean;
  /** Closing the window hides it in the tray instead of closing the app (issue #158). */
  closeToTray: boolean;
  /**
   * Not saved: whether the tray icon exists right now and something shows it. It can change
   * while the app runs (for example a GNOME extension enabled or disabled).
   */
  trayAvailable: boolean;
  /** Automatic addon updates (D41, issue #199): nothing, notify only, or install. */
  autoUpdate: AutoUpdateMode;
  /** The hours between two automatic update rounds: one of 1, 3, 6, 12 and 24. */
  autoUpdateIntervalHours: number;
};

/** What automatic updating does (the backend's `AutoUpdateMode`, in lowercase). */
export type AutoUpdateMode = "off" | "notify" | "install";

/**
 * Automatic updating of an installation (`get_auto_update`, issue #199). An exclusion is
 * `{ source, key }`: it matches a row through `project.source` and `project.key` of its
 * `AddonUpdate`, never through `project.id`, which is `0` outside CurseForge.
 */
export type InstallationAutoUpdate = {
  /** The installation does not take part at all. */
  disabled: boolean;
  excluded: { source: SourceKind; key: string }[];
};

/** A project with an update found by an automatic round (`AvailableUpdate`). */
export type AvailableUpdate = {
  source: SourceKind;
  key: string;
  name: string;
  latest: FileSummary;
};

/**
 * An error from an automatic round. With `source` and `key` null, `name` is a folder that no
 * source recognized; with `name` empty, it is the previous version that could not be kept
 * for Undo (the updates stay installed).
 */
export type AutoUpdateError = {
  source: SourceKind | null;
  key: string | null;
  name: string;
  message: string;
};

/** The outcome of an automatic round on an installation (`InstallationReport`, issue #200). */
export type InstallationReport = {
  installationId: string;
  installationName: string;
  /** ISO 8601 UTC. */
  checkedAt: string;
  available: AvailableUpdate[];
  /** Only in `install`. */
  updated: UpdateOutcome[];
  /** The projects excluded by the user: a choice, not a problem. */
  excluded: UpdateOutcome[];
  /** Only in `install`: the projects the source skips, with the reason. */
  skipped: UpdateOutcome[];
  errors: AutoUpdateError[];
  /** The check of the installation failed entirely. */
  checkError: string | null;
};

/** A finished automatic round (`RoundReport`): `mode` is `notify` or `install`. */
export type RoundReport = {
  mode: Exclude<AutoUpdateMode, "off">;
  startedAt: string;
  finishedAt: string;
  /** A check failed because of the network: the next round is brought forward. */
  retrying: boolean;
  /** Only the installations that take part. */
  installations: InstallationReport[];
};

/** A round that can be undone with `undo_last_auto_update` (issue #201). */
export type UndoableRound = {
  installationId: string;
  /** ISO 8601 UTC. */
  roundAt: string;
  /** How many addons Undo would put back. */
  addons: number;
};

/**
 * The automatic update status: the response of `get_auto_update_status` and the payload of
 * the `auto-update` event (emitted at the end of every round and after an Undo).
 */
export type AutoUpdateStatus = {
  running: boolean;
  /** ISO 8601 UTC, or `null` (`off` mode, round in progress, first round not yet scheduled). */
  nextRoundAt: string | null;
  /** `null` before the first round. */
  lastRound: RoundReport | null;
  undoable: UndoableRound[];
};

/** The outcome of a project's Undo; `message` explains `skipped` and `error`. */
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
 * An installation found on the computer and not yet added (`discover_installations`): the
 * same `path`, `flavor` and `name` as `Installation`, plus where it comes from (English text).
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
  /** `## Group` of the `.toc`: the folder under which the game groups this addon. */
  group: string | null;
  curseProjectId: string | null;
  wagoId: string | null;
  wowiId: string | null;
  modifiedMs: number | null;
  fingerprint: number | null;
  error: string | null;
  /** The project the app installed in this folder (`installed-addons.json`), if any. */
  installedProject: InstalledProject | null;
};

/**
 * A CurseForge project installed by the app: the id and its folders still present, with the
 * name they have on disk. A folder that appears in the records of several projects has none.
 */
export type InstalledProject = { id: number; folders: string[] };

/**
 * Outcome of `install_zip` (D32): the zip's folders now in `AddOns` and those that already
 * existed with the same name and were replaced, with the name they had on disk.
 */
export type LocalInstall = {
  installed: string[];
  replaced: string[];
  /** A warning about a successful installation (for example app records not updated). */
  warning?: string;
};

export type FileSummary = { id: number; name: string; date: string };

/** The addon sources (D12, D31); on disk and in the backend types they are lowercase. */
export type SourceKind = "curseforge" | "github" | "wowinterface" | "tukui";

export type Project = {
  /** The CurseForge id; `0` for the other sources, where `source` and `key` apply. */
  id: number;
  source: SourceKind;
  /** The project id in the source, as a string (`61284`, `owner/repo`, `-2`). */
  key: string;
  name: string;
  url: string | null;
  allowModDistribution: boolean;
};

/** Result of `check_updates` for a folder; `status` says which variant it is. */
export type AddonUpdate = {
  folder: string;
  /** The source that claimed the folder, if any. */
  source: SourceKind | null;
  project: Project | null;
  installedFile: FileSummary | null;
  /** Game version forced for the project (`gameVersionTypeId`), if any. */
  versionOverride: number | null;
  /** The folders of the file recognized for this folder; empty if not recognized. */
  projectFolders: string[];
  /**
   * The GitHub mirror of a blocked CurseForge project with an update (D34, issue #163): with a
   * mirror, Update and Reinstall download from GitHub. `null` when it was not computed (project
   * not blocked, up to date or from another source) or cannot be used.
   */
  githubMirror: GithubMirror | null;
  /** Why the mirror cannot be used (in English); `null` with a mirror or without computation. */
  githubMirrorReason: string | null;
} & (
  | { status: "notMatched" }
  | { status: "upToDate" }
  | { status: "updateAvailable"; latest: FileSummary }
  /** No Release for the version, but a newer Beta exists, with the option turned off. */
  | { status: "betaOnly"; latest: FileSummary }
  | { status: "error"; message: string }
);

/** The GitHub mirror of a blocked CurseForge project: `owner/repo` and the release tag. */
export type GithubMirror = { repo: string; tag: string };

/**
 * Outcome of `github_mirror` (issue #164) for a Search project. `available`: `install_addon`
 * installs it from GitHub (`repo`, `tag`). Otherwise `reason` says why; `reason` is `null` for
 * a project that CurseForge lets you download (no request to GitHub).
 */
export type MirrorAvailability = {
  available: boolean;
  repo: string | null;
  tag: string | null;
  reason: string | null;
};

/** Outcome of `update_addons` for a project; `message` explains `skipped` and `error`. */
export type UpdateOutcome = {
  /** The CurseForge id; `0` for the other sources. */
  modId: number;
  source: SourceKind;
  key: string;
  name: string;
  status: "updated" | "skipped" | "error";
  message?: string;
  /** The installed file, only for `updated`. */
  file?: FileSummary;
};

/**
 * Preview of `resolve_link` (issue #109): the GitHub, WoWInterface or Tukui project pointed to
 * by a link. `id` must be passed to `install_from_link`; `message` explains a project that
 * cannot be downloaded, or warns.
 */
export type LinkPreview = {
  source: SourceKind;
  id: string;
  name: string;
  version: string;
  /** WoW-don can download it for the installation's game. */
  downloadable: boolean;
  message?: string;
  /** The project page on the source's site. */
  url: string;
  /** The app has already installed it. */
  installed: boolean;
};

/**
 * Outcome of `install_addon` for a project: the first is the chosen one, then the required
 * dependencies breadth-first. `name` can be empty; `message` explains `skipped` and `error` or
 * warns about a successful installation.
 */
export type InstallOutcome = {
  modId: number;
  name: string;
  status: "installed" | "skipped" | "error";
  message?: string;
  /** The installed file, only for `installed`. */
  file?: FileSummary;
  /** For a dependency, the id of the project that requires it. */
  dependencyOf?: number;
};

/** A game version type that can be forced for an addon (`list_version_types`). */
export type VersionType = { id: number; name: string };

/** A backup zip of `WTF` and `Interface`. */
export type BackupInfo = {
  name: string;
  path: string;
  size: number;
  modifiedMs: number | null;
};

/** A WoW addon category on CurseForge (`list_categories`). */
export type CatalogCategory = { id: number; name: string; parentCategoryId: number | null };

/** The latest Release file of a project for the game version of the search. */
/** `releaseType` 1 Release, 2 Beta (only with the Beta option, issue #141). */
export type CatalogFile = { id: number; name: string; gameVersion: string; releaseType: number };

/** An addon from the CurseForge catalog, as returned by `search_catalog`. */
export type CatalogAddon = {
  id: number;
  name: string;
  summary: string;
  authors: string[];
  downloadCount: number;
  /** ISO 8601 date of the project's last modification. */
  dateModified: string;
  categories: CatalogCategory[];
  thumbnailUrl: string | null;
  url: string | null;
  /** False: CurseForge does not allow downloading it from a third-party app. */
  allowModDistribution: boolean;
  /**
   * The GitHub repository `owner/repo` derived from `links.sourceUrl`, with no extra requests.
   * It counts only with `allowModDistribution = false`: `null` means the GitHub mirror is
   * impossible (go straight to the site); a value means it is possible, but `github_mirror` is
   * needed to know whether the same version is there (issue #222, #223).
   */
  githubRepo: string | null;
  latestFile: CatalogFile | null;
  /** The project is already in the installation. */
  installed: boolean;
};

/** A page of `search_catalog`: `totalCount` counts all the results of the search. */
export type CatalogPage = {
  addons: CatalogAddon[];
  index: number;
  pageSize: number;
  totalCount: number;
  /** Results read from the API in this page, hidden ones included: 0 means it is over. */
  fetched: number;
  /** Results removed because they have no Release file for the game version (issue #140). */
  hidden: number;
};

/** A project file in the detail; `releaseType` 1 Release, 2 Beta. */
export type DetailFile = { id: number; name: string; date: string; releaseType: number };

/** A required dependency; `name` is empty if CurseForge does not return the project. */
export type Dependency = { id: number; name: string };

/** A gallery image: public URLs of the CurseForge CDN, to load with `<img src>`.
 *  Title and description are plain text (empty if missing): they are shown only as text. */
export type Screenshot = {
  title: string;
  description: string;
  thumbnailUrl: string;
  url: string;
};

/**
 * The detail of an addon (`addon_detail`): the fields of `CatalogAddon` plus the game version
 * used, the description, the gallery, the recent files and the required dependencies.
 */
export type AddonDetail = CatalogAddon & {
  versionType: number;
  /** Plain text, which can contain `<` and `>`: shown only as text, never as HTML. */
  description: string;
  /** The gallery images, in the API's order. */
  screenshots: Screenshot[];
  /** The Release and Beta files for the version, newest first, at most 20. */
  files: DetailFile[];
  dependencies: Dependency[];
};

export type CatalogSort = "popularity" | "lastUpdated" | "name" | "totalDownloads";
export type SortOrder = "asc" | "desc";

/** The parameters of `search_catalog`; `null` lets the backend decide (all categories, the
 *  installation's game version). */
export type CatalogSearch = {
  query: string;
  categoryId: number | null;
  versionType: number | null;
  sort: CatalogSort;
  sortOrder: SortOrder;
};

/** The result of `refresh_addons` (issue #126). */
export type Refresh = {
  addons: InstalledAddon[];
  updates: AddonUpdate[];
  checkError: string | null;
};

/** A project of the import/export string (issue #133). */
export type ListEntry = {
  source: SourceKind;
  key: string;
  name: string;
  /** The game version forced for the project (`gameVersionTypeId`). */
  versionType?: number;
};

/** The result of `export_addons`: the string, the exported projects and the folders left out. */
export type ExportResult = {
  text: string;
  exported: ListEntry[];
  skipped: string[];
  /** The check did not succeed: the projects come only from the app records. */
  checkError: string | null;
};

/** An entry of the string that could not be read: position (from 1) and reason. */
export type InvalidEntry = { position: number; reason: string };

export type PreviewState = "new" | "installed" | "notDownloadable";

export type PreviewEntry = {
  source: SourceKind;
  key: string;
  name: string;
  state: PreviewState;
  message?: string;
  /** The addon page, for those that cannot be downloaded. */
  url?: string;
};

/** The preview of `preview_import` (issue #134): it writes nothing. */
export type ImportPreview = {
  flavor: string;
  entries: PreviewEntry[];
  invalid: InvalidEntry[];
  /** The folders that "Replace all" would remove now. */
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

/** The result of `import_addons`. */
export type ImportResult = {
  entries: ImportEntryOutcome[];
  invalid: InvalidEntry[];
  removed: string[];
  /** Folders that appeared after the preview: not confirmed, they stay. */
  notConfirmed: string[];
  removalErrors: { folder: string; message: string }[];
  /** Why "Replace all" removed nothing. */
  removalSkipped?: string;
  lookupError?: string;
};

/** The outcome of `remove_addons` for a folder; `message` explains `error`. */
export type FolderRemoval = {
  folder: string;
  status: "removed" | "error";
  message: string | null;
};

/** The result of `remove_addons` (issue #147): one outcome per folder, in the order asked. */
export type RemoveResult = {
  folders: FolderRemoval[];
  /** The folders are removed but the registry was not updated. */
  warning: string | null;
};

export const api = {
  appStatus: () => invoke<AppStatus>("app_status"),
  listInstallations: () => invoke<Installation[]>("list_installations"),
  addInstallation: (path: string) => invoke<Installation>("add_installation", { path }),
  /** Installations already added are excluded by the backend; none found is an empty list. */
  discoverInstallations: () => invoke<DiscoveredInstallation[]>("discover_installations"),
  removeInstallation: (id: string) => invoke<void>("remove_installation", { id }),
  /** `force` recomputes all fingerprints instead of taking them from the backend cache. */
  listAddons: (installationId: string, force = false) =>
    invoke<InstalledAddon[]>("list_addons", { installationId, force }),
  checkUpdates: (installationId: string) =>
    invoke<AddonUpdate[]>("check_updates", { installationId }),
  /** List and statuses from a single scan; a failed check arrives in `checkError`. */
  refreshAddons: (installationId: string, force = false) =>
    invoke<Refresh>("refresh_addons", { installationId, force }),
  /**
   * `modIds` are CurseForge projects, `projects` those of the other sources. With `reinstall`
   * (issue #148) it downloads again even the projects already up to date.
   */
  updateAddons: (
    installationId: string,
    modIds: number[],
    projects: { source: SourceKind; key: string }[] = [],
    reinstall = false,
  ) => invoke<UpdateOutcome[]>("update_addons", { installationId, modIds, projects, reinstall }),
  /** Removes the folders from `Interface/AddOns`, no backup; `WTF` is left alone (issue #147). */
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
  /** `versionType` `null`: the version forced for the project, otherwise the installation's
   *  one. */
  addonDetail: (installationId: string, modId: number, versionType: number | null) =>
    invoke<AddonDetail>("addon_detail", { installationId, modId, versionType }),
  /** `fileId` absent: the latest Release for the project's game version. */
  installAddon: (installationId: string, modId: number, fileId?: number) =>
    invoke<InstallOutcome[]>("install_addon", { installationId, modId, fileId: fileId ?? null }),
  /**
   * The GitHub mirror of a blocked CurseForge project (issue #164), for the same `fileId` as
   * `installAddon`. It makes requests to GitHub (60 per hour): it is called only from the detail
   * or from Install, never for all the search results.
   */
  githubMirror: (installationId: string, modId: number, fileId?: number) =>
    invoke<MirrorAvailability>("github_mirror", { installationId, modId, fileId: fileId ?? null }),
  /** An unrecognized or malformed link is rejected with a message that says what to paste. */
  resolveLink: (installationId: string, url: string) =>
    invoke<LinkPreview>("resolve_link", { installationId, url }),
  /** The outcome has the shape of `UpdateOutcome`: `updated` means installed. */
  installFromLink: (installationId: string, source: SourceKind, id: string) =>
    invoke<UpdateOutcome>("install_from_link", { installationId, source, id }),
  /** `path` is the path of the dragged or chosen zip: the backend reads the file. */
  installZip: (installationId: string, path: string) =>
    invoke<LocalInstall>("install_zip", { installationId, path }),
  listCategories: () => invoke<CatalogCategory[]>("list_categories"),
  getSettings: () => invoke<AppSettings>("get_settings"),
  /** An unknown value is rejected by the backend. */
  setTheme: (theme: Theme) => invoke<AppSettings>("set_theme", { theme }),
  setBetaWhenNoRelease: (enabled: boolean) =>
    invoke<AppSettings>("set_beta_when_no_release", { enabled }),
  setCloseToTray: (enabled: boolean) => invoke<AppSettings>("set_close_to_tray", { enabled }),
  /** The real state, read back from the operating system: not in `settings.json` (issue #159). */
  getAutostart: () => invoke<boolean>("get_autostart"),
  /** Returns the state read back from the operating system after the change. */
  setAutostart: (enabled: boolean) => invoke<boolean>("set_autostart", { enabled }),
  /** An unknown value (mode or interval) is rejected by the backend. */
  setAutoUpdate: (mode: AutoUpdateMode, intervalHours: number) =>
    invoke<AppSettings>("set_auto_update", { mode, intervalHours }),
  getAutoUpdate: (installationId: string) =>
    invoke<InstallationAutoUpdate>("get_auto_update", { installationId }),
  /** Excludes the project (`excluded`) or readmits it; returns the saved state. */
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
  /** Backend memory and the previous version's manifest, no network. */
  getAutoUpdateStatus: () => invoke<AutoUpdateStatus>("get_auto_update_status"),
  /** Rejects if there is nothing to undo; emits `auto-update` with the new state. */
  undoLastAutoUpdate: (installationId: string) =>
    invoke<UndoOutcome[]>("undo_last_auto_update", { installationId }),
  exportAddons: (installationId: string) =>
    invoke<ExportResult>("export_addons", { installationId }),
  previewImport: (installationId: string, text: string) =>
    invoke<ImportPreview>("preview_import", { installationId, text }),
  /** `confirmedRemovals`: the folders the user saw and confirmed for "Replace all". */
  importAddons: (
    installationId: string,
    text: string,
    mode: ImportMode,
    confirmedRemovals: string[] = [],
  ) => invoke<ImportResult>("import_addons", { installationId, text, mode, confirmedRemovals }),
};

/** Commands reject with the backend's message; anything else becomes text. */
export function errorMessage(error: unknown): string {
  return typeof error === "string" ? error : String(error);
}
