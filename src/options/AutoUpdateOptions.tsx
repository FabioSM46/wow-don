import { useEffect, useRef, useState } from "react";
import { api, errorMessage, type AutoUpdateMode, type Installation } from "../api";
import {
  AUTO_UPDATE_INTERVALS,
  AUTO_UPDATE_MODES,
  installationHint,
  intervalDisabled,
  intervalLabel,
  modeHint,
  shownValue,
} from "./autoUpdate";
import { Segmented, SettingRow, SettingsSection, SwitchRow } from "./Setting";

type Props = {
  /** The Options tab is open: the settings are read again when it becomes so. */
  active: boolean;
  installations: Installation[];
  /** The saved mode, on every read and save (the status line of Installed, #203). */
  onModeChange?: (mode: AutoUpdateMode) => void;
};

type Schedule = { mode: AutoUpdateMode; intervalHours: number };

/**
 * The "Automatic updates" section of Options (D41, issue #204): mode and interval
 * (`autoUpdate` and `autoUpdateIntervalHours` from `get_settings`, saved with `set_auto_update`)
 * and a switch per installation (`get_auto_update`, `set_installation_auto_update`). The rounds
 * and the updates stay in the backend: here there are only the choices.
 *
 * Each control shows the saved value; during a save the chosen one. A failed save shows the
 * error and the control goes back to the saved value.
 */
export function AutoUpdateOptions({ active, installations, onModeChange }: Props) {
  const [schedule, setSchedule] = useState<Schedule | null>(null);
  const [pending, setPending] = useState<Schedule | undefined>(undefined);
  // Installation id → takes part in automatic updates.
  const [enabled, setEnabled] = useState<ReadonlyMap<string, boolean>>(new Map());
  const [pendingInstallation, setPendingInstallation] = useState<
    { id: string; enabled: boolean } | undefined
  >(undefined);
  const [error, setError] = useState<string | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  // As in `SystemOptions`: a read counts only if nothing else has started in the meantime, and
  // no read starts while a save is in progress.
  const generation = useRef(0);
  const savingRef = useRef(false);
  // A read skipped during a save, or discarded because the save changed generation, would
  // never come back: for example an installation added while saving would stay without a
  // value and with its checkbox disabled. Every finished save therefore restarts the read by
  // changing `retry`.
  const [retry, setRetry] = useState(0);
  const modeChanged = useRef(onModeChange);
  useEffect(() => {
    modeChanged.current = onModeChange;
  });
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
        modeChanged.current?.(settings.value.autoUpdate);
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
      // The same error for several installations (for example a corrupted `settings.json`) once.
      setLoadError(failures.length > 0 ? [...new Set(failures)].join(" ") : null);
    }

    void refresh();
  }, [active, ids, retry]);

  async function saveSchedule(next: Schedule) {
    savingRef.current = true;
    generation.current++;
    setError(null);
    setPending(next);
    try {
      const saved = await api.setAutoUpdate(next.mode, next.intervalHours);
      setSchedule({ mode: saved.autoUpdate, intervalHours: saved.autoUpdateIntervalHours });
      modeChanged.current?.(saved.autoUpdate);
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      savingRef.current = false;
      setRetry((n) => n + 1);
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
      setRetry((n) => n + 1);
      setPendingInstallation(undefined);
    }
  }

  const shown = schedule ? shownValue(schedule, pending) : null;
  const saving = pending !== undefined || pendingInstallation !== undefined;
  const shownError = error ?? loadError;

  return (
    <SettingsSection id="auto-update-title" title="Automatic updates" className="auto-update-option">
      <div className="settings-card">
        <SettingRow
          label="Mode"
          labelId="auto-update-mode-label"
          description={modeHint(shown?.mode ?? null)}
          descriptionId="auto-update-mode-hint"
        >
          <Segmented
            labelledBy="auto-update-mode-label"
            describedBy="auto-update-mode-hint"
            options={AUTO_UPDATE_MODES}
            value={shown?.mode ?? null}
            disabled={schedule === null || saving}
            onChange={(mode) => {
              if (schedule) void saveSchedule({ ...schedule, mode });
            }}
          />
        </SettingRow>
        <SettingRow label="Check" htmlFor="auto-update-interval">
          <select
            id="auto-update-interval"
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
        </SettingRow>
      </div>

      {installations.length === 0 ? (
        <p className="empty">Add an installation to choose which ones update automatically.</p>
      ) : (
        <div className="settings-card">
          {installations.map((installation) => {
            const saved = enabled.get(installation.id);
            const value =
              pendingInstallation?.id === installation.id
                ? pendingInstallation.enabled
                : saved;
            return (
              <SwitchRow
                key={installation.id}
                label={installation.name}
                describedBy="auto-update-installations-hint"
                checked={value ?? false}
                disabled={saved === undefined || saving}
                onChange={(next) => void saveInstallation(installation.id, next)}
              />
            );
          })}
          <p className="setting-note" id="auto-update-installations-hint">
            {installationHint(shown?.mode ?? null)}
          </p>
        </div>
      )}

      {shownError && (
        <p className="message error" role="alert">
          {shownError}
        </p>
      )}
    </SettingsSection>
  );
}
