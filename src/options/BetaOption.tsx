import { useState } from "react";
import { api, errorMessage } from "../api";

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
    <section className="updates-option" aria-labelledby="updates-title">
      <header className="section-header">
        <h2 id="updates-title">Updates</h2>
      </header>
      <label className="toggle">
        <input
          type="checkbox"
          checked={enabled ?? false}
          disabled={enabled === null || busy}
          onChange={(event) => void toggle(event.target.checked)}
        />
        Use beta files when there is no release
      </label>
      <span className="hint">
        Some addons publish only betas for a game version (for example WoW Forever). A release
        always wins when there is one.
      </span>
      {error && (
        <p className="message error" role="alert">
          {error}
        </p>
      )}
    </section>
  );
}
