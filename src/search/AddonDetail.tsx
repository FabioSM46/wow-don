import { useEffect, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
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
import { Gallery } from "./Gallery";
import { galleryLabel, tabIndexForKey } from "./gallery";

type Props = {
  installationId: string;
  /** La riga scelta nei risultati: l'intestazione si mostra subito, prima del dettaglio. */
  addon: CatalogAddon;
  /** La versione del gioco scelta nella ricerca; `null` lascia decidere al backend. */
  versionType: number | null;
  versionTypes: VersionType[];
  /** Cambia quando gli addon dell'installazione sono cambiati: il dettaglio si rilegge. */
  refresh?: number;
  /** La tab è visibile: Esc torna ai risultati solo allora. */
  active: boolean;
  onBack: () => void;
  /** Le azioni sull'addon (per esempio Install), a destra dell'intestazione. */
  renderActions?: (addon: CatalogAddon) => ReactNode;
  /** Messaggi sull'addon (per esempio l'esito di un'installazione), sotto l'intestazione. */
  children?: ReactNode;
  /** Per un addon che CurseForge non lascia scaricare: la frase sotto il nome, con l'esito del
   *  mirror GitHub (issue #165). Senza, resta la frase sul download dal sito. */
  blockedNote?: (addon: CatalogAddon) => ReactNode;
};

type Load =
  | { status: "loading" }
  | { status: "error"; message: string }
  | { status: "ready"; detail: AddonDetail };

/** Il dettaglio di un addon dentro la tab Search: descrizione, gallery, file e dipendenze. */
export function AddonDetailView({
  installationId,
  addon,
  versionType,
  versionTypes,
  refresh = 0,
  active,
  onBack,
  renderActions,
  children,
  blockedNote,
}: Props) {
  const [load, setLoad] = useState<Load>({ status: "loading" });
  const [retry, setRetry] = useState(0);
  const [linkError, setLinkError] = useState<string | null>(null);
  const backRef = useRef<HTMLButtonElement>(null);

  // Il dettaglio già caricato resta in vista mentre si rilegge lo stesso addon (`refresh`).
  const loadedKey = useRef<string | null>(null);
  useEffect(() => {
    // Solo la risposta dell'ultima richiesta si applica (un altro addon, un nuovo tentativo).
    let current = true;
    const key = `${installationId}:${addon.id}:${versionType}`;
    const same = loadedKey.current === key;
    loadedKey.current = key;
    setLoad((previous) => (same && previous.status === "ready" ? previous : { status: "loading" }));
    api
      .addonDetail(installationId, addon.id, versionType)
      .then((detail) => current && setLoad({ status: "ready", detail }))
      .catch((err) => current && setLoad({ status: "error", message: errorMessage(err) }));
    return () => {
      current = false;
    };
  }, [installationId, addon.id, versionType, retry, refresh]);

  // Il focus va sul pulsante per tornare indietro: da tastiera la vista si lascia subito.
  useEffect(() => {
    backRef.current?.focus({ preventScroll: true });
  }, [addon.id]);

  useEffect(() => {
    if (!active) return;
    function onKeyDown(event: globalThis.KeyboardEvent) {
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
          {!shown.allowModDistribution &&
            (blockedNote ? (
              blockedNote(shown)
            ) : (
              <p className="hint">
                The author does not allow downloads from other apps: get it from the CurseForge
                website.
              </p>
            ))}
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

      {/* La chiave riapre la tab Description quando cambia l'addon. */}
      {detail && (
        <DetailBody
          key={detail.id}
          detail={detail}
          versionTypes={versionTypes}
          active={active}
        />
      )}
    </article>
  );
}

type DetailTab = "description" | "gallery";

const DETAIL_TABS: DetailTab[] = ["description", "gallery"];

function DetailBody({
  detail,
  versionTypes,
  active,
}: {
  detail: AddonDetail;
  versionTypes: VersionType[];
  active: boolean;
}) {
  const [tab, setTab] = useState<DetailTab>("description");
  const tabRefs = useRef<Partial<Record<DetailTab, HTMLButtonElement | null>>>({});
  const text = paragraphs(detail.description);
  const version = versionName(versionTypes, detail.versionType);
  const labels: Record<DetailTab, string> = {
    description: "Description",
    gallery: galleryLabel(detail.screenshots.length),
  };

  function focusTab(next: DetailTab) {
    setTab(next);
    tabRefs.current[next]?.focus();
  }

  // Stesso pattern delle tab dell'app: frecce con giro, Home e Fine; la tab si attiva insieme
  // al focus.
  function onTabKeyDown(event: KeyboardEvent<HTMLDivElement>) {
    const next = tabIndexForKey(event.key, DETAIL_TABS.indexOf(tab), DETAIL_TABS.length);
    if (next === null) return;
    event.preventDefault();
    focusTab(DETAIL_TABS[next]);
  }

  return (
    <div className="detail-body">
      <div className="detail-main">
        <div
          className="tabs detail-tabs"
          role="tablist"
          aria-label="Addon details"
          onKeyDown={onTabKeyDown}
        >
          {DETAIL_TABS.map((id) => (
            <button
              key={id}
              ref={(element) => {
                tabRefs.current[id] = element;
              }}
              id={`detail-tab-${id}`}
              className="tab"
              role="tab"
              aria-selected={tab === id}
              aria-controls={`detail-panel-${id}`}
              tabIndex={tab === id ? 0 : -1}
              onClick={() => focusTab(id)}
            >
              {labels[id]}
            </button>
          ))}
        </div>

        <div
          id="detail-panel-description"
          className="detail-panel detail-description"
          role="tabpanel"
          aria-labelledby="detail-tab-description"
          hidden={tab !== "description"}
        >
          {text.length === 0 ? (
            <p className="empty">The author has not written a description.</p>
          ) : (
            // Testo semplice: React lo mostra come testo, mai come HTML.
            text.map((paragraph, index) => <p key={index}>{paragraph}</p>)
          )}
        </div>

        <div
          id="detail-panel-gallery"
          className="detail-panel"
          role="tabpanel"
          aria-labelledby="detail-tab-gallery"
          hidden={tab !== "gallery"}
        >
          {tab === "gallery" && (
            <Gallery screenshots={detail.screenshots} active={active} />
          )}
        </div>
      </div>

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
