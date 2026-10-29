// Catalog search: parameters, pages, out-of-order answers and formatting. Pure functions,
// no React or I/O.

import type {
  CatalogAddon,
  CatalogCategory,
  CatalogPage,
  CatalogSearch,
  CatalogSort,
  SortOrder,
} from "../api";

/** Typing pause after which the search starts on its own. */
export const SEARCH_DELAY_MS = 300;

/** Beyond this index the CurseForge API returns no results (see `catalog.rs`). */
export const MAX_RESULTS = 10_000;

/** The sort orders, in menu order, with the direction they start with. */
export const SORTS: readonly { id: CatalogSort; label: string; order: SortOrder }[] = [
  { id: "popularity", label: "Popularity", order: "desc" },
  { id: "lastUpdated", label: "Last updated", order: "desc" },
  { id: "name", label: "Name", order: "asc" },
  { id: "totalDownloads", label: "Downloads", order: "desc" },
];

export const defaultSearch: CatalogSearch = {
  query: "",
  categoryId: null,
  versionType: null,
  sort: "popularity",
  sortOrder: "desc",
};

/** Changes sort order, starting again from its natural direction (A→Z for the name, highest
 *  first for the others). */
export function withSort<T extends Pick<CatalogSearch, "sort" | "sortOrder">>(
  search: T,
  sort: CatalogSort,
): T {
  const order = SORTS.find((s) => s.id === sort)?.order ?? "desc";
  return { ...search, sort, sortOrder: order };
}

/** An entry of the categories menu; `depth` 1 is a subcategory. */
export type CategoryOption = { id: number; name: string; depth: 0 | 1 };

/**
 * The categories for the menu: each main category followed by its subcategories, in the
 * order (by name) they arrive from the backend. A subcategory whose parent is missing stays
 * among the main ones. The menu has two levels: a deeper category (or one in a cycle of
 * parents) is not lost, it appears at the end among the main ones.
 */
export function categoryOptions(categories: CatalogCategory[]): CategoryOption[] {
  const ids = new Set(categories.map((category) => category.id));
  const isChild = (category: CatalogCategory) =>
    category.parentCategoryId !== null &&
    category.parentCategoryId !== category.id &&
    ids.has(category.parentCategoryId);
  const options: CategoryOption[] = [];
  const emitted = new Set<number>();
  const push = (category: CatalogCategory, depth: 0 | 1) => {
    if (emitted.has(category.id)) return;
    emitted.add(category.id);
    options.push({ id: category.id, name: category.name, depth });
  };
  for (const parent of categories.filter((category) => !isChild(category))) {
    push(parent, 0);
    for (const child of categories) {
      if (isChild(child) && child.parentCategoryId === parent.id) push(child, 1);
    }
  }
  // Safety net: every category appears at least once, even those the two
  // levels above do not reach.
  for (const category of categories) push(category, 0);
  return options;
}

/** The box text as the backend receives it: without spaces at the edges, so "abc " and
 *  "abc" do not make two searches. */
export function normalizeQuery(text: string): string {
  return text.trim();
}

/** The state of the results: the pages loaded so far, without duplicates. */
export type Results = {
  addons: CatalogAddon[];
  /** The index to request for the next page. */
  nextIndex: number;
  totalCount: number;
  /** How many results read so far are hidden because they have no Release file (issue #140). */
  hidden: number;
  /** The API returned an empty page: there is nothing more to request even if `totalCount` says
   * more. A page empty only because of the filter does not count. */
  exhausted: boolean;
};

/** The results of the first page of a new search. */
export function firstPage(page: CatalogPage): Results {
  return appendPage(
    { addons: [], nextIndex: 0, totalCount: 0, hidden: 0, exhausted: false },
    page,
  );
}

/**
 * Appends a page. Between one page and the next the catalog can change order: an addon already
 * shown that reappears in the next page is not repeated.
 */
export function appendPage(results: Results, page: CatalogPage): Results {
  const seen = new Set(results.addons.map((addon) => addon.id));
  const fresh = page.addons.filter((addon) => {
    if (seen.has(addon.id)) return false;
    seen.add(addon.id);
    return true;
  });
  return {
    addons: [...results.addons, ...fresh],
    nextIndex: page.index + page.pageSize,
    totalCount: page.totalCount,
    hidden: results.hidden + page.hidden,
    exhausted: page.fetched === 0,
  };
}

/** Whether "Load more" still has something to request. */
export function hasMore(results: Results): boolean {
  return (
    !results.exhausted &&
    results.nextIndex < results.totalCount &&
    results.nextIndex < MAX_RESULTS
  );
}

/**
 * Numbers the requests: only the answer to the latest one started applies, so a slow
 * answer to an old search does not cover the most recent one.
 */
export class RequestTracker {
  private latest = 0;

  /** The number of a new request, which makes all the previous ones obsolete. */
  next(): number {
    this.latest += 1;
    return this.latest;
  }

  isLatest(id: number): boolean {
    return id === this.latest;
  }
}

const compactFormat = new Intl.NumberFormat("en", {
  notation: "compact",
  maximumFractionDigits: 1,
});
const fullFormat = new Intl.NumberFormat("en");
// In UTC, like CurseForge's dates: the day shown does not change with the machine's time zone.
const dateFormat = new Intl.DateTimeFormat("en", { dateStyle: "medium", timeZone: "UTC" });

/** Downloads in short form (`1.2M`); the whole number goes in the hint. */
export function formatDownloads(count: number): string {
  return compactFormat.format(count);
}

export function formatCount(count: number): string {
  return fullFormat.format(count);
}

/** An ISO 8601 date like "Oct 24, 2026"; a missing or invalid date shows "—". */
export function formatDate(iso: string): string {
  if (!iso) return "—";
  const date = new Date(iso);
  return Number.isNaN(date.getTime()) ? "—" : dateFormat.format(date);
}

/** The authors on one line; without authors "—". */
export function formatAuthors(authors: string[]): string {
  return authors.length > 0 ? authors.join(", ") : "—";
}

/** The sentence above the table: how many results are shown out of how many. */
export function resultCount(results: Results): string {
  const shown = results.addons.length;
  const total = results.totalCount;
  if (total === 0) return "No results";
  // With hidden results the API total would also count addons that will never be seen.
  if (results.hidden > 0) {
    return `${formatCount(shown)} shown · ${formatCount(results.hidden)} without a release for this game version`;
  }
  if (shown >= total) return `${formatCount(total)} ${total === 1 ? "result" : "results"}`;
  return `${formatCount(shown)} of ${formatCount(total)} results`;
}

/**
 * The empty-state sentence when the results read are all hidden because they have no Release
 * file (issue #140), or `null` if that is not the reason for the empty list. "Load more"
 * stays under the list, even an empty one, as long as `hasMore` allows it.
 */
export function hiddenOnlyMessage(results: Results): string | null {
  if (results.addons.length > 0 || results.hidden === 0) return null;
  return hasMore(results)
    ? "No add-on with a release for this game version among the first results. Load more to keep looking."
    : "No add-on with a release for this game version.";
}
