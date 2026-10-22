import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import "./App.css";

type AppStatus = {
  version: string;
  curseforgeKeyConfigured: boolean;
};

function App() {
  const [status, setStatus] = useState<AppStatus | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    invoke<AppStatus>("app_status")
      .then(setStatus)
      .catch((err) => setError(String(err)));
  }, []);

  return (
    <main className="container">
      <h1>WoW-don</h1>
      <p className="subtitle">Gestore di addon per World of Warcraft su Linux</p>

      {error && <p className="status error">{error}</p>}
      {status && (
        <ul className="status">
          <li>Versione: {status.version}</li>
          <li>
            Chiave CurseForge:{" "}
            {status.curseforgeKeyConfigured
              ? "configurata"
              : "mancante (imposta CURSEFORGE_API_KEY in .env)"}
          </li>
        </ul>
      )}
    </main>
  );
}

export default App;
