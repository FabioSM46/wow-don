// Context menu and multiple selection of the Installed tab (issue #149): selection state,
// menu items and confirmation texts. Pure functions, no React or I/O.

import type { AddonUpdate, RemoveResult } from "../api";
import {
  exclusionChoice,
  exclusionDisabledReason,
  exclusionLabel,
  exclusionProjects,
} from "./exclusion.ts";
import { groupUpdate, type AddonGroup } from "./filter.ts";
import { reinstallBlockedReason } from "./mirror.ts";
import { projectId, type ProjectRef } from "./sources.ts";

/**
 * The selected rows, by the group's main folder; `null` outside selection mode. An active
 * mode can stay with no rows: a left click adds them.
 */
export type Selection = ReadonlySet<string> | null;

/** "Select" from the menu: the row joins the selection, and the mode turns on if it was off. */
export function selectRow(selection: Selection, key: string): Set<string> {
  return new Set(selection ?? []).add(key);
}

/** The left click in selection mode: adds the row or removes it. */
export function toggleRow(selection: Selection, key: string): Set<string> {
  const next = new Set(selection ?? []);
  if (next.has(key)) next.delete(key);
  else next.add(key);
  return next;
}

/**
 * The selection after re-reading the list: only rows that still exist remain (a removed
 * row leaves). If none of those that were there remains, the mode ends.
 */
export function pruneSelection(selection: Selection, existing: Iterable<string>): Selection {
  if (selection === null) return null;
  const present = new Set(existing);
  const kept = [...selection].filter((key) => present.has(key));
  if (kept.length === selection.size) return selection;
  return kept.length > 0 ? new Set(kept) : null;
}

/** The groups the menu acts on: the selection in selection mode, otherwise the row. */
export function menuTargets(
  groups: AddonGroup[],
  selection: Selection,
  row: string,
): AddonGroup[] {
  const keys = selection ?? new Set([row]);
  return groups.filter((group) => keys.has(group.main.folder));
}

/** All the groups' folders, once each: those "Remove" removes. */
export function foldersOf(groups: AddonGroup[]): string[] {
  const folders = new Set<string>();
  for (const group of groups) {
    folders.add(group.main.folder);
    for (const other of group.others) folders.add(other.folder);
  }
  return [...folders];
}

/**
 * The projects to reinstall: one per source and id, from the groups that have a source. Left
 * out are the projects the backend would surely skip (`reinstallBlockedReason`, issue #165).
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

export type MenuAction = "select" | "reinstall" | "autoUpdate" | "remove" | "clear";

export type MenuItem = {
  action: MenuAction;
  label: string;
  /** The reason the item is disabled, shown in the `title`; `null` if it is enabled. */
  disabledReason: string | null;
};

export type MenuState = {
  /** The statuses of the last check; `undefined` until there is one. */
  updates?: Map<string, AddonUpdate>;
  /** An update, a reinstall or a removal in progress. */
  busy: boolean;
  /** An update check in progress. */
  checking: boolean;
  /**
   * The projects excluded from automatic updates (`projectId`); `undefined` until
   * `get_auto_update` has answered (issue #204).
   */
  excluded?: ReadonlySet<string>;
  /** The error of reading the excluded projects, if it failed. */
  excludedError?: string | null;
  /** A "Don't auto-update" / "Allow auto-update" choice being saved. */
  savingExclusion?: boolean;
};

const BUSY_REASON = "Wait for the current operation to finish.";

/**
 * The menu items. Outside selection mode: Select, Reinstall, the automatic update item and
 * Remove for the row; inside: Reinstall N, the same item,
 * Remove N and Clear selection for the selection.
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
  const autoUpdate = autoUpdateItem(targets, state);
  if (!selecting) {
    return [
      { action: "select", label: "Select", disabledReason: null },
      { action: "reinstall", label: "Reinstall", disabledReason: reinstallReason },
      autoUpdate,
      { action: "remove", label: "Remove", disabledReason: removeReason },
    ];
  }
  return [
    { action: "reinstall", label: `Reinstall ${count}`, disabledReason: reinstallReason },
    autoUpdate,
    { action: "remove", label: `Remove ${count}`, disabledReason: removeReason },
    { action: "clear", label: "Clear selection", disabledReason: null },
  ];
}

/** "Don't auto-update" or "Allow auto-update" for the groups' projects (issue #204). */
function autoUpdateItem(targets: AddonGroup[], state: MenuState): MenuItem {
  const projects = exclusionProjects(targets, state.updates);
  return {
    action: "autoUpdate",
    label: exclusionLabel(exclusionChoice(projects, state.excluded)),
    disabledReason: exclusionDisabledReason(projects, {
      updates: state.updates,
      checking: state.checking,
      excluded: state.excluded,
      saving: state.savingExclusion ?? false,
      loadError: state.excludedError,
      count: targets.length,
    }),
  };
}

/** Why Reinstall is off when none of the groups has a project to reinstall. */
function noProjectsReason(targets: AddonGroup[], state: MenuState): string {
  const count = targets.length;
  // Projects blocked by CurseForge whose GitHub mirror cannot be used (issue #165).
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

/** The question of the confirmation bar: "Remove 2 addons (3 folders)?". */
export function confirmText(groups: AddonGroup[]): string {
  const folders = foldersOf(groups).length;
  return `Remove ${plural(groups.length, "addon", "addons")} (${plural(folders, "folder", "folders")})?`;
}

/** The count next to the buttons in selection mode. */
export function selectedText(selection: Selection): string {
  return `${selection?.size ?? 0} selected`;
}

/** The summary of a removal: the main line and the reason for each folder not removed. */
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

/** The folders actually removed, so their rows leave the selection. */
export function removedFolders(result: RemoveResult): string[] {
  return result.folders.filter((f) => f.status === "removed").map((f) => f.folder);
}
