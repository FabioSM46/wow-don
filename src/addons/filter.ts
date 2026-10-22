// Ricerca, filtri e ordinamento degli addon: funzioni pure, senza React né I/O.

import type { AddonUpdate, InstalledAddon } from "../api";

export type Filters = {
  query: string;
  /** `null` = tutti gli autori. */
  author: string | null;
  hideLoadOnDemand: boolean;
  onlyErrors: boolean;
  /** Solo gli addon con un aggiornamento disponibile (serve il risultato del controllo). */
  onlyUpdates: boolean;
};

export type SortKey = "title" | "author" | "modified";
export type Sort = { key: SortKey; direction: "asc" | "desc" };

export const defaultFilters: Filters = {
  query: "",
  author: null,
  hideLoadOnDemand: false,
  onlyErrors: false,
  onlyUpdates: false,
};

export const defaultSort: Sort = { key: "title", direction: "asc" };

/** Il titolo da mostrare: quello del `.toc`, altrimenti il nome della cartella. */
export function displayTitle(addon: InstalledAddon): string {
  return addon.title ?? addon.folder;
}

/** Gli autori presenti, senza ripetizioni, in ordine alfabetico. */
export function authorsOf(addons: InstalledAddon[]): string[] {
  const authors = new Set<string>();
  for (const addon of addons) {
    if (addon.author) authors.add(addon.author);
  }
  return [...authors].sort((a, b) => a.localeCompare(b, "it", { sensitivity: "base" }));
}

export function matchesFilters(
  addon: InstalledAddon,
  filters: Filters,
  update?: AddonUpdate,
): boolean {
  if (filters.onlyUpdates && update?.status !== "updateAvailable") return false;
  if (filters.author !== null && addon.author !== filters.author) return false;
  if (filters.hideLoadOnDemand && addon.loadOnDemand) return false;
  if (filters.onlyErrors && addon.error === null) return false;
  const query = filters.query.trim().toLocaleLowerCase("it");
  if (query === "") return true;
  return [addon.title, addon.folder, addon.author, addon.notes].some(
    (field) => field?.toLocaleLowerCase("it").includes(query) ?? false,
  );
}

function compare(a: InstalledAddon, b: InstalledAddon, key: SortKey): number {
  const text = (x: string, y: string) => x.localeCompare(y, "it", { sensitivity: "base" });
  switch (key) {
    case "title":
      return text(displayTitle(a), displayTitle(b));
    case "author":
      // Gli addon senza autore vanno in fondo in entrambe le direzioni (vedi sortAddons).
      return text(a.author ?? "", b.author ?? "");
    case "modified":
      return (a.modifiedMs ?? 0) - (b.modifiedMs ?? 0);
  }
}

export function sortAddons(addons: InstalledAddon[], sort: Sort): InstalledAddon[] {
  const sign = sort.direction === "asc" ? 1 : -1;
  return [...addons].sort((a, b) => {
    if (sort.key === "author" && (a.author === null) !== (b.author === null)) {
      return a.author === null ? 1 : -1;
    }
    return (
      sign * compare(a, b, sort.key) ||
      // A parità, il titolo e poi la cartella rendono l'ordine stabile.
      compare(a, b, "title") ||
      a.folder.localeCompare(b.folder)
    );
  });
}

export function visibleAddons(
  addons: InstalledAddon[],
  filters: Filters,
  sort: Sort,
  updates?: Map<string, AddonUpdate>,
): InstalledAddon[] {
  return sortAddons(
    addons.filter((addon) => matchesFilters(addon, filters, updates?.get(addon.folder))),
    sort,
  );
}

/** Clic su un'intestazione: stessa colonna inverte la direzione, colonna nuova parte da quella naturale. */
export function nextSort(current: Sort, key: SortKey): Sort {
  if (current.key === key) {
    return { key, direction: current.direction === "asc" ? "desc" : "asc" };
  }
  // Per la data, la scelta naturale è "più recenti prima".
  return { key, direction: key === "modified" ? "desc" : "asc" };
}
