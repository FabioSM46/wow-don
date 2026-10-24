import type { KeyboardEvent, ReactNode } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import type { CatalogAddon } from "../api";
import { formatAuthors, formatCount, formatDate, formatDownloads } from "./catalog";

type Props = {
  addons: CatalogAddon[];
  /** Con un gestore la riga si sceglie con un clic o con Invio (vista di dettaglio). */
  onSelect?: (addon: CatalogAddon) => void;
  selectedId?: number | null;
  /** Le azioni di una riga (per esempio Install), in un'ultima colonna. */
  renderActions?: (addon: CatalogAddon) => ReactNode;
};

/** La tabella dei risultati del catalogo: logo, nome, sommario, autore, download, data. */
export function CatalogTable({ addons, onSelect, selectedId = null, renderActions }: Props) {
  function onRowKeyDown(event: KeyboardEvent<HTMLTableRowElement>, addon: CatalogAddon) {
    if (event.target !== event.currentTarget) return;
    if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      onSelect?.(addon);
    }
  }

  return (
    <div className="table-wrap">
      <table className="addon-table catalog-table">
        <thead>
          <tr>
            <th className="logo-cell" aria-label="Logo" />
            <th>Name</th>
            <th>Summary</th>
            <th>Author</th>
            <th className="numeric">Downloads</th>
            <th>Updated</th>
            {renderActions && <th aria-label="Actions" />}
          </tr>
        </thead>
        <tbody>
          {addons.map((addon) => (
            <tr
              key={addon.id}
              className={onSelect ? "selectable" : undefined}
              tabIndex={onSelect ? 0 : undefined}
              aria-selected={onSelect ? selectedId === addon.id : undefined}
              onClick={onSelect ? () => onSelect(addon) : undefined}
              onKeyDown={onSelect ? (event) => onRowKeyDown(event, addon) : undefined}
            >
              <td className="logo-cell">
                {addon.thumbnailUrl ? (
                  <img
                    className="catalog-logo"
                    src={addon.thumbnailUrl}
                    alt=""
                    width={28}
                    height={28}
                    loading="lazy"
                  />
                ) : (
                  <span className="catalog-logo" />
                )}
              </td>
              <td>
                <NameCell addon={addon} />
              </td>
              <td>
                <span className="summary" title={addon.summary || undefined}>
                  {addon.summary || "—"}
                </span>
              </td>
              <td>
                <span className="clip" title={formatAuthors(addon.authors)}>
                  {formatAuthors(addon.authors)}
                </span>
              </td>
              <td className="numeric nowrap" title={formatCount(addon.downloadCount)}>
                {formatDownloads(addon.downloadCount)}
              </td>
              <td className="nowrap">{formatDate(addon.dateModified)}</td>
              {renderActions && <td className="nowrap">{renderActions(addon)}</td>}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

/** Il nome (link alla pagina su CurseForge), il segno "Installed" e il download solo manuale. */
function NameCell({ addon }: { addon: CatalogAddon }) {
  const name = addon.name || `#${addon.id}`;
  return (
    <>
      {addon.url ? (
        <button
          className="link inline addon-title"
          title="Open on CurseForge"
          onClick={(event) => {
            // Il link non sceglie la riga.
            event.stopPropagation();
            void openUrl(addon.url!);
          }}
        >
          {name}
        </button>
      ) : (
        <span className="addon-title">{name}</span>
      )}
      {addon.installed && <span className="tag">Installed</span>}
      {!addon.allowModDistribution && (
        <span
          className="hint"
          title="The author does not allow downloads from other apps: get it from the CurseForge website."
        >
          Manual download only
        </span>
      )}
    </>
  );
}
