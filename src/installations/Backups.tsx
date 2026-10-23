import { useCallback, useEffect, useRef, useState } from "react";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { api, errorMessage, type BackupInfo } from "../api";

/** Quanti backup si vedono sotto l'installazione; gli altri si trovano nella cartella. */
const VISIBLE = 3;

const dateFormat = new Intl.DateTimeFormat("it-IT", { dateStyle: "medium", timeStyle: "short" });
const numberFormat = new Intl.NumberFormat("it-IT", { maximumFractionDigits: 1 });

/** `format` lancia un'eccezione su una data non valida: un `mtime` strano mostra "—". */
function formatDate(ms: number | null): string {
  if (ms === null) return "—";
  const date = new Date(ms);
  return Number.isNaN(date.getTime()) ? "—" : dateFormat.format(date);
}

/** Byte in unità leggibili, in base 1024 (come i gestori file più diffusi). */
function formatSize(bytes: number): string {
  const units = ["B", "KB", "MB", "GB", "TB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${numberFormat.format(value)} ${units[unit]}`;
}

export type Backups = {
  /** `null` finché la prima lettura non è finita. */
  list: BackupInfo[] | null;
  error: string | null;
  creating: boolean;
  create: () => Promise<void>;
  reveal: (backup: BackupInfo) => Promise<void>;
};

/** Stato dei backup di un'installazione: elenco, creazione in corso ed errore in linea. */
export function useBackups(installationId: string): Backups {
  const [list, setList] = useState<BackupInfo[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);
  // Vale solo l'ultima lettura chiesta: una risposta vecchia non sovrascrive una più nuova.
  const request = useRef(0);

  const reload = useCallback(async () => {
    const id = ++request.current;
    try {
      const backups = await api.listBackups(installationId);
      if (id === request.current) setList(backups);
    } catch (err) {
      if (id === request.current) setError(errorMessage(err));
    }
  }, [installationId]);

  useEffect(() => {
    void reload();
    // Smontato o installazione cambiata: le risposte in volo non contano più.
    return () => {
      request.current += 1;
    };
  }, [reload]);

  const create = useCallback(async () => {
    setError(null);
    setCreating(true);
    try {
      await api.createBackup(installationId);
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setCreating(false);
    }
    // L'elenco si rilegge dal backend anche dopo un errore: un backup parziale non c'è, ma
    // la cartella può essere cambiata nel frattempo.
    await reload();
  }, [installationId, reload]);

  const reveal = useCallback(async (backup: BackupInfo) => {
    setError(null);
    try {
      await revealItemInDir(backup.path);
    } catch (err) {
      setError(errorMessage(err));
    }
  }, []);

  return { list, error, creating, create, reveal };
}

/** L'elenco compatto degli ultimi backup, sotto la riga dell'installazione. */
export function BackupList({ backups }: { backups: Backups }) {
  const { list, error, reveal } = backups;
  const hidden = list ? list.length - VISIBLE : 0;

  return (
    <div className="backups">
      {error && (
        <p className="backups-error error" role="alert">
          {error}
        </p>
      )}
      {list === null ? (
        // Prima lettura fallita: basta l'errore, senza una frase che direbbe "nessun backup".
        !error && <p className="backups-note">Lettura dei backup…</p>
      ) : list.length === 0 ? (
        <p className="backups-note">
          Nessun backup. Usa «Backup» per salvare <code>WTF</code> e <code>Interface</code> prima
          di cambiare molti addon.
        </p>
      ) : (
        <ul className="backup-list" aria-label="Ultimi backup">
          {list.slice(0, VISIBLE).map((backup) => (
            <li key={backup.path} className="backup">
              <span className="backup-date" title={backup.name}>
                {formatDate(backup.modifiedMs)}
              </span>
              <span className="backup-size">{formatSize(backup.size)}</span>
              <button
                className="link subtle"
                onClick={() => void reveal(backup)}
                aria-label={`Apri la cartella del backup ${backup.name}`}
              >
                Apri cartella
              </button>
            </li>
          ))}
          {hidden > 0 && (
            <li className="backup backup-more">
              {hidden === 1 ? "1 backup meno recente" : `${hidden} backup meno recenti`}
            </li>
          )}
        </ul>
      )}
    </div>
  );
}
