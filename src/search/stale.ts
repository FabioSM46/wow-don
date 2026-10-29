// The Search tab after an addon change made elsewhere (removal, reinstall, zip, import):
// which installations have results to read again and which session state no longer
// holds. Pure functions, no React or I/O.

import type { InstallRun } from "./InstallAction";

/** Installations and installed projects are per installation: the key holds them together. */
export function installKey(installationId: string, modId: number): string {
  return `${installationId}:${modId}`;
}

/** The installation of the key `installKey`: everything before the last `:`. */
function installationOf(key: string): string {
  return key.slice(0, key.lastIndexOf(":"));
}

/** `stale` with the installation `id`; the same set if it was already there. */
export function markStale(stale: ReadonlySet<string>, id: string): ReadonlySet<string> {
  return stale.has(id) ? stale : new Set(stale).add(id);
}

/** `stale` without the installations `done`; the same set if it removes none. */
export function clearStale(
  stale: ReadonlySet<string>,
  done: ReadonlySet<string>,
): ReadonlySet<string> {
  if (![...done].some((id) => stale.has(id))) return stale;
  return new Set([...stale].filter((id) => !done.has(id)));
}

/**
 * The projects installed in the session, without those of the `stale` installations: for those
 * the re-read search gives the "installed" mark again. The same set if it does not change.
 */
export function forgetInstalled(
  keys: Set<string>,
  stale: ReadonlySet<string>,
): Set<string> {
  const next = new Set([...keys].filter((key) => !stale.has(installationOf(key))));
  return next.size === keys.size ? keys : next;
}

/**
 * The installs started in the session, without those finished in the `stale` installations: an
 * earlier outcome no longer describes the addons on disk. Those in progress stay. The same map
 * if it does not change.
 */
export function forgetFinishedRuns<R extends { status: InstallRun["status"] }>(
  runs: Map<string, R>,
  stale: ReadonlySet<string>,
): Map<string, R> {
  const next = new Map(
    [...runs].filter(([key, run]) => run.status === "running" || !stale.has(installationOf(key))),
  );
  return next.size === runs.size ? runs : next;
}
