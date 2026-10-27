// Gli addon esclusi dall'aggiornamento automatico nella tab Installed (D41, issue #204): chi è
// escluso, la voce del menu contestuale e l'effetto di una scelta sulla selezione multipla.
// Funzioni pure, senza React né I/O.

import type { AddonUpdate, InstallationAutoUpdate } from "../api";
import { groupUpdate, type AddonGroup } from "./filter.ts";
import { projectId, type ProjectRef } from "./sources.ts";

/**
 * Gli esclusi di `get_auto_update`, per identità del progetto (`projectId`): un escluso si
 * abbina a una riga con `project.source` e `project.key`, mai con `project.id`.
 */
export function excludedIds(state: InstallationAutoUpdate): Set<string> {
  return new Set(state.excluded.map(projectId));
}

/** La riga è esclusa: il suo progetto, se ce l'ha, è tra gli esclusi. */
export function isExcluded(
  update: AddonUpdate | undefined,
  excluded: ReadonlySet<string> | undefined,
): boolean {
  const project = update?.project;
  return Boolean(project && excluded?.has(projectId(project)));
}

/** I progetti dei gruppi, uno per fonte e id: quelli su cui agisce la voce del menu. */
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
 * Cosa fa la voce del menu: esclude (`true`) se almeno uno dei progetti partecipa ancora,
 * altrimenti li riammette tutti (`false`). Così con una selezione mista la prima scelta li
 * esclude tutti, e la seconda li riammette.
 */
export function exclusionChoice(
  projects: ProjectRef[],
  excluded: ReadonlySet<string> | undefined,
): boolean {
  return projects.some((project) => !excluded?.has(projectId(project)));
}

/** Il testo della voce: "Don't auto-update" quando esclude, "Allow auto-update" quando riammette. */
export function exclusionLabel(exclude: boolean): string {
  return exclude ? "Don't auto-update" : "Allow auto-update";
}

/** Chi è escluso dopo la scelta su `projects`: gli altri esclusi restano come sono. */
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
 * Il motivo per cui la voce è disattivata, o `null`. `excluded` è `undefined` finché gli esclusi
 * non sono letti; `loadError` è l'errore di quella lettura.
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

/** Il `title` del segno nella riga di un addon escluso. */
export const EXCLUDED_TITLE =
  "Excluded from automatic updates. Right-click the row and choose Allow auto-update to include it again.";
