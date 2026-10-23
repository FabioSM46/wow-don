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

/**
 * Un addon come lo vede l'utente: una cartella principale e le cartelle che arrivano con lei
 * (per esempio `QuestieDB` con `Questie`, o le cartelle di DBM).
 */
export type AddonGroup = {
  main: InstalledAddon;
  /** Le altre cartelle del gruppo, in ordine alfabetico. */
  others: InstalledAddon[];
};

const lower = (s: string) => s.toLocaleLowerCase("it");

/**
 * Raggruppa le cartelle che formano un solo addon. Tre regole:
 *
 * - **dai `.toc`, senza rete**: B va con A se A dichiara B tra le dipendenze obbligatorie e il
 *   nome di B inizia con il nome di A (`Questie` richiede `QuestieDB`);
 * - **dal campo `## Group`**, come fa il gioco: B va con A se il `group` di B è il nome di A
 *   (i moduli `EllesmereUI*` dichiarano `EllesmereUI`); se A non è installata, niente;
 * - **da CurseForge**, se c'è il risultato del controllo: le cartelle dello stesso progetto.
 *
 * Una cartella finisce in un solo gruppo. La principale è la cartella indicata da `group`, se
 * ce n'è una nel gruppo; poi, tra le cartelle che non sono moduli `LoadOnDemand` (se ce ne
 * sono), quella che nessun'altra del gruppo richiede; a parità il nome più corto, poi l'ordine
 * alfabetico. Così `EllesmereUI` guida i suoi moduli, `Questie` guida `QuestieDB` e `DBM-Core`
 * guida i moduli di DBM che caricano su richiesta.
 */
export function groupAddons(
  addons: InstalledAddon[],
  updates?: Map<string, AddonUpdate>,
): AddonGroup[] {
  const byFolder = new Map(addons.map((a) => [lower(a.folder), a]));
  const parent = new Map<string, string>(addons.map((a) => [a.folder, a.folder]));
  const root = (folder: string): string => {
    let current = folder;
    while (parent.get(current) !== current) current = parent.get(current)!;
    parent.set(folder, current);
    return current;
  };
  const join = (a: string, b: string) => {
    const ra = root(a);
    const rb = root(b);
    if (ra !== rb) parent.set(rb, ra);
  };

  for (const addon of addons) {
    for (const dependency of addon.dependencies) {
      const other = byFolder.get(lower(dependency));
      if (other && other !== addon && lower(other.folder).startsWith(lower(addon.folder))) {
        join(addon.folder, other.folder);
      }
    }
  }
  for (const addon of addons) {
    const target = addon.group ? byFolder.get(lower(addon.group)) : undefined;
    if (target && target !== addon) join(target.folder, addon.folder);
  }
  if (updates) {
    const firstOfProject = new Map<number, string>();
    for (const addon of addons) {
      const project = updates.get(addon.folder)?.project;
      if (!project) continue;
      const first = firstOfProject.get(project.id);
      if (first) join(first, addon.folder);
      else firstOfProject.set(project.id, addon.folder);
    }
  }

  const members = new Map<string, InstalledAddon[]>();
  for (const addon of addons) {
    const key = root(addon.folder);
    members.set(key, [...(members.get(key) ?? []), addon]);
  }
  return [...members.values()].map((group) => {
    const main = pickMain(group);
    const others = group
      .filter((a) => a !== main)
      .sort((a, b) => a.folder.localeCompare(b.folder));
    return { main, others };
  });
}

function pickMain(group: InstalledAddon[]): InstalledAddon {
  // La cartella indicata dal `## Group` delle cartelle del gruppo lo guida.
  const named = new Set(group.flatMap((a) => (a.group ? [lower(a.group)] : [])));
  const leaders = group.filter((a) => named.has(lower(a.folder)));
  const scope = leaders.length > 0 ? leaders : group;
  const loaded = scope.filter((a) => !a.loadOnDemand);
  const candidates = loaded.length > 0 ? loaded : scope;
  const required = new Set(
    candidates.flatMap((a) => a.dependencies.map(lower)).filter(Boolean),
  );
  const notRequired = candidates.filter((a) => !required.has(lower(a.folder)));
  const pool = notRequired.length > 0 ? notRequired : candidates;
  return [...pool].sort(
    (a, b) => a.folder.length - b.folder.length || a.folder.localeCompare(b.folder),
  )[0];
}

/**
 * Lo stato di aggiornamento del gruppo. Riguarda il **progetto**, non una cartella: le
 * cartelle di un progetto arrivano dallo stesso file pubblicato. Vince un aggiornamento
 * disponibile in una cartella qualsiasi, poi uno stato con progetto, poi quello della
 * principale.
 */
export function groupUpdate(
  group: AddonGroup,
  updates?: Map<string, AddonUpdate>,
): AddonUpdate | undefined {
  if (!updates) return undefined;
  const all = [group.main, ...group.others].map((a) => updates.get(a.folder));
  return (
    all.find((u) => u?.status === "updateAvailable") ??
    all.find((u) => u?.project) ??
    updates.get(group.main.folder)
  );
}

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

/**
 * I filtri su un gruppo: autore, LoadOnDemand e testo guardano la cartella principale (il testo
 * anche i nomi delle altre cartelle); un errore in una cartella qualsiasi conta; l'aggiornamento
 * è quello del gruppo.
 */
export function matchesGroup(
  group: AddonGroup,
  filters: Filters,
  updates?: Map<string, AddonUpdate>,
): boolean {
  const update = groupUpdate(group, updates);
  if (filters.onlyErrors && ![group.main, ...group.others].some((a) => Boolean(a.error))) {
    return false;
  }
  const withoutErrorFilter = { ...filters, onlyErrors: false, query: "" };
  if (!matchesFilters(group.main, withoutErrorFilter, update)) return false;
  const query = filters.query.trim();
  if (query === "") return true;
  return (
    matchesFilters(group.main, { ...defaultFilters, query }) ||
    group.others.some((other) => lower(other.folder).includes(lower(query)))
  );
}

/** Filtra e ordina gruppi già calcolati con `groupAddons`: conteggio e righe usano gli stessi. */
export function visibleGroups(
  groups: AddonGroup[],
  filters: Filters,
  sort: Sort,
  updates?: Map<string, AddonUpdate>,
): AddonGroup[] {
  const filtered = groups.filter((g) => matchesGroup(g, filters, updates));
  const order = new Map(
    sortAddons(
      filtered.map((g) => g.main),
      sort,
    ).map((main, index) => [main.folder, index]),
  );
  return filtered.sort((a, b) => order.get(a.main.folder)! - order.get(b.main.folder)!);
}

/** Clic su un'intestazione: stessa colonna inverte la direzione, colonna nuova parte da quella naturale. */
export function nextSort(current: Sort, key: SortKey): Sort {
  if (current.key === key) {
    return { key, direction: current.direction === "asc" ? "desc" : "asc" };
  }
  // Per la data, la scelta naturale è "più recenti prima".
  return { key, direction: key === "modified" ? "desc" : "asc" };
}
