import { useCallback, useEffect, useMemo, useRef, useState } from "react";
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
  groupAddons,
  groupUpdate,
  nextSort,
  visibleGroups,
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
/** Installazioni con un controllo automatico già partito, per non chiamare CurseForge due volte. */
const autoChecked = new Set<string>();
/** L'ultimo errore del controllo per installazione, per mostrarlo anche tornandoci. */
const errorCache = new Map<string, string>();

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
  // L'ultimo risultato valido resta visibile anche durante un nuovo controllo, così la tabella
  // e il filtro "Solo con aggiornamenti" non si svuotano mentre si aspetta.
  const [updates, setUpdates] = useState<Map<string, AddonUpdate> | undefined>();
  const [checking, setChecking] = useState(false);
  const [checkError, setCheckError] = useState<string | null>(null);
  // Il componente non si rimonta cambiando installazione: ogni risposta, automatica o dal
  // pulsante, si applica solo se l'installazione è ancora quella per cui è partita.
  const currentId = useRef(installation.id);
  currentId.current = installation.id;

  const runCheck = useCallback(() => {
    const id = installation.id;
    setChecking(true);
    setCheckError(null);
    api
      .checkUpdates(id)
      .then((result) => {
        updatesCache.set(id, result);
        errorCache.delete(id);
        if (currentId.current !== id) return;
        setUpdates(byFolder(result));
        setChecking(false);
      })
      .catch((err) => {
        errorCache.set(id, errorMessage(err));
        if (currentId.current !== id) return;
        setCheckError(errorMessage(err));
        setChecking(false);
        // Senza nessun risultato il filtro non ha dati su cui agire: lo si toglie.
        if (!updatesCache.has(id)) setFilters((f) => ({ ...f, onlyUpdates: false }));
      });
  }, [installation.id]);

  // Primo accesso nella sessione: controllo automatico; poi solo con il pulsante.
  useEffect(() => {
    const cached = updatesCache.get(installation.id);
    setUpdates(cached ? byFolder(cached) : undefined);
    setChecking(false);
    setCheckError(errorCache.get(installation.id) ?? null);
    if (!cached && !autoChecked.has(installation.id)) {
      autoChecked.add(installation.id);
      runCheck();
    }
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
  // Un addon fatto di più cartelle è una riga sola: conteggi e filtri contano i gruppi.
  const groups = useMemo(() => groupAddons(addons, updates), [addons, updates]);
  const authors = useMemo(() => authorsOf(groups.map((g) => g.main)), [groups]);
  const visible = useMemo(
    () => visibleGroups(addons, filters, sort, updates),
    [addons, filters, sort, updates],
  );
  const updateCount = groups.filter(
    (g) => groupUpdate(g, updates)?.status === "updateAvailable",
  ).length;

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
              {visible.length} di {groups.length} addon
              {updateCount > 0 && ` · ${updateCount} da aggiornare`}
            </span>
          )}
          <button
            className="button secondary"
            onClick={() => runCheck()}
            disabled={checking}
          >
            {checking ? "Controllo in corso…" : "Controlla aggiornamenti"}
          </button>
        </div>
      </header>

      {checkError && (
        <p className="message error" role="alert">
          {checkError}
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
                  {visible.map(({ main: addon, others }) => {
                    const error = addon.error ?? others.find((o) => o.error)?.error ?? null;
                    const update = groupUpdate({ main: addon, others }, updates);
                    return (
                      <tr key={addon.folder} className={error ? "has-error" : undefined}>
                        <td>
                          <span className="addon-title" title={addon.notes ?? undefined}>
                            {displayTitle(addon)}
                          </span>
                          {error && <span className="addon-error">{error}</span>}
                        </td>
                        <td className="mono">
                          {addon.folder}
                          {others.length > 0 && (
                            <span
                              className="more-folders"
                              title={[addon.folder, ...others.map((o) => o.folder)].join("\n")}
                            >
                              +{others.length}
                            </span>
                          )}
                        </td>
                        <td>{addon.version ?? "—"}</td>
                        <td>{addon.author ?? "—"}</td>
                        <td className="mono">
                          {addon.interface.length > 0 ? addon.interface.join(", ") : "—"}
                        </td>
                        <td className="nowrap">{formatModified(addon.modifiedMs)}</td>
                        <ProjectCell update={update} />
                        <UpdateCell update={update} checking={checking} />
                      </tr>
                    );
                  })}
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
