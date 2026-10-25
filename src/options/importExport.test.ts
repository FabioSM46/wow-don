// Tests for the import/export texts: `pnpm test` (node --test, Node strips the types).

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import type { ExportResult, ImportPreview, ImportResult, PreviewEntry } from "../api";
import {
  exportSummary,
  importProgressText,
  importSummary,
  newProjects,
  previewCounts,
  previewSummary,
} from "./importExport.ts";

function entry(key: string, state: PreviewEntry["state"]): PreviewEntry {
  return { source: "curseforge", key, name: `Addon ${key}`, state };
}

function preview(entries: PreviewEntry[], invalid = 0): ImportPreview {
  return {
    flavor: "retail",
    entries,
    invalid: Array.from({ length: invalid }, (_, i) => ({ position: i + 1, reason: "bad" })),
    removals: [],
  };
}

function result(partial: Partial<ImportResult>): ImportResult {
  return {
    entries: [],
    invalid: [],
    removed: [],
    notConfirmed: [],
    removalErrors: [],
    ...partial,
  };
}

describe("exportSummary", () => {
  it("counts the exported addons and the folders left out", () => {
    const base: ExportResult = { text: "WOWDON1:", exported: [], skipped: [], checkError: null };
    const one = { source: "tukui" as const, key: "-2", name: "ElvUI" };
    assert.equal(exportSummary({ ...base, exported: [one] }), "1 addon exported");
    assert.equal(
      exportSummary({ ...base, exported: [one, one], skipped: ["Zip"] }),
      "2 addons exported · 1 folder not exportable",
    );
  });
});

describe("previewSummary", () => {
  it("lists only the counts that are not zero", () => {
    const p = preview([entry("1", "new"), entry("2", "installed"), entry("3", "installed")], 1);
    assert.deepEqual(previewCounts(p), { fresh: 1, installed: 2, notDownloadable: 0, invalid: 1 });
    assert.equal(previewSummary(p), "1 new · 2 already installed · 1 invalid entry");
  });

  it("says when the list is empty", () => {
    assert.equal(previewSummary(preview([])), "This list has no addons.");
  });
});

describe("importProgressText", () => {
  const p = preview([entry("1", "new"), entry("2", "installed"), entry("3", "new")]);
  const fresh = newProjects(p);

  it("takes only the new projects", () => {
    assert.deepEqual(fresh, ["curseforge:1", "curseforge:3"]);
  });

  it("counts the projects that have started, at least the first", () => {
    assert.equal(importProgressText(new Set(), fresh), "Importing 1 of 2");
    assert.equal(importProgressText(new Set(["curseforge:1"]), fresh), "Importing 1 of 2");
    assert.equal(importProgressText(new Set(["curseforge:1", "curseforge:3"]), fresh), "Importing 2 of 2");
    // Una dipendenza non conta come progetto della lista.
    assert.equal(importProgressText(new Set(["curseforge:99"]), fresh), "Importing 1 of 2");
    assert.equal(importProgressText(new Set(), []), "Importing…");
  });
});

describe("importSummary", () => {
  it("sums up a clean import", () => {
    const summary = importSummary(
      result({
        entries: [
          { source: "curseforge", key: "1", name: "A", status: "installed" },
          { source: "curseforge", key: "2", name: "B", status: "alreadyInstalled" },
        ],
        removed: ["Old"],
      }),
    );
    assert.equal(summary.headline, "Installed 1 · 1 already installed · removed 1 folder");
    assert.deepEqual(summary.problems, []);
    assert.equal(summary.failed, false);
  });

  it("explains every problem", () => {
    const summary = importSummary(
      result({
        entries: [
          { source: "curseforge", key: "1", name: "A", status: "skipped", message: "not allowed" },
          { source: "tukui", key: "-9", name: "", status: "error", message: "boom" },
        ],
        removalErrors: [{ folder: "X", message: "Could not remove X (busy)." }],
        removalSkipped: "No addon could be installed, so no folder was removed.",
        notConfirmed: ["Late", "Later"],
        invalid: [{ position: 3, reason: "unknown source" }],
        lookupError: "CurseForge did not respond in time.",
      }),
    );
    assert.equal(summary.headline, "Installed 0 · 1 skipped · 1 failed");
    assert.deepEqual(summary.problems, [
      "A: not allowed",
      "-9: boom",
      "Could not remove X (busy).",
      "No addon could be installed, so no folder was removed.",
      "Not removed because they were not in the confirmed list: Late, Later.",
      "1 invalid entry was ignored.",
      "CurseForge did not respond in time.",
    ]);
    assert.equal(summary.failed, true);
  });
});
