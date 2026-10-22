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

export const api = {
  appStatus: () => invoke<AppStatus>("app_status"),
  listInstallations: () => invoke<Installation[]>("list_installations"),
  addInstallation: (path: string) => invoke<Installation>("add_installation", { path }),
  removeInstallation: (id: string) => invoke<void>("remove_installation", { id }),
};

/** I comandi rifiutano con il messaggio del backend; qualunque altra cosa diventa testo. */
export function errorMessage(error: unknown): string {
  return typeof error === "string" ? error : String(error);
}
