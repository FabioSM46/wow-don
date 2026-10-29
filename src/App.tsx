import { useCallback, useEffect, useRef, useState, type KeyboardEvent } from "react";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import {
  api,
  errorMessage,
  type AppStatus,
  type AutoUpdateMode,
  type AutoUpdateStatus,
  type Installation,
  type Theme,
} from "./api";
import { AddonTable } from "./addons/AddonTable";
import { AutoUpdateLine } from "./addons/AutoUpdateLine";
import { AUTO_UPDATE_EVENT, changedInstallations } from "./addons/autoUpdateStatus";
import { ZipDrop } from "./addons/ZipDrop";
import { ProgressBar, useInstallProgress } from "./addons/ProgressBar";
import { progressLabel, type InstallProgress } from "./addons/progress";
import { describeZipInstall, fileName, pickZip } from "./addons/zip";
import { Installations } from "./installations/Installations";
import { Appearance } from "./options/Appearance";
import { AutoUpdateOptions } from "./options/AutoUpdateOptions";
import { BetaOption } from "./options/BetaOption";
import { SystemOptions } from "./options/SystemOptions";
import { ImportExport } from "./options/ImportExport";
import { applyTheme } from "./options/theme";
import { Search } from "./search/Search";
import { clearStale, markStale } from "./search/stale";
import "./App.css";

/** The app's tabs, in the order they appear. */
const TABS = [
  { id: "search", label: "Search" },
  { id: "installed", label: "Installed" },
  { id: "options", label: "Options" },
] as const;

type Tab = (typeof TABS)[number]["id"];

function App() {
  const [status, setStatus] = useState<AppStatus | null>(null);
  const [installations, setInstallations] = useState<Installation[] | null>(null);
  // The active tab and installation last only for the session.
  const [tab, setTab] = useState<Tab>("installed");
  const [activeId, setActiveId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  // An installation from the Search tab changes the addons on disk: going back to the Installed
  // tab rereads the list (`reload` changes), only once however many installations there were.
  const [installedStale, setInstalledStale] = useState(false);
  const [reload, setReload] = useState(0);
  // The reverse: an addon removed, reinstalled or installed outside Search (Installed tab, zip,
  // import) makes the Search results for that installation stale. Search rereads them when it
  // becomes visible again.
  const [searchStale, setSearchStale] = useState<ReadonlySet<string>>(() => new Set());
  const markSearchStale = useCallback(
    (installationId: string) => setSearchStale((previous) => markStale(previous, installationId)),
    [],
  );
  const searchRefreshed = useCallback(
    (installationIds: ReadonlySet<string>) =>
      setSearchStale((previous) => clearStale(previous, installationIds)),
    [],
  );
  const tabRefs = useRef<Partial<Record<Tab, HTMLButtonElement | null>>>({});
  // The theme saved by the backend; until it arrives the system one applies (no localStorage:
  // the state lives in the backend, a brief flash at startup is accepted).
  const [theme, setTheme] = useState<Theme | null>(null);
  // The Beta option (issue #141); `null` until the settings are read.
  const [betaWhenNoRelease, setBetaWhenNoRelease] = useState<boolean | null>(null);
  const [settingsError, setSettingsError] = useState<string | null>(null);
  // Automatic updating (D41, issue #203): the saved mode (`null` until it is read) and the state
  // of the last round, from `get_auto_update_status` and from the `auto-update` event.
  const [autoUpdateMode, setAutoUpdateMode] = useState<AutoUpdateMode | null>(null);
  const [autoUpdateStatus, setAutoUpdateStatus] = useState<AutoUpdateStatus | null>(null);
  const lastAutoUpdate = useRef<AutoUpdateStatus | null>(null);
  // Installation id → `finishedAt` of the round the user undid: its outcome no longer describes
  // the disk and the row does not show it.
  const [undoneRounds, setUndoneRounds] = useState<ReadonlyMap<string, string>>(() => new Map());
  // Installation of a dragged or chosen zip (D32): one at a time, with its outcome.
  const [zipBusy, setZipBusy] = useState(false);
  // The phase of the zip installation (issue #128). The ref is updated immediately: the first
  // event can arrive before `zipBusy` is read again.
  const [zipProgress, setZipProgress] = useState<InstallProgress | undefined>(undefined);
  const zipRunning = useRef(false);
  const [zipMessage, setZipMessage] = useState<{ text: string; error: boolean } | null>(null);

  useEffect(() => {
    if (theme !== null) applyTheme(document.documentElement, theme);
  }, [theme]);

  // The browser's default menu (Reload, Inspect…) is not needed in a desktop app (issue #149): it
  // stays only in text fields, for Copy and Paste. The app's menus open from their own elements.
  useEffect(() => {
    const suppress = (event: MouseEvent) => {
      const target = event.target instanceof Element ? event.target : null;
      if (target?.closest("input, textarea, [contenteditable]:not([contenteditable='false'])")) {
        return;
      }
      event.preventDefault();
    };
    document.addEventListener("contextmenu", suppress);
    return () => document.removeEventListener("contextmenu", suppress);
  }, []);

  useEffect(() => {
    api
      .getSettings()
      .then((settings) => {
        setTheme(settings.theme);
        setBetaWhenNoRelease(settings.betaWhenNoRelease);
        setAutoUpdateMode(settings.autoUpdate);
      })
      .catch((err) => setSettingsError(errorMessage(err)));
    api
      .appStatus()
      .then(setStatus)
      .catch((err) => setError(errorMessage(err)));
    api
      .listInstallations()
      .then((list) => {
        setInstallations(list);
        setActiveId(list[0]?.id ?? null);
        // With no installations there is only one thing to do: add one, in Options.
        if (list.length === 0) setTab("options");
      })
      .catch((err) => setError(errorMessage(err)));
  }, []);

  // The automatic update status: read on opening, then on every event. A new `install` round
  // changes the addons on disk: Installed and Search reread, as after an installation from
  // Search.
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | null = null;
    const receive = (next: AutoUpdateStatus, fromEvent: boolean) => {
      if (disposed) return;
      const previous = lastAutoUpdate.current;
      lastAutoUpdate.current = next;
      setAutoUpdateStatus(next);
      // Only the read on opening is the baseline: the list just opened already matches the
      // disk. An event that arrives first may come from a round finished after the list was
      // read, and so it counts.
      if (previous === null && !fromEvent) return;
      const changed = changedInstallations(previous, next);
      if (changed.length === 0) return;
      setInstalledStale(true);
      changed.forEach(markSearchStale);
    };
    listen<AutoUpdateStatus>(AUTO_UPDATE_EVENT, ({ payload }) => receive(payload, true))
      .then((stop) => {
        if (disposed) stop();
        else unlisten = stop;
      })
      // Outside Tauri nothing arrives: the row stays hidden.
      .catch(() => {});
    api
      .getAutoUpdateStatus()
      .then((first) => {
        // An event that arrived before the response is more recent: the read does not replace it.
        if (lastAutoUpdate.current === null) receive(first, false);
      })
      .catch(() => {});
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [markSearchStale]);

  // An installation that finishes while the Installed tab is already open also makes it reread.
  useEffect(() => {
    if (tab !== "installed" || !installedStale) return;
    setInstalledStale(false);
    setReload((n) => n + 1);
  }, [tab, installedStale]);

  const active = installations?.find((installation) => installation.id === activeId) ?? null;
  // Only the events of a local zip (without a source), while one is being installed.
  useInstallProgress(active?.id ?? null, (event) => {
    if (zipRunning.current && event.source === null) setZipProgress(event);
  });

  async function installZip(paths: string[]) {
    if (zipBusy) {
      setZipMessage({ text: "A zip is already being installed: wait for it to finish.", error: true });
      return;
    }
    const picked = pickZip(paths, active !== null);
    if ("problem" in picked) {
      setZipMessage({ text: picked.problem, error: true });
      return;
    }
    if (active === null) return;
    const target = active;
    setZipBusy(true);
    zipRunning.current = true;
    setZipProgress(undefined);
    setZipMessage(null);
    try {
      const result = await api.installZip(target.id, picked.path);
      setZipMessage({
        text: describeZipInstall(result, fileName(picked.path), target.name),
        // A backend warning (for example records not updated) is shown as an error.
        error: Boolean(result.warning),
      });
      // The addons on disk changed: the Installed tab rereads.
      setInstalledStale(true);
    } catch (err) {
      setZipMessage({
        text: `Could not install ${fileName(picked.path)}: ${errorMessage(err)}`,
        error: true,
      });
    } finally {
      // Even a failed zip may have changed the disk: Search rereads its results.
      markSearchStale(target.id);
      setZipBusy(false);
      // There is no final phase: the bar closes when the command returns the outcome.
      zipRunning.current = false;
      setZipProgress(undefined);
    }
  }

  async function chooseZip() {
    try {
      const selected = await open({
        multiple: false,
        directory: false,
        title: "Choose an addon .zip",
        filters: [{ name: "Zip archives", extensions: ["zip", "ZIP"] }],
      });
      // Picker cancelled: nothing happens.
      if (typeof selected !== "string") return;
      await installZip([selected]);
    } catch (err) {
      setZipMessage({ text: `The file picker could not be opened: ${errorMessage(err)}`, error: true });
    }
  }

  function changeInstallations(list: Installation[]) {
    setInstallations(list);
    if (!list.some((installation) => installation.id === activeId)) {
      setActiveId(list[0]?.id ?? null);
    }
  }

  /** Switches tab and moves focus to the chosen tab (arrows and links to Options). */
  function focusTab(next: Tab) {
    setTab(next);
    tabRefs.current[next]?.focus();
  }

  // Tab bar keyboard (WAI-ARIA pattern): arrows to move, Home and End to go to the ends. The tab
  // is activated together with the focus.
  function onTabKeyDown(event: KeyboardEvent<HTMLDivElement>) {
    const index = TABS.findIndex((t) => t.id === tab);
    let next: number;
    switch (event.key) {
      case "ArrowRight":
        next = (index + 1) % TABS.length;
        break;
      case "ArrowLeft":
        next = (index - 1 + TABS.length) % TABS.length;
        break;
      case "Home":
        next = 0;
        break;
      case "End":
        next = TABS.length - 1;
        break;
      default:
        return;
    }
    event.preventDefault();
    focusTab(TABS[next].id);
  }

  return (
    <div className="app">
      <header className="app-header">
        <h1>WoW-don</h1>
        <div className="tabs" role="tablist" aria-label="Sections" onKeyDown={onTabKeyDown}>
          {TABS.map((t) => (
            <button
              key={t.id}
              ref={(element) => {
                tabRefs.current[t.id] = element;
              }}
              id={`tab-${t.id}`}
              className="tab"
              role="tab"
              aria-selected={tab === t.id}
              aria-controls={`panel-${t.id}`}
              tabIndex={tab === t.id ? 0 : -1}
              onClick={() => focusTab(t.id)}
            >
              {t.label}
            </button>
          ))}
        </div>
        {installations !== null && installations.length > 0 && (
          <select
            className="select header-select"
            aria-label="Active installation"
            title={active?.path}
            value={activeId ?? ""}
            onChange={(event) => setActiveId(event.target.value)}
          >
            {installations.map((installation) => (
              <option key={installation.id} value={installation.id}>
                {installation.name}
              </option>
            ))}
          </select>
        )}
      </header>

      <main className="app-main">
        {error && (
          <p className="message error" role="alert">
            {error}
          </p>
        )}
        {installations === null && !error && <p className="muted">Loading…</p>}
        {zipBusy && (
          <p className="message">
            <ProgressBar value={null} label={progressLabel(zipProgress, "Starting…")} name="Zip" />
          </p>
        )}
        {zipMessage && (
          <p
            className={zipMessage.error ? "message error" : "message"}
            role={zipMessage.error ? "alert" : "status"}
          >
            {zipMessage.text}{" "}
            <button className="link subtle" onClick={() => setZipMessage(null)}>
              Dismiss
            </button>
          </p>
        )}

        {/* The tabs stay mounted and hidden: search, filters and checks in progress are not
            lost when switching from one tab to another. */}
        <div
          id="panel-installed"
          role="tabpanel"
          aria-labelledby="tab-installed"
          hidden={tab !== "installed"}
        >
          {active && (
            <AddonTable
              installation={active}
              reload={reload}
              onInstallZip={() => void chooseZip()}
              zipBusy={zipBusy}
              onChanged={markSearchStale}
              autoUpdateLine={
                <AutoUpdateLine
                  key={active.id}
                  status={autoUpdateStatus}
                  installationId={active.id}
                  mode={autoUpdateMode}
                  undoneRound={undoneRounds.get(active.id) ?? null}
                  onUndone={(installationId, round) => {
                    if (round !== null) {
                      setUndoneRounds((previous) => new Map(previous).set(installationId, round));
                    }
                    setInstalledStale(true);
                    markSearchStale(installationId);
                  }}
                />
              }
            />
          )}
          {installations !== null && installations.length === 0 && (
            <p className="empty">
              No installations yet. Add one in{" "}
              <button className="link inline" onClick={() => focusTab("options")}>
                Options
              </button>
              .
            </p>
          )}
        </div>

        <div
          id="panel-search"
          role="tabpanel"
          aria-labelledby="tab-search"
          hidden={tab !== "search"}
        >
          {active && (
            <Search
              installation={active}
              active={tab === "search"}
              keyConfigured={status?.curseforgeKeyConfigured ?? null}
              betaWhenNoRelease={betaWhenNoRelease}
              onInstalled={() => setInstalledStale(true)}
              stale={searchStale}
              onRefreshed={searchRefreshed}
            />
          )}
          {installations !== null && installations.length === 0 && (
            <p className="empty">
              Search needs an installation to know the game version. Add one in{" "}
              <button className="link inline" onClick={() => focusTab("options")}>
                Options
              </button>
              .
            </p>
          )}
        </div>

        <div
          id="panel-options"
          role="tabpanel"
          aria-labelledby="tab-options"
          hidden={tab !== "options"}
          className="options-page"
        >
          <h2 className="options-title">Options</h2>
          {installations !== null && (
            <Installations
              active={tab === "options"}
              installations={installations}
              onAdded={setActiveId}
              onChange={changeInstallations}
            />
          )}
          <Appearance theme={theme} loadError={settingsError} onChange={setTheme} />
          <BetaOption
            enabled={betaWhenNoRelease}
            onChange={(enabled) => {
              setBetaWhenNoRelease(enabled);
              // The Installed tab statuses depend on the option: they are reread.
              setInstalledStale(true);
            }}
          />
          {installations !== null && (
            <AutoUpdateOptions
              active={tab === "options"}
              installations={installations}
              onModeChange={setAutoUpdateMode}
            />
          )}
          <SystemOptions active={tab === "options"} />
          {installations !== null && (
            <ImportExport
              installation={active ?? null}
              onImported={(installationId) => {
                setInstalledStale(true);
                markSearchStale(installationId);
              }}
            />
          )}
        </div>
      </main>

      <ZipDrop target={active?.name ?? null} onDrop={(paths) => void installZip(paths)} />

      {status && (
        <footer className="app-footer">
          <span>v{status.version}</span>
          <span>
            CurseForge:{" "}
            {status.curseforgeKeyConfigured ? "key configured" : "key missing"}
          </span>
        </footer>
      )}
    </div>
  );
}

export default App;
