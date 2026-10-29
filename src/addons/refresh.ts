// The result of `refresh_addons` (issue #126): the list always arrives, the statuses only if
// the check succeeded. Pure functions, no React or I/O.

import type { AddonUpdate, Refresh } from "../api";

/** The update statuses of a `refresh_addons`, or the reason they are missing. */
export type RefreshCheck = { error: null; updates: AddonUpdate[] } | { error: string };

export function refreshCheck(result: Refresh): RefreshCheck {
  // An empty message is not a reason worth showing: it counts as a generic error.
  if (result.checkError !== null) {
    return { error: result.checkError.trim() || "The update check failed." };
  }
  return { error: null, updates: result.updates };
}
