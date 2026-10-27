// Tests for the Installed tab selection and context menu: `pnpm test` (node --test).

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import type { AddonUpdate, InstalledAddon, RemoveResult } from "../api";
import type { AddonGroup } from "./filter.ts";
import {
  confirmText,
  foldersOf,
  menuItems,
  menuTargets,
  pruneSelection,
  reinstallProjects,
  removalSummary,
  removedFolders,
  selectRow,
  selectedText,
  toggleRow,
} from "./selection.ts";

const addon = (folder: string) => ({ folder, title: folder }) as InstalledAddon;
const group = (main: string, ...others: string[]): AddonGroup => ({
  main: addon(main),
  others: others.map(addon),
});
const linked = (folder: string, source: "curseforge" | "github", key: string) =>
  ({
    folder,
    status: "upToDate",
    project: { id: 1, source, key, name: folder, url: null, allowModDistribution: true },
  }) as AddonUpdate;

const dbm = group("DBM-Core", "DBM-StatusBarTimers");
const details = group("Details");
const loose = group("MyNotes");
const updates = new Map<string, AddonUpdate>([
  ["DBM-Core", linked("DBM-Core", "curseforge", "3358")],
  ["DBM-StatusBarTimers", linked("DBM-StatusBarTimers", "curseforge", "3358")],
  ["Details", linked("Details", "github", "Tercioo/Details")],
]);
const idle = { updates, busy: false, checking: false, excluded: new Set<string>() };

describe("selection state", () => {
  it("Select starts the mode with the row and adds rows later", () => {
    const first = selectRow(null, "Details");
    assert.deepEqual([...first], ["Details"]);
    assert.deepEqual([...selectRow(first, "DBM-Core")], ["Details", "DBM-Core"]);
  });

  it("a left click adds or removes a row, and the mode survives an empty selection", () => {
    const one = toggleRow(new Set(["Details"]), "DBM-Core");
    assert.deepEqual([...one].sort(), ["DBM-Core", "Details"]);
    const none = toggleRow(new Set(["Details"]), "Details");
    assert.equal(none.size, 0);
  });

  it("keeps only rows that still exist, and leaves the mode when none is left", () => {
    const selection = new Set(["Details", "DBM-Core"]);
    assert.deepEqual([...(pruneSelection(selection, ["Details", "MyNotes"]) ?? [])], ["Details"]);
    assert.equal(pruneSelection(selection, ["MyNotes"]), null);
    assert.equal(pruneSelection(selection, ["Details", "DBM-Core"]), selection);
    assert.equal(pruneSelection(null, ["Details"]), null);
    const empty = new Set<string>();
    assert.equal(pruneSelection(empty, ["Details"]), empty);
  });

  it("counts the selection", () => {
    assert.equal(selectedText(new Set(["a", "b"])), "2 selected");
    assert.equal(selectedText(null), "0 selected");
  });
});

describe("menu targets and folders", () => {
  const groups = [dbm, details, loose];

  it("acts on the row outside the mode and on the selection inside it", () => {
    assert.deepEqual(menuTargets(groups, null, "Details"), [details]);
    assert.deepEqual(menuTargets(groups, new Set(["MyNotes", "DBM-Core"]), "Details"), [
      dbm,
      loose,
    ]);
  });

  it("removes every folder of each group, once", () => {
    assert.deepEqual(foldersOf([dbm, details, dbm]), [
      "DBM-Core",
      "DBM-StatusBarTimers",
      "Details",
    ]);
  });

  it("reinstalls one project per source and id, skipping rows without a source", () => {
    assert.deepEqual(reinstallProjects([dbm, details, loose], updates), [
      { source: "curseforge", key: "3358" },
      { source: "github", key: "Tercioo/Details" },
    ]);
    assert.deepEqual(reinstallProjects([dbm], undefined), []);
  });
});

describe("menuItems", () => {
  it("offers Select, Reinstall, Don't auto-update and Remove for one row", () => {
    const items = menuItems([details], false, idle);
    assert.deepEqual(
      items.map((i) => [i.label, i.disabledReason]),
      [
        ["Select", null],
        ["Reinstall", null],
        ["Don't auto-update", null],
        ["Remove", null],
      ],
    );
  });

  it("disables Reinstall with a reason for a row without a source", () => {
    const [, reinstall, , remove] = menuItems([loose], false, idle);
    assert.equal(/No recognized source/.test(reinstall.disabledReason ?? ""), true);
    assert.equal(remove.disabledReason, null);
  });

  it("explains a missing check instead of blaming the source", () => {
    const [, waiting] = menuItems([details], false, { busy: false, checking: true });
    assert.equal(/Wait for the update check/.test(waiting.disabledReason ?? ""), true);
    const [, never] = menuItems([details], false, { busy: false, checking: false });
    assert.equal(/Check for updates first/.test(never.disabledReason ?? ""), true);
  });

  it("offers counted actions and Clear selection in the mode", () => {
    const items = menuItems([dbm, loose], true, idle);
    assert.deepEqual(
      items.map((i) => [i.action, i.label, i.disabledReason]),
      [
        ["reinstall", "Reinstall 2", null],
        ["autoUpdate", "Don't auto-update", null],
        ["remove", "Remove 2", null],
        ["clear", "Clear selection", null],
      ],
    );
  });

  it("disables both actions on an empty selection or while busy", () => {
    const empty = menuItems([], true, idle);
    assert.equal(empty[0].disabledReason, "No addons selected.");
    assert.equal(empty[1].disabledReason, "No addons selected.");
    assert.equal(empty[2].disabledReason, "No addons selected.");
    const busy = menuItems([details], false, { ...idle, busy: true });
    assert.equal(/Wait/.test(busy[1].disabledReason ?? ""), true);
    assert.equal(/Wait/.test(busy[3].disabledReason ?? ""), true);
    assert.equal(busy[0].disabledReason, null);
    // Escludere un addon non tocca il disco: resta possibile durante un aggiornamento.
    assert.equal(busy[2].disabledReason, null);
  });
});

describe("confirmation and result texts", () => {
  it("counts addons and folders", () => {
    assert.equal(confirmText([dbm, details]), "Remove 2 addons (3 folders)?");
    assert.equal(confirmText([details]), "Remove 1 addon (1 folder)?");
  });

  it("summarizes a removal with the reason of each folder left", () => {
    const result: RemoveResult = {
      folders: [
        { folder: "DBM-Core", status: "removed", message: null },
        { folder: "DBM-StatusBarTimers", status: "removed", message: null },
        { folder: "Details", status: "error", message: "Permission denied" },
        { folder: "MyNotes", status: "error", message: " " },
      ],
      warning: null,
    };
    assert.deepEqual(removalSummary(result), {
      text: "2 folders removed, 2 folders not removed",
      errors: [
        { folder: "Details", message: "Permission denied" },
        { folder: "MyNotes", message: "Could not be removed." },
      ],
    });
    assert.deepEqual(removedFolders(result), ["DBM-Core", "DBM-StatusBarTimers"]);
    assert.equal(removalSummary({ folders: [], warning: null }).text, "Nothing was removed");
    assert.equal(
      removalSummary({
        folders: [{ folder: "Details", status: "removed", message: null }],
        warning: null,
      }).text,
      "1 folder removed",
    );
  });
});

describe("Reinstall of projects CurseForge blocks (issue #165)", () => {
  const blockedUpdate = (folder: string, mirror: Partial<AddonUpdate>) =>
    ({
      folder,
      status: "updateAvailable",
      latest: { id: 2, name: `${folder}-2.zip`, date: "2026-10-01T00:00:00Z" },
      project: {
        id: 7,
        source: "curseforge",
        key: folder === "Blocked" ? "7" : "8",
        name: folder,
        url: null,
        allowModDistribution: false,
      },
      githubMirror: null,
      githubMirrorReason: null,
      ...mirror,
    }) as AddonUpdate;
  const blocked = group("Blocked");
  const mirrored = group("Mirrored");
  const withBlocked = new Map<string, AddonUpdate>([
    ...updates,
    ["Blocked", blockedUpdate("Blocked", { githubMirrorReason: "No release on GitHub." })],
    ["Mirrored", blockedUpdate("Mirrored", { githubMirror: { repo: "o/r", tag: "v2" } })],
  ]);
  const state = { updates: withBlocked, busy: false, checking: false };

  it("leaves out a blocked update whose GitHub copy the check ruled out", () => {
    assert.deepEqual(reinstallProjects([blocked, mirrored, details], withBlocked), [
      { source: "curseforge", key: "8" },
      { source: "github", key: "Tercioo/Details" },
    ]);
  });

  it("disables Reinstall for that row with the reason", () => {
    const [, reinstall] = menuItems([blocked], false, state);
    assert.equal(
      reinstall.disabledReason,
      "CurseForge blocks third-party downloads and the GitHub copy cannot be used: No release on GitHub.",
    );
    const [, viaGithub] = menuItems([mirrored], false, state);
    assert.equal(viaGithub.disabledReason, null);
  });

  it("keeps Reinstall for a selection where another project can be reinstalled", () => {
    const [reinstall] = menuItems([blocked, details], true, state);
    assert.equal(reinstall.disabledReason, null);
    const [mixed] = menuItems([blocked, loose], true, state);
    assert.equal(/^None of these addons can be reinstalled/.test(mixed.disabledReason ?? ""), true);
  });
});

describe("menuItems: automatic updates", () => {
  it("allows again when every target is excluded, and excludes a mixed selection", () => {
    const excluded = new Set(["github:Tercioo/Details"]);
    const [, , one] = menuItems([details], false, { ...idle, excluded });
    assert.equal(one.label, "Allow auto-update");
    const [, mixed] = menuItems([details, dbm], true, { ...idle, excluded });
    assert.equal(mixed.label, "Don't auto-update");
  });

  it("disables the item while the choices load, save or have no source", () => {
    const [, , loading] = menuItems([details], false, { ...idle, excluded: undefined });
    assert.equal(loading.disabledReason, "Wait for the automatic update choices to be loaded.");
    const [, , failed] = menuItems([details], false, {
      ...idle,
      excluded: undefined,
      excludedError: "settings.json is damaged",
    });
    assert.equal(
      failed.disabledReason,
      "Could not read the automatic update choices: settings.json is damaged",
    );
    const [, , saving] = menuItems([details], false, { ...idle, savingExclusion: true });
    assert.equal(/Wait/.test(saving.disabledReason ?? ""), true);
    const [, , noSource] = menuItems([loose], false, idle);
    assert.equal(/No recognized source/.test(noSource.disabledReason ?? ""), true);
    const [, , unchecked] = menuItems([details], false, { ...idle, updates: undefined });
    assert.equal(/Check for updates first/.test(unchecked.disabledReason ?? ""), true);
  });
});
