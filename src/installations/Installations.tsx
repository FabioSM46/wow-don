import { useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { api, errorMessage, type Installation } from "../api";
import { SettingsSection } from "../options/Setting";
import { BackupList, useBackups } from "./Backups";
import { Discovered } from "./Discovered";
import { RowMenu } from "./RowMenu";

type Props = {
  /** The Options tab is open: the installation scan starts the first time. */
  active: boolean;
  installations: Installation[];
  /** The installation just added becomes the active one (the selector is in the header). */
  onAdded: (id: string) => void;
  onChange: (installations: Installation[]) => void;
};

/**
 * The "Installations" section of Options: a panel with the added installations and, below,
 * those found on the computer (issue #231). "Add installation" is the main button.
 */
export function Installations({ active, installations, onAdded, onChange }: Props) {
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [confirmingId, setConfirmingId] = useState<string | null>(null);

  async function add() {
    setError(null);
    // Locked already while the picker is open: a second click does not open another picker.
    setBusy(true);
    try {
      const selected = await open({
        directory: true,
        multiple: false,
        title: "Choose the World of Warcraft _<flavor>_ folder",
      });
      // Picker cancelled: nothing happens.
      if (typeof selected !== "string") return;
      const added = await api.addInstallation(selected);
      // The list is read again from the backend instead of adding to the one seen at the start.
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
 * A row: name, path and latest backups on the left; "Back up" in view and removal in the
 * "⋯" menu, with an inline confirmation in place of the actions.
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
    // The menu comes back in place of the confirmation: focus returns to its button.
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
          {/* The button stays mounted while the backup runs: focus does not jump and the
              state is read on the control itself. */}
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
