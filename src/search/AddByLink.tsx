import { useEffect, useRef, useState, type FormEvent } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { api, errorMessage, type LinkPreview } from "../api";
import { ProgressBar, useInstallProgress } from "../addons/ProgressBar";
import { progressKey, progressLabel, progressPercent, type InstallProgress } from "../addons/progress";
import { SOURCE_LABELS, projectId } from "../addons/sources";
import { RequestTracker } from "./catalog";
import { ExternalLinkIcon } from "./icons";
import { checkLink, linkOutcome, previewNote, type LinkResult } from "./link";

type Props = {
  /** L'id del riquadro, per `aria-controls` del pulsante che lo apre. */
  id?: string;
  /** Il campo del link è aperto. Chiuso si nasconde solo il campo (con l'errore del link
   *  scritto): l'anteprima, un'installazione in corso e il suo esito restano in vista finché
   *  non si chiudono con ×, così chiudere il campo non nasconde mai un avanzamento o un errore. */
  open: boolean;
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
 * "Add by link" (issue #110): il campo e il pulsante sotto la barra della ricerca, aperti dal
 * pulsante "Add by link" della barra (issue #229).
 * L'invio chiama `resolve_link` e mostra l'anteprima come una riga di tabella, con Install e
 * Open page; un progetto che l'app non può scaricare ha solo Open page e la spiegazione.
 * Lo stato vale per l'installazione: chi lo usa lo rimonta con `key` quando cambia.
 */
export function AddByLink({ id, open, installationId, onInstalled }: Props) {
  const [text, setText] = useState("");
  const [resolving, setResolving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [preview, setPreview] = useState<LinkPreview | null>(null);
  const [run, setRun] = useState<Run | null>(null);
  const [pageError, setPageError] = useState<string | null>(null);
  const tracker = useRef(new RequestTracker());
  const inputRef = useRef<HTMLInputElement>(null);
  // Il focus va sul campo quando il riquadro si apre, non quando compare già aperto.
  const wasOpen = useRef(open);
  useEffect(() => {
    if (open && !wasOpen.current) inputRef.current?.focus();
    wasOpen.current = open;
  }, [open]);
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
        // Un'installazione in corso non si perde: il suo esito resta da mostrare.
        setRun((previous) => (previous?.status === "running" ? previous : null));
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
    // Durante una lettura l'anteprima sta per essere sostituita.
    if (installing || resolving) return;
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
    // Anche una lettura in corso si annulla: la sua risposta non si applicherà più.
    tracker.current.next();
    setResolving(false);
    setPreview(null);
    setRun(null);
    setError(null);
    setPageError(null);
  }

  const note = preview ? previewNote(preview) : null;

  return (
    <div id={id} className="add-by-link">
      {open && (
        <form className="link-bar" onSubmit={submit}>
          <input
            ref={inputRef}
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
            {resolving ? "Looking up…" : "Look up"}
          </button>
        </form>
      )}

      {open && error && (
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
                  {preview.installed && (
                    <span className="link-states">
                      <span className="pill ok">Installed</span>
                    </span>
                  )}
                  {note && <span className="hint">{note}</span>}
                </td>
                <td className="nowrap">{SOURCE_LABELS[preview.source] ?? preview.source}</td>
                <td>{preview.version || "—"}</td>
                <td className="nowrap">
                  <span className="link-actions">
                    <PreviewInstall
                      preview={preview}
                      run={run}
                      disabled={resolving}
                      onInstall={install}
                    />
                    <button className="button ghost small" onClick={() => openPage(preview.url)}>
                      <ExternalLinkIcon />
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
  disabled,
  onInstall,
}: {
  preview: LinkPreview;
  run: Run | null;
  /** Una lettura di un altro link è in corso. */
  disabled: boolean;
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
    <button className="button soft small" disabled={disabled} onClick={() => onInstall(preview)}>
      Install
    </button>
  );
}
