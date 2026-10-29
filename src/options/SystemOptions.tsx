import { useEffect, useRef, useState } from "react";
import { api, errorMessage } from "../api";
import { SettingsSection, SwitchRow } from "./Setting";
import { autostartHint, closeToTrayHint } from "./system";

type Props = {
  /** The Options tab is open: the settings are read again when it becomes so. */
  active: boolean;
};

type Tray = { closeToTray: boolean; trayAvailable: boolean };
type Saving = "tray" | "autostart" | null;

/**
 * The "System" section of Options (issue #160): "Close to tray" (`closeToTray` from
 * `get_settings`, saved with `set_close_to_tray`) and "Start with the system" (`get_autostart`
 * and `set_autostart`, the real state of the operating system). Tray and autostart stay in the
 * backend: here there are only the app's commands.
 *
 * `trayAvailable` can change while the app runs, so the values are read again every time
 * Options opens and when the window comes back to the foreground with Options open.
 */
export function SystemOptions({ active }: Props) {
  const [tray, setTray] = useState<Tray | null>(null);
  const [autostart, setAutostart] = useState<boolean | null>(null);
  const [saving, setSaving] = useState<Saving>(null);
  const [error, setError] = useState<string | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  // Counts reads and saves: a read counts only if nothing else has started in the meantime, so
  // an old answer does not cover the value just saved.
  const generation = useRef(0);
  // A save in progress wins: no read starts until it finishes, because the backend could serve
  // it before the write and its answer arrive later. A ref and not `saving`, which the effect
  // does not see again.
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
      // The switch is controlled: without a new saved value it stays on the previous one.
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
