// The header of the Installed tab (issue #314): which action is the primary one, the label of
// "Update all", the segments of the filter and the text of the status line under the title.
// Pure functions, no React or I/O.

import type { Show } from "./filter.ts";

/**
 * What stands in the header's primary place: the bulk actions while a selection exists,
 * "Update all" while there is something to update (or an update is running, so its progress
 * stays in view), otherwise "Check for updates".
 */
export type PrimaryAction = "selection" | "updateAll" | "check";

export function primaryAction(state: {
  /** Selection mode is on, even with no row selected yet. */
  selecting: boolean;
  /** An update or a reinstall is running. */
  running: boolean;
  /** The projects "Update all" would send. */
  updatable: number;
}): PrimaryAction {
  if (state.selecting) return "selection";
  return state.running || state.updatable > 0 ? "updateAll" : "check";
}

/** The text of "Update all": its count, or the progress of the run that locks it. */
export function updateAllLabel(state: {
  running: boolean;
  reinstalling: boolean;
  /** How many projects of the run are done or in progress; 0 before the first event. */
  position: number;
  /** The projects in the run. */
  total: number;
  /** The projects "Update all" would send. */
  updatable: number;
  /** The held addons are not known: a count could still include them, so it is not shown. */
  heldUnknown: boolean;
}): string {
  if (state.running) {
    const verb = state.reinstalling ? "Reinstalling" : "Updating";
    return state.position > 0 ? `${verb} ${state.position} of ${state.total}` : `${verb}…`;
  }
  return state.heldUnknown ? "Update all" : `Update all (${state.updatable})`;
}

export type ShowOption = { id: Show; label: string; count: number | null; disabled: boolean };

/**
 * The segments of the filter, each with its count. "Updates" has neither a count nor a use
 * until a check result exists.
 */
export function showOptions(counts: Record<Show, number>, checked: boolean): ShowOption[] {
  return [
    { id: "all", label: "All", count: counts.all, disabled: false },
    { id: "updates", label: "Updates", count: checked ? counts.updates : null, disabled: !checked },
    { id: "errors", label: "Errors", count: counts.errors, disabled: false },
  ];
}

/**
 * The counts of the status line, after the installation's name: "120 addons" (or "12 of 120
 * addons" when the filters hide some), "3 to update" and "checked 14:05". `toUpdate` is `null`
 * while the held addons are unknown, `checked` (the time, already formatted) until a check
 * result exists.
 */
export function statusParts(state: {
  total: number;
  visible: number;
  toUpdate: number | null;
  checked: string | null;
}): string[] {
  const shown = state.visible === state.total ? `${state.total}` : `${state.visible} of ${state.total}`;
  const parts = [`${shown} ${state.total === 1 ? "addon" : "addons"}`];
  if (state.toUpdate !== null && state.toUpdate > 0) parts.push(`${state.toUpdate} to update`);
  if (state.checked !== null) parts.push(`checked ${state.checked}`);
  return parts;
}
