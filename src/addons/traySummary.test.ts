// Tests for the summary of the tray's "Update all" (issue #351): `pnpm test`.

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import type { AutoUpdateStatus, InstallationReport, RoundReport, UpdateOutcome } from "../api";
import { traySummary } from "./traySummary.ts";

function outcome(name: string, status: UpdateOutcome["status"], message?: string): UpdateOutcome {
  return {
    modId: 0,
    source: "github",
    key: name.toLowerCase(),
    name,
    status,
    message,
  };
}

function report(overrides: Partial<InstallationReport> = {}): InstallationReport {
  return {
    installationId: "retail",
    installationName: "Retail",
    checkedAt: "T1",
    available: [],
    updated: [],
    excluded: [],
    skipped: [],
    errors: [],
    checkError: null,
    ...overrides,
  };
}

function status(reports: InstallationReport[], round: Partial<RoundReport> = {}): AutoUpdateStatus {
  return {
    running: false,
    nextRoundAt: null,
    lastRound: {
      mode: "install",
      startedAt: "T0",
      finishedAt: "T2",
      retrying: false,
      requested: true,
      installations: reports,
      ...round,
    },
    undoable: [],
  };
}

const none = new Set<string>();

describe("traySummary", () => {
  it("counts the outcome of a requested install round and lists the problems", () => {
    const s = status([
      report({
        updated: [outcome("A", "updated"), outcome("B", "updated", "Installed from GitHub")],
        skipped: [outcome("C", "skipped", "Distribution not allowed.")],
        errors: [{ source: "github", key: "d", name: "D", message: "Locked file." }],
      }),
    ]);
    assert.deepEqual(traySummary(s, "retail", null, none), {
      key: "retail\nT2",
      text: "2 updated, 1 skipped, 1 error",
      details: [
        { name: "D", message: "Locked file.", failed: true },
        { name: "C", message: "Distribution not allowed.", failed: false },
        { name: "B", message: "Installed from GitHub", failed: false },
      ],
    });
  });

  it("says nothing was updated when the round had nothing to do", () => {
    const summary = traySummary(status([report()]), "retail", null, none);
    assert.deepEqual(summary, {
      key: "retail\nT2",
      text: "Nothing was updated",
      details: [],
    });
  });

  it("shows a failed check as an error", () => {
    const s = status([report({ checkError: "No source answered." })]);
    assert.deepEqual(traySummary(s, "retail", null, none), {
      key: "retail\nT2",
      error: "No source answered.",
    });
  });

  it("names the previous version that could not be kept for Undo", () => {
    const s = status([
      report({
        updated: [outcome("A", "updated")],
        errors: [{ source: null, key: null, name: "", message: "Disk full." }],
      }),
    ]);
    assert.deepEqual(traySummary(s, "retail", null, none), {
      key: "retail\nT2",
      text: "1 updated, 1 error",
      details: [{ name: "Undo", message: "Disk full.", failed: true }],
    });
  });

  it("shows nothing for a scheduled round or a requested check", () => {
    assert.equal(traySummary(status([report()], { requested: false }), "retail", null, none), null);
    assert.equal(traySummary(status([report()], { mode: "notify" }), "retail", null, none), null);
  });

  it("shows nothing without a round or without an entry for the installation", () => {
    const empty: AutoUpdateStatus = {
      running: false,
      nextRoundAt: null,
      lastRound: null,
      undoable: [],
    };
    assert.equal(traySummary(null, "retail", null, none), null);
    assert.equal(traySummary(empty, "retail", null, none), null);
    assert.equal(traySummary(status([report()]), "classic", null, none), null);
  });

  it("hides an undone or dismissed round, but not a newer one", () => {
    const s = status([report()]);
    assert.equal(traySummary(s, "retail", "T2", none), null);
    assert.equal(traySummary(s, "retail", null, new Set(["retail\nT2"])), null);
    const newer = status([report()], { finishedAt: "T3" });
    assert.equal(traySummary(newer, "retail", "T2", new Set(["retail\nT2"]))?.key, "retail\nT3");
  });

  it("keys the summary per installation", () => {
    const s = status([report(), report({ installationId: "classic" })]);
    assert.equal(traySummary(s, "classic", null, new Set(["retail\nT2"]))?.key, "classic\nT2");
  });
});
