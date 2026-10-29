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
  /** The id of the panel, for `aria-controls` of the button that opens it. */
  id?: string;
  /** The link field is open. Closed, only the field is hidden (with the error of the typed
   *  link): the preview, an install in progress and its outcome stay in view until they
   *  are closed with ×, so closing the field never hides a progress or an error. */
  open: boolean;
  installationId: string;
  /** An install put the project into the installation (the Installed tab). */
  onInstalled?: () => void;
};

/** The preview's install: in progress, finished with the outcome, or rejected by the command. */
type Run =
  | { status: "running"; progress?: InstallProgress }
  | { status: "done"; result: LinkResult }
  | { status: "failed"; message: string };

/**
 * "Add by link" (issue #110): the field and the button under the search bar, opened by the
 * bar's "Add by link" button (issue #229).
 * Submitting calls `resolve_link` and shows the preview as a table row, with Install and
 * Open page; a project the app cannot download has only Open page and the explanation.
 * The state is per installation: whoever uses it remounts it with `key` when it changes.
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
  // Focus goes to the field when the panel opens, not when it appears already open.
  const wasOpen = useRef(open);
  useEffect(() => {
    if (open && !wasOpen.current) inputRef.current?.focus();
    wasOpen.current = open;
  }, [open]);
  // The key of the progress events of the install in progress, read by the listener.
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
        // An install in progress is not lost: its outcome is still to be shown.
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
    // During a read the preview is about to be replaced.
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
    // A read in progress is cancelled too: its answer will no longer apply.
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

/** Install, the bar during the install, or nothing for an installed or non-downloadable project. */
function PreviewInstall({
  preview,
  run,
  disabled,
  onInstall,
}: {
  preview: LinkPreview;
  run: Run | null;
  /** A read of another link is in progress. */
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
