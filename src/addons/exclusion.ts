// The addons excluded from automatic updates in the Installed tab (D41, issue #204): who is
// excluded, the context menu item and the effect of a choice on the multiple selection.
// Pure functions, no React or I/O.

import type { AddonUpdate, InstallationAutoUpdate } from "../api";
import { groupUpdate, type AddonGroup } from "./filter.ts";
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

/** The item text: "Don't auto-update" when it excludes, "Allow auto-update" when it readmits. */
export function exclusionLabel(exclude: boolean): string {
  return exclude ? "Don't auto-update" : "Allow auto-update";
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
  if (state.saving) return "Wait for the automatic update choice to be saved.";
  if (state.count === 0) return "No addons selected.";
  if (projects.length === 0) {
    if (state.updates === undefined) {
      return state.checking
        ? "Wait for the update check to finish: it finds the source of each addon."
        : "Check for updates first: it finds the source of each addon.";
    }
    return state.count === 1
      ? "No recognized source: only addons matched to a source update automatically."
      : "No recognized source: none of these addons is matched to a source.";
  }
  if (state.excluded === undefined) {
    return state.loadError
      ? `Could not read the automatic update choices: ${state.loadError}`
      : "Wait for the automatic update choices to be loaded.";
  }
  return null;
}

/** The `title` of the mark in the row of an excluded addon. */
export const EXCLUDED_TITLE =
  "Excluded from automatic updates. Right-click the row and choose Allow auto-update to include it again.";
