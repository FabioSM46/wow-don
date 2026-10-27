// Import/export degli addon in Options (issue #135): i conteggi e i testi di export, anteprima
// e risultato. Funzioni pure, senza React né I/O.

import type { ExportResult, ImportPreview, ImportResult, PreviewEntry } from "../api";
import { PHASE_ORDER, type ProgressPhase } from "../addons/progress.ts";
import { projectId } from "../addons/sources.ts";

function plural(count: number, one: string, many: string): string {
  return `${count} ${count === 1 ? one : many}`;
}

/** "12 addons exported · 2 folders not exportable". */
export function exportSummary(result: ExportResult): string {
  const parts = [`${plural(result.exported.length, "addon", "addons")} exported`];
  if (result.skipped.length > 0) {
    parts.push(`${plural(result.skipped.length, "folder", "folders")} not exportable`);
  }
  return parts.join(" · ");
}

export type PreviewCounts = {
  fresh: number;
  installed: number;
  notDownloadable: number;
  invalid: number;
};

export function previewCounts(preview: ImportPreview): PreviewCounts {
  const count = (state: PreviewEntry["state"]) =>
    preview.entries.filter((entry) => entry.state === state).length;
  return {
    fresh: count("new"),
    installed: count("installed"),
    notDownloadable: count("notDownloadable"),
    invalid: preview.invalid.length,
  };
}

/** "3 new · 8 already installed · 1 not downloadable · 1 invalid entry", senza i conteggi a zero. */
export function previewSummary(preview: ImportPreview): string {
  const counts = previewCounts(preview);
  const parts: string[] = [];
  if (counts.fresh > 0) parts.push(`${counts.fresh} new`);
  if (counts.installed > 0) parts.push(`${counts.installed} already installed`);
  if (counts.notDownloadable > 0) parts.push(`${counts.notDownloadable} not downloadable`);
  if (counts.invalid > 0) parts.push(plural(counts.invalid, "invalid entry", "invalid entries"));
  return parts.length > 0 ? parts.join(" · ") : "This list has no addons.";
}

/** Gli id (`projectId`) dei progetti che l'import installerà. */
export function newProjects(preview: ImportPreview): string[] {
  return preview.entries.filter((entry) => entry.state === "new").map(projectId);
}

/** La fase più avanzata tra quella già vista per un progetto e quella di un nuovo evento. */
export function furthestPhase(seen: ProgressPhase | undefined, phase: ProgressPhase): ProgressPhase {
  return seen !== undefined && PHASE_ORDER.indexOf(seen) > PHASE_ORDER.indexOf(phase) ? seen : phase;
}

/**
 * "Downloading 7 of 12", poi "Installing 3 of 12" (issue #180): il backend scarica più progetti
 * nuovi insieme, poi li installa uno alla volta. `reached` è la fase più avanzata vista per ogni
 * progetto, `current` quella dell'ultimo evento: dice se l'import sta scaricando o installando.
 * "Downloading" conta gli zip già scaricati; "Installing" la posizione di quello in corso,
 * almeno 1.
 */
export function importProgressText(
  reached: ReadonlyMap<string, ProgressPhase>,
  fresh: readonly string[],
  current?: ProgressPhase,
): string {
  if (fresh.length === 0) return "Importing…";
  const atLeast = (phase: ProgressPhase) =>
    fresh.filter((id) => {
      const seen = reached.get(id);
      return seen !== undefined && PHASE_ORDER.indexOf(seen) >= PHASE_ORDER.indexOf(phase);
    }).length;
  if (current === "extracting" || current === "installing") {
    const position = Math.min(fresh.length, Math.max(1, atLeast("extracting")));
    return `Installing ${position} of ${fresh.length}`;
  }
  return `Downloading ${atLeast("downloaded")} of ${fresh.length}`;
}

export type ImportSummary = {
  /** "Installed 3 · 8 already installed · 1 skipped · 1 failed · removed 4 folders". */
  headline: string;
  /** Una riga per ogni progetto non installato e per ogni problema di "Replace all". */
  problems: string[];
  /** Vero se qualcosa è andato storto e il riepilogo va mostrato come errore. */
  failed: boolean;
};

export function importSummary(result: ImportResult): ImportSummary {
  const count = (status: ImportResult["entries"][number]["status"]) =>
    result.entries.filter((entry) => entry.status === status).length;
  const installed = count("installed");
  const already = count("alreadyInstalled");
  const skipped = count("skipped");
  const errors = count("error");

  const parts = [`Installed ${installed}`];
  if (already > 0) parts.push(`${already} already installed`);
  if (skipped > 0) parts.push(`${skipped} skipped`);
  if (errors > 0) parts.push(`${errors} failed`);
  if (result.removed.length > 0) {
    parts.push(`removed ${plural(result.removed.length, "folder", "folders")}`);
  }

  const problems: string[] = [];
  for (const entry of result.entries) {
    if (entry.status === "skipped" || entry.status === "error") {
      const name = entry.name || entry.key;
      problems.push(entry.message ? `${name}: ${entry.message}` : name);
    }
  }
  for (const error of result.removalErrors) problems.push(error.message);
  if (result.removalSkipped) problems.push(result.removalSkipped);
  if (result.notConfirmed.length > 0) {
    problems.push(
      `Not removed because they were not in the confirmed list: ${result.notConfirmed.join(", ")}.`,
    );
  }
  if (result.invalid.length > 0) {
    problems.push(`${plural(result.invalid.length, "invalid entry was", "invalid entries were")} ignored.`);
  }
  if (result.lookupError) problems.push(result.lookupError);

  return {
    headline: parts.join(" · "),
    problems,
    failed: errors > 0 || result.removalErrors.length > 0,
  };
}
