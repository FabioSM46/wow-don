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
import { ExternalLinkIcon } from "./icons";
import { galleryLabel, tabIndexForKey } from "./screenshots";

type Props = {
  installationId: string;
  /** The row chosen in the results: the header shows at once, before the detail. */
  addon: CatalogAddon;
  /** The game version chosen in the search; `null` lets the backend decide. */
  versionType: number | null;
  versionTypes: VersionType[];
  /** Changes when the installation's addons have changed: the detail is read again. */
  refresh?: number;
  /** The tab is visible: only then does Esc go back to the results. */
  active: boolean;
  onBack: () => void;
  /** The actions on the addon (for example Install), on the right of the header. */
  renderActions?: (addon: CatalogAddon) => ReactNode;
  /** Messages about the addon (for example the outcome of an install), under the header. */
  children?: ReactNode;
  /** For an addon CurseForge does not let you download: the box under the header, with the
   *  outcome of the GitHub mirror (issue #165). Without it, the sentence on downloading from
   *  the website remains. */
  blockedNote?: (addon: CatalogAddon) => ReactNode;
};

type Load =
  | { status: "loading" }
  | { status: "error"; message: string }
  | { status: "ready"; detail: AddonDetail };

/** An addon's detail inside the Search tab: description, gallery, files and dependencies. */
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

  // The detail already loaded stays in view while the same addon is read again (`refresh`).
  const loadedKey = useRef<string | null>(null);
  useEffect(() => {
    // Only the answer to the latest request applies (another addon, a new attempt).
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

  // Focus goes to the back button: from the keyboard the view can be left at once.
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
  // After loading, the detail's data applies, newer than the row's; the row's "installed"
  // mark applies afterwards too, because an install made after loading
  // changes it only there.
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
            {shown.installed && <span className="pill ok">Installed</span>}
          </h2>
          <p className="detail-meta">
            <span>{formatAuthors(shown.authors)}</span>
            <span title={formatCount(shown.downloadCount)}>
              {formatDownloads(shown.downloadCount)} downloads
            </span>
            <span>Updated {formatDate(shown.dateModified)}</span>
          </p>
          <p className="detail-meta">{formatCategories(shown.categories)}</p>
          {linkError && (
            <p className="detail-meta error" role="alert">
              {linkError}
            </p>
          )}
        </div>
        {(shown.url || renderActions) && (
          <div className="detail-actions">
            {/* The detail's only link to CurseForge (issue #229). */}
            {shown.url && (
              <button
                className="button ghost"
                onClick={() => {
                  setLinkError(null);
                  openUrl(shown.url!).catch((err) =>
                    setLinkError(`The page could not be opened: ${errorMessage(err)}`),
                  );
                }}
              >
                <ExternalLinkIcon />
                Open on CurseForge
              </button>
            )}
            {renderActions?.(shown)}
          </div>
        )}
      </header>

      {/* Why a blocked addon does not install (or how it installs): a box under the
          header, not a tiny note (issue #229). */}
      {!shown.allowModDistribution &&
        (blockedNote ? (
          blockedNote(shown)
        ) : (
          <p className="detail-notice">
            The author does not allow downloads from other apps: get it from the CurseForge
            website.
          </p>
        ))}

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

      {/* The key reopens the Description tab when the addon changes. */}
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

  // Same pattern as the app's tabs: arrows that wrap around, Home and End; the tab activates
  // together with the focus.
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
            // Plain text: React shows it as text, never as HTML.
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
