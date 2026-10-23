import { invoke } from "@tauri-apps/api/core";

// Tipi che rispecchiano quelli serializzati dal backend (camelCase).

export type AppStatus = {
  version: string;
  curseforgeKeyConfigured: boolean;
};

export type Installation = {
  id: string;
  path: string;
  flavor: string;
  name: string;
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
};

export type FileSummary = { id: number; name: string; date: string };

export type Project = {
  id: number;
  name: string;
  url: string | null;
  allowModDistribution: boolean;
};

/** Risultato di `check_updates` per una cartella; `status` dice quale variante è. */
export type AddonUpdate = {
  folder: string;
  project: Project | null;
  installedFile: FileSummary | null;
  /** Versione del gioco forzata per il progetto (`gameVersionTypeId`), se c'è. */
  versionOverride: number | null;
} & (
  | { status: "notMatched" }
  | { status: "upToDate" }
  | { status: "updateAvailable"; latest: FileSummary }
  | { status: "error"; message: string }
);

/** Esito di `update_addons` per un progetto; `message` spiega `skipped` ed `error`. */
export type UpdateOutcome = {
  modId: number;
  name: string;
  status: "updated" | "skipped" | "error";
  message?: string;
  /** Il file installato, solo per `updated`. */
  file?: FileSummary;
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

export const api = {
  appStatus: () => invoke<AppStatus>("app_status"),
  listInstallations: () => invoke<Installation[]>("list_installations"),
  addInstallation: (path: string) => invoke<Installation>("add_installation", { path }),
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
};

/** I comandi rifiutano con il messaggio del backend; qualunque altra cosa diventa testo. */
export function errorMessage(error: unknown): string {
  return typeof error === "string" ? error : String(error);
}
