import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  api,
  errorMessage,
  type CatalogAddon,
  type CatalogCategory,
  type CatalogSearch,
  type CatalogSort,
  type Installation,
  type VersionType,
} from "../api";
import { AddByLink } from "./AddByLink";
import { AddonDetailView } from "./AddonDetail";
import { CatalogTable } from "./CatalogTable";
import { InstallAction, InstallResult, type InstallRun } from "./InstallAction";
import { useInstallProgress } from "../addons/ProgressBar";
import {
  noRunProgress,
  progressPercent,
  runLabel,
  withRunProgress,
  withoutRun,
  type RunProgress,
} from "../addons/progress";
import { projectId } from "../addons/sources";
import { installedIds, summarizeInstall, withInstalled } from "./install";
import { forgetFinishedRuns, forgetInstalled, installKey } from "./stale";
import {
  MIRROR_CHECKS_AT_ONCE,
  blockedHint,
  detailNote,
  isBlocked,
  mirrorCheckFor,
  mirrorStep,
  mirrorsToCheck,
  type MirrorCheck,
} from "./mirror";
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
  hiddenOnlyMessage,
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
  /** L'opzione della Beta (issue #141): quando cambia, la ricerca riparte. Il backend la legge
   * da sé; qui serve solo a sapere quando i risultati non valgono più. */
  betaWhenNoRelease?: boolean | null;
  /** Un'installazione ha messo almeno un progetto nell'installazione (la tab Installed). */
  onInstalled?: () => void;
  /** Le installazioni i cui addon sono cambiati altrove (tab Installed, zip, importazione):
   * quando la tab è visibile, i loro risultati si rileggono. */
  stale?: ReadonlySet<string>;
  /** Le installazioni di `stale` sono state rilette (o lo saranno alla prossima ricerca). */
  onRefreshed?: (installationIds: ReadonlySet<string>) => void;
};

/** I risultati: quelli mostrati restano visibili mentre arriva una ricerca nuova. */
type Load = {
  results: Results | null;
  loading: boolean;
  error: string | null;
};

/** La versione del gioco scelta vale per l'installazione per cui è stata scelta. */
type VersionChoice = { installationId: string; versionType: number | null };

/** L'addon aperto nel dettaglio vale per l'installazione in cui è stato aperto. */
type Selection = { installationId: string; addon: CatalogAddon };

export function Search({
  installation,
  active,
  keyConfigured,
  betaWhenNoRelease,
  onInstalled,
  stale,
  onRefreshed,
}: Props) {
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
  // Cambia quando gli addon dell'installazione sono cambiati: ricerca e dettaglio si rileggono
  // con gli stessi filtri, lasciando in vista quello che c'è.
  const [refresh, setRefresh] = useState(0);
  const tracker = useRef(new RequestTracker());
  // L'installazione dei risultati in `load`: dopo un cambio restano in vista quelli di prima
  // finché non arriva la risposta nuova.
  const resultsOf = useRef<string | null>(null);

  const [categories, setCategories] = useState<CatalogCategory[]>([]);
  const [categoriesError, setCategoriesError] = useState<string | null>(null);
  const [versionTypes, setVersionTypes] = useState<VersionType[]>([]);

  // Il dettaglio prende il posto dei risultati, che restano montati e nascosti: ricerca,
  // filtri e pagine caricate non cambiano, e lo scorrimento si ripristina al ritorno.
  const [selection, setSelection] = useState<Selection | null>(null);
  // L'ultima riga aperta resta segnata dopo il ritorno, e riceve il focus.
  const [lastSelectedId, setLastSelectedId] = useState<number | null>(null);
  // Un'altra installazione ha altri risultati: il dettaglio aperto nella precedente si chiude
  // e non torna quando si torna a quella.
  if (selection && selection.installationId !== installation.id) {
    setSelection(null);
    setLastSelectedId(null);
  }
  const selected = selection?.installationId === installation.id ? selection.addon : null;
  const resultsRef = useRef<HTMLDivElement>(null);
  const savedScroll = useRef(0);
  const shown = useRef({ open: false, installationId: installation.id });

  const missingKey = keyConfigured === false;

  // Le installazioni avviate nella sessione e i progetti installati dopo la risposta della
  // ricerca, che il segno "installato" della risposta non conosce ancora.
  const [runs, setRuns] = useState<Map<string, InstallRun>>(new Map());
  const [installedKeys, setInstalledKeys] = useState<Set<string>>(new Set());
  // L'ultima installazione avviata: il suo esito si mostra sopra i risultati.
  const [lastInstall, setLastInstall] = useState<string | null>(null);
  const [pageError, setPageError] = useState<string | null>(null);
  const installedHere = useMemo(() => {
    const prefix = `${installation.id}:`;
    const ids = new Set<number>();
    for (const key of installedKeys) {
      if (key.startsWith(prefix)) ids.add(Number(key.slice(prefix.length)));
    }
    return ids;
  }, [installedKeys, installation.id]);

  // L'avanzamento delle installazioni in corso (issue #128), dipendenze comprese.
  // Le installazioni in corso stanno anche in un ref, aggiornato subito: il primo evento può
  // arrivare prima che lo stato `runs` sia riletto.
  const [runProgress, setRunProgress] = useState<RunProgress>(noRunProgress);
  const runningRuns = useRef(new Map<string, Set<string>>());
  useInstallProgress(installation.id, (event) => {
    const running = runningRuns.current.get(installation.id) ?? new Set<string>();
    setRunProgress((previous) => withRunProgress(previous, event, installation.id, running));
  });

  // Il mirror GitHub dei progetti bloccati (issue #165), per installazione e progetto: chiesto
  // da solo per i bloccati visibili con un repository GitHub (issue #223, al massimo
  // `MIRROR_CHECKS_AT_ONCE` alla volta), dal dettaglio o da Install, e ricordato per la sessione.
  // `checking` evita due richieste per lo stesso progetto; `mirrorGen` scarta le risposte
  // arrivate dopo che gli esiti sono stati dimenticati. `autoRunning` conta i controlli
  // automatici ancora senza risposta, anche di una generazione vecchia; `autoSettled` cambia a
  // ogni risposta e fa partire i successivi.
  const [mirrors, setMirrors] = useState<Map<string, MirrorCheck>>(new Map());
  const checking = useRef(new Set<string>());
  const mirrorGen = useRef(0);
  const autoRunning = useRef(0);
  const [autoSettled, setAutoSettled] = useState(0);
  /** L'esito che vale per `addon` in questa installazione (un bloccato senza repository è già
   *  "website only"). */
  const mirrorOf = (addon: CatalogAddon) =>
    mirrorCheckFor(addon, mirrors.get(installKey(installation.id, addon.id)));

  // L'opzione della Beta cambia il file che l'app installerebbe, e con lui il mirror: gli esiti,
  // le richieste in corso e i motivi già mostrati non valgono più. Una risposta arrivata dopo
  // si scarta (`mirrorGen`), così un nuovo clic su Install fa subito una richiesta nuova.
  useEffect(() => {
    mirrorGen.current += 1;
    checking.current.clear();
    setMirrors((previous) => (previous.size === 0 ? previous : new Map()));
    setRuns((previous) => {
      if (![...previous.values()].some((run) => run.status === "unavailable")) return previous;
      return new Map([...previous].filter(([, run]) => run.status !== "unavailable"));
    });
  }, [betaWhenNoRelease]);

  function setRunFor(key: string, run: InstallRun) {
    setRuns((previous) => new Map(previous).set(key, run));
  }

  /** Con l'esito del controllo: installa, oppure dice perché il progetto resta sul sito. */
  function installChecked(addon: CatalogAddon, check: MirrorCheck) {
    const step = mirrorStep(check);
    if (step.kind === "install") {
      install(addon);
      return;
    }
    if (step.kind !== "website") return;
    const key = installKey(installation.id, addon.id);
    setRunFor(key, {
      status: "unavailable",
      name: addon.name || `#${addon.id}`,
      reason: step.reason,
      url: addon.url,
    });
    setLastInstall(key);
    setPageError(null);
  }

  /** Chiede a `github_mirror` il mirror di `addon`; con `installAfter` poi lo installa. `auto`:
   *  il controllo automatico della lista, contato in `autoRunning`. */
  function checkMirror(addon: CatalogAddon, installAfter: boolean, auto = false) {
    const id = installation.id;
    const key = installKey(id, addon.id);
    if (checking.current.has(key)) return;
    checking.current.add(key);
    if (auto) autoRunning.current += 1;
    const gen = mirrorGen.current;
    // Una risposta di una generazione vecchia non scrive niente e non installa: il file che ha
    // controllato non è più quello che `install_addon` sceglierebbe.
    const current = () => gen === mirrorGen.current;
    const setCheck = (check: MirrorCheck) =>
      setMirrors((previous) => new Map(previous).set(key, check));
    setCheck(installAfter ? { status: "checking", forInstall: true } : { status: "checking" });
    if (installAfter) {
      setLastInstall(key);
      setPageError(null);
      // L'esito di prima (un errore) non descrive questo tentativo.
      setRuns((previous) => {
        if (!previous.has(key)) return previous;
        const next = new Map(previous);
        next.delete(key);
        return next;
      });
    }
    api
      .githubMirror(id, addon.id)
      .then(
        (result) => {
          if (!current()) return;
          const check: MirrorCheck = { status: "done", result };
          setCheck(check);
          if (installAfter) installChecked(addon, check);
        },
        (err) => {
          if (!current()) return;
          const message = errorMessage(err);
          setCheck({ status: "failed", message });
          if (installAfter) {
            setRunFor(key, { status: "failed", message: `GitHub could not be checked: ${message}` });
          }
        },
      )
      .finally(() => {
        if (current()) checking.current.delete(key);
        if (auto) {
          autoRunning.current -= 1;
          setAutoSettled((n) => n + 1);
        }
      });
  }

  /** Install: subito per un progetto scaricabile, dal mirror GitHub per uno bloccato. */
  function requestInstall(addon: CatalogAddon) {
    if (!isBlocked(addon)) {
      install(addon);
      return;
    }
    const check = mirrorOf(addon);
    const step = mirrorStep(check);
    if (step.kind === "check") checkMirror(addon, true);
    else if (check && step.kind !== "wait") installChecked(addon, check);
  }

  function install(addon: CatalogAddon) {
    const id = installation.id;
    const key = installKey(id, addon.id);
    if (runs.get(key)?.status === "running") return;
    const setRun = (run: InstallRun) => setRuns((previous) => new Map(previous).set(key, run));
    setRun({ status: "running" });
    const run = curseforgeRun(addon.id);
    const running = runningRuns.current.get(id) ?? new Set<string>();
    runningRuns.current.set(id, running.add(run));
    setRunProgress((previous) => withoutRun(previous, run));
    setLastInstall(key);
    setPageError(null);
    api.installAddon(id, addon.id).then(
      (outcomes) => {
        const ids = installedIds(outcomes);
        if (ids.length > 0) {
          setInstalledKeys((previous) => {
            const next = new Set(previous);
            for (const modId of ids) next.add(installKey(id, modId));
            return next;
          });
          onInstalled?.();
        }
        setRun({ status: "done", summary: summarizeInstall(outcomes, addon) });
      },
      (err) => setRun({ status: "failed", message: errorMessage(err) }),
    )
      // Non c'è una fase finale: la barra si chiude quando il comando restituisce gli esiti.
      .finally(() => {
        running.delete(run);
        setRunProgress((previous) => withoutRun(previous, run));
      });
  }

  function openPage(url: string) {
    setPageError(null);
    openUrl(url).catch((err) => setPageError(`The page could not be opened: ${errorMessage(err)}`));
  }

  function runProgressOf(modId: number) {
    const run = curseforgeRun(modId);
    const event = runProgress.runs.get(run);
    return { value: progressPercent(event), label: runLabel(event, run) };
  }

  function renderInstall(variant: "row" | "detail") {
    return (addon: CatalogAddon) => (
      <InstallAction
        addon={addon}
        run={runs.get(installKey(installation.id, addon.id))}
        progress={runProgressOf(addon.id)}
        variant={variant}
        mirror={mirrorOf(addon)}
        onInstall={requestInstall}
        onOpenPage={openPage}
      />
    );
  }

  /** La frase del dettaglio per un addon bloccato: l'esito del mirror, o l'errore con un nuovo
   *  tentativo. */
  function renderBlockedNote(addon: CatalogAddon) {
    if (addon.installed) {
      return (
        <p className="hint">
          The author does not allow downloads from other apps: updates use GitHub when it has
          the same release.
        </p>
      );
    }
    const check = mirrorOf(addon);
    if (check?.status === "failed") {
      return (
        <p className="detail-meta error" role="alert">
          {detailNote(check)}{" "}
          <button className="link inline" onClick={() => checkMirror(addon, false)}>
            Try again
          </button>
        </p>
      );
    }
    return (
      <p className="hint" role="status">
        {detailNote(check)}
      </p>
    );
  }

  function openDetail(addon: CatalogAddon) {
    if (!selected) savedScroll.current = window.scrollY;
    setSelection({ installationId: installation.id, addon });
    setLastSelectedId(addon.id);
    setPageError(null);
  }

  // L'errore di un link aperto nel dettaglio non riguarda i risultati, e viceversa.
  const closeDetail = useCallback(() => {
    setSelection(null);
    setPageError(null);
  }, []);

  // Il dettaglio di un addon bloccato e non installato chiede subito il mirror GitHub, una
  // volta per sessione: Install e la frase sotto il nome lo mostrano (issue #165). L'esito già
  // chiesto dalla riga (anche in corso) vale anche qui; senza repository non si chiede niente.
  const detailBlocked =
    selected !== null &&
    isBlocked(selected) &&
    selected.githubRepo !== null &&
    !withInstalled(selected, installedHere).installed;
  useEffect(() => {
    if (!selected || !detailBlocked) return;
    if (mirrors.has(installKey(installation.id, selected.id))) return;
    checkMirror(selected, false);
    // Solo all'apertura (o quando l'addon diventa da controllare): un errore non riprova da sé.
  }, [selected?.id, detailBlocked, installation.id]);

  // Prima del disegno: il dettaglio parte dall'alto, i risultati tornano dove erano, con il
  // focus sulla riga da cui si era partiti.
  // Con un cambio di installazione non si ripristina niente: lo scorrimento salvato e la riga
  // appartengono ai risultati dell'installazione di prima.
  useLayoutEffect(() => {
    const open = selected !== null;
    const previous = shown.current;
    shown.current = { open, installationId: installation.id };
    if (previous.installationId !== installation.id || open === previous.open) return;
    if (open) {
      window.scrollTo(0, 0);
      return;
    }
    window.scrollTo(0, savedScroll.current);
    resultsRef.current
      ?.querySelector<HTMLElement>('tr[aria-selected="true"]')
      ?.focus({ preventScroll: true });
  }, [selected, installation.id]);

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
        resultsOf.current = installation.id;
        setLoad({ results: firstPage(page), loading: false, error: null });
      })
      .catch((err) => {
        if (!tracker.current.isLatest(id)) return;
        setLoad({ results: null, loading: false, error: errorMessage(err) });
      });
  }, [started, missingKey, installation.id, params, retry, refresh, betaWhenNoRelease]);

  // I bloccati visibili con un repository GitHub si controllano da soli (issue #223), nell'ordine
  // della lista e al massimo `MIRROR_CHECKS_AT_ONCE` alla volta. Solo con i risultati della
  // ricerca attuale: durante una ricerca nuova quelli in vista sono di prima. Quelli non ancora
  // partiti non sono una coda: cambiando ricerca o pagina si sceglie di nuovo tra le righe in
  // vista, e gli esiti già arrivati restano per la sessione.
  useEffect(() => {
    if (load.loading || !load.results || resultsOf.current !== installation.id) return;
    const slots = MIRROR_CHECKS_AT_ONCE - autoRunning.current;
    if (slots <= 0) return;
    const id = installation.id;
    const visible = load.results.addons.map((addon) => withInstalled(addon, installedHere));
    const known = (addon: CatalogAddon) => {
      const key = installKey(id, addon.id);
      return mirrors.has(key) || checking.current.has(key);
    };
    for (const addon of mirrorsToCheck(visible, known, slots)) checkMirror(addon, false, true);
  }, [load.loading, load.results, installedHere, mirrors, autoSettled, installation.id]);

  // Addon tolti, reinstallati o installati fuori dalla Search: il segno "installato" dei
  // risultati, i progetti installati nella sessione e gli esiti finiti non valgono più. Le
  // installazioni in corso restano. Le altre installazioni rileggono comunque la ricerca quando
  // si torna su di loro.
  useEffect(() => {
    if (!active || !stale || stale.size === 0) return;
    setInstalledKeys((previous) => forgetInstalled(previous, stale));
    setRuns((previous) => forgetFinishedRuns(previous, stale));
    // Prima ricerca mai partita (la tab si apre ora): l'effetto della ricerca la fa in questo
    // stesso passaggio, già con gli addon nuovi; un `refresh` ne farebbe una seconda uguale.
    const firstSearch = load.results === null && !load.loading && load.error === null;
    if (stale.has(installation.id) && !firstSearch) {
      setRefresh((n) => n + 1);
      // Il segno "installato" della riga è una copia di prima: il dettaglio lo riprende dal
      // `AddonDetail` riletto (`detail.installed || addon.installed` in AddonDetailView), che
      // per un addon ancora presente (Update, Reinstall) resta vero. Senza questo un addon
      // rimosso resterebbe "Installed" per via della copia.
      setSelection((previous) =>
        previous && stale.has(previous.installationId) && previous.addon.installed
          ? { ...previous, addon: { ...previous.addon, installed: false } }
          : previous,
      );
    }
    onRefreshed?.(stale);
  }, [active, stale, installation.id, onRefreshed]);

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

  // "Add by link" non usa CurseForge: resta anche senza la chiave. Il suo stato vale per
  // l'installazione, quindi si rimonta quando cambia.
  const addByLink = (
    <AddByLink
      key={installation.id}
      installationId={installation.id}
      onInstalled={() => {
        onInstalled?.();
        // Il progetto installato dal link può essere nei risultati.
        setRefresh((n) => n + 1);
      }}
    />
  );

  if (missingKey) {
    return (
      <section aria-label="Search">
        {addByLink}
        <p className="empty">
          Search needs a CurseForge API key. Set <code>CURSEFORGE_API_KEY</code> in the{" "}
          <code>.env</code> file and restart the app.
        </p>
      </section>
    );
  }

  const options = categoryOptions(categories);
  const results = load.results;
  const shownAddons = results?.addons.map((addon) => withInstalled(addon, installedHere)) ?? [];
  const lastRun =
    lastInstall?.startsWith(`${installation.id}:`) ? runs.get(lastInstall) : undefined;
  const selectedRun = selected ? runs.get(installKey(installation.id, selected.id)) : undefined;
  const sortLabel = SORTS.find((s) => s.id === params.sort)?.label ?? "";
  const ascending = params.sortOrder === "asc";

  return (
    <section aria-label="Search CurseForge">
      {selected && (
        <AddonDetailView
          installationId={installation.id}
          addon={withInstalled(selected, installedHere)}
          versionType={versionType}
          versionTypes={versionTypes}
          refresh={refresh}
          active={active}
          onBack={closeDetail}
          renderActions={renderInstall("detail")}
          blockedNote={renderBlockedNote}
        >
          {pageError && (
            <p className="message error" role="alert">
              {pageError}
            </p>
          )}
          {selectedRun && <InstallResult run={selectedRun} onOpenPage={openPage} />}
        </AddonDetailView>
      )}

      <div ref={resultsRef} hidden={selected !== null}>
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

        {addByLink}

        {pageError && (
          <p className="message error" role="alert">
            {pageError}
          </p>
        )}
        {lastRun && <InstallResult run={lastRun} onOpenPage={openPage} />}

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
            {(load.loading || results.addons.length > 0 || results.hidden > 0) && (
              <p className="count muted" role="status">
                {load.loading ? "Searching…" : resultCount(results)}
              </p>
            )}
            {results.addons.length === 0 ? (
              <p className="empty">
                {hiddenOnlyMessage(results) ?? (filtered ? (
                  <>
                    No add-ons match this search. Try another name or{" "}
                    <button className="link inline" onClick={clearFilters}>
                      clear the filters
                    </button>
                    .
                  </>
                ) : (
                  "No add-ons for this game version. Try another version in the menu above."
                ))}
              </p>
            ) : (
              <CatalogTable
                addons={shownAddons}
                onSelect={openDetail}
                selectedId={lastSelectedId}
                renderActions={renderInstall("row")}
                hintFor={(addon) => blockedHint(mirrorOf(addon))}
              />
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
      </div>
    </section>
  );
}

/** La chiave di un'installazione della Search negli eventi di avanzamento: un progetto CurseForge. */
function curseforgeRun(modId: number): string {
  return projectId({ source: "curseforge", key: String(modId) });
}
