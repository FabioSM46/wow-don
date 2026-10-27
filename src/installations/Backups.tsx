import { useCallback, useEffect, useRef, useState } from "react";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { api, errorMessage, type BackupInfo } from "../api";

/** Quanti backup si vedono sotto l'installazione; gli altri si trovano nella cartella. */
const VISIBLE = 3;

const dateFormat = new Intl.DateTimeFormat("en", { dateStyle: "medium", timeStyle: "short" });
const numberFormat = new Intl.NumberFormat("en", { maximumFractionDigits: 1 });

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
  /** `null` finché la prima lettura non è riuscita. */
  list: BackupInfo[] | null;
  /** Errore dell'ultima lettura dell'elenco; si azzera alla lettura successiva riuscita. */
  listError: string | null;
  /** Errore dell'ultima azione (Backup, Apri cartella); si azzera all'azione successiva. */
  actionError: string | null;
  creating: boolean;
  reload: () => Promise<void>;
  create: () => Promise<void>;
  reveal: (backup: BackupInfo) => Promise<void>;
};

/** Stato dei backup di un'installazione: elenco, creazione in corso ed errori in linea. */
export function useBackups(installationId: string): Backups {
  const [list, setList] = useState<BackupInfo[] | null>(null);
  const [listError, setListError] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);
  // Vale solo l'ultima lettura chiesta: una risposta vecchia non sovrascrive una più nuova.
  const request = useRef(0);

  const reload = useCallback(async () => {
    const id = ++request.current;
    try {
      const backups = await api.listBackups(installationId);
      if (id === request.current) {
        setList(backups);
        setListError(null);
      }
    } catch (err) {
      if (id === request.current) setListError(errorMessage(err));
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
    setActionError(null);
    setCreating(true);
    try {
      await api.createBackup(installationId);
    } catch (err) {
      setActionError(errorMessage(err));
    } finally {
      setCreating(false);
    }
    // L'elenco si rilegge dal backend anche dopo un errore: un backup parziale non c'è, ma
    // la cartella può essere cambiata nel frattempo.
    await reload();
  }, [installationId, reload]);

  const reveal = useCallback(async (backup: BackupInfo) => {
    setActionError(null);
    try {
      await revealItemInDir(backup.path);
    } catch (err) {
      setActionError(errorMessage(err));
    }
  }, []);

  return { list, listError, actionError, creating, reload, create, reveal };
}

/** L'elenco compatto degli ultimi backup, sotto la riga dell'installazione. */
export function BackupList({ backups }: { backups: Backups }) {
  const { list, listError, actionError, reload, reveal } = backups;
  // `list_backups` restituisce i backup dal più recente (ordinati nel backend, `backup.rs`):
  // i primi `VISIBLE` sono gli ultimi fatti.
  const hidden = list ? list.length - VISIBLE : 0;

  return (
    <div className="backups">
      {actionError && (
        <p className="backups-error error" role="alert">
          {actionError}
        </p>
      )}
      {listError && (
        <p className="backups-error error" role="alert">
          {listError}{" "}
          <button className="link inline" onClick={() => void reload()}>
            Retry
          </button>
        </p>
      )}
      {list === null ? (
        // Prima lettura fallita: basta l'errore, senza una frase che direbbe "nessun backup".
        !listError && <p className="backups-note">Loading backups…</p>
      ) : list.length === 0 ? (
        <p className="backups-note">
          No backups yet. Use “Backup” to save <code>WTF</code> and <code>Interface</code> before
          changing many addons.
        </p>
      ) : (
        <ul className="backup-list" aria-label="Latest backups">
          {list.slice(0, VISIBLE).map((backup) => (
            <li key={backup.path} className="backup">
              <span className="backup-date" title={backup.name}>
                {formatDate(backup.modifiedMs)}
              </span>
              <span className="backup-size">{formatSize(backup.size)}</span>
              <button
                className="link subtle"
                onClick={() => void reveal(backup)}
                aria-label={`Open the folder of backup ${backup.name}`}
              >
                Open folder
              </button>
            </li>
          ))}
          {hidden > 0 && (
            <li className="backup backup-more">
              {hidden === 1 ? "1 older backup" : `${hidden} older backups`}
            </li>
          )}
        </ul>
      )}
    </div>
  );
}
