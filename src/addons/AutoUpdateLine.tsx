import { Fragment, useState, type ReactNode } from "react";
import { api, errorMessage, type AutoUpdateMode, type AutoUpdateStatus } from "../api";
import { StatusMessage } from "../StatusArea";
import { ProgressBar } from "./ProgressBar";
import { describeUndo, statusLine, type StatusProblem } from "./autoUpdateStatus";
import { traySummary } from "./traySummary";

/**
 * The tray summaries dismissed in this session, by key (installation and round). Outside the
 * component: switching tab or installation does not bring a dismissed summary back.
 */
const dismissedSummaries = new Set<string>();

type Props = {
  /**
   * The start of the line, from the Installed tab: the installation and its counts, or the
   * selection (issue #314). What the automatic updates add follows it.
   */
  summary: ReactNode;
  status: AutoUpdateStatus | null;
  installationId: string;
  /** The saved mode; `null` until it has been read. */
  mode: AutoUpdateMode | null;
  /** The `finishedAt` of the round undone for the installation, or `null`: its outcome is hidden. */
  undoneRound: string | null;
  /**
   * An Undo changed (or may have changed) the addons on disk. `undoneRound` is the
   * `finishedAt` of the last round when the Undo restored at least one addon, otherwise `null`.
   */
  onUndone: (installationId: string, undoneRound: string | null) => void;
};

type UndoResult = { text: string; problems: StatusProblem[] } | { error: string };

/**
 * The status line under the title of the Installed tab (issue #314): the `summary` the tab
 * passes in, then what the last automatic round did for the installation (D41, issue #203),
 * its problems inline on a click and "Undo" for an undoable round. The status comes from
 * `get_auto_update_status` and the `auto-update` event; this component owns the Undo, whose
 * progress and result are messages of the status area (issue #315): they would move the table
 * if they opened here. So is the summary of a round asked for with the tray's "Update all"
 * (issue #351), which stays until it is dismissed or a newer round replaces it.
 */
export function AutoUpdateLine({
  summary,
  status,
  installationId,
  mode,
  undoneRound,
  onUndone,
}: Props) {
  const [expanded, setExpanded] = useState(false);
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<UndoResult | null>(null);
  // Bumped by "Dismiss" on the tray summary: the dismissed set is outside React.
  const [, setDismissals] = useState(0);
  const line = statusLine(status, installationId, mode, undefined, undoneRound);
  const tray = traySummary(status, installationId, undoneRound, dismissedSummaries);

  async function undo() {
    // The round whose outcome is being undone: the one shown now.
    const round = status?.lastRound?.finishedAt ?? null;
    let restored = false;
    setBusy(true);
    setResult(null);
    try {
      const outcomes = await api.undoLastAutoUpdate(installationId);
      restored = outcomes.some((outcome) => outcome.status === "restored");
      setResult(describeUndo(outcomes));
    } catch (err) {
      setResult({ error: errorMessage(err) });
    } finally {
      setBusy(false);
      // Even a failed Undo may have restored some folder: the list is read again.
      onUndone(installationId, restored ? round : null);
    }
  }

  // The segments after the summary, each one preceded by " · ".
  const segments: ReactNode[] = [];
  if (line?.text) segments.push(line.text);
  if (line?.problemText) {
    segments.push(
      <button
        className={line.failed ? "link inline error" : "link inline subtle"}
        aria-expanded={expanded}
        onClick={() => setExpanded((open) => !open)}
      >
        {line.problemText}
      </button>,
    );
  }
  if (line?.undo && !busy) {
    segments.push(
      <button
        className="link inline"
        title={`Put back the previous version of ${line.undo.addons} ${
          line.undo.addons === 1 ? "addon" : "addons"
        }`}
        onClick={() => void undo()}
      >
        Undo
      </button>,
    );
  }

  return (
    <div className="status-line">
      <p>
        {summary}
        {/* Only this part is announced: the counts before it change at every keystroke in the
            search box. */}
        <span role="status">
          {segments.map((segment, index) => (
            <Fragment key={index}>
              {" · "}
              {segment}
            </Fragment>
          ))}
        </span>
      </p>
      {line && expanded && line.problems.length > 0 && <ProblemList problems={line.problems} />}
      {tray && (
        <StatusMessage
          key={tray.key}
          error={"error" in tray}
          onDismiss={() => {
            dismissedSummaries.add(tray.key);
            setDismissals((n) => n + 1);
          }}
        >
          {"error" in tray ? (
            `Update failed: ${tray.error}`
          ) : (
            <>
              <p>{tray.text}</p>
              {tray.details.length > 0 && (
                <ul>
                  {tray.details.map((detail, index) => (
                    <li key={index} className={detail.failed ? "error" : "muted"}>
                      {detail.name}: {detail.message}
                    </li>
                  ))}
                </ul>
              )}
            </>
          )}
        </StatusMessage>
      )}
      {busy && (
        <StatusMessage progress>
          <ProgressBar value={null} label="Undoing…" name="Undo" />
        </StatusMessage>
      )}
      {result && (
        <StatusMessage error={"error" in result} onDismiss={() => setResult(null)}>
          <p>{"error" in result ? `Undo failed: ${result.error}` : result.text}</p>
          {"problems" in result && result.problems.length > 0 && (
            <ProblemList problems={result.problems} />
          )}
        </StatusMessage>
      )}
    </div>
  );
}

function ProblemList({ problems }: { problems: StatusProblem[] }) {
  return (
    <ul>
      {problems.map((problem, index) => (
        <li key={index} className={problem.kind === "failed" ? "error" : undefined}>
          {problem.name}: {problem.message}
        </li>
      ))}
    </ul>
  );
}
