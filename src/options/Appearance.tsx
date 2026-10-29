import { useState } from "react";
import { api, errorMessage, type Theme } from "../api";
import { SettingRow, SettingsSection, Segmented } from "./Setting";
import { isTheme, THEMES } from "./theme";

type Props = {
  /** The theme in use; `null` until `get_settings` has answered. */
  theme: Theme | null;
  /** Error from `get_settings` (for example a corrupted `settings.json`). */
  loadError: string | null;
  /** Applies the theme to the whole app. */
  onChange: (theme: Theme) => void;
};

/** The "Appearance" section of Options: a single segmented control for the theme. */
export function Appearance({ theme, loadError, onChange }: Props) {
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  async function choose(value: string) {
    if (!isTheme(value) || theme === null) return;
    const previous = theme;
    setError(null);
    // The theme applies at once; if saving fails it goes back to the previous one, so what
    // you see is what there will be at the next start.
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
