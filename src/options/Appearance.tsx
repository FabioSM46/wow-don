import { useState } from "react";
import { api, errorMessage, type Theme } from "../api";
import { isTheme, THEMES } from "./theme";

type Props = {
  /** Il tema in uso; `null` finché `get_settings` non ha risposto. */
  theme: Theme | null;
  /** Errore di `get_settings` (per esempio `settings.json` danneggiato). */
  loadError: string | null;
  /** Applica il tema a tutta l'app. */
  onChange: (theme: Theme) => void;
};

/** La sezione "Appearance" di Options: un solo controllo per il tema. */
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
    <section className="appearance" aria-labelledby="appearance-title">
      <header className="section-header">
        <h2 id="appearance-title">Appearance</h2>
        <select
          className="select"
          aria-label="Theme"
          value={theme ?? "system"}
          disabled={theme === null || busy}
          onChange={(event) => void choose(event.target.value)}
        >
          {THEMES.map((option) => (
            <option key={option.id} value={option.id}>
              {option.label}
            </option>
          ))}
        </select>
      </header>

      {shownError && (
        <p className="message error" role="alert">
          {shownError}
        </p>
      )}
    </section>
  );
}
