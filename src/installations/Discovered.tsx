import { useEffect, useRef, useState } from "react";
import { api, errorMessage, type DiscoveredInstallation, type Installation } from "../api";
import { flavorFolder, pendingDiscoveries } from "./discovery";

type Props = {
  /** The Options tab is open: the first scan starts the first time it becomes so. */
  active: boolean;
  installations: Installation[];
  /** Like the manual add: the added installation becomes the active one. */
  onAdded: (id: string) => void;
  onChange: (installations: Installation[]) => void;
};

type Scan =
  | { status: "idle" }
  | { status: "loading" }
  | { status: "done"; found: DiscoveredInstallation[] }
  | { status: "error"; message: string };

/**
 * "Found on this computer", in the Installations panel of Options: the installations that
 * `discover_installations` finds in the standard paths, each with its own "Add". It never
 * adds anything on its own.
 */
export function Discovered({ active, installations, onAdded, onChange }: Props) {
  const [scan, setScan] = useState<Scan>({ status: "idle" });
  // The rows added from here, hidden at once even if re-reading the list fails.
  const [added, setAdded] = useState<ReadonlySet<string>>(new Set());
  const [addingPath, setAddingPath] = useState<string | null>(null);
  const [addError, setAddError] = useState<string | null>(null);
  // Only the answer of the latest scan counts.
  const scanId = useRef(0);

  async function runScan() {
    const id = ++scanId.current;
    setScan({ status: "loading" });
    setAdded(new Set());
    setAddError(null);
    try {
      const found = await api.discoverInstallations();
      if (id === scanId.current) setScan({ status: "done", found });
    } catch (err) {
      if (id === scanId.current) setScan({ status: "error", message: errorMessage(err) });
    }
  }

  useEffect(() => {
    if (active && scan.status === "idle") void runScan();
    // The scan starts only once from here (only from `idle`); later ones with "Scan again".
  }, [active, scan.status]);

  async function add(item: DiscoveredInstallation) {
    setAddError(null);
    setAddingPath(item.path);
    try {
      const installation = await api.addInstallation(item.path);
      setAdded((previous) => new Set(previous).add(item.path));
      // Like the manual add: the list is read again from the backend. If re-reading fails
      // the add has still succeeded: the shown list receives the returned installation,
      // so it becomes active anyway, and the re-read error stays in view.
      let list: Installation[];
      try {
        list = await api.listInstallations();
      } catch (err) {
        list = [...installations, installation];
        setAddError(errorMessage(err));
      }
      onChange(list);
      onAdded(installation.id);
    } catch (err) {
      setAddError(errorMessage(err));
    } finally {
      setAddingPath(null);
    }
  }

  const pending =
    scan.status === "done" ? pendingDiscoveries(scan.found, installations, added) : [];
  const busy = scan.status === "loading" || addingPath !== null;

  return (
    <section className="discovered" aria-labelledby="discovered-title" aria-busy={busy}>
      <div className="discovered-header">
        <h4 id="discovered-title" className="setting-label">
          Found on this computer
        </h4>
        {scan.status !== "idle" && (
          <button className="button ghost small" onClick={() => void runScan()} disabled={busy}>
            Scan again
          </button>
        )}
      </div>

      {scan.status === "loading" && (
        <p className="discovered-note" role="status">
          Looking for World of Warcraft…
        </p>
      )}

      {scan.status === "error" && (
        <p className="discovered-note error" role="alert">
          {scan.message}
        </p>
      )}

      {addError && (
        <p className="discovered-note error" role="alert">
          {addError}
        </p>
      )}

      {scan.status === "done" && pending.length === 0 && (
        <p className="discovered-note">
          No new World of Warcraft installations found. Choose the folder yourself with “Add
          installation”.
        </p>
      )}

      {pending.length > 0 && (
        <ul className="installation-list" aria-label="Installations found on this computer">
          {pending.map((item) => (
            <li key={item.path} className="installation">
              <div className="installation-main">
                <span>
                  <span className="installation-name">{item.name}</span>
                  <span className="discovered-meta">
                    <code>{flavorFolder(item.flavor)}</code> · {item.origin}
                  </span>
                </span>
                <span className="installation-path" title={item.path}>
                  {item.path}
                </span>
              </div>
              <span className="installation-actions">
                <button
                  className="button soft small"
                  onClick={() => void add(item)}
                  disabled={busy}
                  aria-busy={addingPath === item.path}
                  aria-label={`Add ${item.name} (${item.path})`}
                >
                  {addingPath === item.path ? "Adding…" : "Add"}
                </button>
              </span>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
