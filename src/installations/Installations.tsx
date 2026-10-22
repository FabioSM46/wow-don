import { useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { api, errorMessage, type Installation } from "../api";

type Props = {
  installations: Installation[];
  activeId: string | null;
  onSelect: (id: string) => void;
  onChange: (installations: Installation[]) => void;
};

export function Installations({ installations, activeId, onSelect, onChange }: Props) {
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [confirmingId, setConfirmingId] = useState<string | null>(null);

  async function add() {
    setError(null);
    const selected = await open({
      directory: true,
      multiple: false,
      title: "Scegli la cartella _<flavor>_ di World of Warcraft",
    });
    // Selettore annullato: non succede niente.
    if (typeof selected !== "string") return;
    setBusy(true);
    try {
      const added = await api.addInstallation(selected);
      onChange([...installations, added]);
      onSelect(added.id);
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setBusy(false);
    }
  }

  async function remove(id: string) {
    setError(null);
    setBusy(true);
    try {
      await api.removeInstallation(id);
      onChange(installations.filter((installation) => installation.id !== id));
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setBusy(false);
      setConfirmingId(null);
    }
  }

  return (
    <section className="installations" aria-labelledby="installations-title">
      <header className="section-header">
        <h2 id="installations-title">Installazioni</h2>
        <button className="button" onClick={add} disabled={busy}>
          Aggiungi installazione
        </button>
      </header>

      {error && (
        <p className="message error" role="alert">
          {error}
        </p>
      )}

      {installations.length === 0 ? (
        <p className="empty">
          Nessuna installazione. Aggiungi la cartella <code>_retail_</code>,{" "}
          <code>_classic_era_</code> o un'altra cartella <code>_&lt;flavor&gt;_</code> dentro{" "}
          <code>World of Warcraft</code>.
        </p>
      ) : (
        <ul className="installation-list">
          {installations.map((installation) => {
            const active = installation.id === activeId;
            const confirming = installation.id === confirmingId;
            return (
              <li key={installation.id} className={active ? "installation active" : "installation"}>
                <button
                  className="installation-main"
                  onClick={() => onSelect(installation.id)}
                  aria-pressed={active}
                >
                  <span className="installation-name">{installation.name}</span>
                  <span className="installation-path" title={installation.path}>
                    {installation.path}
                  </span>
                </button>
                {confirming ? (
                  <span className="confirm">
                    <span>Rimuovere?</span>
                    <button
                      className="link danger"
                      onClick={() => remove(installation.id)}
                      disabled={busy}
                    >
                      Rimuovi
                    </button>
                    <button className="link" onClick={() => setConfirmingId(null)}>
                      Annulla
                    </button>
                  </span>
                ) : (
                  <button
                    className="link subtle"
                    onClick={() => setConfirmingId(installation.id)}
                    aria-label={`Rimuovi ${installation.name}`}
                  >
                    Rimuovi
                  </button>
                )}
              </li>
            );
          })}
        </ul>
      )}
    </section>
  );
}
