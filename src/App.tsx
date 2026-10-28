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

/** Le tab dell'app, nell'ordine in cui compaiono. */
const TABS = [
  { id: "search", label: "Search" },
  { id: "installed", label: "Installed" },
  { id: "options", label: "Options" },
] as const;

type Tab = (typeof TABS)[number]["id"];

function App() {
  const [status, setStatus] = useState<AppStatus | null>(null);
  const [installations, setInstallations] = useState<Installation[] | null>(null);
  // La tab e l'installazione attive valgono solo per la sessione.
  const [tab, setTab] = useState<Tab>("installed");
  const [activeId, setActiveId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  // Un'installazione dalla tab Search cambia gli addon sul disco: tornando sulla tab Installed
  // la lista si rilegge (`reload` cambia), una volta sola per quante installazioni ci sono state.
  const [installedStale, setInstalledStale] = useState(false);
  const [reload, setReload] = useState(0);
  // Il contrario: un addon tolto, reinstallato o installato fuori dalla Search (tab Installed,
  // zip, importazione) rende vecchi i risultati della Search per quell'installazione. La Search
  // li rilegge quando torna visibile.
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
  // Il tema salvato dal backend; finché non arriva vale quello del sistema (niente localStorage:
  // lo stato sta nel backend, un breve lampo all'avvio è accettato).
  const [theme, setTheme] = useState<Theme | null>(null);
  // L'opzione della Beta (issue #141); `null` finché le impostazioni non sono lette.
  const [betaWhenNoRelease, setBetaWhenNoRelease] = useState<boolean | null>(null);
  const [settingsError, setSettingsError] = useState<string | null>(null);
  // L'aggiornamento automatico (D41, issue #203): la modalità salvata (`null` finché non è letta)
  // e lo stato dell'ultimo giro, da `get_auto_update_status` e dall'evento `auto-update`.
  const [autoUpdateMode, setAutoUpdateMode] = useState<AutoUpdateMode | null>(null);
  const [autoUpdateStatus, setAutoUpdateStatus] = useState<AutoUpdateStatus | null>(null);
  const lastAutoUpdate = useRef<AutoUpdateStatus | null>(null);
  // Id dell'installazione → `finishedAt` del giro che l'utente ha annullato: il suo esito non
  // descrive più il disco e la riga non lo mostra.
  const [undoneRounds, setUndoneRounds] = useState<ReadonlyMap<string, string>>(() => new Map());
  // Installazione di uno zip trascinato o scelto (D32): una alla volta, con il suo esito.
  const [zipBusy, setZipBusy] = useState(false);
  // La fase dell'installazione da zip (issue #128). Il ref è aggiornato subito: il primo evento
  // può arrivare prima che `zipBusy` sia riletto.
  const [zipProgress, setZipProgress] = useState<InstallProgress | undefined>(undefined);
  const zipRunning = useRef(false);
  const [zipMessage, setZipMessage] = useState<{ text: string; error: boolean } | null>(null);

  useEffect(() => {
    if (theme !== null) applyTheme(document.documentElement, theme);
  }, [theme]);

  // Il menu di default del browser (Reload, Inspect…) non serve in un'app desktop (issue #149):
  // resta solo nei campi di testo, per Copy e Paste. I menu dell'app si aprono dai loro elementi.
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
        // Senza installazioni c'è una sola cosa da fare: aggiungerne una, in Options.
        if (list.length === 0) setTab("options");
      })
      .catch((err) => setError(errorMessage(err)));
  }, []);

  // Lo stato dell'aggiornamento automatico: letto all'apertura, poi a ogni evento. Un giro
  // `install` nuovo cambia gli addon sul disco: Installed e Search si rileggono, come dopo
  // un'installazione dalla Search.
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | null = null;
    const receive = (next: AutoUpdateStatus, fromEvent: boolean) => {
      if (disposed) return;
      const previous = lastAutoUpdate.current;
      lastAutoUpdate.current = next;
      setAutoUpdateStatus(next);
      // Solo la lettura all'apertura è la linea di base: la lista appena aperta è già quella del
      // disco. Un evento arrivato per primo può essere di un giro finito dopo la lettura della
      // lista, e quindi conta.
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
      // Fuori da Tauri non arriva niente: la riga resta nascosta.
      .catch(() => {});
    api
      .getAutoUpdateStatus()
      .then((first) => {
        // Un evento arrivato prima della risposta è più recente: la lettura non lo sostituisce.
        if (lastAutoUpdate.current === null) receive(first, false);
      })
      .catch(() => {});
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [markSearchStale]);

  // Anche un'installazione che finisce mentre la tab Installed è già aperta la fa rileggere.
  useEffect(() => {
    if (tab !== "installed" || !installedStale) return;
    setInstalledStale(false);
    setReload((n) => n + 1);
  }, [tab, installedStale]);

  const active = installations?.find((installation) => installation.id === activeId) ?? null;
  // Solo gli eventi di uno zip locale (senza fonte), mentre se ne installa uno.
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
        // Un avviso del backend (per esempio record non aggiornati) si mostra come errore.
        error: Boolean(result.warning),
      });
      // Gli addon sul disco sono cambiati: la tab Installed si rilegge.
      setInstalledStale(true);
    } catch (err) {
      setZipMessage({
        text: `Could not install ${fileName(picked.path)}: ${errorMessage(err)}`,
        error: true,
      });
    } finally {
      // Anche uno zip fallito può aver cambiato il disco: la Search rilegge i suoi risultati.
      markSearchStale(target.id);
      setZipBusy(false);
      // Non c'è una fase finale: la barra si chiude quando il comando restituisce l'esito.
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
      // Selettore annullato: non succede niente.
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

  /** Cambia tab e porta il focus sulla tab scelta (frecce e link verso Options). */
  function focusTab(next: Tab) {
    setTab(next);
    tabRefs.current[next]?.focus();
  }

  // Tastiera della barra di tab (pattern WAI-ARIA): frecce per spostarsi, Home e Fine per
  // andare agli estremi. La tab si attiva insieme al focus.
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

        {/* Le tab restano montate e nascoste: ricerca, filtri e controlli in corso non si
            perdono passando da una tab all'altra. */}
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
              // Gli stati della tab Installed dipendono dall'opzione: si rileggono.
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
