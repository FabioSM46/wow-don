// Tests for the progress helpers: `pnpm test` (node --test, Node strips the types).

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  noRunProgress,
  progressKey,
  progressLabel,
  progressPercent,
  progressPosition,
  runLabel,
  withProgress,
  withRunProgress,
  withoutRun,
  type InstallProgress,
} from "./progress.ts";

function event(patch: Partial<InstallProgress> = {}): InstallProgress {
  return {
    installationId: "a",
    source: "curseforge",
    key: "1",
    name: "Details",
    phase: "checking",
    ...patch,
  };
}

describe("progressPercent", () => {
  it("is the share of the declared length, clamped and rounded down", () => {
    assert.equal(progressPercent(event({ phase: "downloading", received: 42, total: 100 })), 42);
    assert.equal(progressPercent(event({ phase: "downloading", received: 999, total: 1000 })), 99);
    assert.equal(progressPercent(event({ phase: "downloading", received: 150, total: 100 })), 100);
    assert.equal(progressPercent(event({ phase: "downloading", received: 0, total: 100 })), 0);
  });

  it("is indeterminate without a length, with a zero length and outside the download", () => {
    assert.equal(progressPercent(event({ phase: "downloading", received: 10 })), null);
    assert.equal(progressPercent(event({ phase: "downloading", received: 10, total: 0 })), null);
    assert.equal(progressPercent(event({ phase: "installing", received: 1, total: 2 })), null);
    assert.equal(progressPercent(undefined), null);
  });
});

describe("progressLabel", () => {
  it("names the phase, with the percentage while downloading", () => {
    assert.equal(progressLabel(event()), "Checking…");
    assert.equal(
      progressLabel(event({ phase: "downloading", received: 1, total: 4 })),
      "Downloading 25%",
    );
    assert.equal(progressLabel(event({ phase: "downloading", received: 1 })), "Downloading…");
    assert.equal(progressLabel(event({ phase: "extracting" })), "Extracting…");
    assert.equal(progressLabel(event({ phase: "installing" })), "Installing…");
    assert.equal(progressLabel(undefined), "Waiting…");
    assert.equal(progressLabel(undefined, "Updating…"), "Updating…");
  });
});

describe("withProgress", () => {
  it("keeps the last event per project and ignores other installations", () => {
    let map = new Map<string, InstallProgress>();
    map = withProgress(map, event(), "a");
    map = withProgress(map, event({ phase: "downloading", received: 1, total: 2 }), "a");
    map = withProgress(map, event({ source: "tukui", key: "1", phase: "installing" }), "a");
    const other = withProgress(map, event({ installationId: "b", phase: "installing" }), "a");
    assert.equal(other, map);
    assert.equal(map.get("curseforge:1")?.phase, "downloading");
    assert.equal(map.get("tukui:1")?.phase, "installing");
  });

  it("files a local zip under its own key", () => {
    assert.equal(progressKey(event({ source: null, key: "" })), "zip");
  });
});

describe("progressPosition", () => {
  it("counts the projects that left the initial check", () => {
    let map = new Map<string, InstallProgress>();
    for (const key of ["1", "2", "3"]) map = withProgress(map, event({ key }), "a");
    assert.equal(progressPosition(map, ["curseforge:1", "curseforge:2", "curseforge:3"]), 0);
    map = withProgress(map, event({ key: "1", phase: "installing" }), "a");
    map = withProgress(map, event({ key: "2", phase: "downloading" }), "a");
    assert.equal(progressPosition(map, ["curseforge:1", "curseforge:2", "curseforge:3"]), 2);
  });
});

describe("withRunProgress", () => {
  const running = new Set(["curseforge:1", "curseforge:2"]);

  it("gives dependency events to the chosen project being installed", () => {
    let state = withRunProgress(noRunProgress, event({ key: "1" }), "a", running);
    state = withRunProgress(
      state,
      event({ key: "77", name: "LibDeflate", phase: "downloading", received: 1, total: 2 }),
      "a",
      running,
    );
    assert.equal(state.current, "curseforge:1");
    assert.equal(state.runs.get("curseforge:1")?.key, "77");
    assert.equal(runLabel(state.runs.get("curseforge:1"), "curseforge:1"), "Downloading 50% · LibDeflate");
  });

  it("moves to the next chosen project when its events start", () => {
    let state = withRunProgress(noRunProgress, event({ key: "1" }), "a", running);
    state = withRunProgress(state, event({ key: "2", phase: "installing" }), "a", running);
    assert.equal(state.current, "curseforge:2");
    assert.equal(runLabel(state.runs.get("curseforge:2"), "curseforge:2"), "Installing…");
  });

  it("drops events with no chosen project and from other installations", () => {
    assert.equal(withRunProgress(noRunProgress, event({ key: "77" }), "a", running), noRunProgress);
    assert.equal(
      withRunProgress(noRunProgress, event({ installationId: "b" }), "a", running),
      noRunProgress,
    );
  });

  it("forgets a finished run", () => {
    const state = withRunProgress(noRunProgress, event({ key: "1" }), "a", running);
    const after = withoutRun(state, "curseforge:1");
    assert.equal(after.current, null);
    assert.equal(after.runs.size, 0);
    assert.equal(withoutRun(after, "curseforge:1"), after);
  });

  it("shows a start text before the first event", () => {
    assert.equal(runLabel(undefined, "curseforge:1"), "Starting…");
  });
});
