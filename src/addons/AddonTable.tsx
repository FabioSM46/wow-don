import { useEffect, useMemo, useState } from "react";
import { api, errorMessage, type InstalledAddon, type Installation } from "../api";
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
  const visible = useMemo(() => visibleAddons(addons, filters, sort), [addons, filters, sort]);

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
        {load.state === "ready" && (
          <span className="muted count">
            {visible.length} di {addons.length} addon
          </span>
        )}
      </header>

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
