// The held addons of the Installed tab ("Don't update", D41, issues #204 and #291): who is
// held, the context menu item, the effect of a choice on the multiple selection and what
// "Update all" leaves out. A held project is skipped by the automatic rounds and by Update all;
// it is updated only on an explicit request for that project. Pure functions, no React or I/O.

import type { AddonUpdate, InstallationAutoUpdate } from "../api";
import { groupUpdate, type AddonGroup } from "./filter.ts";
import { isUpdatable } from "./mirror.ts";
import { projectId, type ProjectRef } from "./sources.ts";

/**
 * The excluded projects of `get_auto_update`, by project identity (`projectId`): an excluded
 * project matches a row by `project.source` and `project.key`, never by `project.id`.
 */
export function excludedIds(state: InstallationAutoUpdate): Set<string> {
  return new Set(state.excluded.map(projectId));
}

/** The row is excluded: its project, if it has one, is among the excluded. */
export function isExcluded(
  update: AddonUpdate | undefined,
  excluded: ReadonlySet<string> | undefined,
): boolean {
  const project = update?.project;
  return Boolean(project && excluded?.has(projectId(project)));
}

/** The projects of the groups, one per source and id: those the menu item acts on. */
export function exclusionProjects(
  groups: AddonGroup[],
  updates?: Map<string, AddonUpdate>,
): ProjectRef[] {
  const projects = new Map<string, ProjectRef>();
  for (const group of groups) {
    const project = groupUpdate(group, updates)?.project;
    if (project) projects.set(projectId(project), { source: project.source, key: project.key });
  }
  return [...projects.values()];
}

/**
 * What the menu item does: it excludes (`true`) if at least one of the projects still takes
 * part, otherwise it readmits them all (`false`). So with a mixed selection the first choice
 * excludes them all, and the second readmits them.
 */
export function exclusionChoice(
  projects: ProjectRef[],
  excluded: ReadonlySet<string> | undefined,
): boolean {
  return projects.some((project) => !excluded?.has(projectId(project)));
}

/** The item text: "Don't update" when it holds, "Allow updates" when it releases. */
export function exclusionLabel(exclude: boolean): string {
  return exclude ? "Don't update" : "Allow updates";
}

/**
 * Who is excluded after the choice on `projects`: the other excluded projects stay as they are.
 * The Installed rows show it while the choice is being saved; then the state returned by the
 * backend applies.
 */
export function excludedAfter(
  excluded: ReadonlySet<string>,
  projects: ProjectRef[],
  exclude: boolean,
): Set<string> {
  const next = new Set(excluded);
  for (const project of projects) {
    if (exclude) next.add(projectId(project));
    else next.delete(projectId(project));
  }
  return next;
}

/**
 * The reason the item is disabled, or `null`. `excluded` is `undefined` until the excluded
 * projects have been read; `loadError` is the error of that read.
 */
export function exclusionDisabledReason(
  projects: ProjectRef[],
  state: {
    updates?: Map<string, AddonUpdate>;
    checking: boolean;
    excluded?: ReadonlySet<string>;
    saving: boolean;
    loadError?: string | null;
    count: number;
  },
): string | null {
  if (state.saving) return "Wait for the update hold to be saved.";
  if (state.count === 0) return "No addons selected.";
  if (projects.length === 0) {
    if (state.updates === undefined) {
      return state.checking
        ? "Wait for the update check to finish: it finds the source of each addon."
        : "Check for updates first: it finds the source of each addon.";
    }
    return state.count === 1
      ? "No recognized source: only addons matched to a source can be held."
      : "No recognized source: none of these addons is matched to a source.";
  }
  return heldUnknownReason(state.excluded, state.loadError);
}

/**
 * Why the held addons are not known yet, or `null` once they are: until then neither the menu
 * item nor "Update all" can act, so nothing held is updated by mistake.
 */
export function heldUnknownReason(
  excluded: ReadonlySet<string> | undefined,
  loadError?: string | null,
): string | null {
  if (excluded !== undefined) return null;
  return loadError
    ? `Could not read the held addons: ${loadError}`
    : "Wait for the held addons to be loaded.";
}

/** The `title` of the mark in the row of a held addon. */
export const EXCLUDED_TITLE =
  "Held: not updated by automatic updates or by Update all. Right-click the row and choose Allow updates to update it again.";

/** The `title` of the version pill of a held addon with an update available. */
export function heldUpdateTitle(latest: string): string {
  return `Update available: ${latest}. Held: right-click the row to update it anyway or to allow updates.`;
}

/**
 * The groups with an update available that are not held: the header's "N to update". Unknown
 * held addons (`undefined`) hold nothing.
 */
export function updateCount(
  groups: AddonGroup[],
  updates: Map<string, AddonUpdate> | undefined,
  excluded: ReadonlySet<string> | undefined,
): number {
  return groups.filter((group) => {
    const update = groupUpdate(group, updates);
    return update?.status === "updateAvailable" && !isExcluded(update, excluded);
  }).length;
}

/**
 * The projects "Update all" sends: those the app can update on its own, one per source and
 * id, without the held ones. The number on the button is the length of this list.
 */
export function updatableProjects(
  groups: AddonGroup[],
  updates: Map<string, AddonUpdate> | undefined,
  excluded: ReadonlySet<string> | undefined,
): ProjectRef[] {
  return projectsWhere(groups, updates, (update) => !isExcluded(update, excluded));
}

/**
 * The held projects the app could update: those the menu's "Update to X" / "Update N" sends,
 * on purpose, one at a time or for the selection. Unknown held addons give none.
 */
export function heldUpdateProjects(
  groups: AddonGroup[],
  updates: Map<string, AddonUpdate> | undefined,
  excluded: ReadonlySet<string> | undefined,
): ProjectRef[] {
  return projectsWhere(groups, updates, (update) => isExcluded(update, excluded));
}

function projectsWhere(
  groups: AddonGroup[],
  updates: Map<string, AddonUpdate> | undefined,
  keep: (update: AddonUpdate) => boolean,
): ProjectRef[] {
  const projects = new Map<string, ProjectRef>();
  for (const group of groups) {
    const update = groupUpdate(group, updates);
    if (update?.project && isUpdatable(update) && keep(update)) {
      projects.set(projectId(update.project), update.project);
    }
  }
  return [...projects.values()];
}
