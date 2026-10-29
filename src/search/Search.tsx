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
import { SearchIcon } from "./icons";
import { blockedPill } from "./pills";
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
  /** The tab is visible: the first search starts only when it is opened. */
  active: boolean;
  /** `false` when the CurseForge key is missing; `null` until the state is known. */
  keyConfigured: boolean | null;
  /** The Beta option (issue #141): when it changes, the search restarts. The backend reads it
   * on its own; here it only serves to know when the results are no longer valid. */
  betaWhenNoRelease?: boolean | null;
  /** An installation put at least one project into the installation (the Installed tab). */
  onInstalled?: () => void;
  /** The installations whose addons changed elsewhere (Installed tab, zip, import): when the
   * tab is visible, their results are reread. */
  stale?: ReadonlySet<string>;
  /** The installations in `stale` have been reread (or will be at the next search). */
  onRefreshed?: (installationIds: ReadonlySet<string>) => void;
};

/** The results: the ones shown stay visible while a new search arrives. */
type Load = {
  results: Results | null;
  loading: boolean;
  error: string | null;
};

/** The chosen game version applies to the installation it was chosen for. */
type VersionChoice = { installationId: string; versionType: number | null };

/** The addon open in the detail applies to the installation it was opened in. */
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

  // The text in the box becomes a search after a typing pause or with Enter.
  const [text, setText] = useState("");
  // "Add by link" opened from the bar's button (issue #229).
  const [linkOpen, setLinkOpen] = useState(false);
  const [search, setSearch] = useState<Omit<CatalogSearch, "versionType">>(defaultSearch);
  const [versionChoice, setVersionChoice] = useState<VersionChoice | null>(null);
  const versionType =
    versionChoice?.installationId === installation.id ? versionChoice.versionType : null;
  const params = useMemo<CatalogSearch>(() => ({ ...search, versionType }), [search, versionType]);

  const [load, setLoad] = useState<Load>({ results: null, loading: false, error: null });
  const [loadingMore, setLoadingMore] = useState(false);
  const [moreError, setMoreError] = useState<string | null>(null);
  const [retry, setRetry] = useState(0);
  // Changes when the installation's addons have changed: search and detail are reread with the
  // same filters, leaving in view what is there.
  const [refresh, setRefresh] = useState(0);
  const tracker = useRef(new RequestTracker());
  // The installation of the results in `load`: after a change the previous ones stay in view
  // until the new response arrives.
  const resultsOf = useRef<string | null>(null);

  const [categories, setCategories] = useState<CatalogCategory[]>([]);
  const [categoriesError, setCategoriesError] = useState<string | null>(null);
  const [versionTypes, setVersionTypes] = useState<VersionType[]>([]);

  // The detail takes the place of the results, which stay mounted and hidden: search, filters
  // and loaded pages do not change, and the scroll position is restored on return.
  const [selection, setSelection] = useState<Selection | null>(null);
  // The last opened row stays marked after returning, and receives focus.
  const [lastSelectedId, setLastSelectedId] = useState<number | null>(null);
  // Another installation has other results: the detail opened in the previous one closes and
  // does not come back when returning to it.
  if (selection && selection.installationId !== installation.id) {
    setSelection(null);
    setLastSelectedId(null);
  }
  const selected = selection?.installationId === installation.id ? selection.addon : null;
  const resultsRef = useRef<HTMLDivElement>(null);
  const savedScroll = useRef(0);
  const shown = useRef({ open: false, installationId: installation.id });

  const missingKey = keyConfigured === false;

  // The installations started in the session and the projects installed after the search
  // response, which the response's "installed" mark does not know about yet.
  const [runs, setRuns] = useState<Map<string, InstallRun>>(new Map());
  const [installedKeys, setInstalledKeys] = useState<Set<string>>(new Set());
  // The last installation started: its outcome is shown above the results.
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

  // The progress of the installations in progress (issue #128), dependencies included.
  // The installations in progress are also kept in a ref, updated immediately: the first event
  // can arrive before the `runs` state is read again.
  const [runProgress, setRunProgress] = useState<RunProgress>(noRunProgress);
  const runningRuns = useRef(new Map<string, Set<string>>());
  useInstallProgress(installation.id, (event) => {
    const running = runningRuns.current.get(installation.id) ?? new Set<string>();
    setRunProgress((previous) => withRunProgress(previous, event, installation.id, running));
  });

  // The GitHub mirror of blocked projects (issue #165), per installation and project: requested
  // automatically for the visible blocked ones with a GitHub repository (issue #223, at most
  // `MIRROR_CHECKS_AT_ONCE` at a time), from the detail or from Install, and remembered for the
  // session. `checking` avoids two requests for the same project; `mirrorGen` discards responses
  // that arrive after the outcomes have been forgotten. `autoRunning` counts the automatic
  // checks still without a response, even from an old generation; `autoSettled` changes on
  // every response and starts the next ones. `autoPaused`: an automatic check failed
  // (CurseForge or the network is not responding), so the others no longer start on their own
  // until the next search or generation; Install and the detail keep requesting.
  const [mirrors, setMirrors] = useState<Map<string, MirrorCheck>>(new Map());
  const checking = useRef(new Set<string>());
  const mirrorGen = useRef(0);
  const autoRunning = useRef(0);
  const autoPaused = useRef(false);
  const [autoSettled, setAutoSettled] = useState(0);
  /** The outcome that applies to `addon` in this installation (a blocked one without a
   *  repository is already "website only"). */
  const mirrorOf = (addon: CatalogAddon) =>
    mirrorCheckFor(addon, mirrors.get(installKey(installation.id, addon.id)));

  // The Beta option changes the file the app would install, and with it the mirror: the
  // outcomes, the requests in progress and the reasons already shown are no longer valid. A
  // response arriving later is discarded (`mirrorGen`), so a new click on Install makes a new
  // request right away.
  useEffect(() => {
    mirrorGen.current += 1;
    autoPaused.current = false;
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

  /** With the check's outcome: installs, or says why the project stays on the site. */
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

  /** Asks `github_mirror` for the mirror of `addon`; with `installAfter` it then installs it.
   *  `auto`: the automatic check of the list, counted in `autoRunning`. */
  function checkMirror(addon: CatalogAddon, installAfter: boolean, auto = false) {
    const id = installation.id;
    const key = installKey(id, addon.id);
    if (checking.current.has(key)) return;
    checking.current.add(key);
    if (auto) autoRunning.current += 1;
    const gen = mirrorGen.current;
    // A response from an old generation writes nothing and does not install: the file it
    // checked is no longer the one `install_addon` would choose.
    const current = () => gen === mirrorGen.current;
    const setCheck = (check: MirrorCheck) =>
      setMirrors((previous) => new Map(previous).set(key, check));
    setCheck(installAfter ? { status: "checking", forInstall: true } : { status: "checking" });
    if (installAfter) {
      setLastInstall(key);
      setPageError(null);
      // The previous outcome (an error) does not describe this attempt.
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
          if (auto) autoPaused.current = true;
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

  /** Install: right away for a downloadable project, from the GitHub mirror for a blocked one. */
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
      // There is no final phase: the bar closes when the command returns the outcomes.
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

  /** The detail's sentence for a blocked addon: the mirror outcome, or the error with a
   *  retry. */
  function renderBlockedNote(addon: CatalogAddon) {
    if (addon.installed) {
      return (
        <p className="detail-notice">
          The author does not allow downloads from other apps: updates use GitHub when it has
          the same release.
        </p>
      );
    }
    const check = mirrorOf(addon);
    if (check?.status === "failed") {
      return (
        <p className="detail-notice error" role="alert">
          {detailNote(check)}{" "}
          <button className="link inline" onClick={() => checkMirror(addon, false)}>
            Try again
          </button>
        </p>
      );
    }
    return (
      <p className="detail-notice" role="status">
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

  // The error of a link opened in the detail does not concern the results, and vice versa.
  const closeDetail = useCallback(() => {
    setSelection(null);
    setPageError(null);
  }, []);

  // The detail of a blocked, not installed addon asks for the GitHub mirror right away, once
  // per session: Install and the sentence below the name show it (issue #165). An outcome
  // already requested by the row (even in progress) applies here too; without a repository
  // nothing is requested.
  const detailBlocked =
    selected !== null &&
    isBlocked(selected) &&
    selected.githubRepo !== null &&
    !withInstalled(selected, installedHere).installed;
  useEffect(() => {
    if (!selected || !detailBlocked) return;
    if (mirrors.has(installKey(installation.id, selected.id))) return;
    checkMirror(selected, false);
    // Only on opening (or when the addon becomes one to check): an error does not retry by itself.
  }, [selected?.id, detailBlocked, installation.id]);

  // Before painting: the detail starts from the top, the results go back to where they were,
  // with focus on the row they started from.
  // On an installation change nothing is restored: the saved scroll position and the row
  // belong to the results of the previous installation.
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

  // Categories and game versions do not change during the session: a single request.
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

  // A new search restarts from the first page; only the last response applies.
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
        // A new search retries the automatic checks of the new projects; the failed ones stay
        // failed (Install retries).
        autoPaused.current = false;
        setLoad({ results: firstPage(page), loading: false, error: null });
      })
      .catch((err) => {
        if (!tracker.current.isLatest(id)) return;
        setLoad({ results: null, loading: false, error: errorMessage(err) });
      });
  }, [started, missingKey, installation.id, params, retry, refresh, betaWhenNoRelease]);

  // The visible blocked ones with a GitHub repository are checked automatically (issue #223),
  // in list order and at most `MIRROR_CHECKS_AT_ONCE` at a time. Only with the results of the
  // current search: during a new search the ones in view are from before. Those not yet
  // started are not a queue: changing search or page picks again among the rows in view, and
  // the outcomes already received stay for the session.
  useEffect(() => {
    if (load.loading || !load.results || resultsOf.current !== installation.id) return;
    if (autoPaused.current) return;
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

  // Addons removed, reinstalled or installed outside Search: the results' "installed" mark,
  // the projects installed in the session and the finished outcomes are no longer valid. The
  // installations in progress stay. The other installations reread the search anyway when
  // the user goes back to them.
  useEffect(() => {
    if (!active || !stale || stale.size === 0) return;
    setInstalledKeys((previous) => forgetInstalled(previous, stale));
    setRuns((previous) => forgetFinishedRuns(previous, stale));
    // First search never started (the tab opens now): the search effect runs it in this same
    // pass, already with the new addons; a `refresh` would run a second identical one.
    const firstSearch = load.results === null && !load.loading && load.error === null;
    if (stale.has(installation.id) && !firstSearch) {
      setRefresh((n) => n + 1);
      // The row's "installed" mark is a copy from before: the detail takes it again from the
      // reread `AddonDetail` (`detail.installed || addon.installed` in AddonDetailView), which
      // for an addon still present (Update, Reinstall) stays true. Without this a removed
      // addon would stay "Installed" because of the copy.
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

  // "Add by link" does not use CurseForge: it remains even without the key, and then it is
  // always open. With the key it opens from the bar's button (issue #229), which exists only in
  // that branch: without the key the early return below does not draw the bar. Closed hides
  // only the field: preview, progress and outcome stay in view, and stay mounted. Its state
  // applies to the installation, so it remounts when that changes.
  const addByLink = (
    <AddByLink
      key={installation.id}
      id="add-by-link"
      open={missingKey || linkOpen}
      installationId={installation.id}
      onInstalled={() => {
        onInstalled?.();
        // The project installed from the link can be in the results.
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
        <div className="toolbar search-toolbar">
          <span className="search-field">
            <SearchIcon />
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
          </span>
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
          <button
            className="button secondary"
            aria-expanded={linkOpen}
            aria-controls="add-by-link"
            onClick={() => setLinkOpen((open) => !open)}
          >
            Add by link
          </button>
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
                pillFor={(addon) => blockedPill(mirrorOf(addon))}
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

/** The key of a Search installation in progress events: a CurseForge project. */
function curseforgeRun(modId: number): string {
  return projectId({ source: "curseforge", key: String(modId) });
}
