import { useEffect, useRef, useState, type ReactNode } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { api, errorMessage, type AddonDetail, type CatalogAddon, type VersionType } from "../api";
import { formatAuthors, formatCount, formatDate, formatDownloads } from "./catalog";
import {
  dependencyName,
  fileMeta,
  fileName,
  formatCategories,
  paragraphs,
  versionName,
} from "./detail";

type Props = {
  installationId: string;
  /** La riga scelta nei risultati: l'intestazione si mostra subito, prima del dettaglio. */
  addon: CatalogAddon;
  /** La versione del gioco scelta nella ricerca; `null` lascia decidere al backend. */
  versionType: number | null;
  versionTypes: VersionType[];
  /** La tab è visibile: Esc torna ai risultati solo allora. */
  active: boolean;
  onBack: () => void;
  /** Le azioni sull'addon (per esempio Install), a destra dell'intestazione. */
  renderActions?: (addon: CatalogAddon) => ReactNode;
  /** Messaggi sull'addon (per esempio l'esito di un'installazione), sotto l'intestazione. */
  children?: ReactNode;
};

type Load =
  | { status: "loading" }
  | { status: "error"; message: string }
  | { status: "ready"; detail: AddonDetail };

/** Il dettaglio di un addon dentro la tab Search: descrizione, file e dipendenze. */
export function AddonDetailView({
  installationId,
  addon,
  versionType,
  versionTypes,
  active,
  onBack,
  renderActions,
  children,
}: Props) {
  const [load, setLoad] = useState<Load>({ status: "loading" });
  const [retry, setRetry] = useState(0);
  const [linkError, setLinkError] = useState<string | null>(null);
  const backRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    // Solo la risposta dell'ultima richiesta si applica (un altro addon, un nuovo tentativo).
    let current = true;
    setLoad({ status: "loading" });
    api
      .addonDetail(installationId, addon.id, versionType)
      .then((detail) => current && setLoad({ status: "ready", detail }))
      .catch((err) => current && setLoad({ status: "error", message: errorMessage(err) }));
    return () => {
      current = false;
    };
  }, [installationId, addon.id, versionType, retry]);

  // Il focus va sul pulsante per tornare indietro: da tastiera la vista si lascia subito.
  useEffect(() => {
    backRef.current?.focus({ preventScroll: true });
  }, [addon.id]);

  useEffect(() => {
    if (!active) return;
    function onKeyDown(event: KeyboardEvent) {
      if (event.key !== "Escape" || event.defaultPrevented) return;
      event.preventDefault();
      onBack();
    }
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [active, onBack]);

  const detail = load.status === "ready" ? load.detail : null;
  // Dopo il caricamento valgono i dati del dettaglio, più recenti di quelli della riga; il segno
  // "installato" della riga vale anche dopo, perché un'installazione fatta dopo il caricamento
  // lo cambia solo lì.
  const shown: CatalogAddon = detail
    ? { ...detail, installed: detail.installed || addon.installed }
    : addon;
  const name = shown.name || `#${shown.id}`;

  return (
    <article className="detail" aria-label={`${name} details`}>
      <button ref={backRef} className="link inline back" onClick={onBack}>
        ← Back to results
      </button>

      <header className="detail-header">
        {shown.thumbnailUrl ? (
          <img className="detail-logo" src={shown.thumbnailUrl} alt="" width={64} height={64} />
        ) : (
          <span className="detail-logo" />
        )}
        <div className="detail-heading">
          <h2 className="detail-name">
            {name}
            {shown.installed && <span className="tag">Installed</span>}
          </h2>
          <p className="detail-meta">
            <span>{formatAuthors(shown.authors)}</span>
            <span title={formatCount(shown.downloadCount)}>
              {formatDownloads(shown.downloadCount)} downloads
            </span>
            <span>Updated {formatDate(shown.dateModified)}</span>
          </p>
          <p className="detail-meta">{formatCategories(shown.categories)}</p>
          {!shown.allowModDistribution && (
            <p className="hint">
              The author does not allow downloads from other apps: get it from the CurseForge
              website.
            </p>
          )}
          {shown.url && (
            <button
              className="link inline"
              onClick={() => {
                setLinkError(null);
                openUrl(shown.url!).catch((err) =>
                  setLinkError(`The page could not be opened: ${errorMessage(err)}`),
                );
              }}
            >
              View on CurseForge
            </button>
          )}
          {linkError && (
            <p className="detail-meta error" role="alert">
              {linkError}
            </p>
          )}
        </div>
        {renderActions && <div className="detail-actions">{renderActions(shown)}</div>}
      </header>

      {children}

      {load.status === "loading" && <p className="muted">Loading details…</p>}

      {load.status === "error" && (
        <p className="message error" role="alert">
          {load.message}{" "}
          <button className="link inline" onClick={() => setRetry((n) => n + 1)}>
            Try again
          </button>
        </p>
      )}

      {detail && <DetailBody detail={detail} versionTypes={versionTypes} />}
    </article>
  );
}

function DetailBody({
  detail,
  versionTypes,
}: {
  detail: AddonDetail;
  versionTypes: VersionType[];
}) {
  const text = paragraphs(detail.description);
  const version = versionName(versionTypes, detail.versionType);

  return (
    <div className="detail-body">
      <section className="detail-section detail-description" aria-label="Description">
        <h3>Description</h3>
        {text.length === 0 ? (
          <p className="empty">The author has not written a description.</p>
        ) : (
          // Testo semplice: React lo mostra come testo, mai come HTML.
          text.map((paragraph, index) => <p key={index}>{paragraph}</p>)
        )}
      </section>

      <div className="detail-side">
        <section className="detail-section" aria-label="Files">
          <h3>{version ? `Files for ${version}` : "Files"}</h3>
          {detail.files.length === 0 ? (
            <p className="empty">
              No files for this game version. Go back and choose another version in the search
              filters.
            </p>
          ) : (
            <ul className="detail-list">
              {detail.files.map((file) => (
                <li key={file.id}>
                  <span className="detail-file-name">{fileName(file)}</span>
                  <span className="detail-file-meta">{fileMeta(file)}</span>
                </li>
              ))}
            </ul>
          )}
        </section>

        <section className="detail-section" aria-label="Required dependencies">
          <h3>Required dependencies</h3>
          {detail.dependencies.length === 0 ? (
            <p className="empty">None.</p>
          ) : (
            <ul className="detail-list">
              {detail.dependencies.map((dependency) => (
                <li key={dependency.id}>{dependencyName(dependency)}</li>
              ))}
            </ul>
          )}
        </section>
      </div>
    </div>
  );
}
