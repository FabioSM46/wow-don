import { useEffect, useState } from "react";
import { api, errorMessage, type AppStatus, type Installation } from "./api";
import { AddonTable } from "./addons/AddonTable";
import { Installations } from "./installations/Installations";
import "./App.css";

function App() {
  const [status, setStatus] = useState<AppStatus | null>(null);
  const [installations, setInstallations] = useState<Installation[] | null>(null);
  // L'installazione attiva vale solo per la sessione.
  const [activeId, setActiveId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

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

  return (
    <div className="app">
      <header className="app-header">
        <h1>WoW-don</h1>
      </header>

      <main className="app-main">
        {error && (
          <p className="message error" role="alert">
            {error}
          </p>
        )}
        {installations === null && !error && <p className="muted">Caricamento…</p>}
        {installations !== null && (
          <Installations
            installations={installations}
            activeId={activeId}
            onSelect={setActiveId}
            onChange={changeInstallations}
          />
        )}
        {active && <AddonTable installation={active} />}
      </main>

      {status && (
        <footer className="app-footer">
          <span>v{status.version}</span>
          <span>
            CurseForge:{" "}
            {status.curseforgeKeyConfigured ? "chiave configurata" : "chiave mancante"}
          </span>
        </footer>
      )}
    </div>
  );
}

export default App;
