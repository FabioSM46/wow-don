import { useEffect, useMemo, useRef, useState } from "react";
import {
  api,
  errorMessage,
  type CatalogCategory,
  type CatalogSearch,
  type CatalogSort,
  type Installation,
  type VersionType,
} from "../api";
import { CatalogTable } from "./CatalogTable";
import {
  RequestTracker,
  SEARCH_DELAY_MS,
  SORTS,
  appendPage,
  categoryOptions,
  defaultSearch,
  firstPage,
  hasMore,
  normalizeQuery,
  resultCount,
  withSort,
  type Results,
} from "./catalog";

type Props = {
  installation: Installation;
  /** La tab è visibile: la prima ricerca parte solo quando la si apre. */
  active: boolean;
  /** `false` quando la chiave CurseForge manca; `null` finché lo stato non è noto. */
  keyConfigured: boolean | null;
};

/** I risultati: quelli mostrati restano visibili mentre arriva una ricerca nuova. */
type Load = {
  results: Results | null;
  loading: boolean;
  error: string | null;
};

/** La versione del gioco scelta vale per l'installazione per cui è stata scelta. */
type VersionChoice = { installationId: string; versionType: number | null };

export function Search({ installation, active, keyConfigured }: Props) {
  const [started, setStarted] = useState(active);
  if (active && !started) setStarted(true);

  // Il testo della casella diventa ricerca dopo una pausa di scrittura o con Invio.
  const [text, setText] = useState("");
  const [search, setSearch] = useState<Omit<CatalogSearch, "versionType">>(defaultSearch);
  const [versionChoice, setVersionChoice] = useState<VersionChoice | null>(null);
  const versionType =
    versionChoice?.installationId === installation.id ? versionChoice.versionType : null;
  const params = useMemo<CatalogSearch>(() => ({ ...search, versionType }), [search, versionType]);

  const [load, setLoad] = useState<Load>({ results: null, loading: false, error: null });
  const [loadingMore, setLoadingMore] = useState(false);
  const [moreError, setMoreError] = useState<string | null>(null);
  const [retry, setRetry] = useState(0);
  const tracker = useRef(new RequestTracker());

  const [categories, setCategories] = useState<CatalogCategory[]>([]);
  const [categoriesError, setCategoriesError] = useState<string | null>(null);
  const [versionTypes, setVersionTypes] = useState<VersionType[]>([]);

  const missingKey = keyConfigured === false;

  // Categorie e versioni del gioco non cambiano durante la sessione: una richiesta sola.
  useEffect(() => {
    if (!started || missingKey) return;
    api
      .listCategories()
      .then(setCategories)
      .catch((err) => setCategoriesError(errorMessage(err)));
    api
      .listVersionTypes()
      .then(setVersionTypes)
      .catch(() => setVersionTypes([]));
  }, [started, missingKey]);

  useEffect(() => {
    const query = normalizeQuery(text);
    const timer = setTimeout(
      () => setSearch((previous) => (previous.query === query ? previous : { ...previous, query })),
      SEARCH_DELAY_MS,
    );
    return () => clearTimeout(timer);
  }, [text]);

  // Una ricerca nuova riparte dalla prima pagina; solo l'ultima risposta si applica.
  useEffect(() => {
    if (!started || missingKey) return;
    const id = tracker.current.next();
    setLoad((previous) => ({ ...previous, loading: true, error: null }));
    setLoadingMore(false);
    setMoreError(null);
    api
      .searchCatalog(installation.id, params, 0)
      .then((page) => {
        if (!tracker.current.isLatest(id)) return;
        setLoad({ results: firstPage(page), loading: false, error: null });
      })
      .catch((err) => {
        if (!tracker.current.isLatest(id)) return;
        setLoad({ results: null, loading: false, error: errorMessage(err) });
      });
  }, [started, missingKey, installation.id, params, retry]);

  function loadMore() {
    const results = load.results;
    if (!results || !hasMore(results) || loadingMore) return;
    const id = tracker.current.next();
    setLoadingMore(true);
    setMoreError(null);
    api
      .searchCatalog(installation.id, params, results.nextIndex)
      .then((page) => {
        if (!tracker.current.isLatest(id)) return;
        setLoad((previous) =>
          previous.results ? { ...previous, results: appendPage(previous.results, page) } : previous,
        );
        setLoadingMore(false);
      })
      .catch((err) => {
        if (!tracker.current.isLatest(id)) return;
        setMoreError(errorMessage(err));
        setLoadingMore(false);
      });
  }

  function submitNow() {
    const query = normalizeQuery(text);
    setSearch((previous) => (previous.query === query ? previous : { ...previous, query }));
  }

  const filtered = params.query !== "" || params.categoryId !== null;

  function clearFilters() {
    setText("");
    setSearch((previous) => ({ ...previous, query: "", categoryId: null }));
  }

  if (missingKey) {
    return (
      <p className="empty">
        Search needs a CurseForge API key. Set <code>CURSEFORGE_API_KEY</code> in the{" "}
        <code>.env</code> file and restart the app.
      </p>
    );
  }

  const options = categoryOptions(categories);
  const results = load.results;
  const sortLabel = SORTS.find((s) => s.id === params.sort)?.label ?? "";
  const ascending = params.sortOrder === "asc";

  return (
    <section aria-label="Search CurseForge">
      <div className="toolbar">
        <input
          className="search"
          type="search"
          placeholder="Search CurseForge add-ons…"
          aria-label="Search CurseForge add-ons"
          value={text}
          onChange={(event) => setText(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "Enter") submitNow();
          }}
        />
        <select
          className="select"
          aria-label="Category"
          title={categoriesError ?? undefined}
          value={params.categoryId ?? ""}
          onChange={(event) =>
            setSearch((previous) => ({
              ...previous,
              categoryId: event.target.value === "" ? null : Number(event.target.value),
            }))
          }
        >
          <option value="">All categories</option>
          {options.map((option) => (
            <option key={option.id} value={option.id}>
              {option.depth === 1 ? `   ${option.name}` : option.name}
            </option>
          ))}
        </select>
        <select
          className="select"
          aria-label="Game version"
          value={versionType ?? ""}
          onChange={(event) =>
            setVersionChoice({
              installationId: installation.id,
              versionType: event.target.value === "" ? null : Number(event.target.value),
            })
          }
        >
          <option value="">Installation version</option>
          {versionTypes.map((type) => (
            <option key={type.id} value={type.id}>
              {type.name}
            </option>
          ))}
        </select>
        <div className="sort-control">
          <select
            className="select"
            aria-label="Sort by"
            value={params.sort}
            onChange={(event) =>
              setSearch((previous) => withSort(previous, event.target.value as CatalogSort))
            }
          >
            {SORTS.map((sort) => (
              <option key={sort.id} value={sort.id}>
                {sort.label}
              </option>
            ))}
          </select>
          <button
            className="direction"
            aria-label={`${sortLabel}, ${ascending ? "ascending" : "descending"}. Reverse order`}
            title={ascending ? "Ascending" : "Descending"}
            onClick={() =>
              setSearch((previous) => ({
                ...previous,
                sortOrder: previous.sortOrder === "asc" ? "desc" : "asc",
              }))
            }
          >
            {ascending ? "↑" : "↓"}
          </button>
        </div>
      </div>

      {categoriesError && (
        <p className="message hint" role="status">
          Categories are unavailable: {categoriesError}
        </p>
      )}

      {load.error && (
        <p className="message error" role="alert">
          {load.error}{" "}
          <button className="link inline" onClick={() => setRetry((n) => n + 1)}>
            Try again
          </button>
        </p>
      )}

      {!results && !load.error && <p className="muted">Searching…</p>}

      {results && (
        <div className={load.loading ? "results busy" : "results"} aria-busy={load.loading}>
          {(load.loading || results.addons.length > 0) && (
            <p className="count muted" role="status">
              {load.loading ? "Searching…" : resultCount(results)}
            </p>
          )}
          {results.addons.length === 0 ? (
            <p className="empty">
              {filtered ? (
                <>
                  No add-ons match this search. Try another name or{" "}
                  <button className="link inline" onClick={clearFilters}>
                    clear the filters
                  </button>
                  .
                </>
              ) : (
                "No add-ons for this game version. Try another version in the menu above."
              )}
            </p>
          ) : (
            <CatalogTable addons={results.addons} />
          )}
          {hasMore(results) && (
            <div className="load-more">
              {moreError && (
                <span className="error" role="alert">
                  {moreError}
                </span>
              )}
              <button
                className="button secondary"
                onClick={loadMore}
                disabled={loadingMore || load.loading}
              >
                {loadingMore ? "Loading…" : moreError ? "Try again" : "Load more"}
              </button>
            </div>
          )}
        </div>
      )}
    </section>
  );
}
