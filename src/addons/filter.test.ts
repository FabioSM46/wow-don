// Regression tests for groupAddons: `pnpm test` (node --test, Node strips the types).

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import type { AddonUpdate, InstalledAddon } from "../api";
import { groupAddons } from "./filter.ts";

function addon(folder: string, fields: Partial<InstalledAddon> = {}): InstalledAddon {
  return {
    folder,
    tocFile: `${folder}.toc`,
    title: folder,
    interface: [],
    version: null,
    author: null,
    notes: null,
    dependencies: [],
    optionalDeps: [],
    loadOnDemand: false,
    group: null,
    curseProjectId: null,
    wagoId: null,
    wowiId: null,
    modifiedMs: null,
    fingerprint: 1,
    error: null,
    installedProject: null,
    ...fields,
  };
}

/** A check result where each recognised folder maps to [project id, published folders]. */
function checked(
  addons: InstalledAddon[],
  recognised: Record<string, [number, string[]]>,
): Map<string, AddonUpdate> {
  return new Map(
    addons.map((a): [string, AddonUpdate] => {
      const match = recognised[a.folder];
      const update: AddonUpdate = match
        ? {
            folder: a.folder,
            project: { id: match[0], name: `P${match[0]}`, url: null, allowModDistribution: true },
            installedFile: { id: 1, name: "file", date: "2026-10-01T00:00:00Z" },
            versionOverride: null,
            projectFolders: match[1],
            status: "upToDate",
          }
        : {
            folder: a.folder,
            project: null,
            installedFile: null,
            versionOverride: null,
            projectFolders: [],
            status: "notMatched",
          };
      return [a.folder, update];
    }),
  );
}

/** The groups as "main: others" strings, sorted, so a whole layout is one assertion. */
function layout(addons: InstalledAddon[], updates?: Map<string, AddonUpdate>): string[] {
  return groupAddons(addons, updates)
    .map((g) => [g.main.folder, ...g.others.map((o) => o.folder)].join(" "))
    .sort();
}

// The real Details! .toc shape: only the main folder declares the CurseForge id, the modules
// require it by name (RequiredDeps or Dependencies) and carry their own Wago ids.
const DETAILS_MODULES = [
  "Details_Compare2",
  "Details_DataStorage",
  "Details_EncounterDetails",
  "Details_RaidCheck",
  "Details_Streamer",
  "Details_TinyThreat",
  "Details_Vanguard",
];
const details = (): InstalledAddon[] => [
  addon("Details", { curseProjectId: "61284", wagoId: "qv63A7Gb" }),
  ...DETAILS_MODULES.map((folder) =>
    addon(folder, {
      dependencies: ["Details"],
      loadOnDemand: folder === "Details_DataStorage",
      wagoId: folder === "Details_Compare2" ? null : `wago-${folder}`,
    }),
  ),
];
const DETAILS_ROW = ["Details " + DETAILS_MODULES.join(" ")];

describe("groupAddons", () => {
  it("joins a required folder that extends the requiring one (Questie + QuestieDB)", () => {
    const addons = [addon("Questie", { dependencies: ["QuestieDB"] }), addon("QuestieDB")];
    assert.deepEqual(layout(addons), ["Questie QuestieDB"]);
  });

  it("joins Questie + QuestieDB whatever their ## Author fields say", () => {
    // The real `.toc` fields: free text, names separated by `/`, different prefixes.
    const addons = [
      addon("Questie", {
        author: "Aero/Logon/Muehe/TheCrux(BreakBB)/Drejjmit/Dyaxler/Cheeq/TechnoHunter/Yttrium/Everyone else",
        dependencies: ["QuestieDB"],
        curseProjectId: "334372",
      }),
      addon("QuestieDB", {
        author: "Code: Logonz Data: Muehe/TheCrux(BreakBB)/Drejjmit/Dyaxler/Cheeq/TechnoHunter/Yttrium/Everyone else",
      }),
    ];
    assert.deepEqual(layout(addons), ["Questie QuestieDB"]);
    // Even with no name in common, the main folder requiring its own data folder wins.
    const strangers = [
      addon("Questie", { author: "Aero", dependencies: ["QuestieDB"] }),
      addon("QuestieDB", { author: "Logonz" }),
    ];
    assert.deepEqual(layout(strangers), ["Questie QuestieDB"]);
  });

  it("joins the modules that declare ## Group (EllesmereUI)", () => {
    const addons = [
      addon("EllesmereUI", { curseProjectId: "1477613" }),
      addon("EllesmereUIActionBars", { group: "EllesmereUI", dependencies: ["EllesmereUI"] }),
      addon("EllesmereUIOptions", {
        group: "EllesmereUI",
        dependencies: ["EllesmereUI"],
        loadOnDemand: true,
      }),
      // A Group with no installed target joins nothing.
      addon("Orphan", { group: "Missing" }),
    ];
    assert.deepEqual(layout(addons), [
      "EllesmereUI EllesmereUIActionBars EllesmereUIOptions",
      "Orphan",
    ]);
  });

  it("joins the Details modules that require the main folder, before the check", () => {
    assert.deepEqual(layout(details()), DETAILS_ROW);
  });

  it("keeps Details on one row after the check, recognised folders or not", () => {
    const addons = details();
    const published = ["Details", ...DETAILS_MODULES];
    // Only the main folder and one module are recognised one by one.
    const updates = checked(addons, {
      Details: [61284, published],
      Details_TinyThreat: [61284, published],
    });
    assert.deepEqual(layout(addons, updates), DETAILS_ROW);
    const [group] = groupAddons(addons, updates);
    assert.equal(group.main.folder, "Details");
    assert.equal(group.others.length, 7);
  });

  it("joins the folders of one project with no naming rule (DBM)", () => {
    const addons = [
      addon("DBM-Core"),
      addon("DBM-StatusBarTimers"),
      addon("DBM-Raids-MoP", { loadOnDemand: true }),
    ];
    assert.deepEqual(layout(addons), ["DBM-Core", "DBM-Raids-MoP", "DBM-StatusBarTimers"]);
    const published = ["DBM-Core", "DBM-StatusBarTimers", "DBM-Raids-MoP"];
    const updates = checked(addons, {
      "DBM-Core": [3358, published],
      "DBM-StatusBarTimers": [3358, published],
      "DBM-Raids-MoP": [3358, published],
    });
    assert.deepEqual(layout(addons, updates), ["DBM-Core DBM-Raids-MoP DBM-StatusBarTimers"]);
  });

  it("joins an unrecognised folder listed in the main folder's published file", () => {
    const addons = [addon("Atlas"), addon("Atlas_Data", { fingerprint: 999 })];
    const updates = checked(addons, { Atlas: [77, ["atlas", "atlas_data"]] });
    assert.deepEqual(layout(addons, updates), ["Atlas Atlas_Data"]);
  });

  it("keeps a plugin of another project apart (ElvUI + ElvUI_WindTools)", () => {
    const addons = [
      addon("ElvUI"),
      addon("ElvUI_Options", { dependencies: ["ElvUI"], loadOnDemand: true }),
      addon("ElvUI_WindTools", { dependencies: ["ElvUI"] }),
    ];
    // The plugin is recognised and its file does not contain ElvUI: veto.
    const updates = checked(addons, { ElvUI_WindTools: [343956, ["ElvUI_WindTools"]] });
    assert.deepEqual(layout(addons, updates), ["ElvUI ElvUI_Options", "ElvUI_WindTools"]);
    // Both recognised as different projects: veto, whatever the files say.
    const both = checked(addons, {
      ElvUI: [1, ["ElvUI", "ElvUI_Options", "ElvUI_WindTools"]],
      ElvUI_WindTools: [343956, ["ElvUI_WindTools", "ElvUI"]],
    });
    assert.deepEqual(layout(addons, both), ["ElvUI ElvUI_Options", "ElvUI_WindTools"]);
  });

  it("keeps folders with different X-Curse-Project-ID apart before the check", () => {
    const addons = [
      addon("ElvUI", { curseProjectId: "1" }),
      addon("ElvUI_WindTools", { dependencies: ["ElvUI"], curseProjectId: "343956" }),
      addon("ElvUI_Extra", { group: "ElvUI", curseProjectId: "343956" }),
    ];
    assert.deepEqual(layout(addons), ["ElvUI", "ElvUI_Extra", "ElvUI_WindTools"]);
  });

  it("follows ## Group even when the authors share no name", () => {
    const addons = [
      addon("EllesmereUI", { author: "Ellesmere" }),
      addon("EllesmereUIBags", { author: "Contributor", group: "EllesmereUI", dependencies: ["EllesmereUI"] }),
    ];
    assert.deepEqual(layout(addons), ["EllesmereUI EllesmereUIBags"]);
  });

  it("keeps folders by different authors apart before the check", () => {
    const addons = [
      addon("ElvUI", { author: "Elv, Simpy" }),
      addon("ElvUI_Options", { author: "Elv, Simpy", dependencies: ["ElvUI"] }),
      addon("ElvUI_WindTools", { author: "fang2hou", dependencies: ["ElvUI"] }),
      // A missing author is no evidence either way.
      addon("ElvUI_Libraries", { dependencies: ["ElvUI"] }),
      // One shared name is enough: a module often lists fewer authors than the main folder.
      addon("ElvUI_Skins", { author: "Simpy & Elv", dependencies: ["ElvUI"] }),
      addon("ElvUI_Data", { author: "Elv", dependencies: ["ElvUI"] }),
      // `/` separates names too.
      addon("ElvUI_Extras", { author: "Simpy/Merathilis", dependencies: ["ElvUI"] }),
    ];
    assert.deepEqual(layout(addons), [
      "ElvUI ElvUI_Data ElvUI_Extras ElvUI_Libraries ElvUI_Options ElvUI_Skins",
      "ElvUI_WindTools",
    ]);
  });

  it("leads with the folder no other one extends, whatever the requirements", () => {
    // Same published file, no name prefix: the main is not demoted for requiring the other.
    const addons = [addon("Auctioneer", { dependencies: ["BeanCounter"] }), addon("BeanCounter")];
    const updates = checked(addons, {
      Auctioneer: [5, ["Auctioneer", "BeanCounter"]],
      BeanCounter: [5, ["Auctioneer", "BeanCounter"]],
    });
    assert.deepEqual(layout(addons, updates), ["Auctioneer BeanCounter"]);
    // DBM: the modules require DBM-Core, which leads.
    const dbm = [
      addon("DBM-StatusBarTimers", { dependencies: ["DBM-Core"] }),
      addon("DBM-Core"),
      addon("DBM-GUI", { dependencies: ["DBM-Core"], loadOnDemand: true }),
    ];
    const published = ["DBM-Core", "DBM-GUI", "DBM-StatusBarTimers"];
    const dbmUpdates = checked(dbm, {
      "DBM-Core": [3358, published],
      "DBM-GUI": [3358, published],
      "DBM-StatusBarTimers": [3358, published],
    });
    assert.equal(groupAddons(dbm, dbmUpdates)[0].main.folder, "DBM-Core");
  });

  it("does not treat different Wago ids as a boundary", () => {
    const addons = [
      addon("Details", { wagoId: "a" }),
      addon("Details_Streamer", { dependencies: ["Details"], wagoId: "b" }),
    ];
    assert.deepEqual(layout(addons), ["Details Details_Streamer"]);
  });

  it("never puts a folder in two groups", () => {
    // AlphaBC has evidence for both Alpha (## Group) and AlphaB (dependency + prefix), which
    // are vetoed against each other: the stronger evidence wins and the folder is listed once.
    const addons = [
      addon("Alpha", { curseProjectId: "1" }),
      addon("AlphaB", { curseProjectId: "2" }),
      addon("AlphaBC", { group: "Alpha", dependencies: ["AlphaB"] }),
    ];
    const groups = groupAddons(addons);
    const seen = groups.flatMap((g) => [g.main.folder, ...g.others.map((o) => o.folder)]);
    assert.deepEqual([...seen].sort(), ["Alpha", "AlphaB", "AlphaBC"]);
    assert.deepEqual(layout(addons), ["Alpha AlphaBC", "AlphaB"]);
  });
});
