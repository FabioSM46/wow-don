import { useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { api, errorMessage, type Installation } from "../api";
import { BackupList, useBackups } from "./Backups";

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
    // Bloccato già mentre il selettore è aperto: un secondo clic non apre un altro selettore.
    setBusy(true);
    try {
      const selected = await open({
        directory: true,
        multiple: false,
        title: "Choose the World of Warcraft _<flavor>_ folder",
      });
      // Selettore annullato: non succede niente.
      if (typeof selected !== "string") return;
      const added = await api.addInstallation(selected);
      // La lista si rilegge dal backend invece di aggiungere a quella vista all'inizio.
      onChange(await api.listInstallations());
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
      onChange(await api.listInstallations());
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
        <h2 id="installations-title">Installations</h2>
        <button className="button" onClick={add} disabled={busy}>
          Add installation
        </button>
      </header>

      {error && (
        <p className="message error" role="alert">
          {error}
        </p>
      )}

      {installations.length === 0 ? (
        <p className="empty">
          No installations yet. Add the <code>_retail_</code> folder,{" "}
          <code>_classic_era_</code> or another <code>_&lt;flavor&gt;_</code> folder inside{" "}
          <code>World of Warcraft</code>.
        </p>
      ) : (
        <ul className="installation-list">
          {installations.map((installation) => (
            <InstallationRow
              key={installation.id}
              installation={installation}
              active={installation.id === activeId}
              confirming={installation.id === confirmingId}
              busy={busy}
              onSelect={() => onSelect(installation.id)}
              onConfirm={() => setConfirmingId(installation.id)}
              onCancel={() => setConfirmingId(null)}
              onRemove={() => remove(installation.id)}
            />
          ))}
        </ul>
      )}
    </section>
  );
}

type RowProps = {
  installation: Installation;
  active: boolean;
  confirming: boolean;
  busy: boolean;
  onSelect: () => void;
  onConfirm: () => void;
  onCancel: () => void;
  onRemove: () => void;
};

/** Una riga: selezione, azioni secondarie (Backup, Rimuovi) e sotto gli ultimi backup. */
function InstallationRow({
  installation,
  active,
  confirming,
  busy,
  onSelect,
  onConfirm,
  onCancel,
  onRemove,
}: RowProps) {
  const backups = useBackups(installation.id);

  return (
    <li className={active ? "installation active" : "installation"}>
      <div className="installation-row">
        <button className="installation-main" onClick={onSelect} aria-pressed={active}>
          <span className="installation-name">{installation.name}</span>
          <span className="installation-path" title={installation.path}>
            {installation.path}
          </span>
        </button>
        {confirming ? (
          <span className="confirm">
            <span>Remove?</span>
            <button className="link danger" onClick={onRemove} disabled={busy}>
              Remove
            </button>
            <button className="link" onClick={onCancel}>
              Cancel
            </button>
          </span>
        ) : (
          <span className="installation-actions">
            {/* Il pulsante resta montato mentre il backup gira: il focus non salta e lo
                stato si legge sul controllo stesso. */}
            <button
              className={backups.creating ? "link subtle busy" : "link subtle"}
              onClick={() => void backups.create()}
              disabled={backups.creating}
              aria-busy={backups.creating}
              aria-label={
                backups.creating
                  ? `Backing up ${installation.name}`
                  : `Back up ${installation.name}`
              }
            >
              {backups.creating ? "Backing up…" : "Backup"}
            </button>
            <button
              className="link subtle"
              onClick={onConfirm}
              disabled={backups.creating}
              aria-label={`Remove ${installation.name}`}
            >
              Remove
            </button>
          </span>
        )}
      </div>
      <BackupList backups={backups} />
    </li>
  );
}
