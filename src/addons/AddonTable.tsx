import { useCallback, useEffect, useMemo, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  api,
  errorMessage,
  type AddonUpdate,
  type InstalledAddon,
  type Installation,
} from "../api";
import {
  authorsOf,
  defaultFilters,
  defaultSort,
  displayTitle,
  nextSort,
  visibleAddons,
  type Filters,
  type Sort,
  type SortKey,
} from "./filter";

type Props = { installation: Installation };

/**
 * Risultati del controllo aggiornamenti per installazione, validi per la sessione. Stanno fuori
 * dal componente perché cambiando installazione il componente si ricarica: tornando su una già
 * controllata non si richiama CurseForge.
 */
const updatesCache = new Map<string, AddonUpdate[]>();

type Check =
  | { state: "idle" }
  | { state: "checking" }
  | { state: "error"; message: string }
  | { state: "done"; updates: Map<string, AddonUpdate> };

function byFolder(updates: AddonUpdate[]): Map<string, AddonUpdate> {
  return new Map(updates.map((update) => [update.folder, update]));
}

type Load =
  | { state: "loading" }
  | { state: "error"; message: string }
  | { state: "ready"; addons: InstalledAddon[] };

const dateFormat = new Intl.DateTimeFormat("it-IT", { dateStyle: "medium" });

/** `format` lancia un'eccezione su una data non valida: un `mtime` strano mostra "—". */
function formatModified(ms: number | null): string {
  if (ms === null) return "—";
  const date = new Date(ms);
  return Number.isNaN(date.getTime()) ? "—" : dateFormat.format(date);
}

export function AddonTable({ installation }: Props) {
  const [load, setLoad] = useState<Load>({ state: "loading" });
  const [filters, setFilters] = useState<Filters>(defaultFilters);
  const [sort, setSort] = useState<Sort>(defaultSort);
  const [check, setCheck] = useState<Check>({ state: "idle" });

  const runCheck = useCallback(
    (cancelled: () => boolean = () => false) => {
      setCheck({ state: "checking" });
      api
        .checkUpdates(installation.id)
        .then((updates) => {
          updatesCache.set(installation.id, updates);
          if (!cancelled()) setCheck({ state: "done", updates: byFolder(updates) });
        })
        .catch((err) => {
          if (!cancelled()) setCheck({ state: "error", message: errorMessage(err) });
        });
    },
    [installation.id],
  );

  // Primo accesso nella sessione: controllo automatico; poi solo con il pulsante.
  useEffect(() => {
    const cached = updatesCache.get(installation.id);
    if (cached) {
      setCheck({ state: "done", updates: byFolder(cached) });
      return;
    }
    let current = true;
    runCheck(() => !current);
    return () => {
      current = false;
    };
  }, [installation.id, runCheck]);

  useEffect(() => {
    // Cambiando installazione in fretta, una risposta vecchia non deve sovrascrivere la nuova.
    let current = true;
    setLoad({ state: "loading" });
    setFilters(defaultFilters);
    api
      .listAddons(installation.id)
      .then((addons) => current && setLoad({ state: "ready", addons }))
      .catch((err) => current && setLoad({ state: "error", message: errorMessage(err) }));
    return () => {
      current = false;
    };
  }, [installation.id]);

  const addons = load.state === "ready" ? load.addons : [];
  const authors = useMemo(() => authorsOf(addons), [addons]);
  const updates = check.state === "done" ? check.updates : undefined;
  const visible = useMemo(
    () => visibleAddons(addons, filters, sort, updates),
    [addons, filters, sort, updates],
  );
  const updateCount = updates
    ? [...updates.values()].filter((u) => u.status === "updateAvailable").length
    : 0;

  function update(patch: Partial<Filters>) {
    setFilters((previous) => ({ ...previous, ...patch }));
  }

  function header(key: SortKey, label: string) {
    const active = sort.key === key;
    const ariaSort = active ? (sort.direction === "asc" ? "ascending" : "descending") : "none";
    return (
      <th aria-sort={ariaSort}>
        <button className="sort" onClick={() => setSort(nextSort(sort, key))}>
          {label}
          <span className="sort-mark" aria-hidden="true">
            {active ? (sort.direction === "asc" ? "↑" : "↓") : ""}
          </span>
        </button>
      </th>
    );
  }

  return (
    <section className="addons" aria-labelledby="addons-title">
      <header className="section-header">
        <h2 id="addons-title">Addon · {installation.name}</h2>
        <div className="section-actions">
          {load.state === "ready" && (
            <span className="muted count">
              {visible.length} di {addons.length} addon
              {updateCount > 0 && ` · ${updateCount} da aggiornare`}
            </span>
          )}
          <button
            className="button secondary"
            onClick={() => runCheck()}
            disabled={check.state === "checking"}
          >
            {check.state === "checking" ? "Controllo in corso…" : "Controlla aggiornamenti"}
          </button>
        </div>
      </header>

      {check.state === "error" && (
        <p className="message error" role="alert">
          {check.message}
        </p>
      )}

      {load.state === "loading" && <p className="muted">Lettura degli addon…</p>}
      {load.state === "error" && (
        <p className="message error" role="alert">
          {load.message}
        </p>
      )}
      {load.state === "ready" && addons.length === 0 && (
        <p className="empty">
          Nessun addon in questa installazione: la cartella <code>Interface/AddOns</code> è vuota
          o non esiste ancora.
        </p>
      )}

      {load.state === "ready" && addons.length > 0 && (
        <>
          <div className="toolbar">
            <input
              className="search"
              type="search"
              placeholder="Cerca per titolo, cartella, autore o note"
              aria-label="Cerca addon"
              value={filters.query}
              onChange={(event) => update({ query: event.target.value })}
            />
            <select
              className="select"
              aria-label="Filtra per autore"
              value={filters.author ?? ""}
              onChange={(event) => update({ author: event.target.value || null })}
            >
              <option value="">Tutti gli autori</option>
              {authors.map((author) => (
                <option key={author} value={author}>
                  {author}
                </option>
              ))}
            </select>
            <label className="toggle">
              <input
                type="checkbox"
                checked={filters.hideLoadOnDemand}
                onChange={(event) => update({ hideLoadOnDemand: event.target.checked })}
              />
              Nascondi moduli LoadOnDemand
            </label>
            <label className="toggle">
              <input
                type="checkbox"
                checked={filters.onlyErrors}
                onChange={(event) => update({ onlyErrors: event.target.checked })}
              />
              Solo con errori
            </label>
            <label className="toggle">
              <input
                type="checkbox"
                checked={filters.onlyUpdates}
                disabled={!updates}
                onChange={(event) => update({ onlyUpdates: event.target.checked })}
              />
              Solo con aggiornamenti
            </label>
          </div>

          {visible.length === 0 ? (
            <p className="empty">Nessun addon corrisponde alla ricerca o ai filtri.</p>
          ) : (
            <div className="table-wrap">
              <table className="addon-table">
                <thead>
                  <tr>
                    {header("title", "Titolo")}
                    <th>Cartella</th>
                    <th>Versione</th>
                    {header("author", "Autore")}
                    <th>Interface</th>
                    {header("modified", "Modificato")}
                    <th>CurseForge</th>
                    <th>Aggiornamento</th>
                  </tr>
                </thead>
                <tbody>
                  {visible.map((addon) => (
                    <tr key={addon.folder} className={addon.error ? "has-error" : undefined}>
                      <td>
                        <span className="addon-title" title={addon.notes ?? undefined}>
                          {displayTitle(addon)}
                        </span>
                        {addon.error && <span className="addon-error">{addon.error}</span>}
                      </td>
                      <td className="mono">{addon.folder}</td>
                      <td>{addon.version ?? "—"}</td>
                      <td>{addon.author ?? "—"}</td>
                      <td className="mono">
                        {addon.interface.length > 0 ? addon.interface.join(", ") : "—"}
                      </td>
                      <td className="nowrap">{formatModified(addon.modifiedMs)}</td>
                      <ProjectCell update={updates?.get(addon.folder)} />
                      <UpdateCell
                        update={updates?.get(addon.folder)}
                        checking={check.state === "checking"}
                      />
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </>
      )}
    </section>
  );
}

function ProjectCell({ update }: { update?: AddonUpdate }) {
  const project = update?.project;
  if (!project) return <td className="muted">—</td>;
  return (
    <td>
      {project.url ? (
        <button className="link inline" onClick={() => openUrl(project.url!)}>
          {project.name || `#${project.id}`}
        </button>
      ) : (
        project.name || `#${project.id}`
      )}
      {!project.allowModDistribution && (
        <span
          className="hint"
          title="L'autore non permette il download da app di terzi: andrà aggiornato dal sito."
        >
          solo dal sito
        </span>
      )}
    </td>
  );
}

function UpdateCell({ update, checking }: { update?: AddonUpdate; checking: boolean }) {
  if (!update) return <td className="muted">{checking ? "…" : ""}</td>;
  switch (update.status) {
    case "updateAvailable":
      return (
        <td className="update-available" title={update.latest.date}>
          {update.latest.name}
        </td>
      );
    case "upToDate":
      return <td className="muted">Aggiornato</td>;
    case "error":
      return <td className="error">{update.message}</td>;
    case "notMatched":
      return <td />;
  }
}
