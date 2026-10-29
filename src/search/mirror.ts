// The GitHub mirror of blocked CurseForge projects in the Search tab (D34, issue #165): the
// state of the check (`github_mirror`), what Install does and the texts of the row and detail.
// Since issue #223 the check starts on its own for visible blocked projects with a GitHub
// repository, and a blocked project without a repository goes to the website without any request.
// Pure functions, no React or I/O.

import type { CatalogAddon, MirrorAvailability } from "../api";

/**
 * The mirror check of a project in an installation. A `done` outcome holds for the
 * session (a second click does not repeat the request); an error does not: the next click retries.
 */
export type MirrorCheck =
  // `forInstall`: requested by Install, which waits for the outcome; without it, it is the
  // automatic check.
  | { status: "checking"; forInstall?: boolean }
  | { status: "done"; result: MirrorAvailability }
  | { status: "failed"; message: string };

/** CurseForge does not let you download the project: Install goes through the mirror check. */
export function isBlocked(addon: Pick<CatalogAddon, "allowModDistribution">): boolean {
  return !addon.allowModDistribution;
}

/** The reason, the same as `github_mirror`'s, for a blocked project without a GitHub repository. */
export const NO_REPOSITORY = "The CurseForge project does not link a GitHub repository.";

/**
 * The outcome that holds for `addon`: a blocked project without `githubRepo` has no possible
 * mirror, so it is already "website only" without asking anything; for the others the
 * session's check (or `undefined`).
 */
export function mirrorCheckFor(
  addon: Pick<CatalogAddon, "allowModDistribution" | "githubRepo">,
  check: MirrorCheck | undefined,
): MirrorCheck | undefined {
  if (!isBlocked(addon) || addon.githubRepo !== null) return check;
  return {
    status: "done",
    result: { available: false, repo: null, tag: null, reason: NO_REPOSITORY },
  };
}

/** How many automatic mirror checks at a time: each one also reads the project from
 *  CurseForge. */
export const MIRROR_CHECKS_AT_ONCE = 4;

/**
 * The visible projects to check on their own, in list order: blocked, with a GitHub
 * repository, not installed and without an outcome or a check already in progress (`known`).
 * A failed check stays known: only a click on Install retries.
 */
export function mirrorsToCheck<
  T extends Pick<CatalogAddon, "allowModDistribution" | "githubRepo" | "installed">,
>(addons: readonly T[], known: (addon: T) => boolean, limit: number): T[] {
  const pending: T[] = [];
  for (const addon of addons) {
    if (pending.length >= limit) break;
    if (!isBlocked(addon) || addon.githubRepo === null || addon.installed) continue;
    if (!known(addon)) pending.push(addon);
  }
  return pending;
}

/**
 * What Install does with the outcome of the check:
 * - `install`: `install_addon` (from the mirror, or from CurseForge if the project is no
 *   longer blocked);
 * - `website`: nothing to install, the reason and the link to the page are shown;
 * - `check`: the outcome is missing (never requested, or failed): the check first;
 * - `wait`: the check is in progress.
 */
export type MirrorStep =
  | { kind: "install" }
  | { kind: "website"; reason: string }
  | { kind: "check" }
  | { kind: "wait" };

export function mirrorStep(check: MirrorCheck | undefined): MirrorStep {
  if (!check || check.status === "failed") return { kind: "check" };
  if (check.status === "checking") return { kind: "wait" };
  const { result } = check;
  // `reason` `null` without a mirror: CurseForge now lets you download the project.
  if (result.available || result.reason === null) return { kind: "install" };
  return { kind: "website", reason: result.reason };
}

/** `owner/repo tag` of the available mirror, or `null`. */
export function mirrorName(check: MirrorCheck | undefined): string | null {
  if (check?.status !== "done" || !check.result.available) return null;
  const { repo, tag } = check.result;
  return [repo, tag].filter(Boolean).join(" ") || null;
}

/** The label next to the name of a blocked project, with its hint. */
export type BlockedHint = { label: string; title: string };

const BLOCKED = "The author does not allow downloads from other apps";

/**
 * The label of the row and the detail: "via GitHub" with the mirror, "website only" with the
 * reason, "checking GitHub…" during the check, and before the check (or after an error)
 * a label that says Install will look on GitHub.
 */
export function blockedHint(check: MirrorCheck | undefined): BlockedHint {
  const name = mirrorName(check);
  if (name) {
    return {
      label: "via GitHub",
      title: `${name}\nCurseForge blocks third-party downloads: Install uses this GitHub release.`,
    };
  }
  const step = mirrorStep(check);
  if (step.kind === "website") {
    return { label: "website only", title: unavailableText(step.reason) };
  }
  if (step.kind === "wait") {
    return {
      label: "checking GitHub…",
      title: `${BLOCKED}: checking GitHub for the same release…`,
    };
  }
  return {
    label: "blocked on CurseForge",
    title: `${BLOCKED}: Install looks for the same release on GitHub first.`,
  };
}

/** Why the project installs only from the website: the mirror's reason after rule D13. */
export function unavailableText(reason: string): string {
  return `${BLOCKED} and the GitHub copy cannot be used: ${reason}`;
}

/** The detail's sentence under the name, for a blocked project that is not installed. */
export function detailNote(check: MirrorCheck | undefined): string {
  const name = mirrorName(check);
  if (name) return `${BLOCKED}: Install uses the same release from GitHub (${name}).`;
  const step = mirrorStep(check);
  switch (step.kind) {
    case "website":
      return `${unavailableText(step.reason)} Get it from the CurseForge website.`;
    case "wait":
      return `${BLOCKED}: checking GitHub for the same release…`;
    case "install":
      return "CurseForge now allows downloads of this addon.";
    case "check":
      return check?.status === "failed"
        ? `${BLOCKED}, and GitHub could not be checked: ${check.message}`
        : `${BLOCKED}: Install looks for the same release on GitHub first.`;
  }
}
