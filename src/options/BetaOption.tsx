import { useState } from "react";
import { api, errorMessage } from "../api";
import { SettingsSection, SwitchRow } from "./Setting";

type Props = {
  /** The saved option; `null` until `get_settings` has answered or if it cannot be read. */
  enabled: boolean | null;
  /** Applies the saved value to the whole app (search and the Installed tab read it again). */
  onChange: (enabled: boolean) => void;
};

/**
 * The "Updates" section of Options: the option to install and update to the Beta when the
 * game version has no Release (issue #141). The choice of the file stays in the backend.
 */
export function BetaOption({ enabled, onChange }: Props) {
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  async function toggle(next: boolean) {
    setError(null);
    setBusy(true);
    try {
      const saved = await api.setBetaWhenNoRelease(next);
      onChange(saved.betaWhenNoRelease);
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <SettingsSection id="updates-title" title="Updates" className="updates-option">
      <div className="settings-card">
        <SwitchRow
          label="Use beta files when there is no release"
          description="Some addons publish only betas for a game version (for example WoW Forever). A release always wins when there is one."
          checked={enabled ?? false}
          disabled={enabled === null || busy}
          onChange={(next) => void toggle(next)}
        />
      </div>
      {error && (
        <p className="message error" role="alert">
          {error}
        </p>
      )}
    </SettingsSection>
  );
}
