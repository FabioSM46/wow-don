import { useEffect, useRef, useState, type KeyboardEvent } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { api, errorMessage, type AppStatus, type Installation, type Theme } from "./api";
import { AddonTable } from "./addons/AddonTable";
import { ZipDrop } from "./addons/ZipDrop";
import { ProgressBar, useInstallProgress } from "./addons/ProgressBar";
import { progressLabel, type InstallProgress } from "./addons/progress";
import { describeZipInstall, fileName, pickZip } from "./addons/zip";
import { Discovered } from "./installations/Discovered";
import { Installations } from "./installations/Installations";
import { Appearance } from "./options/Appearance";
import { applyTheme } from "./options/theme";
import { Search } from "./search/Search";
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
  const tabRefs = useRef<Partial<Record<Tab, HTMLButtonElement | null>>>({});
  // Il tema salvato dal backend; finché non arriva vale quello del sistema (niente localStorage:
  // lo stato sta nel backend, un breve lampo all'avvio è accettato).
  const [theme, setTheme] = useState<Theme | null>(null);
  const [settingsError, setSettingsError] = useState<string | null>(null);
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

  useEffect(() => {
    api
      .getSettings()
      .then((settings) => setTheme(settings.theme))
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
              onInstalled={() => setInstalledStale(true)}
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
        >
          {installations !== null && (
            <Discovered
              active={tab === "options"}
              installations={installations}
              onAdded={setActiveId}
              onChange={changeInstallations}
            />
          )}
          {installations !== null && (
            <Installations
              installations={installations}
              onAdded={setActiveId}
              onChange={changeInstallations}
            />
          )}
          <Appearance theme={theme} loadError={settingsError} onChange={setTheme} />
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
