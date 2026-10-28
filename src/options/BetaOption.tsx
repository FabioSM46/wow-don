import { useState } from "react";
import { api, errorMessage } from "../api";
import { SettingsSection, SwitchRow } from "./Setting";

type Props = {
  /** L'opzione salvata; `null` finché `get_settings` non ha risposto o se non si legge. */
  enabled: boolean | null;
  /** Applica il valore salvato a tutta l'app (ricerca e tab Installed lo rileggono). */
  onChange: (enabled: boolean) => void;
};

/**
 * La sezione "Updates" di Options: l'opzione per installare e aggiornare alla Beta quando la
 * versione del gioco non ha una Release (issue #141). La scelta del file resta nel backend.
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
