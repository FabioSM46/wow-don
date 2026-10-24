import { useEffect, useRef, useState } from "react";
import { api, errorMessage, type DiscoveredInstallation, type Installation } from "../api";
import { flavorFolder, pendingDiscoveries } from "./discovery";

type Props = {
  /** La tab Options è aperta: la prima ricerca parte la prima volta che lo diventa. */
  active: boolean;
  installations: Installation[];
  /** Come l'aggiunta manuale: l'installazione aggiunta diventa quella attiva. */
  onAdded: (id: string) => void;
  onChange: (installations: Installation[]) => void;
};

type Scan =
  | { status: "idle" }
  | { status: "loading" }
  | { status: "done"; found: DiscoveredInstallation[] }
  | { status: "error"; message: string };

/**
 * La sezione "Found on this computer" di Options: le installazioni che `discover_installations`
 * trova nei percorsi standard, ognuna con il suo "Add". Non aggiunge mai niente da sola.
 */
export function Discovered({ active, installations, onAdded, onChange }: Props) {
  const [scan, setScan] = useState<Scan>({ status: "idle" });
  // Le righe aggiunte da qui, nascoste subito anche se la rilettura della lista fallisce.
  const [added, setAdded] = useState<ReadonlySet<string>>(new Set());
  const [addingPath, setAddingPath] = useState<string | null>(null);
  const [addError, setAddError] = useState<string | null>(null);
  // Solo la risposta dell'ultima ricerca conta.
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
    // La ricerca parte una volta sola da qui (solo da `idle`); le successive con "Scan again".
  }, [active, scan.status]);

  async function add(item: DiscoveredInstallation) {
    setAddError(null);
    setAddingPath(item.path);
    try {
      const installation = await api.addInstallation(item.path);
      setAdded((previous) => new Set(previous).add(item.path));
      // Come l'aggiunta manuale: la lista si rilegge dal backend. Se la rilettura fallisce
      // l'aggiunta è comunque riuscita: la lista mostrata riceve l'installazione restituita,
      // così diventa attiva lo stesso, e l'errore della rilettura resta in vista.
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
      <header className="section-header">
        <h2 id="discovered-title">Found on this computer</h2>
        {scan.status !== "idle" && (
          <button className="link" onClick={() => void runScan()} disabled={busy}>
            Scan again
          </button>
        )}
      </header>

      {scan.status === "loading" && (
        <p className="muted" role="status">
          Looking for World of Warcraft…
        </p>
      )}

      {scan.status === "error" && (
        <p className="message error" role="alert">
          {scan.message}
        </p>
      )}

      {addError && (
        <p className="message error" role="alert">
          {addError}
        </p>
      )}

      {scan.status === "done" && pending.length === 0 && (
        <p className="empty">
          No new World of Warcraft installations found. Choose the folder yourself with “Add
          installation” below.
        </p>
      )}

      {pending.length > 0 && (
        <ul className="installation-list">
          {pending.map((item) => (
            <li key={item.path} className="installation">
              <div className="installation-row">
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
                <button
                  className="button small"
                  onClick={() => void add(item)}
                  disabled={busy}
                  aria-busy={addingPath === item.path}
                  aria-label={`Add ${item.name} (${item.path})`}
                >
                  {addingPath === item.path ? "Adding…" : "Add"}
                </button>
              </div>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
