import { useCallback, useEffect, useRef, useState } from "react";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { api, errorMessage, type BackupInfo } from "../api";

/** How many backups are shown under the installation; the others are in the folder. */
const VISIBLE = 3;

const dateFormat = new Intl.DateTimeFormat("en", { dateStyle: "medium", timeStyle: "short" });
const numberFormat = new Intl.NumberFormat("en", { maximumFractionDigits: 1 });

/** `format` throws on an invalid date: an odd `mtime` shows "—". */
function formatDate(ms: number | null): string {
  if (ms === null) return "—";
  const date = new Date(ms);
  return Number.isNaN(date.getTime()) ? "—" : dateFormat.format(date);
}

/** Bytes in readable units, base 1024 (like the most common file managers). */
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
  /** `null` until the first read has succeeded. */
  list: BackupInfo[] | null;
  /** Error of the last read of the list; cleared on the next successful read. */
  listError: string | null;
  /** Error of the last action (Backup, Open folder); cleared on the next action. */
  actionError: string | null;
  creating: boolean;
  reload: () => Promise<void>;
  create: () => Promise<void>;
  reveal: (backup: BackupInfo) => Promise<void>;
};

/** Backup state of an installation: list, creation in progress and inline errors. */
export function useBackups(installationId: string): Backups {
  const [list, setList] = useState<BackupInfo[] | null>(null);
  const [listError, setListError] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);
  // Only the latest requested read counts: an old answer does not overwrite a newer one.
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
    // Unmounted or installation changed: in-flight answers no longer count.
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
    // The list is read again from the backend even after an error: there is no partial backup,
    // but the folder may have changed in the meantime.
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

/** The compact list of the latest backups, under the installation path. */
export function BackupList({ backups }: { backups: Backups }) {
  const { list, listError, actionError, reload, reveal } = backups;
  // `list_backups` returns the backups newest first (sorted in the backend, `backup.rs`):
  // the first `VISIBLE` are the latest ones made.
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
        // First read failed: the error is enough, without a sentence that would say "no backups".
        !listError && <p className="backups-note">Loading backups…</p>
      ) : list.length === 0 ? (
        <p className="backups-note">
          No backups yet. Use “Back up” to save <code>WTF</code> and <code>Interface</code> before
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
