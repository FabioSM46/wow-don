import { useState } from "react";
import { api, errorMessage, type AutoUpdateMode, type AutoUpdateStatus } from "../api";
import { ProgressBar } from "./ProgressBar";
import { describeUndo, statusLine, type StatusProblem } from "./autoUpdateStatus";

type Props = {
  status: AutoUpdateStatus | null;
  installationId: string;
  /** La modalità salvata; `null` finché non è letta. */
  mode: AutoUpdateMode | null;
  /** Un Undo ha cambiato (o può aver cambiato) gli addon sul disco. */
  onUndone: (installationId: string) => void;
};

type UndoResult = { text: string; problems: StatusProblem[] } | { error: string };

/**
 * La riga dell'aggiornamento automatico sopra la tabella della tab Installed (D41, issue #203):
 * cosa ha fatto l'ultimo giro per l'installazione, i problemi in linea con un clic e "Undo" per
 * un giro annullabile. Lo stato arriva da `get_auto_update_status` e dall'evento `auto-update`.
 */
export function AutoUpdateLine({ status, installationId, mode, onUndone }: Props) {
  const [expanded, setExpanded] = useState(false);
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<UndoResult | null>(null);
  const line = statusLine(status, installationId, mode);

  async function undo() {
    setBusy(true);
    setResult(null);
    try {
      const outcomes = await api.undoLastAutoUpdate(installationId);
      setResult(describeUndo(outcomes));
    } catch (err) {
      setResult({ error: errorMessage(err) });
    } finally {
      setBusy(false);
      // Anche un Undo fallito può aver rimesso qualche cartella: la lista si rilegge.
      onUndone(installationId);
    }
  }

  if (line === null && result === null && !busy) return null;

  return (
    <div className="auto-update-line" role="status">
      {line && (
        <p>
          <span>{line.text}</span>
          {line.problemText && (
            <>
              {" · "}
              <button
                className={line.failed ? "link inline error" : "link inline subtle"}
                aria-expanded={expanded}
                onClick={() => setExpanded((open) => !open)}
              >
                {line.problemText}
              </button>
            </>
          )}
          {line.undo && !busy && (
            <>
              {" · "}
              <button
                className="link inline"
                title={`Put back the previous version of ${line.undo.addons} ${
                  line.undo.addons === 1 ? "addon" : "addons"
                }`}
                onClick={() => void undo()}
              >
                Undo
              </button>
            </>
          )}
        </p>
      )}
      {line && expanded && line.problems.length > 0 && <ProblemList problems={line.problems} />}
      {busy && <ProgressBar value={null} label="Undoing…" name="Undo" />}
      {result && (
        <div className="auto-update-undo">
          <p className={"error" in result ? "error" : undefined}>
            {"error" in result ? `Undo failed: ${result.error}` : result.text}{" "}
            <button className="link inline subtle" onClick={() => setResult(null)}>
              Dismiss
            </button>
          </p>
          {"problems" in result && result.problems.length > 0 && (
            <ProblemList problems={result.problems} />
          )}
        </div>
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
