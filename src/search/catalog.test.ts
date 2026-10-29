// Tests for the catalog search logic: `pnpm test` (node --test, Node strips the types).

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import type { CatalogAddon, CatalogPage } from "../api";
import {
  RequestTracker,
  appendPage,
  categoryOptions,
  defaultSearch,
  firstPage,
  hiddenOnlyMessage,
  formatAuthors,
  formatDate,
  formatDownloads,
  hasMore,
  normalizeQuery,
  resultCount,
  withSort,
} from "./catalog.ts";

function addon(id: number, fields: Partial<CatalogAddon> = {}): CatalogAddon {
  return {
    id,
    name: `Addon ${id}`,
    summary: "",
    authors: [],
    downloadCount: 0,
    dateModified: "2026-10-01T12:00:00Z",
    categories: [],
    thumbnailUrl: null,
    url: null,
    allowModDistribution: true,
    githubRepo: null,
    latestFile: null,
    installed: false,
    ...fields,
  };
}

function page(index: number, ids: number[], totalCount: number, hidden = 0): CatalogPage {
  return {
    addons: ids.map((id) => addon(id)),
    index,
    pageSize: 50,
    totalCount,
    fetched: ids.length + hidden,
    hidden,
  };
}

const ids = (addons: CatalogAddon[]) => addons.map((a) => a.id);

describe("search parameters", () => {
  it("starts from popularity, descending, with the installation's game version", () => {
    assert.deepEqual(defaultSearch, {
      query: "",
      categoryId: null,
      versionType: null,
      sort: "popularity",
      sortOrder: "desc",
    });
  });

  it("switching sort resets to its natural direction", () => {
    const byName = withSort({ ...defaultSearch, query: "dbm", categoryId: 3 }, "name");
    assert.equal(byName.sort, "name");
    assert.equal(byName.sortOrder, "asc");
    assert.equal(byName.query, "dbm");
    assert.equal(byName.categoryId, 3);
    assert.equal(withSort({ ...byName, sortOrder: "desc" }, "lastUpdated").sortOrder, "desc");
    assert.equal(withSort(byName, "totalDownloads").sortOrder, "desc");
  });

  it("trims the query", () => {
    assert.equal(normalizeQuery("  deadly boss  "), "deadly boss");
    assert.equal(normalizeQuery("   "), "");
  });
});

describe("categories", () => {
  it("lists each subcategory under its parent", () => {
    const options = categoryOptions([
      { id: 10, name: "Bags", parentCategoryId: 1 },
      { id: 12, name: "Boss Encounters", parentCategoryId: 11 },
      { id: 11, name: "Combat", parentCategoryId: 1 },
      { id: 13, name: "Damage Meters", parentCategoryId: 11 },
      { id: 14, name: "Orphan", parentCategoryId: 99 },
    ]);
    assert.deepEqual(
      options.map((o) => [o.name, o.depth]),
      [
        ["Bags", 0],
        ["Combat", 0],
        ["Boss Encounters", 1],
        ["Damage Meters", 1],
        ["Orphan", 0],
      ],
    );
  });

  it("keeps categories the two levels cannot reach", () => {
    const options = categoryOptions([
      { id: 1, name: "Addons", parentCategoryId: null },
      { id: 11, name: "Combat", parentCategoryId: 1 },
      { id: 12, name: "Boss Encounters", parentCategoryId: 11 },
      { id: 20, name: "Loop A", parentCategoryId: 21 },
      { id: 21, name: "Loop B", parentCategoryId: 20 },
    ]);
    assert.deepEqual(
      options.map((o) => [o.name, o.depth]),
      [
        ["Addons", 0],
        ["Combat", 1],
        ["Boss Encounters", 0],
        ["Loop A", 0],
        ["Loop B", 0],
      ],
    );
  });
});

describe("pages", () => {
  it("the first page replaces everything", () => {
    const results = firstPage(page(0, [1, 2, 3], 120));
    assert.deepEqual(ids(results.addons), [1, 2, 3]);
    assert.equal(results.nextIndex, 50);
    assert.equal(results.totalCount, 120);
    assert.equal(hasMore(results), true);
  });

  it("appends the next page without repeating an addon already shown", () => {
    const first = firstPage(page(0, [1, 2, 3], 120));
    const second = appendPage(first, page(50, [3, 4, 4, 5], 120));
    assert.deepEqual(ids(second.addons), [1, 2, 3, 4, 5]);
    assert.equal(second.nextIndex, 100);
  });

  it("stops when the total is reached", () => {
    const results = appendPage(firstPage(page(0, [1], 60)), page(50, [2], 60));
    assert.equal(results.nextIndex, 100);
    assert.equal(hasMore(results), false);
  });

  it("stops on an empty page even if the total says more", () => {
    const results = appendPage(firstPage(page(0, [1], 500)), page(50, [], 500));
    assert.equal(hasMore(results), false);
  });

  it("keeps going after a page that the filter emptied", () => {
    const results = appendPage(firstPage(page(0, [1], 500)), page(50, [], 500, 50));
    assert.equal(results.exhausted, false);
    assert.equal(results.nextIndex, 100);
    assert.equal(results.hidden, 50);
    assert.equal(hasMore(results), true);
  });

  it("explains a list emptied by the filter, with or without more to load", () => {
    const more = appendPage(firstPage(page(0, [], 500, 50)), page(50, [], 500, 50));
    assert.equal(hasMore(more), true);
    assert.equal((hiddenOnlyMessage(more) ?? "").includes("Load more"), true);
    const done = firstPage(page(0, [], 3, 3));
    assert.equal(hasMore(done), false);
    assert.equal(hiddenOnlyMessage(done), "No add-on with a release for this game version.");
    // An empty list with nothing hidden, or a list with results, has its own sentences.
    assert.equal(hiddenOnlyMessage(firstPage(page(0, [], 0))), null);
    assert.equal(hiddenOnlyMessage(firstPage(page(0, [1], 10, 2))), null);
  });

  it("stops at the API result limit", () => {
    const results = appendPage(firstPage(page(0, [1], 50_000)), page(9_950, [2], 50_000));
    assert.equal(results.nextIndex, 10_000);
    assert.equal(hasMore(results), false);
  });

  it("an empty search has nothing more", () => {
    const results = firstPage(page(0, [], 0));
    assert.equal(hasMore(results), false);
    assert.equal(resultCount(results), "No results");
  });
});

describe("out-of-order responses", () => {
  it("only the latest request applies", () => {
    const tracker = new RequestTracker();
    const slow = tracker.next();
    const fast = tracker.next();
    // The fast search answers first, then the slow one arrives late.
    assert.equal(tracker.isLatest(fast), true);
    assert.equal(tracker.isLatest(slow), false);
    const later = tracker.next();
    assert.equal(tracker.isLatest(fast), false);
    assert.equal(tracker.isLatest(later), true);
  });
});

describe("formatting", () => {
  it("downloads in compact form", () => {
    assert.equal(formatDownloads(0), "0");
    assert.equal(formatDownloads(950), "950");
    assert.equal(formatDownloads(12_345), "12.3K");
    assert.equal(formatDownloads(1_250_000), "1.3M");
    assert.equal(formatDownloads(512_000_000), "512M");
  });

  it("dates, with a dash for missing or invalid ones", () => {
    assert.equal(formatDate("2026-10-24T09:30:00Z"), "Oct 24, 2026");
    // The day is the UTC one whatever the machine's time zone.
    assert.equal(formatDate("2026-10-24T00:30:00Z"), "Oct 24, 2026");
    assert.equal(formatDate("2026-10-24T23:30:00Z"), "Oct 24, 2026");
    assert.equal(formatDate(""), "—");
    assert.equal(formatDate("not a date"), "—");
  });

  it("authors", () => {
    assert.equal(formatAuthors(["AuthorA", "AuthorB"]), "AuthorA, AuthorB");
    assert.equal(formatAuthors([]), "—");
  });

  it("result count", () => {
    const one = firstPage(page(0, [1], 1));
    assert.equal(resultCount(one), "1 result");
    const partial = firstPage(page(0, [1, 2], 1_234));
    assert.equal(resultCount(partial), "2 of 1,234 results");
    const all = firstPage(page(0, [1, 2], 2));
    assert.equal(resultCount(all), "2 results");
    const filtered = appendPage(firstPage(page(0, [1, 2], 1_234, 3)), page(50, [4], 1_234, 1));
    assert.equal(resultCount(filtered), "3 shown · 4 without a release for this game version");
    const none = firstPage(page(0, [], 4, 4));
    assert.equal(resultCount(none), "0 shown · 4 without a release for this game version");
  });
});
