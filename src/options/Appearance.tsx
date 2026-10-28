import { useState } from "react";
import { api, errorMessage, type Theme } from "../api";
import { SettingRow, SettingsSection, Segmented } from "./Setting";
import { isTheme, THEMES } from "./theme";

type Props = {
  /** Il tema in uso; `null` finché `get_settings` non ha risposto. */
  theme: Theme | null;
  /** Errore di `get_settings` (per esempio `settings.json` danneggiato). */
  loadError: string | null;
  /** Applica il tema a tutta l'app. */
  onChange: (theme: Theme) => void;
};

/** La sezione "Appearance" di Options: un solo controllo segmentato per il tema. */
export function Appearance({ theme, loadError, onChange }: Props) {
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  async function choose(value: string) {
    if (!isTheme(value) || theme === null) return;
    const previous = theme;
    setError(null);
    // Il tema si applica subito; se il salvataggio fallisce si torna a quello di prima, così
    // quello che si vede è quello che ci sarà al prossimo avvio.
    onChange(value);
    setBusy(true);
    try {
      const saved = await api.setTheme(value);
      onChange(saved.theme);
    } catch (err) {
      onChange(previous);
      setError(errorMessage(err));
    } finally {
      setBusy(false);
    }
  }

  const shownError = error ?? loadError;

  return (
    <SettingsSection id="appearance-title" title="Appearance" className="appearance">
      <div className="settings-card">
        <SettingRow
          label="Theme"
          labelId="theme-label"
          description="System follows the light or dark setting of the computer."
          descriptionId="theme-description"
        >
          <Segmented
            labelledBy="theme-label"
            describedBy="theme-description"
            options={THEMES}
            value={theme}
            disabled={theme === null || busy}
            onChange={(value) => void choose(value)}
          />
        </SettingRow>
      </div>

      {shownError && (
        <p className="message error" role="alert">
          {shownError}
        </p>
      )}
    </SettingsSection>
  );
}
