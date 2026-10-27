// La riga di stato dell'aggiornamento automatico sopra la tabella della tab Installed (D41,
// issue #203): il testo dall'esito dell'ultimo giro, i problemi da mostrare nel dettaglio, l'Undo
// e quali installazioni rileggere dopo un giro. Funzioni pure, senza React né I/O.

import type {
  AutoUpdateMode,
  AutoUpdateStatus,
  InstallationReport,
  UndoableRound,
  UndoOutcome,
} from "../api";

/** Il nome dell'evento Tauri, come `auto_update::EVENT` nel backend. */
export const AUTO_UPDATE_EVENT = "auto-update";

/** Un progetto (o una cartella) da mostrare nel dettaglio, con il motivo. */
export type StatusProblem = {
  name: string;
  message: string;
  kind: "failed" | "skipped";
};

export type StatusLine = {
  /** "3 updates available · checked 14:05", "Auto-updated 2 addons · 14:05". */
  text: string;
  /** "2 failed · 1 skipped", o `null` senza problemi. */
  problemText: string | null;
  problems: StatusProblem[];
  /** Il giro che "Undo" annullerebbe, o `null` se non ce n'è. */
  undo: UndoableRound | null;
  /** C'è almeno un errore: il testo dei problemi prende il colore dell'errore. */
  failed: boolean;
};

/** Formatta un'ora ISO 8601 come la mostra la riga. */
export type Clock = (iso: string) => string;

function pad(n: number): string {
  return String(n).padStart(2, "0");
}

/**
 * L'ora locale di `iso`: "14:05" se è dello stesso giorno di `now`, altrimenti con la data
 * ("Oct 26, 14:05"). Un'ora illeggibile resta com'è.
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
 * I problemi di un'installazione nel giro: gli errori, poi i saltati. Gli esclusi dall'utente
 * stanno in `report.excluded` e non sono problemi.
 */
export function reportProblems(report: InstallationReport): StatusProblem[] {
  const problems: StatusProblem[] = [];
  if (report.checkError !== null) {
    problems.push({ name: "Check", message: report.checkError, kind: "failed" });
  }
  for (const error of report.errors) {
    problems.push({
      // Senza nome è la versione precedente che non si è potuta tenere per l'Undo.
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

/** "2 failed · 1 skipped", o `null` senza problemi. */
export function problemSummary(problems: readonly StatusProblem[]): string | null {
  const failed = problems.filter((p) => p.kind === "failed").length;
  const skipped = problems.length - failed;
  const parts: string[] = [];
  if (failed > 0) parts.push(`${failed} failed`);
  if (skipped > 0) parts.push(`${skipped} skipped`);
  return parts.length > 0 ? parts.join(" · ") : null;
}

/** Il testo principale dall'esito del giro, o `null` se il giro non ha niente da dire. */
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

/**
 * La riga di stato per l'installazione `installationId`, o `null` quando non c'è niente da
 * mostrare: aggiornamento automatico spento (o modalità non ancora letta), nessun giro per
 * l'installazione, oppure un giro che non ha trovato niente e niente da annullare.
 *
 * `undoneRound` è il `finishedAt` di un giro che l'utente ha annullato per l'installazione: il
 * suo esito si ignora.
 *
 * Un giro annullabile (`undoable`) vale anche senza l'esito in memoria, per esempio dopo un
 * riavvio o dopo un giro successivo che non ha trovato niente: la riga dice allora quanti addon
 * ha aggiornato quel giro.
 */
export function statusLine(
  status: AutoUpdateStatus | null,
  installationId: string,
  mode: AutoUpdateMode | null,
  clock: Clock = (iso) => clockTime(iso),
  undoneRound: string | null = null,
): StatusLine | null {
  if (status === null || mode === null || mode === "off") return null;
  const undo = status.undoable.find((round) => round.installationId === installationId) ?? null;
  const round = status.lastRound;
  // Dopo un Undo il backend tiene `lastRound` com'era: l'esito di quel giro non descrive più il
  // disco e non si mostra (`undoneRound` è il suo `finishedAt`).
  const report =
    round && round.finishedAt !== undoneRound
      ? round.installations.find((r) => r.installationId === installationId)
      : undefined;
  const problems = report ? reportProblems(report) : [];
  let text = round && report ? reportText(round.mode, report, clock) : null;
  if (text === null && undo !== null) {
    text = `Auto-updated ${plural(undo.addons, "addon", "addons")} · ${clock(undo.roundAt)}`;
  }
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
 * Le installazioni da rileggere quando arriva lo stato `next` dopo `previous`: quelle con addon
 * aggiornati (o errori, che possono aver toccato il disco) da un giro `install` nuovo. Lo stesso
 * giro riletto (l'evento dopo un Undo, la lettura all'avvio) non conta due volte.
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

/** L'esito di un Undo: il riepilogo e i progetti non rimessi, con il motivo. */
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
