import { useEffect, useRef, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  api,
  errorMessage,
  type ExportResult,
  type ImportMode,
  type ImportPreview,
  type ImportResult,
  type Installation,
} from "../api";
import { ProgressBar, useInstallProgress } from "../addons/ProgressBar";
import {
  progressKey,
  progressLabel,
  progressPercent,
  type InstallProgress,
  type ProgressPhase,
} from "../addons/progress";
import {
  exportSummary,
  furthestPhase,
  importProgressText,
  importSummary,
  newProjects,
  previewSummary,
} from "./addonList";

type Props = {
  /** L'installazione attiva; `null` se non ce n'è nessuna. */
  installation: Installation | null;
  /** Gli addon sul disco sono cambiati: la tab Installed si rilegge. */
  /** L'importazione nell'installazione `installationId` è finita, anche con un errore. */
  onImported: (installationId: string) => void;
};

type Load<T> =
  | { state: "idle" }
  | { state: "loading" }
  | { state: "ready"; value: T }
  | { state: "error"; message: string };

/**
 * La sezione "Import / export" di Options (issue #135): copia la stringa degli addon
 * dell'installazione attiva e ne importa una, con un'anteprima prima di cambiare qualcosa.
 * "Replace all" toglie cartelle senza backup (scelta dell'utente): la conferma elenca
 * esattamente quelle, e solo quelle arrivano al backend.
 */
export function ImportExport({ installation, onImported }: Props) {
  const id = installation?.id ?? null;
  const [exported, setExported] = useState<Load<ExportResult>>({ state: "idle" });
  const [copied, setCopied] = useState<string | null>(null);
  const exportRef = useRef<HTMLTextAreaElement>(null);

  const [text, setText] = useState("");
  const [preview, setPreview] = useState<Load<ImportPreview>>({ state: "idle" });
  const [mode, setMode] = useState<ImportMode>("keep");
  const [confirming, setConfirming] = useState(false);
  const [importing, setImporting] = useState(false);
  const [result, setResult] = useState<Load<ImportResult>>({ state: "idle" });
  const [progress, setProgress] = useState<InstallProgress | undefined>();
  const [reached, setReached] = useState<ReadonlyMap<string, ProgressPhase>>(new Map());
  const cancelRef = useRef<HTMLButtonElement>(null);
  const importRef = useRef<HTMLButtonElement>(null);
  // Una risposta vale solo per l'installazione e il testo da cui è partita la richiesta.
  const idRef = useRef(id);
  const textRef = useRef(text);
  useEffect(() => {
    idRef.current = id;
    textRef.current = text;
  });

  useInstallProgress(id, (event) => {
    if (!importing) return;
    setProgress(event);
    const key = progressKey(event);
    setReached((previous) => {
      const phase = furthestPhase(previous.get(key), event.phase);
      return previous.get(key) === phase ? previous : new Map(previous).set(key, phase);
    });
  });

  // Un'altra installazione: export e anteprima non valgono più.
  useEffect(() => {
    setExported({ state: "idle" });
    setPreview({ state: "idle" });
    setResult({ state: "idle" });
    setConfirming(false);
    setCopied(null);
  }, [id]);

  // La conferma prende il focus quando compare, così tastiera e lettori di schermo ci arrivano.
  useEffect(() => {
    if (confirming) cancelRef.current?.focus();
  }, [confirming]);

  // Il messaggio di "Copy" sparisce da solo.
  useEffect(() => {
    if (copied === null) return;
    const timer = window.setTimeout(() => setCopied(null), 2500);
    return () => window.clearTimeout(timer);
  }, [copied]);

  if (installation === null) {
    return (
      <section className="import-export" aria-labelledby="import-export-title">
        <header className="section-header">
          <h2 id="import-export-title">Import / export</h2>
        </header>
        <p className="empty">Add an installation above to import or export its addons.</p>
      </section>
    );
  }
  const current = installation;

  async function runExport() {
    const requested = current.id;
    setExported({ state: "loading" });
    setCopied(null);
    try {
      const value = await api.exportAddons(requested);
      if (idRef.current === requested) setExported({ state: "ready", value });
    } catch (err) {
      if (idRef.current === requested) setExported({ state: "error", message: errorMessage(err) });
    }
  }

  async function copy(value: string) {
    try {
      await navigator.clipboard.writeText(value);
      setCopied("Copied");
    } catch {
      // Senza accesso agli appunti si seleziona il testo: basta la scorciatoia di sistema.
      exportRef.current?.select();
      setCopied("Press Ctrl+C (⌘C on macOS) to copy");
    }
  }

  function changeText(value: string) {
    setText(value);
    // L'anteprima era della stringa di prima.
    setPreview({ state: "idle" });
    setConfirming(false);
  }

  async function runPreview() {
    const requested = { id: current.id, text };
    const stillCurrent = () =>
      idRef.current === requested.id && textRef.current === requested.text;
    setPreview({ state: "loading" });
    setResult({ state: "idle" });
    setConfirming(false);
    try {
      const value = await api.previewImport(requested.id, requested.text);
      // Un'anteprima di un altro testo o di un'altra installazione mostrerebbe cartelle da
      // togliere che non c'entrano.
      if (stillCurrent()) setPreview({ state: "ready", value });
    } catch (err) {
      if (stillCurrent()) setPreview({ state: "error", message: errorMessage(err) });
    }
  }

  async function runImport(confirmed: string[]) {
    const requested = current.id;
    setConfirming(false);
    setImporting(true);
    setProgress(undefined);
    setReached(new Map());
    setResult({ state: "loading" });
    try {
      const value = await api.importAddons(requested, text, mode, confirmed);
      if (idRef.current === requested) {
        setResult({ state: "ready", value });
        // L'anteprima descriveva lo stato di prima.
        setPreview({ state: "idle" });
      }
    } catch (err) {
      if (idRef.current === requested) setResult({ state: "error", message: errorMessage(err) });
    } finally {
      setImporting(false);
      setProgress(undefined);
      // Gli addon sul disco possono essere cambiati anche se il comando ha fallito.
      onImported(requested);
    }
  }

  function cancelConfirm() {
    setConfirming(false);
    // Il pulsante Import torna al posto della conferma: il focus ci torna con lui.
    window.setTimeout(() => importRef.current?.focus(), 0);
  }

  function startImport(shown: ImportPreview) {
    if (mode === "replace" && shown.removals.length > 0) {
      setConfirming(true);
      return;
    }
    void runImport([]);
  }

  const shownPreview = preview.state === "ready" ? preview.value : null;
  const fresh = shownPreview ? newProjects(shownPreview) : [];
  const notDownloadable = shownPreview?.entries.filter((e) => e.state === "notDownloadable") ?? [];
  const summary = result.state === "ready" ? importSummary(result.value) : null;

  return (
    <section className="import-export" aria-labelledby="import-export-title">
      <header className="section-header">
        <h2 id="import-export-title">Import / export · {current.name}</h2>
      </header>

      <div className="import-export-block">
        <div className="import-export-actions">
          <button
            className="button secondary"
            onClick={() => void runExport()}
            disabled={exported.state === "loading" || importing}
          >
            {exported.state === "loading" ? "Exporting…" : "Export addons"}
          </button>
          {exported.state === "ready" && (
            <>
              <button className="button secondary" onClick={() => void copy(exported.value.text)}>
                Copy
              </button>
              {copied && (
                <span className="muted" role="status">
                  {copied}
                </span>
              )}
            </>
          )}
        </div>
        {exported.state === "error" && (
          <p className="message error" role="alert">
            {exported.message}
          </p>
        )}
        {exported.state === "ready" && (
          <>
            <textarea
              ref={exportRef}
              className="import-text"
              aria-label="Exported addon list"
              readOnly
              rows={3}
              value={exported.value.text}
              onFocus={(event) => event.currentTarget.select()}
            />
            <p className="muted">{exportSummary(exported.value)}</p>
            {exported.value.checkError && (
              <p className="message error" role="alert">
                The update check failed, so only addons installed by the app were recognized:{" "}
                {exported.value.checkError}
              </p>
            )}
            {exported.value.skipped.length > 0 && (
              <details className="import-details">
                <summary>Folders not exported</summary>
                <p className="hint">
                  No source is known for these folders (for example addons installed from a zip):
                  they cannot be downloaded again.
                </p>
                <ul>
                  {exported.value.skipped.map((folder) => (
                    <li key={folder}>{folder}</li>
                  ))}
                </ul>
              </details>
            )}
          </>
        )}
      </div>

      <div className="import-export-block">
        <textarea
          className="import-text"
          aria-label="Addon list to import"
          placeholder="Paste an addon list (WOWDON1:…)"
          rows={3}
          value={text}
          disabled={importing}
          onChange={(event) => changeText(event.target.value)}
        />
        <div className="import-export-actions">
          <button
            className="button secondary"
            onClick={() => void runPreview()}
            disabled={text.trim() === "" || preview.state === "loading" || importing}
          >
            {preview.state === "loading" ? "Reading…" : "Preview"}
          </button>
        </div>
        {preview.state === "error" && (
          <p className="message error" role="alert">
            {preview.message}
          </p>
        )}

        {shownPreview && (
          <div className="import-preview">
            <p>{previewSummary(shownPreview)}</p>
            {shownPreview.flavor !== "" && shownPreview.flavor !== current.flavor && (
              <p className="hint">
                This list was exported from another game version: each addon installs the file
                for {current.name}.
              </p>
            )}
            {(shownPreview.checkError || shownPreview.lookupError) && (
              <p className="message error" role="alert">
                {[shownPreview.checkError, shownPreview.lookupError].filter(Boolean).join(" ")}
              </p>
            )}
            {notDownloadable.length > 0 && (
              <ul className="import-list">
                {notDownloadable.map((entry) => (
                  <li key={`${entry.source}:${entry.key}`}>
                    {entry.url ? (
                      <button className="link inline" onClick={() => void openUrl(entry.url!)}>
                        {entry.name || entry.key}
                      </button>
                    ) : (
                      entry.name || entry.key
                    )}
                    {entry.message && <span className="hint">{entry.message}</span>}
                  </li>
                ))}
              </ul>
            )}

            <fieldset className="import-mode" disabled={importing}>
              <legend className="visually-hidden">Import mode</legend>
              <label className="toggle">
                <input
                  type="radio"
                  name="import-mode"
                  checked={mode === "keep"}
                  onChange={() => {
                    setMode("keep");
                    setConfirming(false);
                  }}
                />
                Keep my addons
              </label>
              <label className="toggle">
                <input
                  type="radio"
                  name="import-mode"
                  checked={mode === "replace"}
                  onChange={() => setMode("replace")}
                />
                Replace all
              </label>
            </fieldset>
            <span className="hint">
              {mode === "keep"
                ? "Adds the missing addons and leaves the others as they are."
                : `Adds the missing addons, then removes ${
                    shownPreview.removals.length === 1
                      ? "the 1 folder"
                      : `the ${shownPreview.removals.length} folders`
                  } not in the list. There is no backup.`}
            </span>

            {confirming ? (
              <div className="import-confirm" role="alertdialog" aria-labelledby="import-confirm-title">
                <p id="import-confirm-title">
                  These folders will be deleted from Interface/AddOns, without a backup:
                </p>
                <ul className="import-list">
                  {shownPreview.removals.map((folder) => (
                    <li key={folder}>{folder}</li>
                  ))}
                </ul>
                <div className="import-export-actions">
                  <button
                    className="button danger"
                    onClick={() => void runImport(shownPreview.removals)}
                  >
                    Remove {shownPreview.removals.length}{" "}
                    {shownPreview.removals.length === 1 ? "folder" : "folders"} and import
                  </button>
                  <button ref={cancelRef} className="button secondary" onClick={cancelConfirm}>
                    Cancel
                  </button>
                </div>
              </div>
            ) : (
              <div className="import-export-actions">
                <button
                  ref={importRef}
                  className="button"
                  onClick={() => startImport(shownPreview)}
                  disabled={
                    importing || (fresh.length === 0 && (mode === "keep" || shownPreview.removals.length === 0))
                  }
                >
                  Import
                </button>
              </div>
            )}
          </div>
        )}

        {importing && (
          <div className="import-progress">
            <span className="muted">{importProgressText(reached, fresh, progress?.phase)}</span>
            <ProgressBar
              value={progressPercent(progress)}
              label={progressLabel(progress, "Preparing…")}
              name={progress?.name || "Import"}
            />
          </div>
        )}
        {result.state === "error" && (
          <p className="message error" role="alert">
            {result.message}
          </p>
        )}
        {summary && (
          <div className="import-result" role="status">
            <p className={summary.failed ? "error" : undefined}>{summary.headline}</p>
            {summary.problems.length > 0 && (
              <ul className="import-list">
                {summary.problems.map((problem, index) => (
                  <li key={index}>{problem}</li>
                ))}
              </ul>
            )}
          </div>
        )}
      </div>
    </section>
  );
}
