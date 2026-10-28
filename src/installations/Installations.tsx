import { useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { api, errorMessage, type Installation } from "../api";
import { SettingsSection } from "../options/Setting";
import { BackupList, useBackups } from "./Backups";
import { Discovered } from "./Discovered";
import { RowMenu } from "./RowMenu";

type Props = {
  /** La tab Options è aperta: la ricerca delle installazioni parte la prima volta. */
  active: boolean;
  installations: Installation[];
  /** L'installazione appena aggiunta diventa quella attiva (il selettore è nell'header). */
  onAdded: (id: string) => void;
  onChange: (installations: Installation[]) => void;
};

/**
 * La sezione "Installations" di Options: una scheda con le installazioni aggiunte e, sotto,
 * quelle trovate sul computer (issue #231). "Add installation" è il pulsante principale.
 */
export function Installations({ active, installations, onAdded, onChange }: Props) {
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
      onAdded(added.id);
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
    <SettingsSection
      id="installations-title"
      title="Installations"
      className="installations"
      action={
        <button className="button" onClick={add} disabled={busy}>
          Add installation
        </button>
      }
    >
      {error && (
        <p className="message error" role="alert">
          {error}
        </p>
      )}

      <div className="settings-card">
        {installations.length === 0 ? (
          <p className="setting-note">
            No installations yet. Add the <code>_retail_</code> folder,{" "}
            <code>_classic_era_</code> or another <code>_&lt;flavor&gt;_</code> folder inside{" "}
            <code>World of Warcraft</code>.
          </p>
        ) : (
          <ul className="installation-list" aria-label="Added installations">
            {installations.map((installation) => (
              <InstallationRow
                key={installation.id}
                installation={installation}
                confirming={installation.id === confirmingId}
                busy={busy}
                onConfirm={() => setConfirmingId(installation.id)}
                onCancel={() => setConfirmingId(null)}
                onRemove={() => remove(installation.id)}
              />
            ))}
          </ul>
        )}
        <Discovered
          active={active}
          installations={installations}
          onAdded={onAdded}
          onChange={onChange}
        />
      </div>
    </SettingsSection>
  );
}

type RowProps = {
  installation: Installation;
  confirming: boolean;
  busy: boolean;
  onConfirm: () => void;
  onCancel: () => void;
  onRemove: () => void;
};

/**
 * Una riga: nome, percorso e ultimi backup a sinistra; "Back up" in vista e la rimozione nel
 * menu "⋯", con una conferma in linea al posto delle azioni.
 */
function InstallationRow({
  installation,
  confirming,
  busy,
  onConfirm,
  onCancel,
  onRemove,
}: RowProps) {
  const backups = useBackups(installation.id);
  const menuTrigger = useRef<HTMLButtonElement | null>(null);

  function cancel() {
    onCancel();
    // Il menu torna al posto della conferma: il focus torna sul suo pulsante.
    window.setTimeout(() => menuTrigger.current?.focus(), 0);
  }

  return (
    <li className="installation">
      <div className="installation-main">
        <span className="installation-name">{installation.name}</span>
        <span className="installation-path" title={installation.path}>
          {installation.path}
        </span>
        <BackupList backups={backups} />
      </div>
      {confirming ? (
        <span className="installation-actions confirm" role="group" aria-label={`Remove ${installation.name}?`}>
          <span>Remove?</span>
          <button className="button danger small" onClick={onRemove} disabled={busy}>
            Remove
          </button>
          <button className="button ghost small" onClick={cancel} autoFocus>
            Cancel
          </button>
        </span>
      ) : (
        <span className="installation-actions">
          {/* Il pulsante resta montato mentre il backup gira: il focus non salta e lo
              stato si legge sul controllo stesso. */}
          <button
            className="button secondary small"
            onClick={() => void backups.create()}
            disabled={backups.creating}
            aria-busy={backups.creating}
            aria-label={
              backups.creating
                ? `Backing up ${installation.name}`
                : `Back up ${installation.name}`
            }
          >
            {backups.creating ? "Backing up…" : "Back up"}
          </button>
          <RowMenu
            label={`More actions for ${installation.name}`}
            triggerRef={menuTrigger}
            items={[
              {
                label: "Remove…",
                danger: true,
                disabledReason: backups.creating ? "Wait for the backup to finish." : null,
                onSelect: onConfirm,
              },
            ]}
          />
        </span>
      )}
    </li>
  );
}
