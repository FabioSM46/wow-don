// The GitHub mirror of blocked CurseForge projects in the Installed tab (D34, issue #165):
// the label next to the project, whether Update is enabled and whether Reinstall can help.
// Pure functions, no React or I/O.

import type { AddonUpdate } from "../api";

/** The text of a blocked project without a usable mirror. */
export const WEBSITE_ONLY_TITLE =
  "The author does not allow downloads from third-party apps: download it from the website, then drop the zip into this window.";

/** The label next to the project name: the text and its hint. */
export type BlockedHint = { label: string; title: string };

/** The update's usable mirror, if the check found one. */
function mirrorOf(update: AddonUpdate) {
  return update.project && !update.project.allowModDistribution
    ? (update.githubMirror ?? null)
    : null;
}

/**
 * The label of a project CurseForge does not let you download (D13): "via GitHub" with the
 * mirror found by the check, otherwise "website only" with the mirror's reason, if any.
 * `null` for a downloadable project or for no project.
 */
export function blockedHint(update: AddonUpdate | undefined): BlockedHint | null {
  const project = update?.project;
  if (!update || !project || project.allowModDistribution) return null;
  const mirror = mirrorOf(update);
  if (mirror) {
    return {
      label: "via GitHub",
      title: `${mirror.repo} ${mirror.tag}\nCurseForge blocks third-party downloads: Update installs this GitHub release.`,
    };
  }
  const reason = update.githubMirrorReason?.trim();
  return {
    label: "website only",
    title: reason ? `${WEBSITE_ONLY_TITLE}\nGitHub: ${reason}` : WEBSITE_ONLY_TITLE,
  };
}

/**
 * An update the app can install on its own: a downloadable project, or a blocked one for
 * which the check found the GitHub mirror (issue #163).
 */
export function isUpdatable(update: AddonUpdate): boolean {
  if (update.status !== "updateAvailable" || !update.project) return false;
  return update.project.allowModDistribution || mirrorOf(update) !== null;
}

/**
 * Why Reinstall cannot help for this project; `null` if the backend can try.
 *
 * A blocked project is reinstalled only from the mirror, which the backend looks for at
 * reinstall time (`update_project` in `updates.rs`): for an up-to-date project the check
 * does not compute it, so Reinstall stays possible. With an update available the backend
 * reinstalls that file, and the check has already said its mirror cannot be used
 * (`githubMirrorReason`): the backend would skip it, and Reinstall is not offered.
 */
export function reinstallBlockedReason(update: AddonUpdate | undefined): string | null {
  const project = update?.project;
  if (!update || !project || project.allowModDistribution) return null;
  if (update.status !== "updateAvailable" || mirrorOf(update)) return null;
  const reason = update.githubMirrorReason?.trim();
  if (!reason) return null;
  return `CurseForge blocks third-party downloads and the GitHub copy cannot be used: ${reason}`;
}
