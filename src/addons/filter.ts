// Addon search, filters and sorting: pure functions, no React or I/O.

import type { AddonUpdate, InstalledAddon, SourceKind } from "../api";
import { projectId } from "./sources.ts";

export type Filters = {
  query: string;
  /** `null` = all authors. */
  author: string | null;
  hideLoadOnDemand: boolean;
  onlyErrors: boolean;
  /** Only the addons with an update available (needs the result of the check). */
  onlyUpdates: boolean;
  /** Only the addons recognised by this source (needs the result of the check); `null` = all. */
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
 * An addon as the user sees it: a main folder and the folders that come with it
 * (for example `QuestieDB` with `Questie`, or the DBM folders).
 */
export type AddonGroup = {
  main: InstalledAddon;
  /** The other folders of the group, in alphabetical order. */
  others: InstalledAddon[];
};

const lower = (s: string) => s.toLocaleLowerCase("it");

/** The names of an `## Author`, separated by comma, semicolon, `&` or `/`. */
const authorNames = (author: string | null): Set<string> =>
  new Set(
    (author ?? "")
      .split(/[,;&/]/)
      .map((name) => lower(name.trim()))
      .filter(Boolean),
  );

/**
 * Different authors between two groups: no name in common between all the names of one and all
 * those of the other (`Elv, Simpy` and `fang2hou`). `Elv` and `Elv, Simpy` are not, and a group
 * without authors is no evidence. Groups are compared, not single folders: two modules of the
 * same addon can cite different authors among those of the main one.
 */
function disjointAuthors(x: InstalledAddon[], y: InstalledAddon[]): boolean {
  const names = (group: InstalledAddon[]) =>
    new Set(group.flatMap((a) => [...authorNames(a.author)]));
  const a = names(x);
  const b = names(y);
  return a.size > 0 && b.size > 0 && ![...a].some((name) => b.has(name));
}

/**
 * Groups the folders that form a single addon. The evidence, strongest first:
 *
 * - **the published file** (after the check): the installed folders that appear in the
 *   `projectFolders` of a recognised folder go with it, even if CurseForge does not
 *   recognise them one by one; folders recognised as the same project stay together.
 *   **The app's record** (`installedProject`, the zip folders the app
 *   installed) is evidence of the same strength and holds even before the check; for a
 *   folder the check recognises the published file prevails, because the addon may have
 *   been updated with another manager;
 * - **the `## Group` field**, as the game does: B goes with A if B's `group` is A's name
 *   (the `EllesmereUI*` modules declare `EllesmereUI`); if A is not installed, nothing;
 * - **a required dependency plus the name prefix**, in both directions: A
 *   requires B and B's name starts with A's (`Questie` → `QuestieDB`), or B
 *   requires A and B's name starts with A's (`Details_Compare2` → `Details`).
 *
 * **Veto**, which prevails over all evidence: two folders never stay in the same group if
 * they are recognised (by the check or by the record) as different projects, if one is recognised
 * and its file does not contain the other (even when the other has a record of the same project:
 * the published file counts), or (without the check or the record saying otherwise) if their `.toc`
 * files declare different `X-Curse-Project-ID`s or authors with no names in common. So a plugin by
 * another author (`ElvUI_WindTools` under
 * `ElvUI`) stays a row of its own. The author veto holds only in the plugin direction, a
 * folder that requires the main one whose name it extends: not when it is the main one that
 * requires the folder extending its name (`Questie` → `QuestieDB`), because an addon does not
 * work without its required dependencies, nor with `## Group`, which is a declaration
 * the game follows too. The `## Author` field is free text that two folders of the same
 * addon write differently. The Wago id is not a boundary: Wago lists as separate projects
 * folders that come in the same CurseForge file (the Details plugins).
 *
 * Edges are joined from strongest to weakest and two groups merge only if no
 * pair among their members is vetoed: a folder ends up in only one group. The main one is
 * the folder named by `group`, if there is one in the group; then, among the folders that are
 * not `LoadOnDemand` modules (if there are any), those that do not require a folder of the group
 * whose name is a prefix of theirs (`Details_Compare2` requires `Details`; `Questie`, which
 * requires `QuestieDB`, stays a candidate); on a tie the shortest name, then alphabetical order.
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

  // The recognised project and the folders of its file, per folder: from the check, or
  // from the app's record for the folders the check does not recognise (or before the check).
  // The identity is the source plus the id within the source (`projectId`): two sources never join.
  const recognised = new Map<string, { id: string; folders: Set<string>; checked: boolean }>();
  for (const addon of addons) {
    const update = updates?.get(addon.folder);
    // The record read by `list_addons` (`installedProject`) is CurseForge only.
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
      // Same project, one from the check and one from the record: the published file decides.
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

  // Edges by strength: 0 published file or record, 1 `## Group`, 2 the main one requires the
  // folder that extends its name, 3 the folder requires the main one whose name it extends.
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
    // The author veto holds only without the check or the record saying otherwise.
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
  // The folder named by the `## Group` of the group's folders leads it.
  const named = new Set(group.flatMap((a) => (a.group ? [lower(a.group)] : [])));
  const leaders = group.filter((a) => named.has(lower(a.folder)));
  const scope = leaders.length > 0 ? leaders : group;
  const loaded = scope.filter((a) => !a.loadOnDemand);
  const candidates = loaded.length > 0 ? loaded : scope;
  // A module requires a folder of the group whose name is a prefix of its own
  // (`Details_Compare2` → `Details`); `Questie`, which requires `QuestieDB`, is not one.
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
 * The update status of the group. It concerns the **project**, not a folder: the
 * folders of a project come from the same published file. An update available in any
 * folder wins, then a status with a project, then that of the
 * main one.
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

/** The title to show: the one from the `.toc`, otherwise the folder name. */
export function displayTitle(addon: InstalledAddon): string {
  return addon.title ?? addon.folder;
}

/** The authors present, without repetitions, in alphabetical order. */
export function authorsOf(addons: InstalledAddon[]): string[] {
  const authors = new Set<string>();
  for (const addon of addons) {
    if (addon.author) authors.add(addon.author);
  }
  return [...authors].sort((a, b) => a.localeCompare(b, "it", { sensitivity: "base" }));
}

/**
 * An addon's source with the same rule as `groupAddons`: the source of the check's project,
 * otherwise CurseForge if the app's record (`installedProject`, CurseForge only) knows it.
 */
export function sourceOf(addon: InstalledAddon, update?: AddonUpdate): SourceKind | null {
  if (update?.project) return update.project.source;
  return addon.installedProject ? "curseforge" : null;
}

export function matchesFilters(
  addon: InstalledAddon,
  filters: Filters,
  update?: AddonUpdate,
): boolean {
  if (filters.onlyUpdates && update?.status !== "updateAvailable") return false;
  if (filters.source !== null && sourceOf(addon, update) !== filters.source) return false;
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
      // Addons without an author go to the bottom in both directions (see sortAddons).
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
      // On a tie, the title and then the folder make the order stable.
      compare(a, b, "title") ||
      a.folder.localeCompare(b.folder)
    );
  });
}

/**
 * The filters on a group: author, LoadOnDemand and text look at the main folder (the text
 * also at the names of the other folders); an error in any folder counts; the update
 * is the group's.
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

/** Filters and sorts groups already computed with `groupAddons`: count and rows use the same. */
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

/** Header click: the same column flips the direction, a new column starts from its natural one. */
export function nextSort(current: Sort, key: SortKey): Sort {
  if (current.key === key) {
    return { key, direction: current.direction === "asc" ? "desc" : "asc" };
  }
  // For the date, the natural choice is "newest first".
  return { key, direction: key === "modified" ? "desc" : "asc" };
}
