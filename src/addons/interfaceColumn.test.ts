// Tests for the Interface column of the Installed tab (issue #230): `pnpm test` (node --test).

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { describe, it } from "node:test";

import { FLAVOR_RANGES, fitsFlavor, interfaceSummary } from "./interfaceColumn.ts";

// The list EllesmereUI declares (the screenshot of the issue).
const ellesmere = [120000, 120001, 120005, 120007, 120100, 120105, 16001];

describe("interfaceSummary", () => {
  it("shows the version of the installation flavor and counts the others", () => {
    assert.deepEqual(interfaceSummary(ellesmere, "forever"), {
      shown: 16001,
      more: 6,
      title: "120000, 120001, 120005, 120007, 120100, 120105, 16001",
    });
  });

  it("takes the first version of the flavor when there are several", () => {
    assert.equal(interfaceSummary(ellesmere, "retail")?.shown, 120000);
  });

  it("falls back to the first version when none belongs to the flavor", () => {
    const summary = interfaceSummary(ellesmere, "classic_era");
    assert.equal(summary?.shown, 120000);
    assert.equal(summary?.more, 6);
  });

  it("falls back to the first version for an unknown flavor", () => {
    assert.equal(interfaceSummary([50500, 11507], "ptr")?.shown, 50500);
  });

  it("shows a single version with nothing more", () => {
    assert.deepEqual(interfaceSummary([11507], "classic_era"), {
      shown: 11507,
      more: 0,
      title: "11507",
    });
  });

  it("has nothing to show without versions", () => {
    assert.equal(interfaceSummary([], "retail"), null);
  });

  it("accepts the folder name and any case as the flavor", () => {
    assert.equal(interfaceSummary(ellesmere, "_Classic_Beta_")?.shown, 16001);
  });
});

describe("fitsFlavor", () => {
  it("follows the ranges of every known game", () => {
    assert.equal(fitsFlavor("retail", 100000), true);
    assert.equal(fitsFlavor("retail", 99999), false);
    assert.equal(fitsFlavor("classic_era", 11507), true);
    assert.equal(fitsFlavor("classic_era", 11600), false);
    assert.equal(fitsFlavor("forever", 16999), true);
    assert.equal(fitsFlavor("forever", 17000), false);
    assert.equal(fitsFlavor("anniversary", 20505), true);
    assert.equal(fitsFlavor("classic", 50500), true);
    assert.equal(fitsFlavor("classic", 100000), false);
  });
});

// The same table lives in the backend (`fits` in src-tauri/src/sources/tukui.rs): this reads
// that function and fails when the two tables diverge, so a new or changed game breaks here.
describe("FLAVOR_RANGES", () => {
  it("matches fits() in the Tukui source of the backend", () => {
    const rust = readFileSync(
      new URL("../../src-tauri/src/sources/tukui.rs", import.meta.url),
      "utf8",
    );
    const start = rust.indexOf("fn fits(flavor: &str, interface: u32) -> bool {");
    assert.equal(start >= 0, true, "fits() not found in tukui.rs");
    const body = rust.slice(start, rust.indexOf("\n}\n", start));
    const parsed: Record<string, [number, number]> = {};
    const arm = /^\s*((?:"\w+"\s*\|?\s*)+)=>\s*(.+?),?\s*$/gm;
    for (const [, patterns, condition] of body.matchAll(arm)) {
      const number = (text: string) => Number(text.replace(/_/g, ""));
      const atLeast = /^interface >= ([\d_]+)$/.exec(condition);
      const between = /^\(([\d_]+)\.\.([\d_]+)\)\.contains\(&interface\)$/.exec(condition);
      const range: [number, number] | null = atLeast
        ? [number(atLeast[1]), Number.POSITIVE_INFINITY]
        : between
          ? [number(between[1]), number(between[2])]
          : null;
      if (range === null) throw new Error(`unreadable arm in fits(): ${condition}`);
      for (const [, name] of patterns.matchAll(/"(\w+)"/g)) parsed[name] = range;
    }
    assert.deepEqual(parsed, { ...FLAVOR_RANGES });
  });
});
