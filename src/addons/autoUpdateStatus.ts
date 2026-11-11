// The automatic update part of the Installed status line (D41, issues #203 and #314): the text
// from the outcome of the last round, the problems to show in the detail, Undo and which
// installations to read again after a round. Pure functions, no React or I/O.

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
  /**
   * "auto-updated 2 addons at 14:05", "automatic check failed at 14:05": a segment of the
   * Installed status line, after its counts. `null` when the round adds only problems: the
   * updates available and the time of the check are already in that line (issue #314).
   */
  text: string | null;
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
  if (report.checkError !== null) return `automatic check failed at ${at}`;
  // What a `notify` round found is the table's own "n to update · checked HH:MM".
  if (mode === "notify") return null;
  const n = report.updated.length;
  return n > 0 ? `auto-updated ${plural(n, "addon", "addons")} at ${at}` : null;
}

/** "auto-updated 3 addons at 14:05": the undoable round, without the outcome in memory. */
function undoText(undo: UndoableRound, clock: Clock): string {
  return `auto-updated ${plural(undo.addons, "addon", "addons")} at ${clock(undo.roundAt)}`;
}

/** The line made only of the undoable round, without problems. */
function undoLine(undo: UndoableRound, clock: Clock): StatusLine {
  return { text: undoText(undo, clock), problemText: null, problems: [], undo, failed: false };
}

/**
 * What the automatic updates add to the status line of the installation `installationId`, or
 * `null` when there is nothing to add: mode not read yet, no round for the installation, or a
 * round with no addon updated, no problem and nothing to undo. With `off` the line exists only for an undoable round, and
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
  if (text === null && problems.length === 0) return null;
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

/**
 * The installations whose update statuses in the Installed tab are stale when the state `next`
 * arrives after `previous`: those for which a new round, `notify` or `install`, found updates,
 * updated addons or hit errors (issue #268). A `notify` round touches no disk, but the table
 * checked once at first access and would keep showing the older result. The same round read
 * again does not count twice, even with a different outcome. After a manual check or update the
 * backend emits the round again with the same `finishedAt` and the outcome reconciled with that
 * command's result (issue #273). The commands that do it (`check_updates`, `refresh_addons`,
 * `update_addons`) are called only by `AddonTable`, which writes their result to the table itself
 * (`runCheck`, `rescan`, and `forgetUpdateChecks` plus `rescan` after an update), so the event
 * describes nothing the table lacks. The Undo event is followed by `onUndone`, which rereads the
 * list. Rereading on a changed outcome would also loop: the reread runs `refresh_addons`, which
 * reconciles and emits again with a new `checkedAt`.
 */
export function staleUpdateChecks(
  previous: AutoUpdateStatus | null,
  next: AutoUpdateStatus,
): string[] {
  const round = next.lastRound;
  if (round === null) return [];
  if (previous?.lastRound?.finishedAt === round.finishedAt) return [];
  return round.installations
    .filter(
      (report) =>
        report.available.length > 0 || report.updated.length > 0 || report.errors.length > 0,
    )
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
