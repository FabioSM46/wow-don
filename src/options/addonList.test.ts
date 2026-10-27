// Tests for the import/export texts: `pnpm test` (node --test, Node strips the types).

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import type { ExportResult, ImportPreview, ImportResult, PreviewEntry } from "../api";
import { progressKey, type ProgressPhase } from "../addons/progress.ts";
import {
  exportSummary,
  furthestPhase,
  importProgressText,
  importSummary,
  newProjects,
  previewCounts,
  previewSummary,
} from "./addonList.ts";

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

  it("uses the same ids as the progress events", () => {
    // Il contatore confronta gli id della lista con quelli degli eventi: devono coincidere.
    const event = {
      installationId: "test",
      source: "curseforge" as const,
      key: "3",
      name: "Addon 3",
      phase: "downloaded" as const,
    };
    assert.equal(progressKey(event), fresh[1]);
    assert.equal(
      importProgressText(new Map([[progressKey(event), event.phase]]), fresh, event.phase),
      "Downloading 1 of 2",
    );
  });

  it("counts the downloaded zips, then the position of the install", () => {
    const reached = (entries: [string, ProgressPhase][]) => new Map(entries);
    assert.equal(importProgressText(reached([]), fresh), "Downloading 0 of 2");
    assert.equal(
      importProgressText(reached([["curseforge:1", "downloading"]]), fresh, "downloading"),
      "Downloading 0 of 2",
    );
    assert.equal(
      importProgressText(
        reached([
          ["curseforge:1", "downloaded"],
          ["curseforge:3", "downloading"],
        ]),
        fresh,
        "downloading",
      ),
      "Downloading 1 of 2",
    );
    assert.equal(
      importProgressText(
        reached([
          ["curseforge:1", "extracting"],
          ["curseforge:3", "downloaded"],
        ]),
        fresh,
        "extracting",
      ),
      "Installing 1 of 2",
    );
    assert.equal(
      importProgressText(
        reached([
          ["curseforge:1", "installing"],
          ["curseforge:3", "installing"],
        ]),
        fresh,
        "installing",
      ),
      "Installing 2 of 2",
    );
    // Una dipendenza non conta come progetto della lista, ma la sua installazione sì come fase.
    assert.equal(
      importProgressText(reached([["curseforge:99", "installing"]]), fresh, "installing"),
      "Installing 1 of 2",
    );
    assert.equal(importProgressText(reached([]), []), "Importing…");
  });
});

describe("furthestPhase", () => {
  it("never goes back", () => {
    assert.equal(furthestPhase(undefined, "checking"), "checking");
    assert.equal(furthestPhase("downloaded", "downloading"), "downloaded");
    assert.equal(furthestPhase("downloaded", "extracting"), "extracting");
    assert.equal(furthestPhase("installing", "checking"), "installing");
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
