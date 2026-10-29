// The automatic update status line above the table of the Installed tab (D41,
// issue #203): the text from the outcome of the last round, the problems to show in the detail,
// Undo and which installations to read again after a round. Pure functions, no React or I/O.

import type {
  AutoUpdateMode,
  AutoUpdateStatus,
  InstallationReport,
  UndoableRound,
  UndoOutcome,
} from "../api";

/** The name of the Tauri event, like `auto_update::EVENT` in the backend. */
export const AUTO_UPDATE_EVENT = "auto-update";

/** A project (or a folder) to show in the detail, with the reason. */
export type StatusProblem = {
  name: string;
  message: string;
  kind: "failed" | "skipped";
};

export type StatusLine = {
  /** "3 updates available · checked 14:05", "Auto-updated 2 addons · 14:05". */
  text: string;
  /** "2 failed · 1 skipped", or `null` without problems. */
  problemText: string | null;
  problems: StatusProblem[];
  /** The round "Undo" would undo, or `null` if there is none. */
  undo: UndoableRound | null;
  /** There is at least one error: the problems text takes the error color. */
  failed: boolean;
};

/** Formats an ISO 8601 time the way the line shows it. */
export type Clock = (iso: string) => string;

function pad(n: number): string {
  return String(n).padStart(2, "0");
}

/**
 * The local time of `iso`: "14:05" if it is on the same day as `now`, otherwise with the date
 * ("Oct 26, 14:05"). An unreadable time stays as it is.
 */
export function clockTime(iso: string, now: Date = new Date()): string {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return iso;
  const time = `${pad(date.getHours())}:${pad(date.getMinutes())}`;
  const sameDay =
    date.getFullYear() === now.getFullYear() &&
    date.getMonth() === now.getMonth() &&
    date.getDate() === now.getDate();
  if (sameDay) return time;
  const day = date.toLocaleDateString("en-US", { month: "short", day: "numeric" });
  return `${day}, ${time}`;
}

function plural(n: number, one: string, many: string): string {
  return `${n} ${n === 1 ? one : many}`;
}

/**
 * An installation's problems in the round: the errors, then the skipped. Those excluded by the
 * user are in `report.excluded` and are not problems.
 */
export function reportProblems(report: InstallationReport): StatusProblem[] {
  const problems: StatusProblem[] = [];
  if (report.checkError !== null) {
    problems.push({ name: "Check", message: report.checkError, kind: "failed" });
  }
  for (const error of report.errors) {
    problems.push({
      // Without a name it is the previous version that could not be kept for the Undo.
      name: error.name === "" ? "Undo" : error.name,
      message: error.message,
      kind: "failed",
    });
  }
  for (const skipped of report.skipped) {
    problems.push({
      name: skipped.name || skipped.key,
      message: skipped.message ?? "Skipped.",
      kind: "skipped",
    });
  }
  return problems;
}

/** "2 failed · 1 skipped", or `null` without problems. */
export function problemSummary(problems: readonly StatusProblem[]): string | null {
  const failed = problems.filter((p) => p.kind === "failed").length;
  const skipped = problems.length - failed;
  const parts: string[] = [];
  if (failed > 0) parts.push(`${failed} failed`);
  if (skipped > 0) parts.push(`${skipped} skipped`);
  return parts.length > 0 ? parts.join(" · ") : null;
}

/** The main text from the outcome of the round, or `null` if the round has nothing to say. */
function reportText(
  mode: "notify" | "install",
  report: InstallationReport,
  clock: Clock,
): string | null {
  const at = clock(report.checkedAt);
  if (report.checkError !== null) return `Automatic check failed · ${at}`;
  if (mode === "notify") {
    const n = report.available.length;
    return n > 0 ? `${plural(n, "update", "updates")} available · checked ${at}` : null;
  }
  const n = report.updated.length;
  return n > 0 ? `Auto-updated ${plural(n, "addon", "addons")} · ${at}` : null;
}

/** "Auto-updated 3 addons · 14:05": the undoable round, without the outcome in memory. */
function undoText(undo: UndoableRound, clock: Clock): string {
  return `Auto-updated ${plural(undo.addons, "addon", "addons")} · ${clock(undo.roundAt)}`;
}

/** The line made only of the undoable round, without problems. */
function undoLine(undo: UndoableRound, clock: Clock): StatusLine {
  return { text: undoText(undo, clock), problemText: null, problems: [], undo, failed: false };
}

/**
 * The status line for the installation `installationId`, or `null` when there is nothing to
 * show: mode not read yet, no round for the installation, or a round that found nothing
 * and has nothing to undo. With `off` the line exists only for an undoable round, and
 * says only that.
 *
 * `undoneRound` is the `finishedAt` of a round the user undid for the installation: its
 * outcome is ignored.
 *
 * An undoable round (`undoable`) holds even without the outcome in memory, for example after a
 * restart or after a later round that found nothing: the line then says how many addons
 * that round updated.
 */
export function statusLine(
  status: AutoUpdateStatus | null,
  installationId: string,
  mode: AutoUpdateMode | null,
  clock: Clock = (iso) => clockTime(iso),
  undoneRound: string | null = null,
): StatusLine | null {
  if (status === null || mode === null) return null;
  const undo = status.undoable.find((round) => round.installationId === installationId) ?? null;
  // Off no longer checks anything, but an earlier round stays undoable (issue #215): the line
  // says only that, without available updates or problems of the last round.
  if (mode === "off") return undo === null ? null : undoLine(undo, clock);
  const round = status.lastRound;
  // After an Undo the backend keeps `lastRound` as it was: that round's outcome no longer
  // describes the disk and is not shown (`undoneRound` is its `finishedAt`).
  const report =
    round && round.finishedAt !== undoneRound
      ? round.installations.find((r) => r.installationId === installationId)
      : undefined;
  const problems = report ? reportProblems(report) : [];
  let text = round && report ? reportText(round.mode, report, clock) : null;
  if (text === null && undo !== null) text = undoText(undo, clock);
  if (text === null && problems.length > 0 && round && report) {
    text = `Checked ${clock(report.checkedAt)}`;
  }
  if (text === null) return null;
  return {
    text,
    problemText: problemSummary(problems),
    problems,
    undo,
    failed: problems.some((p) => p.kind === "failed"),
  };
}

/**
 * The installations to read again when the state `next` arrives after `previous`: those with
 * addons updated (or errors, which may have touched the disk) by a new `install` round. The same
 * round read again (the event after an Undo, the read at startup) does not count twice.
 */
export function changedInstallations(
  previous: AutoUpdateStatus | null,
  next: AutoUpdateStatus,
): string[] {
  const round = next.lastRound;
  if (round === null || round.mode !== "install") return [];
  if (previous?.lastRound?.finishedAt === round.finishedAt) return [];
  return round.installations
    .filter((report) => report.updated.length > 0 || report.errors.length > 0)
    .map((report) => report.installationId);
}

/** The outcome of an Undo: the summary and the projects not restored, with the reason. */
export function describeUndo(outcomes: readonly UndoOutcome[]): {
  text: string;
  problems: StatusProblem[];
} {
  const restored = outcomes.filter((o) => o.status === "restored").length;
  const problems: StatusProblem[] = outcomes
    .filter((o) => o.status !== "restored")
    .map((o) => ({
      name: o.name || o.key,
      message: o.message ?? (o.status === "skipped" ? "Skipped." : "Failed."),
      kind: o.status === "error" ? "failed" : "skipped",
    }));
  const parts = [`Restored ${plural(restored, "addon", "addons")}`];
  const summary = problemSummary(problems);
  if (summary) parts.push(summary);
  return { text: `${parts.join(" · ")}.`, problems };
}
