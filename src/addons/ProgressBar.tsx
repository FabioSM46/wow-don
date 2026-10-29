import { useEffect, useRef } from "react";
import { listen } from "@tauri-apps/api/event";
import { PROGRESS_EVENT, type InstallProgress } from "./progress";

type Props = {
  /** The percentage from 0 to 100, or `null` for the indeterminate bar. */
  value: number | null;
  /** The phase written next to it: "Downloading 42%", "Installing…". */
  label: string;
  /** The name of what is being installed, for the accessible label. */
  name: string;
};

/**
 * The progress bar of an installation or an update (issue #128): thin, with the phase written
 * next to it. Determinate during the download when the server declares the length,
 * indeterminate otherwise.
 */
export function ProgressBar({ value, label, name }: Props) {
  const determinate = value !== null;
  return (
    <span className="progress">
      <span
        className={determinate ? "progress-track" : "progress-track indeterminate"}
        role="progressbar"
        aria-label={`${name}: ${label}`}
        aria-valuemin={determinate ? 0 : undefined}
        aria-valuemax={determinate ? 100 : undefined}
        aria-valuenow={determinate ? value : undefined}
      >
        <span className="progress-fill" style={determinate ? { width: `${value}%` } : undefined} />
      </span>
      <span className="progress-label">{label}</span>
    </span>
  );
}

/**
 * Listens to `install-progress` for the whole life of the component and passes to `onProgress`
 * only the events of `installationId`. The listener is registered once and always reads the
 * latest values. Outside Tauri (for example `vite` in the browser) nothing arrives and the bars
 * stay indeterminate.
 */
export function useInstallProgress(
  installationId: string | null,
  onProgress: (event: InstallProgress) => void,
) {
  const latest = useRef({ installationId, onProgress });
  // Updated after the commit, not during render: a discarded render does not touch it.
  useEffect(() => {
    latest.current = { installationId, onProgress };
  });

  useEffect(() => {
    let unlisten: (() => void) | null = null;
    let disposed = false;
    listen<InstallProgress>(PROGRESS_EVENT, ({ payload }) => {
      if (payload.installationId !== latest.current.installationId) return;
      latest.current.onProgress(payload);
    })
      .then((stop) => {
        if (disposed) stop();
        else unlisten = stop;
      })
      // Outside Tauri there is nothing to listen to: the bar stays indeterminate.
      .catch(() => {});
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);
}
