import {
  Fragment,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent,
  type MouseEvent,
  type ReactNode,
} from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  api,
  errorMessage,
  type AddonUpdate,
  type InstalledAddon,
  type Installation,
  type RemoveResult,
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
  type AddonGroup,
  type Filters,
  type Sort,
  type SortKey,
} from "./filter";
import { SOURCE_LABELS, projectId, sourcesIn, updateRequest, type ProjectRef } from "./sources";
import { ProgressBar, useInstallProgress } from "./ProgressBar";
import { refreshCheck } from "./refresh";
import { ContextMenu } from "./ContextMenu";
import {
  EXCLUDED_TITLE,
  excludedAfter,
  excludedIds,
  exclusionChoice,
  exclusionProjects,
  isExcluded,
} from "./exclusion";
import { blockedHint, isUpdatable } from "./mirror";
import { interfaceSummary } from "./interfaceColumn";
import {
  confirmText,
  foldersOf,
  menuItems,
  menuTargets,
  pruneSelection,
  reinstallProjects,
  removalSummary,
  removedFolders,
  selectRow,
  selectedText,
  toggleRow,
  type MenuAction,
  type Selection,
} from "./selection";
import {
  progressLabel,
  progressPercent,
  progressPosition,
  withProgress,
  type InstallProgress,
} from "./progress";

type Props = {
  installation: Installation;
  /** Changes when the addons were installed elsewhere in the app: the list is reread. */
  reload?: number;
  /** Opens the picker for a zip to install (D32); without it, the button does not appear. */
  onInstallZip?: () => void;
  /** A zip installation is in progress. */
  zipBusy?: boolean;
  /** Removal, update or reinstall have (perhaps) changed the addons on disk of the
   * `installationId` installation: the Search tab rereads its results. */
  onChanged?: (installationId: string) => void;
  /** The automatic update row (`AutoUpdateLine`): a bar of its own between the title and the
   * filters. */
  autoUpdateLine?: ReactNode;
};

/**
 * Update check results per installation, valid for the session. They live outside the
 * component because changing installation reloads the component: going back to one already
 * checked does not call CurseForge again.
 */
const updatesCache = new Map<string, AddonUpdate[]>();
/** Installations with an automatic check already started, so as not to call CurseForge twice. */
const autoChecked = new Set<string>();
/** The last check error per installation, to show it also when going back to it. */
const errorCache = new Map<string, string>();
/**
 * Projects being updated per installation. Outside the component like the caches above: going
 * back to an installation with an update still in progress, the rows stay locked and a second
 * update of the same project does not start.
 */
const updateRuns = new Map<string, Set<string>>();
/** The installations whose update in progress is a reinstall (issue #149). */
const reinstallRuns = new Set<string>();
/** The outcome of the last update per installation, shown above the table. */
const updateResults = new Map<string, UpdateResult>();

/** `reinstall`: the outcome comes from the menu's "Reinstall", not from an update. */
type UpdateResult = ({ outcomes: UpdateOutcome[] } | { error: string }) & { reinstall?: boolean };

/** The open menu: the row (main folder) and the point where it opens. */
type OpenMenu = { row: string; x: number; y: number };

/** The outcome of the last removal, shown above the table. */
type RemovalResult =
  | { error: string }
  | { text: string; errors: { folder: string; message: string }[]; warning: string | null };

/**
 * The game version types do not change during the session and the command does not use the
 * network: a single request, shared by all installations.
 */
let versionTypesRequest: Promise<VersionType[]> | null = null;
function loadVersionTypes(): Promise<VersionType[]> {
  versionTypesRequest ??= api.listVersionTypes().catch((err) => {
    // An error does not stay in the cache: the next mount retries.
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

/** `format` throws an exception on an invalid date: an odd `mtime` shows "—". */
function formatModified(ms: number | null): string {
  if (ms === null) return "—";
  const date = new Date(ms);
  return Number.isNaN(date.getTime()) ? "—" : dateFormat.format(date);
}

export function AddonTable({
  installation,
  reload = 0,
  onInstallZip,
  zipBusy = false,
  onChanged,
  autoUpdateLine,
}: Props) {
  const [load, setLoad] = useState<Load>({ state: "loading" });
  const [filters, setFilters] = useState<Filters>(defaultFilters);
  const [sort, setSort] = useState<Sort>(defaultSort);
  // Open groups (by main folder): the other folders appear below the row.
  const [expanded, setExpanded] = useState<Set<string>>(new Set());

  function toggle(folder: string) {
    setExpanded((previous) => {
      const next = new Set(previous);
      if (next.has(folder)) next.delete(folder);
      else next.add(folder);
      return next;
    });
  }
  // The last valid result stays visible even during a new check, so the table and the
  // "Only with updates" filter do not empty while waiting.
  const [updates, setUpdates] = useState<Map<string, AddonUpdate> | undefined>();
  const [checking, setChecking] = useState(false);
  const [checkError, setCheckError] = useState<string | null>(null);
  // The component does not remount when changing installation: every response, automatic or
  // from the button, applies only if the installation is still the one it started for.
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
        // With no results at all the filter has no data to act on: it is removed.
        if (!updatesCache.has(id)) setFilters((f) => ({ ...f, onlyUpdates: false }));
      });
  }, [installation.id]);

  // First access in the session: automatic check; afterwards only with the button.
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

  // Rereads `Interface/AddOns` without emptying the table or losing filters and open rows:
  // needed after installing or removing addons outside the app.
  const [rescanning, setRescanning] = useState(false);
  const [rescanError, setRescanError] = useState<string | null>(null);
  // `force` only from the button: recomputes the fingerprints even of folders that look
  // unchanged, for changes made by hand outside the app.
  function rescan(force = false): Promise<void> {
    const id = installation.id;
    setRescanning(true);
    setRescanError(null);
    setChecking(true);
    setCheckError(null);
    // A single scan for list and statuses (issue #126).
    return api
      .refreshAddons(id, force)
      .then((result) => {
        const check = refreshCheck(result);
        if (check.error === null) {
          updatesCache.set(id, check.updates);
          errorCache.delete(id);
        } else {
          errorCache.set(id, check.error);
        }
        if (currentId.current !== id) return;
        setLoad({ state: "ready", addons: result.addons });
        if (check.error === null) {
          setUpdates(byFolder(check.updates));
        } else {
          // As in `runCheck`: the last valid result stays visible next to the error, so a failed
          // check (no network) does not remove all the statuses. After an update
          // `updateProjects` has already emptied the cache, so nothing stale remains there.
          setCheckError(check.error);
          // With no results at all the filter has no data to act on: it is removed.
          if (!updatesCache.has(id)) setFilters((f) => ({ ...f, onlyUpdates: false }));
        }
      })
      .catch((err) => {
        if (currentId.current !== id) return;
        // A read error does not remove the table already shown: it is reported above.
        setLoad((previous) =>
          previous.state === "ready" ? previous : { state: "error", message: errorMessage(err) },
        );
        setRescanError(errorMessage(err));
      })
      .finally(() => {
        if (currentId.current === id) {
          setRescanning(false);
          setChecking(false);
        }
      });
  }

  // An installation made elsewhere (Search tab) shows up with the same reread as the button,
  // only when `reload` changes.
  const seenReload = useRef(reload);
  useEffect(() => {
    if (seenReload.current === reload) return;
    seenReload.current = reload;
    void rescan();
  }, [reload]);

  // Projects (`mod_id`) being updated: their rows show the progress bar and all the update
  // buttons stay disabled until the list and the statuses are reread.
  const [updating, setUpdating] = useState<Set<string>>(new Set());
  // The last progress of each project being updated (issue #128), by `projectId`.
  const [progress, setProgress] = useState<Map<string, InstallProgress>>(new Map());
  useInstallProgress(installation.id, (event) => {
    if (updateRuns.has(installation.id)) {
      setProgress((previous) => withProgress(previous, event, installation.id));
    }
  });
  const [updateResult, setUpdateResult] = useState<UpdateResult | null>(null);
  const [reinstalling, setReinstalling] = useState(false);
  // Context menu, multiple selection and removal (issue #149).
  const [selection, setSelection] = useState<Selection>(null);
  const [menu, setMenu] = useState<OpenMenu | null>(null);
  // The groups to remove, fixed when "Remove" is chosen: the bar asks confirmation for those.
  const [confirming, setConfirming] = useState<AddonGroup[] | null>(null);
  const [removing, setRemoving] = useState(false);
  const [removalResult, setRemovalResult] = useState<RemovalResult | null>(null);
  // The row the menu started from: focus returns to it with Esc, after a choice or with Cancel.
  const returnFocus = useRef<HTMLElement | null>(null);
  const cancelRef = useRef<HTMLButtonElement>(null);
  const sectionRef = useRef<HTMLElement>(null);
  // The row reachable with Tab (roving tabindex): the others are reached with the arrows.
  const [focusRow, setFocusRow] = useState<string | null>(null);
  // `currentId` survives unmounting: a separate signal is needed so as not to reread the list
  // of a component that no longer exists.
  const mounted = useRef(false);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  // The addons excluded from automatic updates (issue #204), by `projectId`: they are read with
  // `get_auto_update` per installation and matched to the rows on `project.source` and
  // `project.key`. `undefined` until the read has succeeded.
  const [excluded, setExcluded] = useState<ReadonlySet<string> | undefined>();
  const [excludedError, setExcludedError] = useState<string | null>(null);
  const [savingExclusion, setSavingExclusion] = useState(false);
  // The choice being saved: the rows show it until the backend responds, then the saved state
  // applies (even after an error, which brings the rows back to what is really there).
  const [pendingExclusion, setPendingExclusion] = useState<{
    projects: ProjectRef[];
    exclude: boolean;
  } | null>(null);
  const [exclusionError, setExclusionError] = useState<string | null>(null);

  useEffect(() => {
    let current = true;
    setExcluded(undefined);
    setExcludedError(null);
    setSavingExclusion(false);
    setPendingExclusion(null);
    setExclusionError(null);
    api
      .getAutoUpdate(installation.id)
      .then((state) => current && setExcluded(excludedIds(state)))
      .catch((err) => current && setExcludedError(errorMessage(err)));
    return () => {
      current = false;
    };
  }, [installation.id]);

  /**
   * Excludes or readmits the projects, one at a time: each response is the saved state and
   * replaces it. At the first error it stops: the rows show what the backend saved.
   */
  async function saveExclusion(projects: ProjectRef[], exclude: boolean) {
    const id = installation.id;
    if (projects.length === 0 || savingExclusion) return;
    setSavingExclusion(true);
    setPendingExclusion({ projects, exclude });
    setExclusionError(null);
    try {
      for (const project of projects) {
        const saved = await api.setAutoUpdateExcluded(id, project, exclude);
        if (!mounted.current || currentId.current !== id) return;
        setExcluded(excludedIds(saved));
        setExcludedError(null);
      }
    } catch (err) {
      if (mounted.current && currentId.current === id) {
        setExclusionError(`Could not save the automatic update choice: ${errorMessage(err)}`);
      }
    } finally {
      if (mounted.current && currentId.current === id) {
        setSavingExclusion(false);
        setPendingExclusion(null);
      }
    }
  }

  // What the rows show: the choice being saved on top of the saved state.
  const shownExcluded =
    pendingExclusion && excluded
      ? excludedAfter(excluded, pendingExclusion.projects, pendingExclusion.exclude)
      : excluded;

  // `reinstall` (issue #149): downloads again even the projects already up to date, with the
  // same bar and the same summary as an update.
  function updateProjects(projects: ProjectRef[], reinstall = false) {
    const id = installation.id;
    const ids = new Set(projects.map(projectId));
    if (ids.size === 0 || updateRuns.has(id) || removing) return;
    updateRuns.set(id, ids);
    if (reinstall) reinstallRuns.add(id);
    else reinstallRuns.delete(id);
    updateResults.delete(id);
    setUpdating(ids);
    setReinstalling(reinstall);
    setProgress(new Map());
    setUpdateResult(null);
    setRemovalResult(null);
    const request = updateRequest(projects);
    api
      .updateAddons(id, request.modIds, request.projects, reinstall)
      .then(
        (outcomes): UpdateResult => ({ outcomes, reinstall }),
        (err): UpdateResult => ({ error: errorMessage(err), reinstall }),
      )
      .then((result) => {
        updateResults.set(id, result);
        // The previous statuses are no longer valid: without a cache the next check rereads them.
        updatesCache.delete(id);
        errorCache.delete(id);
        // With the cache empty a new check is needed even if something else is being viewed now:
        // when going back to the table the automatic check will start again.
        autoChecked.delete(id);
        if (!mounted.current || currentId.current !== id) return;
        setUpdateResult(result);
        return rescan();
      })
      .finally(() => {
        // Even a failed command may have changed something on disk.
        onChanged?.(id);
        updateRuns.delete(id);
        reinstallRuns.delete(id);
        // There is no final phase: the bar closes when the command returns the outcomes.
        if (mounted.current && currentId.current === id) {
          setUpdating(new Set());
          setReinstalling(false);
          setProgress(new Map());
        }
      });
  }

  // Removal (issue #149): removes all the folders of the confirmed groups, without backup; then
  // the list is reread. The removed rows leave the selection, the others stay.
  function removeGroups(targets: AddonGroup[]) {
    const id = installation.id;
    const folders = foldersOf(targets);
    if (folders.length === 0 || removing || updateRuns.has(id)) return;
    setRemoving(true);
    setRemovalResult(null);
    updateResults.delete(id);
    setUpdateResult(null);
    api
      .removeAddons(id, folders)
      .then((result: RemoveResult) => {
        if (!mounted.current || currentId.current !== id) return;
        setRemovalResult({ ...removalSummary(result), warning: result.warning });
        const removed = new Set(removedFolders(result));
        setSelection((previous) =>
          previous === null
            ? null
            : pruneSelection(previous, [...previous].filter((key) => !removed.has(key))),
        );
        return rescan();
      })
      .catch((err) => {
        if (!mounted.current || currentId.current !== id) return;
        setRemovalResult({ error: errorMessage(err) });
      })
      .finally(() => {
        onChanged?.(id);
        if (mounted.current && currentId.current === id) setRemoving(false);
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

  // The game version being saved (project and choice), and errors per project.
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
          // The previous statuses apply to the other version: as after an update, the cache is
          // emptied and the check restarts (or will restart on going back to the installation).
          updatesCache.delete(id);
          errorCache.delete(id);
          autoChecked.delete(id);
          if (!mounted.current || currentId.current !== id) return;
          // The label changes immediately; the update status comes from the check.
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
    // When changing installation quickly, an old response must not overwrite the new one.
    let current = true;
    setVersionSaving(null);
    setVersionErrors(new Map());
    // A reread started for the previous installation is no longer valid for this one.
    setRescanning(false);
    setRescanError(null);
    setUpdating(updateRuns.get(installation.id) ?? new Set());
    setReinstalling(reinstallRuns.has(installation.id));
    setProgress(new Map());
    setUpdateResult(updateResults.get(installation.id) ?? null);
    setSelection(null);
    setMenu(null);
    setConfirming(null);
    setRemoving(false);
    setRemovalResult(null);
    setFocusRow(null);
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
  // An addon made of several folders is a single row: counts and filters count the groups.
  const groups = useMemo(() => groupAddons(addons, updates), [addons, updates]);
  const authors = useMemo(() => authorsOf(groups.map((g) => g.main)), [groups]);
  const visible = useMemo(
    () => visibleGroups(groups, filters, sort, updates),
    [groups, filters, sort, updates],
  );
  const updateCount = groups.filter(
    (g) => groupUpdate(g, updates)?.status === "updateAvailable",
  ).length;
  // "Update all" counts the projects the app can update on its own: one per group, without
  // repetitions (source plus id), so the number on the button is that of the projects sent.
  const updatable = new Map<string, ProjectRef>();
  for (const g of groups) {
    const update = groupUpdate(g, updates);
    if (update && isUpdatable(update)) updatable.set(projectId(update.project!), update.project!);
  }
  const updatableIds = [...updatable.values()];
  const sources = useMemo(() => sourcesIn(updates), [updates]);
  // A chosen source that no longer appears in the check (addon removed, new check) would hide
  // everything without a menu to clear it. A check that is still empty does not clear the choice.
  useEffect(() => {
    if (sources.length > 0 && filters.source !== null && !sources.includes(filters.source)) {
      setFilters((previous) => ({ ...previous, source: null }));
    }
  }, [sources, filters.source]);
  const running = updating.size > 0;
  const busy = running || removing;
  const position = progressPosition(progress, updating);

  // After a reread only the rows that still exist stay selected.
  useEffect(() => {
    if (load.state !== "ready") return;
    setSelection((previous) => pruneSelection(previous, groups.map((g) => g.main.folder)));
  }, [groups, load.state]);

  // Esc leaves selection mode, if the key is not already used by a field, the menu or the
  // confirmation bar, and only with the Installed tab in view (hidden tabs stay mounted).
  const selecting = selection !== null;
  useEffect(() => {
    if (!selecting) return;
    const onKey = (event: globalThis.KeyboardEvent) => {
      if (event.key !== "Escape" || event.defaultPrevented) return;
      if (sectionRef.current?.closest("[hidden]")) return;
      const target = event.target instanceof Element ? event.target : null;
      if (target?.closest("input, textarea, select, [role='menu'], [role='alertdialog']")) return;
      setSelection(null);
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [selecting]);

  // The confirmation bar takes focus on Cancel: an accidental Enter removes nothing.
  useEffect(() => {
    if (confirming) cancelRef.current?.focus();
  }, [confirming]);

  const closeMenu = useCallback((restoreFocus: boolean) => {
    setMenu(null);
    if (restoreFocus) returnFocus.current?.focus();
  }, []);

  function openMenu(row: string, element: HTMLElement, x: number, y: number) {
    returnFocus.current = element;
    setFocusRow(row);
    setMenu({ row, x, y });
  }

  function onRowContextMenu(event: MouseEvent<HTMLTableRowElement>, row: string) {
    event.preventDefault();
    const element = event.currentTarget;
    // From the Menu key the browser may give (0, 0): the menu opens below the start of the row.
    if (event.clientX === 0 && event.clientY === 0) {
      const rect = element.getBoundingClientRect();
      openMenu(row, element, rect.left + 24, rect.bottom);
    } else {
      openMenu(row, element, event.clientX, event.clientY);
    }
  }

  function onRowKeyDown(event: KeyboardEvent<HTMLTableRowElement>, row: string, hasOthers: boolean) {
    // The controls inside the row (links, version menu, buttons) keep their keys.
    if (event.target !== event.currentTarget) return;
    const element = event.currentTarget;
    if (event.key === "ContextMenu" || (event.key === "F10" && event.shiftKey)) {
      event.preventDefault();
      const rect = element.getBoundingClientRect();
      openMenu(row, element, rect.left + 24, rect.bottom);
    } else if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      if (selection !== null) setSelection((previous) => toggleRow(previous, row));
      else if (hasOthers) toggle(row);
    } else if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      const rows = [
        ...(element.parentElement?.querySelectorAll<HTMLElement>("tr[data-row]") ?? []),
      ];
      const next = rows[rows.indexOf(element) + (event.key === "ArrowDown" ? 1 : -1)];
      if (next) {
        event.preventDefault();
        setFocusRow(next.dataset.row ?? null);
        next.focus();
      }
    }
  }

  function chooseAction(action: MenuAction) {
    if (!menu) return;
    const targets = menuTargets(groups, selection, menu.row);
    switch (action) {
      case "select":
        setSelection((previous) => selectRow(previous, menu.row));
        closeMenu(true);
        break;
      case "clear":
        setSelection(null);
        closeMenu(true);
        break;
      case "reinstall":
        closeMenu(true);
        updateProjects(reinstallProjects(targets, updates), true);
        break;
      case "autoUpdate": {
        const projects = exclusionProjects(targets, updates);
        closeMenu(true);
        void saveExclusion(projects, exclusionChoice(projects, excluded));
        break;
      }
      case "remove":
        // Focus goes to the confirmation bar, not to the row.
        closeMenu(false);
        setConfirming(targets);
        break;
    }
  }

  function cancelRemove() {
    setConfirming(null);
    returnFocus.current?.focus();
  }

  const menuState = { updates, busy, checking, excluded, excludedError, savingExclusion };
  // A row the filters have hidden cannot remain the only one reachable with Tab.
  const tabStop =
    focusRow !== null && visible.some((g) => g.main.folder === focusRow)
      ? focusRow
      : (visible[0]?.main.folder ?? null);

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
    <section ref={sectionRef} className="addons" aria-labelledby="addons-title">
      <header className="section-header addons-header">
        <h2 id="addons-title" className="addons-title">
          Addons
          {/* The installation stays in the title: the section is labelled by this text. */}
          <span className="addons-count">
            {installation.name}
            {load.state === "ready" &&
              ` · ${visible.length === groups.length ? groups.length : `${visible.length} of ${groups.length}`} ${groups.length === 1 ? "addon" : "addons"}`}
            {load.state === "ready" && updateCount > 0 && ` · ${updateCount} to update`}
          </span>
        </h2>
        <div className="section-actions">
          {selection !== null && (
            <span className="selection-status">
              <span className="count">{selectedText(selection)}</span>
              <button className="link" onClick={() => setSelection(null)}>
                Clear selection
              </button>
            </span>
          )}
          {onInstallZip && (
            <button
              className="button secondary"
              onClick={onInstallZip}
              disabled={zipBusy || busy}
              title="Install an addon zip downloaded from a website; you can also drop it into the window"
            >
              Install from zip…
            </button>
          )}
          <button
            className="button secondary"
            onClick={() => void rescan(true)}
            disabled={rescanning || busy || load.state === "loading"}
            title="Reads the Interface/AddOns folder again"
          >
            {rescanning ? "Reloading…" : "Reload addons"}
          </button>
          {/* A single primary button (D20, D42): "Update all" is soft like the rows'
              Update buttons. */}
          {(running || updatableIds.length > 0) && (
            <button
              className="button soft"
              onClick={() => updateProjects(updatableIds)}
              disabled={busy || checking || versionSaving !== null}
            >
              {running
                ? position > 0
                  ? `${reinstalling ? "Reinstalling" : "Updating"} ${position} of ${updating.size}`
                  : reinstalling
                    ? "Reinstalling…"
                    : "Updating…"
                : `Update all (${updatableIds.length})`}
            </button>
          )}
          <button className="button" onClick={() => void runCheck()} disabled={checking || busy}>
            {checking ? "Checking…" : "Check for updates"}
          </button>
        </div>
      </header>

      {autoUpdateLine}

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
      {excludedError && (
        <p className="message error" role="alert">
          Could not read the automatic update choices: {excludedError}
        </p>
      )}
      {exclusionError && (
        <p className="message error" role="alert">
          {exclusionError}{" "}
          <button className="link subtle" onClick={() => setExclusionError(null)}>
            Dismiss
          </button>
        </p>
      )}
      {confirming && (
        <div
          className="message remove-confirm"
          role="alertdialog"
          aria-labelledby="remove-confirm-title"
          aria-describedby="remove-confirm-hint"
          onKeyDown={(event) => {
            if (event.key === "Escape") {
              event.preventDefault();
              cancelRemove();
            }
          }}
        >
          <span id="remove-confirm-title">{confirmText(confirming)}</span>
          <span id="remove-confirm-hint" className="muted">
            No backup; saved settings in WTF are kept.
          </span>
          <button
            className="button danger small"
            onClick={() => {
              setConfirming(null);
              removeGroups(confirming);
            }}
          >
            Remove
          </button>
          <button ref={cancelRef} className="button secondary small" onClick={cancelRemove}>
            Cancel
          </button>
        </div>
      )}
      {removing && (
        <p className="message muted" role="status">
          Removing…
        </p>
      )}
      {removalResult && <RemovalSummary result={removalResult} />}
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
                    const isSelected = selection?.has(addon.folder) ?? false;
                    const classes = [
                      error ? "has-error" : "",
                      hasOthers || selection !== null ? "expandable" : "",
                      open ? "open" : "",
                      isSelected ? "selected" : "",
                    ].join(" ");
                    return (
                      <Fragment key={addon.folder}>
                        <tr
                          data-row={addon.folder}
                          className={classes.replace(/\s+/g, " ").trim() || undefined}
                          tabIndex={addon.folder === tabStop ? 0 : -1}
                          aria-selected={selection !== null ? isSelected : undefined}
                          onFocus={(event) => {
                            if (event.target === event.currentTarget) setFocusRow(addon.folder);
                          }}
                          onClick={
                            selection !== null
                              ? (event) => {
                                  // A control inside the row (link, version menu, buttons)
                                  // performs its action and does not change the selection.
                                  if (isControl(event.target, event.currentTarget)) return;
                                  setSelection((previous) => toggleRow(previous, addon.folder));
                                }
                              : hasOthers
                                ? () => toggle(addon.folder)
                                : undefined
                          }
                          onContextMenu={(event) => onRowContextMenu(event, addon.folder)}
                          onKeyDown={(event) => onRowKeyDown(event, addon.folder, hasOthers)}
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
                            {/* `aria-selected` on a table row is not read by every screen
                                reader: the state is also in the row's text. */}
                            {isSelected && <span className="visually-hidden"> (selected)</span>}
                            {isExcluded(update, shownExcluded) && (
                              <span className="pill addon-pill" title={EXCLUDED_TITLE}>
                                No auto-update
                              </span>
                            )}
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
                          <InterfaceCell versions={addon.interface} flavor={installation.flavor} />
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
                            reinstalling={reinstalling}
                            progress={
                              update?.project ? progress.get(projectId(update.project)) : undefined
                            }
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
                              <InterfaceCell
                                versions={other.interface}
                                flavor={installation.flavor}
                              />
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
      {menu && (
        <ContextMenu
          x={menu.x}
          y={menu.y}
          items={menuItems(menuTargets(groups, selection, menu.row), selection !== null, menuState)}
          label={
            selection !== null
              ? `Actions for ${selectedText(selection)}`
              : `Actions for ${menuTitle(groups, menu.row)}`
          }
          onChoose={chooseAction}
          onClose={closeMenu}
        />
      )}
    </section>
  );
}

/** The click comes from a control inside the row, not from the row itself. */
function isControl(target: EventTarget, row: Element): boolean {
  const control =
    target instanceof Element ? target.closest("button, a, select, input, textarea, label") : null;
  return control !== null && row.contains(control);
}

/** The title of the menu's row, for its accessible name. */
function menuTitle(groups: AddonGroup[], row: string): string {
  const group = groups.find((g) => g.main.folder === row);
  return group ? displayTitle(group.main) : row;
}

/** Summary of the last removal, with the reason for each folder not removed. */
function RemovalSummary({ result }: { result: RemovalResult }) {
  if ("error" in result) {
    return (
      <p className="message error" role="alert">
        Remove failed: {result.error}
      </p>
    );
  }
  const problem = result.errors.length > 0 || result.warning !== null;
  return (
    <div className="message update-summary" role={problem ? "alert" : "status"}>
      <p>{result.text}</p>
      {result.warning && <p className="error">{result.warning}</p>}
      {result.errors.length > 0 && (
        <ul>
          {result.errors.map((entry) => (
            <li key={entry.folder} className="error">
              {entry.folder}: {entry.message}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

function withoutKey<V>(map: Map<number, V>, key: number): Map<number, V> {
  if (!map.has(key)) return map;
  const next = new Map(map);
  next.delete(key);
  return next;
}

/** The statuses with the new version forced for the `modId` project, before the recheck. */
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
  /** The choice being saved for this project, shown in the menu until it finishes. */
  saving: VersionChoice | null;
  versionError?: string;
  /**
   * A save, a check or an update in progress: the menu ignores choices. It is not `disabled`
   * because a disabled control loses focus, and keyboard users would start again from the top
   * of the page after every choice.
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
  // "website only" or, with the mirror found by the check, "via GitHub" (issue #165).
  const blocked = blockedHint(update);
  return (
    <td>
      {project.url ? (
        <button
          className="link inline"
          onClick={(event) => {
            // The link does not open and close the group row.
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
      {/* Forced versions are per CurseForge project (`mod_id`, D6). */}
      {versionTypes.length > 0 && project.source === "curseforge" && (
        <select
          className={`select compact version-select${saving || versionError ? " shown" : ""}`}
          aria-label={`Game version for ${project.name || `#${project.key}`}`}
          value={selected ?? ""}
          aria-disabled={locked}
          // The menu does not open and close the group row.
          onClick={(event) => event.stopPropagation()}
          onChange={(event) => {
            // The menu is controlled: an ignored choice returns by itself to the current value.
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
          {/* A forced version the app no longer knows stays visible as the current choice. */}
          {selected !== null && !versionTypes.some((type) => type.id === selected) && (
            <option value={selected}>{`#${selected}`}</option>
          )}
        </select>
      )}
      {blocked && (
        <span className="hint" title={blocked.title}>
          {blocked.label}
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

type UpdateCellProps = {
  update?: AddonUpdate;
  checking: boolean;
  /** This row's project is in the update in progress. */
  updating: boolean;
  /** The update in progress is a reinstall. */
  reinstalling: boolean;
  /** The last progress of the project being updated, if it has arrived. */
  progress?: InstallProgress;
  disabled: boolean;
  onUpdate: (projects: ProjectRef[]) => void;
};

function UpdateCell({
  update,
  checking,
  updating,
  reinstalling,
  progress,
  disabled,
  onUpdate,
}: UpdateCellProps) {
  if (updating) {
    return (
      <td className="nowrap">
        <ProgressBar
          value={progressPercent(progress)}
          label={progressLabel(progress, reinstalling ? "Reinstalling…" : "Updating…")}
          name={update?.project?.name ?? "Addon"}
        />
      </td>
    );
  }
  if (!update) return <td className="muted">{checking ? "…" : ""}</td>;
  switch (update.status) {
    case "updateAvailable":
      return (
        <td className="nowrap" title={installedTitle(update, update.latest.date)}>
          {isUpdatable(update) ? (
            <button
              className="button soft small row-update"
              // The text is shortened with "…": the full version stays in the tooltip.
              title={`Update to ${update.latest.name}`}
              disabled={disabled}
              onClick={(event) => {
                // The button does not open and close the group row.
                event.stopPropagation();
                onUpdate([update.project!]);
              }}
            >
              Update to {update.latest.name}
            </button>
          ) : (
            <span className="pill accent addon-pill">{update.latest.name}</span>
          )}
        </td>
      );
    case "upToDate":
      return (
        <td className="nowrap" title={installedTitle(update)}>
          <span className="pill ok addon-pill">Up to date</span>
        </td>
      );
    case "betaOnly":
      return (
        <td
          className="nowrap"
          title={`${installedTitle(update) ?? ""}\nOnly a beta is newer: ${update.latest.name}. Turn on "Use beta files when there is no release" in Options to update to it.`.trim()}
        >
          <span className="pill addon-pill">Only beta files</span>
        </td>
      );
    case "error":
      return (
        <td className="nowrap">
          {/* The full message in the tooltip; for screen reader users also in the text. */}
          <span className="pill danger addon-pill" title={update.message}>
            Error<span className="visually-hidden">: {update.message}</span>
          </span>
        </td>
      );
    case "notMatched":
      return <td />;
  }
}

/** A single interface version and the rest as `+N`; the full list in the tooltip. */
function InterfaceCell({ versions, flavor }: { versions: number[]; flavor: string }) {
  const summary = interfaceSummary(versions, flavor);
  if (!summary) return <td className="mono">—</td>;
  return (
    <td className="mono" title={summary.title}>
      {summary.shown}
      {summary.more > 0 && (
        <span className="more-folders">
          <span aria-hidden="true">+{summary.more}</span>
          {/* Hidden text and not `aria-label`: a span without a role is not always read. */}
          <span className="visually-hidden">
            {" "}
            and {summary.more} more: {summary.title}
          </span>
        </span>
      )}
    </td>
  );
}

/** The status concerns the project: the tooltip says which file of the project is installed. */
function installedTitle(update: AddonUpdate, latestDate?: string): string | undefined {
  const installed = update.installedFile ? `Installed: ${update.installedFile.name}` : undefined;
  const latest = latestDate ? `New file from ${latestDate.slice(0, 10)}` : undefined;
  return [installed, latest].filter(Boolean).join("\n") || undefined;
}

/** Summary of the last update, with the reason for each project skipped or in error. */
function UpdateSummary({ result }: { result: UpdateResult }) {
  if ("error" in result) {
    return (
      <p className="message error" role="alert">
        {result.reinstall ? "Reinstall failed" : "Update failed"}: {result.error}
      </p>
    );
  }
  const count = (status: UpdateOutcome["status"]) =>
    result.outcomes.filter((o) => o.status === status).length;
  const updated = count("updated");
  const skipped = count("skipped");
  const errors = count("error");
  const parts = [
    updated > 0 && `${updated} ${result.reinstall ? "reinstalled" : "updated"}`,
    skipped > 0 && `${skipped} skipped`,
    errors > 0 && `${errors} ${errors === 1 ? "error" : "errors"}`,
  ].filter(Boolean);
  // An updated project appears only with a message (for example "Installed from GitHub").
  const details = result.outcomes.filter((o) => o.status !== "updated" || o.message);
  return (
    <div className="message update-summary" role="status">
      <p>
        {parts.length > 0
          ? parts.join(", ")
          : result.reinstall
            ? "Nothing was reinstalled"
            : "Nothing was updated"}
      </p>
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
