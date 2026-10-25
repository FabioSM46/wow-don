// Ricerca, filtri e ordinamento degli addon: funzioni pure, senza React né I/O.

import type { AddonUpdate, InstalledAddon, SourceKind } from "../api";
import { projectId } from "./sources.ts";

export type Filters = {
  query: string;
  /** `null` = tutti gli autori. */
  author: string | null;
  hideLoadOnDemand: boolean;
  onlyErrors: boolean;
  /** Solo gli addon con un aggiornamento disponibile (serve il risultato del controllo). */
  onlyUpdates: boolean;
  /** Solo gli addon riconosciuti da questa fonte (serve il risultato del controllo); `null` = tutti. */
  source: SourceKind | null;
};

export type SortKey = "title" | "author" | "modified";
export type Sort = { key: SortKey; direction: "asc" | "desc" };

export const defaultFilters: Filters = {
  query: "",
  author: null,
  hideLoadOnDemand: false,
  onlyErrors: false,
  onlyUpdates: false,
  source: null,
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

/** I nomi di un `## Author`, separati da virgola, punto e virgola, `&` o `/`. */
const authorNames = (author: string | null): Set<string> =>
  new Set(
    (author ?? "")
      .split(/[,;&/]/)
      .map((name) => lower(name.trim()))
      .filter(Boolean),
  );

/**
 * Autori diversi tra due gruppi: nessun nome in comune tra tutti i nomi dell'uno e tutti quelli
 * dell'altro (`Elv, Simpy` e `fang2hou`). `Elv` e `Elv, Simpy` non lo sono, e un gruppo senza
 * autori non è una prova. Si confrontano i gruppi, non le singole cartelle: due moduli dello
 * stesso addon possono citare autori diversi tra quelli del principale.
 */
function disjointAuthors(x: InstalledAddon[], y: InstalledAddon[]): boolean {
  const names = (group: InstalledAddon[]) =>
    new Set(group.flatMap((a) => [...authorNames(a.author)]));
  const a = names(x);
  const b = names(y);
  return a.size > 0 && b.size > 0 && ![...a].some((name) => b.has(name));
}

/**
 * Raggruppa le cartelle che formano un solo addon. Le prove, dalla più forte:
 *
 * - **il file pubblicato** (dopo il controllo): le cartelle installate che compaiono nelle
 *   `projectFolders` di una cartella riconosciuta vanno con lei, anche se CurseForge non le
 *   riconosce una per una; le cartelle riconosciute come lo stesso progetto stanno insieme.
 *   **Il registro dell'app** (`installedProject`, le cartelle dello zip che l'app ha
 *   installato) è una prova della stessa forza e vale anche prima del controllo; per una
 *   cartella che il controllo riconosce prevale il file pubblicato, perché l'addon può essere
 *   stato aggiornato con un altro gestore;
 * - **il campo `## Group`**, come fa il gioco: B va con A se il `group` di B è il nome di A
 *   (i moduli `EllesmereUI*` dichiarano `EllesmereUI`); se A non è installata, niente;
 * - **una dipendenza obbligatoria più il prefisso del nome**, in entrambe le direzioni: A
 *   richiede B e il nome di B inizia con quello di A (`Questie` → `QuestieDB`), oppure B
 *   richiede A e il nome di B inizia con quello di A (`Details_Compare2` → `Details`).
 *
 * **Veto**, che prevale su tutte le prove: due cartelle non stanno mai nello stesso gruppo se
 * sono riconosciute (dal controllo o dal registro) come progetti diversi, se una è riconosciuta
 * e il suo file non contiene l'altra (anche quando l'altra ha un record dello stesso progetto:
 * conta il file pubblicato), o (senza il controllo o il registro a dire il contrario) se i loro `.toc` dichiarano
 * `X-Curse-Project-ID` diversi o autori senza nomi in comune. Così un plugin di un altro autore (`ElvUI_WindTools` sotto
 * `ElvUI`) resta una riga sua. Il veto per autore vale solo nella direzione dei plugin, una
 * cartella che richiede la principale di cui estende il nome: non quando è la principale a
 * richiedere quella che ne estende il nome (`Questie` → `QuestieDB`), perché un addon non
 * funziona senza le sue dipendenze obbligatorie, né con `## Group`, che è una dichiarazione
 * seguita anche dal gioco. Il campo `## Author` è testo libero che due cartelle dello stesso
 * addon scrivono in modo diverso. L'id Wago non è un confine: Wago elenca come progetti separati
 * cartelle che arrivano nello stesso file CurseForge (i plugin di Details).
 *
 * Gli archi si uniscono dal più forte al più debole e due gruppi si fondono solo se nessuna
 * coppia tra i loro membri è vietata: una cartella finisce in un solo gruppo. La principale è
 * la cartella indicata da `group`, se ce n'è una nel gruppo; poi, tra le cartelle che non sono
 * moduli `LoadOnDemand` (se ce ne sono), quelle che non richiedono una cartella del gruppo il
 * cui nome è un prefisso del loro (`Details_Compare2` richiede `Details`; `Questie` che
 * richiede `QuestieDB` resta candidata); a parità il nome più corto, poi l'ordine alfabetico.
 */
export function groupAddons(
  addons: InstalledAddon[],
  updates?: Map<string, AddonUpdate>,
): AddonGroup[] {
  const byFolder = new Map(addons.map((a) => [lower(a.folder), a]));
  const parent = new Map<string, string>(addons.map((a) => [a.folder, a.folder]));
  const members = new Map<string, InstalledAddon[]>(addons.map((a) => [a.folder, [a]]));
  const root = (folder: string): string => {
    let current = folder;
    while (parent.get(current) !== current) current = parent.get(current)!;
    parent.set(folder, current);
    return current;
  };

  // Il progetto riconosciuto e le cartelle del suo file, per cartella: dal controllo, oppure
  // dal registro dell'app per le cartelle che il controllo non riconosce (o prima del controllo).
  // L'identità è la fonte più l'id nella fonte (`projectId`): due fonti non si uniscono mai.
  const recognised = new Map<string, { id: string; folders: Set<string>; checked: boolean }>();
  for (const addon of addons) {
    const update = updates?.get(addon.folder);
    // Il registro letto da `list_addons` (`installedProject`) è solo CurseForge.
    const project = update?.project
      ? { id: projectId(update.project), folders: update.projectFolders ?? [] }
      : addon.installedProject && {
          id: projectId({ source: "curseforge", key: String(addon.installedProject.id) }),
          folders: addon.installedProject.folders,
        };
    if (!project) continue;
    recognised.set(addon.folder, {
      id: project.id,
      folders: new Set(project.folders.map(lower)),
      checked: Boolean(update?.project),
    });
  }
  const vetoed = (a: InstalledAddon, b: InstalledAddon): boolean => {
    const pa = recognised.get(a.folder);
    const pb = recognised.get(b.folder);
    if (pa && pb) {
      if (pa.id !== pb.id) return true;
      // Stesso progetto, una dal controllo e una dal registro: decide il file pubblicato.
      if (pa.checked && !pb.checked) return !pa.folders.has(lower(b.folder));
      if (pb.checked && !pa.checked) return !pb.folders.has(lower(a.folder));
      return false;
    }
    if (pa && !pa.folders.has(lower(b.folder))) return true;
    if (pb && !pb.folders.has(lower(a.folder))) return true;
    if (pa || pb) return false;
    const differ = (x: string | null, y: string | null) =>
      Boolean(x?.trim()) && Boolean(y?.trim()) && lower(x!.trim()) !== lower(y!.trim());
    return differ(a.curseProjectId, b.curseProjectId);
  };

  // Archi per forza: 0 file pubblicato o registro, 1 `## Group`, 2 la principale richiede la
  // cartella che ne estende il nome, 3 la cartella richiede la principale di cui estende il nome.
  const edges: [number, InstalledAddon, InstalledAddon][] = [];
  const firstOfProject = new Map<string, InstalledAddon>();
  for (const addon of addons) {
    const project = recognised.get(addon.folder);
    if (!project) continue;
    const first = firstOfProject.get(project.id);
    if (first) edges.push([0, first, addon]);
    else firstOfProject.set(project.id, addon);
    for (const name of project.folders) {
      const other = byFolder.get(name);
      if (other && other !== addon) edges.push([0, addon, other]);
    }
  }
  for (const addon of addons) {
    const target = addon.group ? byFolder.get(lower(addon.group)) : undefined;
    if (target && target !== addon) edges.push([1, target, addon]);
  }
  for (const addon of addons) {
    for (const dependency of addon.dependencies) {
      const other = byFolder.get(lower(dependency));
      if (!other || other === addon) continue;
      const [a, b] = [lower(addon.folder), lower(other.folder)];
      if (b.startsWith(a)) edges.push([2, addon, other]);
      else if (a.startsWith(b)) edges.push([3, other, addon]);
    }
  }

  edges.sort((x, y) => x[0] - y[0]);
  for (const [strength, a, b] of edges) {
    const ra = root(a.folder);
    const rb = root(b.folder);
    if (ra === rb) continue;
    const left = members.get(ra)!;
    const right = members.get(rb)!;
    if (left.some((x) => right.some((y) => vetoed(x, y)))) continue;
    // Il veto per autore vale solo senza il controllo o il registro a dire il contrario.
    const checked = [...left, ...right].some((x) => recognised.has(x.folder));
    if (strength === 3 && !checked && disjointAuthors(left, right)) continue;
    parent.set(rb, ra);
    members.set(ra, [...left, ...right]);
    members.delete(rb);
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
  // Un modulo richiede una cartella del gruppo il cui nome è un prefisso del suo
  // (`Details_Compare2` → `Details`); `Questie`, che richiede `QuestieDB`, non lo è.
  const inGroup = new Set(group.map((a) => lower(a.folder)));
  const isModule = (a: InstalledAddon) => {
    const self = lower(a.folder);
    return a.dependencies
      .map(lower)
      .some((d) => d !== self && inGroup.has(d) && self.startsWith(d));
  };
  const notModules = candidates.filter((a) => !isModule(a));
  const pool = notModules.length > 0 ? notModules : candidates;
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
  if (filters.source !== null && update?.project?.source !== filters.source) return false;
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
