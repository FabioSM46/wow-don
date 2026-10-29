// Installing from the Search tab: the summary of the `install_addon` outcomes and the
// "installed" state that the rows and the detail show after an install. Pure functions,
// no React or I/O.

import type { CatalogAddon, InstallOutcome } from "../api";

/** A summary line: `error` shows as an error, the rest in a neutral tone. */
export type SummaryLine = { modId: number; text: string; error: boolean };

/** The summary of an install: a sentence on the chosen project and one line per dependency. */
export type InstallSummary = { headline: string; error: boolean; lines: SummaryLine[] };

/** The name of an outcome: the backend may leave it empty, and then the id remains. */
function outcomeName(outcome: InstallOutcome, fallback?: string): string {
  return outcome.name.trim() || fallback?.trim() || `Project #${outcome.modId}`;
}

/**
 * The summary of the `install_addon` outcomes for the project `requested` (id and name the
 * search gives it, for when the outcome has none): the backend puts its outcome first and then
 * those of the dependencies, breadth-first and with `dependencyOf`. The project's outcome is
 * looked up by id, so the sentence never describes a dependency.
 */
export function summarizeInstall(
  outcomes: InstallOutcome[],
  requested: { id: number; name?: string },
): InstallSummary {
  const main =
    outcomes.find((o) => o.modId === requested.id && o.dependencyOf === undefined) ??
    outcomes.find((o) => o.dependencyOf === undefined);
  const dependencies = outcomes.filter((o) => o !== main);

  const names = new Map<number, string>();
  for (const outcome of dependencies) names.set(outcome.modId, outcomeName(outcome));
  if (main) {
    const fallbackName = main.modId === requested.id ? requested.name : undefined;
    names.set(main.modId, outcomeName(main, fallbackName));
  }
  const nameOf = (modId: number) => names.get(modId) ?? `Project #${modId}`;

  const lines = dependencies.map((outcome): SummaryLine => {
    const parent = outcome.dependencyOf;
    const name = nameOf(outcome.modId);
    const label = parent === undefined ? name : `${name} (required by ${nameOf(parent)})`;
    const state =
      outcome.status === "installed"
        ? "installed"
        : outcome.status === "skipped"
          ? "skipped"
          : "error";
    return {
      modId: outcome.modId,
      text: `${label}: ${state}${outcome.message ? ` — ${outcome.message}` : ""}`,
      error: outcome.status === "error",
    };
  });

  // Without the project's outcome (an answer the backend does not give) the lines remain.
  if (!main) return { headline: "Nothing was installed.", error: false, lines };

  const mainName = nameOf(main.modId);
  const reason = main.message ? `: ${main.message}` : ".";
  let headline: string;
  switch (main.status) {
    case "installed": {
      const installed = dependencies.filter((o) => o.status === "installed").length;
      const withDeps =
        installed > 0 ? ` with ${installed} ${installed === 1 ? "dependency" : "dependencies"}` : "";
      // On a successful install the message is a warning: it follows the sentence.
      headline = `${mainName} installed${withDeps}.${main.message ? ` ${main.message}` : ""}`;
      break;
    }
    case "skipped":
      headline = `${mainName} was not installed${reason}`;
      break;
    default:
      headline = `${mainName} could not be installed${reason}`;
  }

  return { headline, error: main.status === "error", lines };
}

/** The ids of the projects the install put into the installation. */
export function installedIds(outcomes: InstallOutcome[]): number[] {
  return outcomes.filter((o) => o.status === "installed").map((o) => o.modId);
}

/**
 * The addon with an updated "installed" mark: the search computes it on every answer, but a
 * project installed after the answer is in `installed`. The same object if nothing changes.
 */
export function withInstalled<T extends Pick<CatalogAddon, "id" | "installed">>(
  addon: T,
  installed: ReadonlySet<number>,
): T {
  return !addon.installed && installed.has(addon.id) ? { ...addon, installed: true } : addon;
}

/** `releaseType` of CurseForge Beta files. */
const RELEASE_TYPE_BETA = 2;

/**
 * The text of the install button: "Install beta" when the file the app will install is
 * a Beta (only with the option, when the version has no Release; issue #141).
 */
export function installLabel(addon: Pick<CatalogAddon, "latestFile">): string {
  return addon.latestFile?.releaseType === RELEASE_TYPE_BETA ? "Install beta" : "Install";
}
