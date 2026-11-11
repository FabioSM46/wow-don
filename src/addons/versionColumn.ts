// The Version column of the Installed tab (D48, issue #313): the installed version and, when
// the check found an update, the version it leads to. Pure functions, no React or I/O.

import type { AddonUpdate } from "../api";

/** What the cell shows: `latest` is `null` unless an update is available. */
export type VersionCell = { installed: string; latest: string | null };

/** Shown in place of a version the `.toc` does not declare. */
export const NO_VERSION = "—";

const clean = (text: string | null | undefined): string | null => text?.trim() || null;

/**
 * `installed` is the `## Version` of the `.toc` (a dash when it is missing or blank); `latest`
 * is the name of the newer file, only for an addon with an update available. Every other
 * status (up to date, only beta files, error, not matched, not checked yet) shows the
 * installed version alone.
 */
export function versionCell(version: string | null, update?: AddonUpdate): VersionCell {
  return {
    installed: clean(version) ?? NO_VERSION,
    latest: update?.status === "updateAvailable" ? clean(update.latest.name) : null,
  };
}

/** The cell as a single line of text: `1.2.0`, `1.2.0 → 1.3.0`, `— → 1.3.0`. */
export function versionText(cell: VersionCell): string {
  return cell.latest === null ? cell.installed : `${cell.installed} → ${cell.latest}`;
}
