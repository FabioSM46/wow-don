// Installations found on the computer (issue #89): which rows to show in the "Found on this
// computer" section of Options. Pure functions, no React or I/O.

import type { DiscoveredInstallation, Installation } from "../api";

/**
 * The found installations still to offer: without those already in the installation list
 * (added from here or with "Add installation" after the scan) and without those in `added`,
 * the rows added from this section. The backend already excludes those known at scan time;
 * the filter here covers those added later. Both paths come from the backend, from the same
 * validation, so they are compared as they are.
 */
export function pendingDiscoveries(
  found: readonly DiscoveredInstallation[],
  installations: readonly Pick<Installation, "path">[],
  added: ReadonlySet<string> = new Set(),
): DiscoveredInstallation[] {
  const known = new Set(installations.map((installation) => installation.path));
  return found.filter((item) => !known.has(item.path) && !added.has(item.path));
}

/** The game folder name for a flavor, for example `_classic_era_`. */
export function flavorFolder(flavor: string): string {
  return `_${flavor}_`;
}
