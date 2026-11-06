// The update check results of the Installed tab, kept for the session per installation. They live
// outside the component because changing installation reloads the component: going back to one
// already checked does not call CurseForge again. No React and no I/O, so `node --test` can read
// it (issue #274).

import type { AddonUpdate } from "../api";

/** Update check results per installation, valid for the session. */
export const updatesCache = new Map<string, AddonUpdate[]>();
/** Installations with an automatic check already started, so as not to call CurseForge twice. */
const autoChecked = new Set<string>();
/** The last check error per installation, to show it also when going back to it. */
export const errorCache = new Map<string, string>();
/**
 * How many times each installation's check results were dropped. A check records its result only
 * if the generation is still the one it started with: one already in flight when the results are
 * dropped would otherwise write back statuses read before the change.
 */
const checkGenerations = new Map<string, number>();

export function checkGeneration(id: string): number {
  return checkGenerations.get(id) ?? 0;
}

/**
 * Whether the visit to `id` must start the automatic check: only the first visit in the session,
 * with no result in the cache. A `true` answer counts as started, so the next visit gets `false`
 * until `forgetUpdateChecks` drops the installation.
 */
export function claimAutoCheck(id: string): boolean {
  if (updatesCache.has(id) || autoChecked.has(id)) return false;
  autoChecked.add(id);
  return true;
}

/**
 * Drops the update check results of `installationIds`: after an update or a version change, and
 * when an automatic round found something newer than what the table checked (issue #268). The
 * next visit to one of them checks again on its own; the table on screen rereads through `reload`.
 */
export function forgetUpdateChecks(installationIds: readonly string[]): void {
  for (const id of installationIds) {
    updatesCache.delete(id);
    autoChecked.delete(id);
    errorCache.delete(id);
    checkGenerations.set(id, checkGeneration(id) + 1);
  }
}
