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
import { SettingRow, SettingsSection } from "./Setting";

type Props = {
  /** The active installation; `null` if there is none. */
  installation: Installation | null;
  /** The addons on disk changed: the Installed tab reads them again. */
  /** The import into installation `installationId` has finished, even with an error. */
  onImported: (installationId: string) => void;
};

type Load<T> =
  | { state: "idle" }
  | { state: "loading" }
  | { state: "ready"; value: T }
  | { state: "error"; message: string };

/**
 * The "Import and export" section of Options (issue #135): copies the addon string of the
 * active installation and imports one, with a preview before changing anything.
 * "Replace all" removes folders without a backup (the user's choice): the confirmation lists
 * exactly those, and only those reach the backend.
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
  // An answer counts only for the installation and the text the request started from.
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

  // Another installation: export and preview no longer hold.
  useEffect(() => {
    setExported({ state: "idle" });
    setPreview({ state: "idle" });
    setResult({ state: "idle" });
    setConfirming(false);
    setCopied(null);
  }, [id]);

  // The confirmation takes focus when it appears, so keyboard and screen readers reach it.
  useEffect(() => {
    if (confirming) cancelRef.current?.focus();
  }, [confirming]);

  // The "Copy" message disappears on its own.
  useEffect(() => {
    if (copied === null) return;
    const timer = window.setTimeout(() => setCopied(null), 2500);
    return () => window.clearTimeout(timer);
  }, [copied]);

  if (installation === null) {
    return (
      <SettingsSection id="import-export-title" title="Import and export" className="import-export">
        <p className="empty">Add an installation above to import or export its addons.</p>
      </SettingsSection>
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
      // Without clipboard access the text is selected: the system shortcut is enough.
      exportRef.current?.select();
      setCopied("Press Ctrl+C (⌘C on macOS) to copy");
    }
  }

  function changeText(value: string) {
    setText(value);
    // The preview was of the previous string.
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
      // A preview of another text or of another installation would show folders to remove
      // that have nothing to do with it.
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
        // The preview described the previous state.
        setPreview({ state: "idle" });
      }
    } catch (err) {
      if (idRef.current === requested) setResult({ state: "error", message: errorMessage(err) });
    } finally {
      setImporting(false);
      setProgress(undefined);
      // The addons on disk may have changed even if the command failed.
      onImported(requested);
    }
  }

  function cancelConfirm() {
    setConfirming(false);
    // The Import button comes back in place of the confirmation: focus returns with it.
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
    <SettingsSection id="import-export-title" title="Import and export" className="import-export">
      <div className="settings-card">
        <div className="import-export-block">
          <SettingRow
            label="Export"
            description={`A text list of the addons in ${current.name}, to install the same addons elsewhere.`}
          >
            <button
              className="button secondary"
              onClick={() => void runExport()}
              disabled={exported.state === "loading" || importing}
            >
              {exported.state === "loading" ? "Exporting…" : "Export addons"}
            </button>
          </SettingRow>
          {exported.state === "ready" && (
            <div className="import-export-actions">
              <button className="button secondary small" onClick={() => void copy(exported.value.text)}>
                Copy
              </button>
              {copied && (
                <span className="muted" role="status">
                  {copied}
                </span>
              )}
            </div>
          )}
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
          <SettingRow
            label="Import"
            description={`Paste a list, check the preview, then install its addons in ${current.name}.`}
          />
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
      </div>
    </SettingsSection>
  );
}
