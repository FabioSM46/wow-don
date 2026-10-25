import { Fragment, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  api,
  errorMessage,
  type AddonUpdate,
  type InstalledAddon,
  type Installation,
  type UpdateOutcome,
  type VersionType,
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
import { SOURCE_LABELS, projectId, sourcesIn, updateRequest, type ProjectRef } from "./sources";

type Props = {
  installation: Installation;
  /** Cambia quando gli addon sono stati installati altrove nell'app: la lista si rilegge. */
  reload?: number;
  /** Apre il selettore di uno zip da installare (D32); senza, il pulsante non compare. */
  onInstallZip?: () => void;
  /** Un'installazione da zip è in corso. */
  zipBusy?: boolean;
};

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
/**
 * Progetti in aggiornamento per installazione. Fuori dal componente come le cache sopra:
 * tornando su un'installazione con un aggiornamento ancora in corso, le righe restano bloccate
 * e non parte un secondo aggiornamento dello stesso progetto.
 */
const updateRuns = new Map<string, Set<string>>();
/** L'esito dell'ultimo aggiornamento per installazione, mostrato sopra la tabella. */
const updateResults = new Map<string, UpdateResult>();

type UpdateResult = { outcomes: UpdateOutcome[] } | { error: string };

/**
 * I tipi di versione del gioco non cambiano durante la sessione e il comando non usa la rete:
 * una sola richiesta, condivisa da tutte le installazioni.
 */
let versionTypesRequest: Promise<VersionType[]> | null = null;
function loadVersionTypes(): Promise<VersionType[]> {
  versionTypesRequest ??= api.listVersionTypes().catch((err) => {
    // Un errore non resta in cache: il prossimo montaggio riprova.
    versionTypesRequest = null;
    throw err;
  });
  return versionTypesRequest;
}

function byFolder(updates: AddonUpdate[]): Map<string, AddonUpdate> {
  return new Map(updates.map((update) => [update.folder, update]));
}

type Load =
  | { state: "loading" }
  | { state: "error"; message: string }
  | { state: "ready"; addons: InstalledAddon[] };

const dateFormat = new Intl.DateTimeFormat("en", { dateStyle: "medium" });

/** `format` lancia un'eccezione su una data non valida: un `mtime` strano mostra "—". */
function formatModified(ms: number | null): string {
  if (ms === null) return "—";
  const date = new Date(ms);
  return Number.isNaN(date.getTime()) ? "—" : dateFormat.format(date);
}

export function AddonTable({ installation, reload = 0, onInstallZip, zipBusy = false }: Props) {
  const [load, setLoad] = useState<Load>({ state: "loading" });
  const [filters, setFilters] = useState<Filters>(defaultFilters);
  const [sort, setSort] = useState<Sort>(defaultSort);
  // Gruppi aperti (per cartella principale): le altre cartelle compaiono sotto la riga.
  const [expanded, setExpanded] = useState<Set<string>>(new Set());

  function toggle(folder: string) {
    setExpanded((previous) => {
      const next = new Set(previous);
      if (next.has(folder)) next.delete(folder);
      else next.add(folder);
      return next;
    });
  }
  // L'ultimo risultato valido resta visibile anche durante un nuovo controllo, così la tabella
  // e il filtro "Solo con aggiornamenti" non si svuotano mentre si aspetta.
  const [updates, setUpdates] = useState<Map<string, AddonUpdate> | undefined>();
  const [checking, setChecking] = useState(false);
  const [checkError, setCheckError] = useState<string | null>(null);
  // Il componente non si rimonta cambiando installazione: ogni risposta, automatica o dal
  // pulsante, si applica solo se l'installazione è ancora quella per cui è partita.
  const currentId = useRef(installation.id);
  currentId.current = installation.id;

  const runCheck = useCallback((): Promise<void> => {
    const id = installation.id;
    setChecking(true);
    setCheckError(null);
    return api
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
      void runCheck();
    }
  }, [installation.id, runCheck]);

  // Rilegge `Interface/AddOns` senza svuotare la tabella né perdere filtri e righe aperte:
  // serve dopo aver installato o tolto addon fuori dall'app.
  const [rescanning, setRescanning] = useState(false);
  const [rescanError, setRescanError] = useState<string | null>(null);
  function rescan(): Promise<void> {
    const id = installation.id;
    setRescanning(true);
    setRescanError(null);
    return api
      .listAddons(id)
      .then((addons) => {
        if (currentId.current !== id) return;
        setLoad({ state: "ready", addons });
        // Gli stati di aggiornamento si riferivano alla lista di prima: si ricontrolla.
        return runCheck();
      })
      .catch((err) => {
        if (currentId.current !== id) return;
        // Un errore di lettura non toglie la tabella già mostrata: lo si segnala sopra.
        setLoad((previous) =>
          previous.state === "ready" ? previous : { state: "error", message: errorMessage(err) },
        );
        setRescanError(errorMessage(err));
      })
      .finally(() => {
        if (currentId.current === id) setRescanning(false);
      });
  }

  // Un'installazione fatta altrove (tab Search) si vede con la stessa rilettura del pulsante,
  // solo quando `reload` cambia.
  const seenReload = useRef(reload);
  useEffect(() => {
    if (seenReload.current === reload) return;
    seenReload.current = reload;
    void rescan();
  }, [reload]);

  // Progetti (`mod_id`) in aggiornamento: le loro righe mostrano "Updating…" e tutti i pulsanti
  // di aggiornamento restano disabilitati finché la lista e gli stati non sono riletti.
  const [updating, setUpdating] = useState<Set<string>>(new Set());
  const [updateResult, setUpdateResult] = useState<UpdateResult | null>(null);
  // `currentId` sopravvive allo smontaggio: serve un segnale a parte per non rileggere la lista
  // di un componente che non c'è più.
  const mounted = useRef(false);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  function updateProjects(projects: ProjectRef[]) {
    const id = installation.id;
    const ids = new Set(projects.map(projectId));
    if (ids.size === 0 || updateRuns.has(id)) return;
    updateRuns.set(id, ids);
    updateResults.delete(id);
    setUpdating(ids);
    setUpdateResult(null);
    const request = updateRequest(projects);
    api
      .updateAddons(id, request.modIds, request.projects)
      .then(
        (outcomes): UpdateResult => ({ outcomes }),
        (err): UpdateResult => ({ error: errorMessage(err) }),
      )
      .then((result) => {
        updateResults.set(id, result);
        // Gli stati di prima non valgono più: senza cache il prossimo controllo li rilegge.
        updatesCache.delete(id);
        errorCache.delete(id);
        // Con la cache vuota serve un nuovo controllo anche se ora si guarda altro: tornando
        // sulla tabella il controllo automatico ripartirà.
        autoChecked.delete(id);
        if (!mounted.current || currentId.current !== id) return;
        setUpdateResult(result);
        return rescan();
      })
      .finally(() => {
        updateRuns.delete(id);
        if (mounted.current && currentId.current === id) setUpdating(new Set());
      });
  }

  const [versionTypes, setVersionTypes] = useState<VersionType[]>([]);
  const [versionTypesError, setVersionTypesError] = useState<string | null>(null);
  useEffect(() => {
    let current = true;
    loadVersionTypes()
      .then((types) => current && setVersionTypes(types))
      .catch((err) => current && setVersionTypesError(errorMessage(err)));
    return () => {
      current = false;
    };
  }, []);

  // La versione del gioco che si sta salvando (progetto e scelta), ed errori per progetto.
  const [versionSaving, setVersionSaving] = useState<VersionChoice | null>(null);
  const [versionErrors, setVersionErrors] = useState<Map<number, string>>(new Map());

  function changeVersion(modId: number, versionType: number | null) {
    const id = installation.id;
    setVersionSaving({ modId, versionType });
    setVersionErrors((previous) => withoutKey(previous, modId));
    const request =
      versionType === null
        ? api.clearVersionOverride(id, modId)
        : api.setVersionOverride(id, modId, versionType);
    request
      .then(
        () => {
          // Gli stati di prima valgono per l'altra versione: come dopo un aggiornamento, la
          // cache si svuota e il controllo riparte (o ripartirà tornando sull'installazione).
          updatesCache.delete(id);
          errorCache.delete(id);
          autoChecked.delete(id);
          if (!mounted.current || currentId.current !== id) return;
          // L'etichetta cambia subito; lo stato dell'aggiornamento arriva dal controllo.
          setUpdates((previous) => withOverride(previous, modId, versionType));
          return runCheck();
        },
        (err) => {
          if (!mounted.current || currentId.current !== id) return;
          setVersionErrors((previous) => new Map(previous).set(modId, errorMessage(err)));
        },
      )
      .finally(() => {
        if (mounted.current && currentId.current === id) setVersionSaving(null);
      });
  }

  useEffect(() => {
    // Cambiando installazione in fretta, una risposta vecchia non deve sovrascrivere la nuova.
    let current = true;
    setVersionSaving(null);
    setVersionErrors(new Map());
    // Una rilettura partita per l'installazione precedente non vale più per questa.
    setRescanning(false);
    setRescanError(null);
    setUpdating(updateRuns.get(installation.id) ?? new Set());
    setUpdateResult(updateResults.get(installation.id) ?? null);
    setLoad({ state: "loading" });
    setFilters(defaultFilters);
    setExpanded(new Set());
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
    () => visibleGroups(groups, filters, sort, updates),
    [groups, filters, sort, updates],
  );
  const updateCount = groups.filter(
    (g) => groupUpdate(g, updates)?.status === "updateAvailable",
  ).length;
  // "Update all" conta i progetti che l'app può aggiornare da sola: uno per gruppo, senza
  // ripetizioni (fonte più id), così il numero sul pulsante è quello dei progetti inviati.
  const updatable = new Map<string, ProjectRef>();
  for (const g of groups) {
    const update = groupUpdate(g, updates);
    if (update && isUpdatable(update)) updatable.set(projectId(update.project!), update.project!);
  }
  const updatableIds = [...updatable.values()];
  const sources = useMemo(() => sourcesIn(updates), [updates]);
  // Una fonte scelta che non compare più nel controllo (addon tolto, nuovo controllo) nasconderebbe
  // tutto senza un menu per toglierla. Un controllo ancora vuoto non cancella la scelta.
  useEffect(() => {
    if (sources.length > 0 && filters.source !== null && !sources.includes(filters.source)) {
      setFilters((previous) => ({ ...previous, source: null }));
    }
  }, [sources, filters.source]);
  const busy = updating.size > 0;

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
        <h2 id="addons-title">Addons · {installation.name}</h2>
        <div className="section-actions">
          {load.state === "ready" && (
            <span className="muted count">
              {visible.length} of {groups.length} {groups.length === 1 ? "addon" : "addons"}
              {updateCount > 0 && ` · ${updateCount} to update`}
            </span>
          )}
          {onInstallZip && (
            <button
              className="button secondary"
              onClick={onInstallZip}
              disabled={zipBusy || busy}
              title="Install an addon zip downloaded from a website; you can also drop it into the window"
            >
              {zipBusy ? "Installing…" : "Install from zip…"}
            </button>
          )}
          <button
            className="button secondary"
            onClick={rescan}
            disabled={rescanning || busy || load.state === "loading"}
            title="Reads the Interface/AddOns folder again"
          >
            {rescanning ? "Reloading…" : "Reload addons"}
          </button>
          <button
            className="button secondary"
            onClick={() => void runCheck()}
            disabled={checking || busy}
          >
            {checking ? "Checking…" : "Check for updates"}
          </button>
          {(busy || updatableIds.length > 0) && (
            <button
              className="button"
              onClick={() => updateProjects(updatableIds)}
              disabled={busy || checking || versionSaving !== null}
            >
              {busy ? "Updating…" : `Update all (${updatableIds.length})`}
            </button>
          )}
        </div>
      </header>

      {checkError && (
        <p className="message error" role="alert">
          {checkError}
        </p>
      )}
      {rescanError && (
        <p className="message error" role="alert">
          {rescanError}
        </p>
      )}
      {updateResult && <UpdateSummary result={updateResult} />}
      {versionTypesError && (
        <p className="message error" role="alert">
          Could not load the game versions: {versionTypesError}
        </p>
      )}

      {load.state === "loading" && <p className="muted">Loading addons…</p>}
      {load.state === "error" && (
        <p className="message error" role="alert">
          {load.message}
        </p>
      )}
      {load.state === "ready" && addons.length === 0 && (
        <p className="empty">
          No addons in this installation: the <code>Interface/AddOns</code> folder is empty or
          does not exist yet.
        </p>
      )}

      {load.state === "ready" && addons.length > 0 && (
        <>
          <div className="toolbar">
            <input
              className="search"
              type="search"
              placeholder="Search by title, folder, author or notes"
              aria-label="Search addons"
              value={filters.query}
              onChange={(event) => update({ query: event.target.value })}
            />
            <select
              className="select"
              aria-label="Filter by author"
              value={filters.author ?? ""}
              onChange={(event) => update({ author: event.target.value || null })}
            >
              <option value="">All authors</option>
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
              Hide LoadOnDemand modules
            </label>
            {sources.length > 1 && (
              <select
                className="select"
                aria-label="Filter by source"
                value={filters.source ?? ""}
                onChange={(event) =>
                  update({ source: (event.target.value || null) as Filters["source"] })
                }
              >
                <option value="">All sources</option>
                {sources.map((source) => (
                  <option key={source} value={source}>
                    {SOURCE_LABELS[source]}
                  </option>
                ))}
              </select>
            )}
            <label className="toggle">
              <input
                type="checkbox"
                checked={filters.onlyErrors}
                onChange={(event) => update({ onlyErrors: event.target.checked })}
              />
              Only with errors
            </label>
            <label className="toggle">
              <input
                type="checkbox"
                checked={filters.onlyUpdates}
                disabled={!updates}
                onChange={(event) => update({ onlyUpdates: event.target.checked })}
              />
              Only with updates
            </label>
          </div>

          {visible.length === 0 ? (
            <p className="empty">No addons match the search or filters: clear them to see every addon.</p>
          ) : (
            <div className="table-wrap">
              <table className="addon-table">
                <thead>
                  <tr>
                    {header("title", "Title")}
                    <th>Folder</th>
                    <th>Version</th>
                    {header("author", "Author")}
                    <th>Interface</th>
                    {header("modified", "Modified")}
                    <th>CurseForge</th>
                    <th>Update</th>
                  </tr>
                </thead>
                <tbody>
                  {visible.map(({ main: addon, others }) => {
                    const error = addon.error ?? others.find((o) => o.error)?.error ?? null;
                    const update = groupUpdate({ main: addon, others }, updates);
                    const hasOthers = others.length > 0;
                    const open = hasOthers && expanded.has(addon.folder);
                    const classes = [
                      error ? "has-error" : "",
                      hasOthers ? "expandable" : "",
                      open ? "open" : "",
                    ].join(" ");
                    return (
                      <Fragment key={addon.folder}>
                        <tr
                          className={classes.trim() || undefined}
                          onClick={hasOthers ? () => toggle(addon.folder) : undefined}
                        >
                          <td>
                            {hasOthers ? (
                              <button
                                className="chevron"
                                aria-expanded={open}
                                aria-label={`${open ? "Hide" : "Show"} the folders of ${displayTitle(addon)}`}
                                onClick={(event) => {
                                  event.stopPropagation();
                                  toggle(addon.folder);
                                }}
                              >
                                {open ? "▾" : "▸"}
                              </button>
                            ) : (
                              <span className="chevron-space" />
                            )}
                            <span className="addon-title" title={addon.notes ?? undefined}>
                              {displayTitle(addon)}
                            </span>
                            {error && <span className="addon-error">{error}</span>}
                          </td>
                          <td className="mono">
                            {addon.folder}
                            {hasOthers && (
                              <span
                                className="more-folders"
                                aria-label={`and ${others.length} more ${others.length === 1 ? "folder" : "folders"}: ${others.map((o) => o.folder).join(", ")}`}
                              >
                                +{others.length}
                              </span>
                            )}
                          </td>
                          <td>{addon.version ?? "—"}</td>
                          <td>
                            <span className="clip" title={addon.author ?? undefined}>
                              {addon.author ?? "—"}
                            </span>
                          </td>
                          <td className="mono interface">
                            {addon.interface.length > 0 ? addon.interface.join(", ") : "—"}
                          </td>
                          <td className="nowrap">{formatModified(addon.modifiedMs)}</td>
                          <ProjectCell
                            update={update}
                            versionTypes={versionTypes}
                            saving={
                              versionSaving !== null &&
                              update?.project?.source === "curseforge" &&
                              versionSaving.modId === update.project.id
                                ? versionSaving
                                : null
                            }
                            versionError={
                              update?.project?.source === "curseforge"
                                ? versionErrors.get(update.project.id)
                                : undefined
                            }
                            locked={versionSaving !== null || busy || checking}
                            onVersionChange={changeVersion}
                          />
                          <UpdateCell
                            update={update}
                            checking={checking}
                            updating={Boolean(
                              update?.project && updating.has(projectId(update.project)),
                            )}
                            disabled={busy || checking || versionSaving !== null}
                            onUpdate={updateProjects}
                          />
                        </tr>
                        {open &&
                          others.map((other) => (
                            <tr
                              key={other.folder}
                              className={other.error ? "sub-row has-error" : "sub-row"}
                            >
                              <td>
                                <span className="sub-title" title={other.notes ?? undefined}>
                                  {displayTitle(other)}
                                </span>
                                {other.error && (
                                  <span className="addon-error">{other.error}</span>
                                )}
                              </td>
                              <td className="mono">{other.folder}</td>
                              <td>{other.version ?? "—"}</td>
                              <td>
                                <span className="clip" title={other.author ?? undefined}>
                                  {other.author ?? "—"}
                                </span>
                              </td>
                              <td className="mono interface">
                                {other.interface.length > 0 ? other.interface.join(", ") : "—"}
                              </td>
                              <td className="nowrap">{formatModified(other.modifiedMs)}</td>
                              <td />
                              <td />
                            </tr>
                          ))}
                      </Fragment>
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

function withoutKey<V>(map: Map<number, V>, key: number): Map<number, V> {
  if (!map.has(key)) return map;
  const next = new Map(map);
  next.delete(key);
  return next;
}

/** Gli stati con la nuova versione forzata per il progetto `modId`, prima del ricontrollo. */
function withOverride(
  updates: Map<string, AddonUpdate> | undefined,
  modId: number,
  versionOverride: number | null,
): Map<string, AddonUpdate> | undefined {
  if (!updates) return updates;
  return new Map(
    [...updates].map(([folder, update]) => [
      folder,
      update.project?.id === modId ? { ...update, versionOverride } : update,
    ]),
  );
}

function versionName(types: VersionType[], id: number): string {
  return types.find((type) => type.id === id)?.name ?? `#${id}`;
}

type VersionChoice = { modId: number; versionType: number | null };

type ProjectCellProps = {
  update?: AddonUpdate;
  versionTypes: VersionType[];
  /** La scelta in salvataggio per questo progetto, mostrata nel menu finché non finisce. */
  saving: VersionChoice | null;
  versionError?: string;
  /**
   * Un salvataggio, un controllo o un aggiornamento in corso: il menu ignora le scelte. Non è
   * `disabled` perché un controllo disabilitato perde il focus, e chi usa la tastiera
   * ricomincerebbe dall'inizio della pagina dopo ogni scelta.
   */
  locked: boolean;
  onVersionChange: (modId: number, versionType: number | null) => void;
};

function ProjectCell({
  update,
  versionTypes,
  saving,
  versionError,
  locked,
  onVersionChange,
}: ProjectCellProps) {
  const project = update?.project;
  if (!project) return <td className="muted">—</td>;
  const override = update?.versionOverride ?? null;
  const selected = saving ? saving.versionType : override;
  return (
    <td>
      {project.url ? (
        <button
          className="link inline"
          onClick={(event) => {
            // Il link non apre e chiude la riga del gruppo.
            event.stopPropagation();
            void openUrl(project.url!);
          }}
        >
          {project.name || `#${project.key}`}
        </button>
      ) : (
        project.name || `#${project.key}`
      )}
      {project.source !== "curseforge" && (
        <span className="hint" title="The source that updates this addon">
          {SOURCE_LABELS[project.source]}
        </span>
      )}
      {/* Le versioni forzate sono per progetto CurseForge (`mod_id`, D6). */}
      {versionTypes.length > 0 && project.source === "curseforge" && (
        <select
          className={`select compact version-select${saving || versionError ? " shown" : ""}`}
          aria-label={`Game version for ${project.name || `#${project.key}`}`}
          value={selected ?? ""}
          aria-disabled={locked}
          // Il menu non apre e chiude la riga del gruppo.
          onClick={(event) => event.stopPropagation()}
          onChange={(event) => {
            // Il menu è controllato: una scelta ignorata torna da sola al valore attuale.
            if (locked) return;
            onVersionChange(project.id, event.target.value ? Number(event.target.value) : null);
          }}
        >
          <option value="">Game version: Automatic</option>
          {versionTypes.map((type) => (
            <option key={type.id} value={type.id}>
              {type.name}
            </option>
          ))}
          {/* Una versione forzata che l'app non conosce più resta visibile come scelta attuale. */}
          {selected !== null && !versionTypes.some((type) => type.id === selected) && (
            <option value={selected}>{`#${selected}`}</option>
          )}
        </select>
      )}
      {!project.allowModDistribution && (
        <span
          className="hint"
          title="The author does not allow downloads from third-party apps: download it from the website, then drop the zip into this window."
        >
          website only
        </span>
      )}
      {override !== null && (
        <span className="hint" title="Updates for this addon use this game version">
          Forced: {versionName(versionTypes, override)}
        </span>
      )}
      {versionError && (
        <span className="addon-error" role="alert">
          Game version not changed: {versionError}
        </span>
      )}
    </td>
  );
}

/** Un aggiornamento che l'app può installare da sola: i progetti non distribuibili no (D13). */
function isUpdatable(update: AddonUpdate): boolean {
  return update.status === "updateAvailable" && Boolean(update.project?.allowModDistribution);
}

type UpdateCellProps = {
  update?: AddonUpdate;
  checking: boolean;
  /** Il progetto di questa riga è nell'aggiornamento in corso. */
  updating: boolean;
  disabled: boolean;
  onUpdate: (projects: ProjectRef[]) => void;
};

function UpdateCell({ update, checking, updating, disabled, onUpdate }: UpdateCellProps) {
  if (updating) return <td className="muted nowrap">Updating…</td>;
  if (!update) return <td className="muted">{checking ? "…" : ""}</td>;
  switch (update.status) {
    case "updateAvailable":
      return (
        <td title={installedTitle(update, update.latest.date)}>
          <span className="update-available">{update.latest.name}</span>
          {isUpdatable(update) && (
            <button
              className="button secondary small row-action"
              disabled={disabled}
              onClick={(event) => {
                // Il pulsante non apre e chiude la riga del gruppo.
                event.stopPropagation();
                onUpdate([update.project!]);
              }}
            >
              Update
            </button>
          )}
        </td>
      );
    case "upToDate":
      return (
        <td className="muted" title={installedTitle(update)}>
          Up to date
        </td>
      );
    case "error":
      return <td className="error">{update.message}</td>;
    case "notMatched":
      return <td />;
  }
}

/** Lo stato riguarda il progetto: il suggerimento dice quale file del progetto è installato. */
function installedTitle(update: AddonUpdate, latestDate?: string): string | undefined {
  const installed = update.installedFile ? `Installed: ${update.installedFile.name}` : undefined;
  const latest = latestDate ? `New file from ${latestDate.slice(0, 10)}` : undefined;
  return [installed, latest].filter(Boolean).join("\n") || undefined;
}

/** Riepilogo dell'ultimo aggiornamento, con il motivo di ogni progetto saltato o in errore. */
function UpdateSummary({ result }: { result: UpdateResult }) {
  if ("error" in result) {
    return (
      <p className="message error" role="alert">
        Update failed: {result.error}
      </p>
    );
  }
  const count = (status: UpdateOutcome["status"]) =>
    result.outcomes.filter((o) => o.status === status).length;
  const updated = count("updated");
  const skipped = count("skipped");
  const errors = count("error");
  const parts = [
    updated > 0 && `${updated} updated`,
    skipped > 0 && `${skipped} skipped`,
    errors > 0 && `${errors} ${errors === 1 ? "error" : "errors"}`,
  ].filter(Boolean);
  const details = result.outcomes.filter((o) => o.status !== "updated");
  return (
    <div className="message update-summary" role="status">
      <p>{parts.length > 0 ? parts.join(", ") : "Nothing was updated"}</p>
      {details.length > 0 && (
        <ul>
          {details.map((outcome) => (
            <li
              key={projectId(outcome)}
              className={outcome.status === "error" ? "error" : "muted"}
            >
              {outcome.name || `#${outcome.key}`}
              {outcome.message && `: ${outcome.message}`}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
