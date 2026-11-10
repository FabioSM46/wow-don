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
  heldUnknownReason,
  heldUpdateProjects,
  isExcluded,
  updatableProjects,
  updateCount,
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
    assert.equal(exclusionLabel(true), "Don't update");
    assert.equal(exclusionLabel(false), "Allow updates");
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

describe("Update all and the header count leave out held projects (issue #291)", () => {
  const latest = { id: 9, name: "11.0.5", date: "2026-11-01T00:00:00Z" };
  const available = (update: AddonUpdate, allow = true) =>
    ({
      ...update,
      status: "updateAvailable",
      latest,
      project: { ...update.project!, allowModDistribution: allow },
    }) as AddonUpdate;
  const blocked = group("Blocked");
  const withUpdates = new Map<string, AddonUpdate>([
    ["DBM-Core", available(linked("DBM-Core", "curseforge", "3358"))],
    ["DBM-StatusBarTimers", available(linked("DBM-StatusBarTimers", "curseforge", "3358"))],
    ["Details", available(linked("Details", "github", "Tercioo/Details"))],
    ["Twin", linked("Twin", "wowinterface", "3358")],
    // Website only: counted as an update, never sent.
    ["Blocked", available(linked("Blocked", "curseforge", "77"), false)],
  ]);
  const groups = [dbm, details, loose, twin, blocked];
  const keys = (projects: { source: string; key: string }[]) =>
    projects.map((p) => `${p.source}:${p.key}`);

  it("sends one project per source and key, without the held ones", () => {
    assert.deepEqual(keys(updatableProjects(groups, withUpdates, new Set())), [
      "curseforge:3358",
      "github:Tercioo/Details",
    ]);
    const held = new Set(["curseforge:3358"]);
    assert.deepEqual(keys(updatableProjects(groups, withUpdates, held)), [
      "github:Tercioo/Details",
    ]);
    assert.deepEqual(updatableProjects(groups, undefined, held), []);
  });

  it("counts the rows with an update that are not held", () => {
    assert.equal(updateCount(groups, withUpdates, new Set()), 3);
    const held = new Set(["curseforge:3358", "curseforge:77"]);
    assert.equal(updateCount(groups, withUpdates, held), 1);
    assert.equal(updateCount(groups, undefined, new Set()), 0);
  });

  it("holds nothing while the held addons are unknown, and says why Update all waits", () => {
    assert.equal(updatableProjects(groups, withUpdates, undefined).length, 2);
    assert.equal(updateCount(groups, withUpdates, undefined), null);
    assert.deepEqual(heldUpdateProjects(groups, withUpdates, undefined), []);
    assert.equal(heldUnknownReason(undefined), "Wait for the held addons to be loaded.");
    assert.equal(
      heldUnknownReason(undefined, "settings.json is damaged"),
      "Could not read the held addons: settings.json is damaged",
    );
    assert.equal(heldUnknownReason(new Set(), "stale"), null);
  });

  it("offers the held projects with an installable update for the manual update", () => {
    const held = new Set(["curseforge:3358", "curseforge:77", "wowinterface:3358"]);
    assert.deepEqual(keys(heldUpdateProjects(groups, withUpdates, held)), ["curseforge:3358"]);
  });
});
