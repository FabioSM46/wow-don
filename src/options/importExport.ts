// Import/export degli addon in Options (issue #135): i conteggi e i testi di export, anteprima
// e risultato. Funzioni pure, senza React né I/O.

import type { ExportResult, ImportPreview, ImportResult, PreviewEntry } from "../api";
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

/**
 * "Importing 3 of 12": la posizione è il numero di progetti nuovi di cui è già arrivato un
 * evento di avanzamento (il backend li installa uno alla volta), almeno 1 appena parte.
 */
export function importProgressText(seen: ReadonlySet<string>, fresh: readonly string[]): string {
  if (fresh.length === 0) return "Importing…";
  const started = fresh.filter((id) => seen.has(id)).length;
  return `Importing ${Math.min(fresh.length, Math.max(1, started))} of ${fresh.length}`;
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
