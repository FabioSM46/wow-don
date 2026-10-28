import { useEffect, useRef, useState } from "react";
import { api, errorMessage } from "../api";
import { SettingsSection, SwitchRow } from "./Setting";
import { autostartHint, closeToTrayHint } from "./system";

type Props = {
  /** La tab Options è aperta: le impostazioni si rileggono quando lo diventa. */
  active: boolean;
};

type Tray = { closeToTray: boolean; trayAvailable: boolean };
type Saving = "tray" | "autostart" | null;

/**
 * La sezione "System" di Options (issue #160): "Close to tray" (`closeToTray` di
 * `get_settings`, salvata con `set_close_to_tray`) e "Start with the system" (`get_autostart` e
 * `set_autostart`, lo stato reale del sistema operativo). Tray e avvio automatico restano nel
 * backend: qui ci sono solo i comandi dell'app.
 *
 * `trayAvailable` può cambiare mentre l'app gira, quindi i valori si rileggono ogni volta che
 * Options si apre e quando la finestra torna in primo piano con Options aperta.
 */
export function SystemOptions({ active }: Props) {
  const [tray, setTray] = useState<Tray | null>(null);
  const [autostart, setAutostart] = useState<boolean | null>(null);
  const [saving, setSaving] = useState<Saving>(null);
  const [error, setError] = useState<string | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  // Conta letture e salvataggi: una lettura vale solo se nel frattempo non è partito altro, così
  // una risposta vecchia non copre il valore appena salvato.
  const generation = useRef(0);
  // Un salvataggio in corso vince: nessuna lettura parte finché non finisce, perché il backend
  // potrebbe servirla prima della scrittura e la sua risposta arrivare dopo. Un ref e non
  // `saving`, che l'effetto non rivede.
  const savingRef = useRef(false);

  useEffect(() => {
    if (!active) return;

    async function refresh() {
      if (savingRef.current) return;
      const id = ++generation.current;
      const [settings, enabled] = await Promise.allSettled([
        api.getSettings(),
        api.getAutostart(),
      ]);
      if (id !== generation.current) return;
      if (settings.status === "fulfilled") {
        setTray({
          closeToTray: settings.value.closeToTray,
          trayAvailable: settings.value.trayAvailable,
        });
      }
      if (enabled.status === "fulfilled") setAutostart(enabled.value);
      const failures = [settings, enabled].flatMap((result) =>
        result.status === "rejected" ? [errorMessage(result.reason)] : [],
      );
      setLoadError(failures.length > 0 ? failures.join(" ") : null);
    }

    void refresh();
    const onFocus = () => void refresh();
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, [active]);

  async function save(which: Exclude<Saving, null>, next: boolean) {
    savingRef.current = true;
    generation.current++;
    setError(null);
    setSaving(which);
    try {
      if (which === "tray") {
        const saved = await api.setCloseToTray(next);
        setTray({ closeToTray: saved.closeToTray, trayAvailable: saved.trayAvailable });
      } else {
        setAutostart(await api.setAutostart(next));
      }
    } catch (err) {
      // L'interruttore è controllato: senza un valore salvato nuovo resta su quello di prima.
      setError(errorMessage(err));
    } finally {
      savingRef.current = false;
      setSaving(null);
    }
  }

  const trayAvailable = tray?.trayAvailable ?? null;
  const shownError = error ?? loadError;

  return (
    <SettingsSection id="system-title" title="System" className="system-option">
      <div className="settings-card">
        <SwitchRow
          label="Close to tray"
          description={closeToTrayHint(trayAvailable)}
          checked={tray?.closeToTray ?? false}
          disabled={tray === null || !tray.trayAvailable || saving !== null}
          onChange={(next) => void save("tray", next)}
        />
        <SwitchRow
          label="Start with the system"
          description={autostartHint(tray?.closeToTray ?? null, trayAvailable)}
          checked={autostart ?? false}
          disabled={autostart === null || saving !== null}
          onChange={(next) => void save("autostart", next)}
        />
      </div>
      {shownError && (
        <p className="message error" role="alert">
          {shownError}
        </p>
      )}
    </SettingsSection>
  );
}
