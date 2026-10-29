// Tests for the automatic update exclusions of the Installed tab: `pnpm test` (node --test).

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import type { AddonUpdate, InstalledAddon } from "../api";
import {
  excludedAfter,
  excludedIds,
  exclusionChoice,
  exclusionLabel,
  exclusionProjects,
  isExcluded,
} from "./exclusion.ts";
import type { AddonGroup } from "./filter.ts";

const addon = (folder: string) => ({ folder, title: folder }) as InstalledAddon;
const group = (main: string, ...others: string[]): AddonGroup => ({
  main: addon(main),
  others: others.map(addon),
});
const linked = (folder: string, source: "curseforge" | "github" | "wowinterface", key: string) =>
  ({
    folder,
    status: "upToDate",
    // `id` is 0 outside CurseForge: the matching must not use it.
    project: { id: 0, source, key, name: folder, url: null, allowModDistribution: true },
  }) as AddonUpdate;

const dbm = group("DBM-Core", "DBM-StatusBarTimers");
const details = group("Details");
const loose = group("MyNotes");
const twin = group("Twin");
const updates = new Map<string, AddonUpdate>([
  ["DBM-Core", linked("DBM-Core", "curseforge", "3358")],
  ["DBM-StatusBarTimers", linked("DBM-StatusBarTimers", "curseforge", "3358")],
  ["Details", linked("Details", "github", "Tercioo/Details")],
  // The same id on another source is another project.
  ["Twin", linked("Twin", "wowinterface", "3358")],
]);

describe("excluded rows", () => {
  it("matches an excluded project on source and key, never on the id", () => {
    const excluded = excludedIds({
      disabled: false,
      excluded: [{ source: "curseforge", key: "3358" }],
    });
    assert.equal(isExcluded(updates.get("DBM-Core"), excluded), true);
    assert.equal(isExcluded(updates.get("Twin"), excluded), false);
    assert.equal(isExcluded(updates.get("Details"), excluded), false);
  });

  it("marks nothing without a project or before the choices are read", () => {
    const excluded = new Set(["curseforge:3358"]);
    assert.equal(isExcluded(undefined, excluded), false);
    assert.equal(isExcluded(updates.get("DBM-Core"), undefined), false);
  });
});

describe("the menu item on a selection", () => {
  it("acts on one project per source and key, skipping rows without a source", () => {
    assert.deepEqual(exclusionProjects([dbm, details, loose, twin], updates), [
      { source: "curseforge", key: "3358" },
      { source: "github", key: "Tercioo/Details" },
      { source: "wowinterface", key: "3358" },
    ]);
    assert.deepEqual(exclusionProjects([dbm], undefined), []);
  });

  it("excludes when any project still takes part, and allows when all are excluded", () => {
    const projects = exclusionProjects([dbm, details], updates);
    assert.equal(exclusionChoice(projects, new Set()), true);
    assert.equal(exclusionChoice(projects, new Set(["curseforge:3358"])), true);
    assert.equal(
      exclusionChoice(projects, new Set(["curseforge:3358", "github:Tercioo/Details"])),
      false,
    );
    assert.equal(exclusionLabel(true), "Don't auto-update");
    assert.equal(exclusionLabel(false), "Allow auto-update");
  });

  it("applies the choice to every selected project and keeps the others", () => {
    const projects = exclusionProjects([dbm, details], updates);
    const before = new Set(["curseforge:3358", "tukui:-2"]);
    assert.deepEqual(
      [...excludedAfter(before, projects, true)].sort(),
      ["curseforge:3358", "github:Tercioo/Details", "tukui:-2"],
    );
    assert.deepEqual([...excludedAfter(before, projects, false)], ["tukui:-2"]);
    // The starting set does not change.
    assert.deepEqual([...before], ["curseforge:3358", "tukui:-2"]);
  });
});
