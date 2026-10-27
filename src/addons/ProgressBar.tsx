import { useEffect, useRef } from "react";
import { listen } from "@tauri-apps/api/event";
import { PROGRESS_EVENT, type InstallProgress } from "./progress";

type Props = {
  /** La percentuale da 0 a 100, o `null` per la barra indeterminata. */
  value: number | null;
  /** La fase scritta accanto: "Downloading 42%", "Installing…". */
  label: string;
  /** Il nome di quello che si installa, per l'etichetta accessibile. */
  name: string;
};

/**
 * La barra di avanzamento di un'installazione o di un aggiornamento (issue #128): sottile, con la
 * fase scritta accanto. Determinata durante il download quando il server dichiara la lunghezza,
 * indeterminata altrimenti.
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
 * Ascolta `install-progress` per tutta la vita del componente e passa a `onProgress` solo gli
 * eventi di `installationId`. Il listener si registra una volta e legge sempre gli ultimi valori.
 * Fuori da Tauri (per esempio `vite` nel browser) non arriva niente e le barre restano
 * indeterminate.
 */
export function useInstallProgress(
  installationId: string | null,
  onProgress: (event: InstallProgress) => void,
) {
  const latest = useRef({ installationId, onProgress });
  // Aggiornato dopo il commit, non durante il render: un render scartato non lo tocca.
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
      // Fuori da Tauri non c'è niente da ascoltare: la barra resta indeterminata.
      .catch(() => {});
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);
}
