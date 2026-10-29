import type { KeyboardEvent, ReactNode } from "react";
import type { CatalogAddon } from "../api";
import { formatAuthors, formatCount, formatDate, formatDownloads } from "./catalog";
import { mirrorCheckFor } from "./mirror";
import { blockedPill, type StatePill } from "./pills";

type Props = {
  addons: CatalogAddon[];
  /** With a handler the row is chosen with a click or with Enter (detail view). */
  onSelect?: (addon: CatalogAddon) => void;
  selectedId?: number | null;
  /** A row's actions (for example Install), in a last column that is always visible. */
  renderActions?: (addon: CatalogAddon) => ReactNode;
  /** The pill of an addon CurseForge does not let you download, with the outcome of the GitHub
   *  mirror if it has been checked (issue #165, #229). */
  pillFor?: (addon: CatalogAddon) => StatePill;
};

/** The catalog results table: logo, name, summary, author, downloads, date. */
export function CatalogTable({
  addons,
  onSelect,
  selectedId = null,
  renderActions,
  pillFor = (addon) => blockedPill(mirrorCheckFor(addon, undefined)),
}: Props) {
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
            {renderActions && <th className="actions-cell" aria-label="Actions" />}
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
                <NameCell addon={addon} pill={addon.allowModDistribution ? null : pillFor(addon)} />
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
              {renderActions && <td className="actions-cell">{renderActions(addon)}</td>}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

/** The name and, below it, the state pills: "Installed" and the one for an addon CurseForge
 *  does not let you download. The CurseForge page opens from the detail (issue #229). */
function NameCell({ addon, pill }: { addon: CatalogAddon; pill: StatePill | null }) {
  const name = addon.name || `#${addon.id}`;
  return (
    <>
      <span className="addon-title catalog-name" title={name}>
        {name}
      </span>
      {(addon.installed || pill) && (
        <span className="catalog-states">
          {addon.installed && <span className="pill ok">Installed</span>}
          {pill && (
            <span className={pill.className} title={pill.title}>
              {pill.label}
            </span>
          )}
        </span>
      )}
    </>
  );
}
