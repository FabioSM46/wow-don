import { useEffect, useRef, useState, type KeyboardEvent } from "react";
import { api, errorMessage, type AppStatus, type Installation } from "./api";
import { AddonTable } from "./addons/AddonTable";
import { Installations } from "./installations/Installations";
import { Search } from "./search/Search";
import "./App.css";

/** Le tab dell'app, nell'ordine in cui compaiono. */
const TABS = [
  { id: "installed", label: "Installed" },
  { id: "search", label: "Search" },
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

  useEffect(() => {
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

  const active = installations?.find((installation) => installation.id === activeId) ?? null;

  function changeInstallations(list: Installation[]) {
    setInstallations(list);
    if (!list.some((installation) => installation.id === activeId)) {
      setActiveId(list[0]?.id ?? null);
    }
  }

  /** Cambia tab e porta il focus sulla tab scelta (frecce e link verso Options). */
  function focusTab(next: Tab) {
    setTab(next);
    if (next === "installed" && installedStale) {
      setInstalledStale(false);
      setReload((n) => n + 1);
    }
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

        {/* Le tab restano montate e nascoste: ricerca, filtri e controlli in corso non si
            perdono passando da una tab all'altra. */}
        <div
          id="panel-installed"
          role="tabpanel"
          aria-labelledby="tab-installed"
          hidden={tab !== "installed"}
        >
          {active && <AddonTable installation={active} reload={reload} />}
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
            <Installations
              installations={installations}
              onAdded={setActiveId}
              onChange={changeInstallations}
            />
          )}
        </div>
      </main>

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
