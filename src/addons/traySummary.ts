// The update summary of the tray's "Update all" (issue #351): the outcome of a requested
// `install` round for one installation, drawn in the status area like the summary of "Update
// all" in the table. Pure functions, no React or I/O.

import type { AutoUpdateStatus } from "../api";
import { reportProblems } from "./autoUpdateStatus.ts";

/** A project to list under the counts, with the reason. */
export type TraySummaryDetail = {
  name: string;
  message: string;
  /** An error: drawn in the danger color. */
  failed: boolean;
};

export type TraySummary = {
  /**
   * Identifies the round and the installation: "Dismiss" hides this summary and no other, and
   * a newer round has another key.
   */
  key: string;
} & ({ error: string } | { text: string; details: TraySummaryDetail[] });

/**
 * The summary to show for the installation `installationId`, or `null`: the last round was not
 * asked for from the tray, it only checked (`notify`, "Check for updates"), it has no entry for
 * the installation, or it was undone (`undoneRound` is its `finishedAt`) or dismissed
 * (`dismissed` holds its key).
 */
export function traySummary(
  status: AutoUpdateStatus | null,
  installationId: string,
  undoneRound: string | null,
  dismissed: ReadonlySet<string>,
): TraySummary | null {
  const round = status?.lastRound;
  if (!round || !round.requested || round.mode !== "install") return null;
  if (undoneRound === round.finishedAt) return null;
  const report = round.installations.find((r) => r.installationId === installationId);
  if (!report) return null;
  const key = `${installationId}\n${round.finishedAt}`;
  if (dismissed.has(key)) return null;
  if (report.checkError !== null) return { key, error: report.checkError };

  const updated = report.updated.length;
  const skipped = report.skipped.length;
  const errors = report.errors.length;
  const parts = [
    updated > 0 && `${updated} updated`,
    skipped > 0 && `${skipped} skipped`,
    errors > 0 && `${errors} ${errors === 1 ? "error" : "errors"}`,
  ].filter(Boolean);
  // As in the table's summary, an updated project appears only with a message (for example
  // "Installed from GitHub").
  const notes: TraySummaryDetail[] = report.updated
    .filter((outcome) => outcome.message)
    .map((outcome) => ({
      name: outcome.name || outcome.key,
      message: outcome.message ?? "",
      failed: false,
    }));
  const problems: TraySummaryDetail[] = reportProblems(report).map((problem) => ({
    name: problem.name,
    message: problem.message,
    failed: problem.kind === "failed",
  }));
  return {
    key,
    text: parts.length > 0 ? parts.join(", ") : "Nothing was updated",
    details: [...problems, ...notes],
  };
}
