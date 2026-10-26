import { useRef, useState, type FormEvent } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { api, errorMessage, type LinkPreview } from "../api";
import { ProgressBar, useInstallProgress } from "../addons/ProgressBar";
import { progressKey, progressLabel, progressPercent, type InstallProgress } from "../addons/progress";
import { SOURCE_LABELS, projectId } from "../addons/sources";
import { RequestTracker } from "./catalog";
import { checkLink, linkOutcome, previewNote, type LinkResult } from "./link";

type Props = {
  installationId: string;
  /** Un'installazione ha messo il progetto nell'installazione (la tab Installed). */
  onInstalled?: () => void;
};

/** L'installazione dell'anteprima: in corso, finita con l'esito, o rifiutata dal comando. */
type Run =
  | { status: "running"; progress?: InstallProgress }
  | { status: "done"; result: LinkResult }
  | { status: "failed"; message: string };

/**
 * "Add by link" (issue #110): una riga con il campo e il pulsante sotto i filtri della ricerca.
 * L'invio chiama `resolve_link` e mostra l'anteprima come una riga di tabella, con Install e
 * Open page; un progetto che l'app non può scaricare ha solo Open page e la spiegazione.
 * Lo stato vale per l'installazione: chi lo usa lo rimonta con `key` quando cambia.
 */
export function AddByLink({ installationId, onInstalled }: Props) {
  const [text, setText] = useState("");
  const [resolving, setResolving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [preview, setPreview] = useState<LinkPreview | null>(null);
  const [run, setRun] = useState<Run | null>(null);
  const [pageError, setPageError] = useState<string | null>(null);
  const tracker = useRef(new RequestTracker());
  // La chiave degli eventi di avanzamento dell'installazione in corso, letta dal listener.
  const runKey = useRef<string | null>(null);

  useInstallProgress(installationId, (event) => {
    if (runKey.current === null || progressKey(event) !== runKey.current) return;
    setRun((previous) => (previous?.status === "running" ? { ...previous, progress: event } : previous));
  });

  const installing = run?.status === "running";

  function submit(event: FormEvent) {
    event.preventDefault();
    if (installing) return;
    const input = checkLink(text);
    setPageError(null);
    if (!input.ok) {
      setError(input.message);
      return;
    }
    const id = tracker.current.next();
    setResolving(true);
    setError(null);
    api.resolveLink(installationId, input.url).then(
      (result) => {
        if (!tracker.current.isLatest(id)) return;
        setResolving(false);
        setPreview(result);
        setRun(null);
      },
      (err) => {
        if (!tracker.current.isLatest(id)) return;
        setResolving(false);
        setPreview(null);
        setRun(null);
        setError(errorMessage(err));
      },
    );
  }

  function install(target: LinkPreview) {
    if (installing) return;
    runKey.current = projectId({ source: target.source, key: target.id });
    setRun({ status: "running" });
    setPageError(null);
    api
      .installFromLink(installationId, target.source, target.id)
      .then(
        (outcome) => {
          const result = linkOutcome(outcome, target);
          if (result.installed) {
            setPreview((previous) => (previous === target ? { ...previous, installed: true } : previous));
            onInstalled?.();
          }
          setRun({ status: "done", result });
        },
        (err) => setRun({ status: "failed", message: errorMessage(err) }),
      )
      .finally(() => {
        runKey.current = null;
      });
  }

  function openPage(url: string) {
    setPageError(null);
    openUrl(url).catch((err) => setPageError(`The page could not be opened: ${errorMessage(err)}`));
  }

  function dismiss() {
    tracker.current.next();
    setPreview(null);
    setRun(null);
    setError(null);
    setPageError(null);
  }

  const note = preview ? previewNote(preview) : null;

  return (
    <div className="add-by-link">
      <form className="link-bar" onSubmit={submit}>
        <input
          className="search link-input"
          type="text"
          inputMode="url"
          spellCheck={false}
          autoComplete="off"
          placeholder="Paste a GitHub, WoWInterface or Tukui link…"
          aria-label="Add by link"
          value={text}
          onChange={(event) => setText(event.target.value)}
        />
        <button className="button secondary" type="submit" disabled={resolving || installing}>
          {resolving ? "Looking up…" : "Add by link"}
        </button>
      </form>

      {error && (
        <p className="message error" role="alert">
          {error}
        </p>
      )}

      {preview && (
        <div className="table-wrap link-preview">
          <table className="addon-table">
            <thead>
              <tr>
                <th>Name</th>
                <th>Source</th>
                <th>Version</th>
                <th aria-label="Actions" />
              </tr>
            </thead>
            <tbody>
              <tr>
                <td>
                  <span className="addon-title">{preview.name || preview.id}</span>
                  {preview.installed && <span className="tag">Installed</span>}
                  {note && <span className="hint">{note}</span>}
                </td>
                <td className="nowrap">{SOURCE_LABELS[preview.source] ?? preview.source}</td>
                <td>{preview.version || "—"}</td>
                <td className="nowrap">
                  <span className="link-actions">
                    <PreviewInstall preview={preview} run={run} onInstall={install} />
                    <button className="button secondary small" onClick={() => openPage(preview.url)}>
                      Open page
                    </button>
                    {!installing && (
                      <button
                        className="link dismiss"
                        aria-label="Dismiss the preview"
                        title="Dismiss"
                        onClick={dismiss}
                      >
                        ×
                      </button>
                    )}
                  </span>
                </td>
              </tr>
            </tbody>
          </table>
        </div>
      )}

      {pageError && (
        <p className="message error" role="alert">
          {pageError}
        </p>
      )}
      {run?.status === "done" && (
        <p className={run.result.error ? "message error" : "message"} role="status">
          {run.result.text}
        </p>
      )}
      {run?.status === "failed" && (
        <p className="message error" role="alert">
          Install failed: {run.message}
        </p>
      )}
    </div>
  );
}

/** Install, la barra durante l'installazione, o niente per un progetto installato o non scaricabile. */
function PreviewInstall({
  preview,
  run,
  onInstall,
}: {
  preview: LinkPreview;
  run: Run | null;
  onInstall: (preview: LinkPreview) => void;
}) {
  if (run?.status === "running") {
    return (
      <ProgressBar
        value={progressPercent(run.progress)}
        label={progressLabel(run.progress, "Starting…")}
        name={preview.name}
      />
    );
  }
  if (preview.installed || !preview.downloadable) return null;
  return (
    <button className="button small" onClick={() => onInstall(preview)}>
      Install
    </button>
  );
}
