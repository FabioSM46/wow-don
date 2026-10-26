// Menu contestuale e selezione multipla della tab Installed (issue #149): stato della
// selezione, voci del menu e testi della conferma. Funzioni pure, senza React né I/O.

import type { AddonUpdate, RemoveResult } from "../api";
import { groupUpdate, type AddonGroup } from "./filter.ts";
import { reinstallBlockedReason } from "./mirror.ts";
import { projectId, type ProjectRef } from "./sources.ts";

/**
 * Le righe selezionate, per cartella principale del gruppo; `null` fuori dalla modalità
 * selezione. Una modalità attiva può restare senza righe: il clic sinistro ne aggiunge.
 */
export type Selection = ReadonlySet<string> | null;

/** "Select" dal menu: la riga entra nella selezione, e la modalità si attiva se non lo era. */
export function selectRow(selection: Selection, key: string): Set<string> {
  return new Set(selection ?? []).add(key);
}

/** Il clic sinistro in modalità selezione: aggiunge la riga o la toglie. */
export function toggleRow(selection: Selection, key: string): Set<string> {
  const next = new Set(selection ?? []);
  if (next.has(key)) next.delete(key);
  else next.add(key);
  return next;
}

/**
 * La selezione dopo una rilettura della lista: restano solo le righe che esistono ancora (una
 * riga rimossa esce). Se non ne resta nessuna di quelle che c'erano, la modalità finisce.
 */
export function pruneSelection(selection: Selection, existing: Iterable<string>): Selection {
  if (selection === null) return null;
  const present = new Set(existing);
  const kept = [...selection].filter((key) => present.has(key));
  if (kept.length === selection.size) return selection;
  return kept.length > 0 ? new Set(kept) : null;
}

/** I gruppi su cui agisce il menu: la selezione in modalità selezione, altrimenti la riga. */
export function menuTargets(
  groups: AddonGroup[],
  selection: Selection,
  row: string,
): AddonGroup[] {
  const keys = selection ?? new Set([row]);
  return groups.filter((group) => keys.has(group.main.folder));
}

/** Tutte le cartelle dei gruppi, una volta sola: quelle che "Remove" toglie. */
export function foldersOf(groups: AddonGroup[]): string[] {
  const folders = new Set<string>();
  for (const group of groups) {
    folders.add(group.main.folder);
    for (const other of group.others) folders.add(other.folder);
  }
  return [...folders];
}

/**
 * I progetti da reinstallare: uno per fonte e id, dai gruppi che hanno una fonte. Restano fuori
 * i progetti che il backend salterebbe di sicuro (`reinstallBlockedReason`, issue #165).
 */
export function reinstallProjects(
  groups: AddonGroup[],
  updates?: Map<string, AddonUpdate>,
): ProjectRef[] {
  const projects = new Map<string, ProjectRef>();
  for (const group of groups) {
    const update = groupUpdate(group, updates);
    const project = update?.project;
    if (project && reinstallBlockedReason(update) === null) {
      projects.set(projectId(project), { source: project.source, key: project.key });
    }
  }
  return [...projects.values()];
}

export type MenuAction = "select" | "reinstall" | "remove" | "clear";

export type MenuItem = {
  action: MenuAction;
  label: string;
  /** Il motivo per cui la voce è disattivata, mostrato nel `title`; `null` se è attiva. */
  disabledReason: string | null;
};

export type MenuState = {
  /** Gli stati dell'ultimo controllo; `undefined` finché non ce n'è uno. */
  updates?: Map<string, AddonUpdate>;
  /** Un aggiornamento, una reinstallazione o una rimozione in corso. */
  busy: boolean;
  /** Un controllo degli aggiornamenti in corso. */
  checking: boolean;
};

const BUSY_REASON = "Wait for the current operation to finish.";

/**
 * Le voci del menu. Fuori dalla modalità selezione: Select, Reinstall e Remove per la riga;
 * dentro: Reinstall N, Remove N e Clear selection per la selezione.
 */
export function menuItems(
  targets: AddonGroup[],
  selecting: boolean,
  state: MenuState,
): MenuItem[] {
  const count = targets.length;
  const projects = reinstallProjects(targets, state.updates);
  let reinstallReason: string | null = null;
  if (state.busy) reinstallReason = BUSY_REASON;
  else if (count === 0) reinstallReason = "No addons selected.";
  else if (projects.length === 0) reinstallReason = noProjectsReason(targets, state);
  const removeReason = state.busy ? BUSY_REASON : count === 0 ? "No addons selected." : null;
  if (!selecting) {
    return [
      { action: "select", label: "Select", disabledReason: null },
      { action: "reinstall", label: "Reinstall", disabledReason: reinstallReason },
      { action: "remove", label: "Remove", disabledReason: removeReason },
    ];
  }
  return [
    { action: "reinstall", label: `Reinstall ${count}`, disabledReason: reinstallReason },
    { action: "remove", label: `Remove ${count}`, disabledReason: removeReason },
    { action: "clear", label: "Clear selection", disabledReason: null },
  ];
}

/** Perché Reinstall è spento quando nessuno dei gruppi ha un progetto da reinstallare. */
function noProjectsReason(targets: AddonGroup[], state: MenuState): string {
  const count = targets.length;
  // Progetti bloccati da CurseForge il cui mirror GitHub non si può usare (issue #165).
  const blocked = targets
    .map((group) => reinstallBlockedReason(groupUpdate(group, state.updates)))
    .filter((reason): reason is string => reason !== null);
  if (blocked.length > 0) {
    if (count === 1) return blocked[0];
    return blocked.length === count
      ? "CurseForge blocks third-party downloads of these addons and their GitHub copies cannot be used."
      : "None of these addons can be reinstalled: no recognized source, or CurseForge blocks the download and the GitHub copy cannot be used.";
  }
  if (state.updates === undefined) {
    return state.checking
      ? "Wait for the update check to finish: it finds the source of each addon."
      : "Check for updates first: it finds the source of each addon.";
  }
  return count === 1
    ? "No recognized source: this addon was not found on CurseForge, Tukui, WoWInterface or GitHub."
    : "No recognized source: none of these addons was found on CurseForge, Tukui, WoWInterface or GitHub.";
}

function plural(count: number, one: string, many: string): string {
  return `${count} ${count === 1 ? one : many}`;
}

/** La domanda della barra di conferma: "Remove 2 addons (3 folders)?". */
export function confirmText(groups: AddonGroup[]): string {
  const folders = foldersOf(groups).length;
  return `Remove ${plural(groups.length, "addon", "addons")} (${plural(folders, "folder", "folders")})?`;
}

/** Il conteggio vicino ai pulsanti in modalità selezione. */
export function selectedText(selection: Selection): string {
  return `${selection?.size ?? 0} selected`;
}

/** Il riepilogo di una rimozione: la riga principale e il motivo di ogni cartella non tolta. */
export function removalSummary(result: RemoveResult): {
  text: string;
  errors: { folder: string; message: string }[];
} {
  const removed = result.folders.filter((f) => f.status === "removed").length;
  const errors = result.folders
    .filter((f) => f.status === "error")
    .map((f) => ({ folder: f.folder, message: f.message?.trim() || "Could not be removed." }));
  const parts = [
    removed > 0 && `${plural(removed, "folder", "folders")} removed`,
    errors.length > 0 && `${plural(errors.length, "folder", "folders")} not removed`,
  ].filter(Boolean);
  return { text: parts.length > 0 ? parts.join(", ") : "Nothing was removed", errors };
}

/** Le cartelle davvero tolte, per far uscire le loro righe dalla selezione. */
export function removedFolders(result: RemoveResult): string[] {
  return result.folders.filter((f) => f.status === "removed").map((f) => f.folder);
}
