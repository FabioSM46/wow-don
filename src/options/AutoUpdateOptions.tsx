import { useEffect, useRef, useState } from "react";
import { api, errorMessage, type AutoUpdateMode, type Installation } from "../api";
import {
  AUTO_UPDATE_INTERVALS,
  AUTO_UPDATE_MODES,
  installationHint,
  intervalDisabled,
  intervalLabel,
  isAutoUpdateMode,
  modeHint,
  shownValue,
} from "./autoUpdate";

type Props = {
  /** La tab Options è aperta: le impostazioni si rileggono quando lo diventa. */
  active: boolean;
  installations: Installation[];
};

type Schedule = { mode: AutoUpdateMode; intervalHours: number };

/**
 * La sezione "Automatic updates" di Options (D41, issue #204): modalità e intervallo
 * (`autoUpdate` e `autoUpdateIntervalHours` di `get_settings`, salvati con `set_auto_update`) e
 * un interruttore per installazione (`get_auto_update`, `set_installation_auto_update`). I giri
 * e gli aggiornamenti restano nel backend: qui ci sono solo le scelte.
 *
 * Ogni controllo mostra il valore salvato; durante un salvataggio quello scelto. Un salvataggio
 * fallito mostra l'errore e il controllo torna al valore salvato.
 */
export function AutoUpdateOptions({ active, installations }: Props) {
  const [schedule, setSchedule] = useState<Schedule | null>(null);
  const [pending, setPending] = useState<Schedule | undefined>(undefined);
  // Id dell'installazione → partecipa all'aggiornamento automatico.
  const [enabled, setEnabled] = useState<ReadonlyMap<string, boolean>>(new Map());
  const [pendingInstallation, setPendingInstallation] = useState<
    { id: string; enabled: boolean } | undefined
  >(undefined);
  const [error, setError] = useState<string | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  // Come in `SystemOptions`: una lettura vale solo se nel frattempo non è partito altro, e
  // nessuna lettura parte mentre un salvataggio è in corso.
  const generation = useRef(0);
  const savingRef = useRef(false);
  const ids = installations.map((installation) => installation.id).join("\n");

  useEffect(() => {
    if (!active) return;

    async function refresh() {
      if (savingRef.current) return;
      const id = ++generation.current;
      const list = ids === "" ? [] : ids.split("\n");
      const [[settings], states] = await Promise.all([
        Promise.allSettled([api.getSettings()]),
        Promise.allSettled(list.map((installationId) => api.getAutoUpdate(installationId))),
      ]);
      if (id !== generation.current) return;
      const failures: string[] = [];
      if (settings.status === "fulfilled") {
        setSchedule({
          mode: settings.value.autoUpdate,
          intervalHours: settings.value.autoUpdateIntervalHours,
        });
      } else {
        failures.push(errorMessage(settings.reason));
      }
      const next = new Map<string, boolean>();
      states.forEach((state, index) => {
        if (state.status === "fulfilled") {
          next.set(list[index], !state.value.disabled);
        } else {
          failures.push(errorMessage(state.reason));
        }
      });
      setEnabled(next);
      // Lo stesso errore per più installazioni (per esempio `settings.json` danneggiato) una volta.
      setLoadError(failures.length > 0 ? [...new Set(failures)].join(" ") : null);
    }

    void refresh();
  }, [active, ids]);

  async function saveSchedule(next: Schedule) {
    savingRef.current = true;
    generation.current++;
    setError(null);
    setPending(next);
    try {
      const saved = await api.setAutoUpdate(next.mode, next.intervalHours);
      setSchedule({ mode: saved.autoUpdate, intervalHours: saved.autoUpdateIntervalHours });
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      savingRef.current = false;
      setPending(undefined);
    }
  }

  async function saveInstallation(installationId: string, next: boolean) {
    savingRef.current = true;
    generation.current++;
    setError(null);
    setPendingInstallation({ id: installationId, enabled: next });
    try {
      const saved = await api.setInstallationAutoUpdate(installationId, next);
      setEnabled((previous) => new Map(previous).set(installationId, !saved.disabled));
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      savingRef.current = false;
      setPendingInstallation(undefined);
    }
  }

  const shown = schedule ? shownValue(schedule, pending) : null;
  const saving = pending !== undefined || pendingInstallation !== undefined;
  const shownError = error ?? loadError;

  return (
    <section className="auto-update-option" aria-labelledby="auto-update-title">
      <header className="section-header">
        <h2 id="auto-update-title">Automatic updates</h2>
      </header>
      <div className="option-row inline-fields">
        <label className="inline-field">
          Mode
          <select
            className="select"
            value={shown?.mode ?? "notify"}
            disabled={schedule === null || saving}
            aria-describedby="auto-update-mode-hint"
            onChange={(event) => {
              const mode = event.target.value;
              if (schedule && isAutoUpdateMode(mode)) void saveSchedule({ ...schedule, mode });
            }}
          >
            {AUTO_UPDATE_MODES.map((mode) => (
              <option key={mode.id} value={mode.id}>
                {mode.label}
              </option>
            ))}
          </select>
        </label>
        <label className="inline-field">
          Check
          <select
            className="select"
            value={shown?.intervalHours ?? 6}
            disabled={schedule === null || saving || intervalDisabled(shown?.mode ?? null)}
            onChange={(event) => {
              const intervalHours = Number(event.target.value);
              if (schedule) void saveSchedule({ ...schedule, intervalHours });
            }}
          >
            {AUTO_UPDATE_INTERVALS.map((hours) => (
              <option key={hours} value={hours}>
                {intervalLabel(hours)}
              </option>
            ))}
          </select>
        </label>
      </div>
      <span className="hint" id="auto-update-mode-hint">
        {modeHint(shown?.mode ?? null)}
      </span>

      {installations.length === 0 ? (
        <p className="empty">Add an installation to choose which ones update automatically.</p>
      ) : (
        <div className="option-row installation-toggles">
          {installations.map((installation) => {
            const saved = enabled.get(installation.id);
            const value =
              pendingInstallation?.id === installation.id
                ? pendingInstallation.enabled
                : saved;
            return (
              <label key={installation.id} className="toggle">
                <input
                  type="checkbox"
                  checked={value ?? false}
                  disabled={saved === undefined || saving}
                  aria-describedby="auto-update-installations-hint"
                  onChange={(event) => void saveInstallation(installation.id, event.target.checked)}
                />
                {installation.name}
              </label>
            );
          })}
          <span className="hint" id="auto-update-installations-hint">
            {installationHint(shown?.mode ?? null)}
          </span>
        </div>
      )}

      {shownError && (
        <p className="message error" role="alert">
          {shownError}
        </p>
      )}
    </section>
  );
}
