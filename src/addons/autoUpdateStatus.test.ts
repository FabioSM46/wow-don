// Tests for the automatic update status line: `pnpm test` (node --test, Node strips the types).

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import type {
  AutoUpdateStatus,
  InstallationReport,
  RoundReport,
  UndoOutcome,
  UpdateOutcome,
} from "../api";
import {
  changedInstallations,
  staleUpdateChecks,
  clockTime,
  describeUndo,
  statusLine,
} from "./autoUpdateStatus.ts";

const clock = (iso: string) => `@${iso}`;
const EXCLUDED = "Excluded from automatic updates.";
const file = { id: 1, name: "v2", date: "2026-10-27T10:00:00Z" };

function outcome(name: string, status: UpdateOutcome["status"], message?: string): UpdateOutcome {
  return { modId: 0, source: "github", key: name.toLowerCase(), name, status, message };
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

function status(
  mode: RoundReport["mode"] | null,
  reports: InstallationReport[],
  overrides: Partial<AutoUpdateStatus> = {},
): AutoUpdateStatus {
  return {
    running: false,
    nextRoundAt: null,
    lastRound:
      mode === null
        ? null
        : { mode, startedAt: "T0", finishedAt: "T2", retrying: false, installations: reports },
    undoable: [],
    ...overrides,
  };
}

const available = (name: string) => ({ source: "github" as const, key: name, name, latest: file });

describe("statusLine in notify", () => {
  it("counts the available updates with the check time", () => {
    const s = status("notify", [report({ available: [available("A"), available("B")] })]);
    const line = statusLine(s, "retail", "notify", clock);
    assert.equal(line?.text, "2 updates available · checked @T1");
    assert.equal(line?.problemText, null);
    assert.equal(line?.undo, null);
  });

  it("uses the singular for one update", () => {
    const s = status("notify", [report({ available: [available("A")] })]);
    assert.equal(statusLine(s, "retail", "notify", clock)?.text, "1 update available · checked @T1");
  });

  it("shows nothing when the round found nothing", () => {
    assert.equal(statusLine(status("notify", [report()]), "retail", "notify", clock), null);
  });

  it("does not count excluded projects as problems", () => {
    const s = status("notify", [report({ excluded: [outcome("A", "skipped", EXCLUDED)] })]);
    assert.equal(statusLine(s, "retail", "notify", clock), null);
  });

  it("shows nothing for an installation without a report", () => {
    const s = status("notify", [report({ available: [available("A")] })]);
    assert.equal(statusLine(s, "classic", "notify", clock), null);
  });
});

describe("statusLine in install", () => {
  it("counts the updated addons", () => {
    const s = status("install", [
      report({ updated: [outcome("A", "updated"), outcome("B", "updated")] }),
    ]);
    assert.equal(statusLine(s, "retail", "install", clock)?.text, "Auto-updated 2 addons · @T1");
  });

  it("names failures and skips, with the reasons for the detail", () => {
    const s = status("install", [
      report({
        updated: [outcome("A", "updated")],
        excluded: [outcome("C", "skipped", EXCLUDED)],
        skipped: [outcome("B", "skipped", "Not distributable.")],
        errors: [
          { source: "github", key: "d", name: "D", message: "File in use." },
          { source: null, key: null, name: "", message: "Previous version not kept." },
        ],
      }),
    ]);
    const line = statusLine(s, "retail", "install", clock);
    assert.equal(line?.text, "Auto-updated 1 addon · @T1");
    assert.equal(line?.problemText, "2 failed · 1 skipped");
    assert.equal(line?.failed, true);
    assert.deepEqual(line?.problems, [
      { name: "D", message: "File in use.", kind: "failed" },
      { name: "Undo", message: "Previous version not kept.", kind: "failed" },
      { name: "B", message: "Not distributable.", kind: "skipped" },
    ]);
  });

  it("says when only problems happened", () => {
    const s = status("install", [
      report({ errors: [{ source: null, key: null, name: "Folder", message: "Unknown." }] }),
    ]);
    const line = statusLine(s, "retail", "install", clock);
    assert.equal(line?.text, "Checked @T1");
    assert.equal(line?.problemText, "1 failed");
  });

  it("reports a failed check", () => {
    const s = status("install", [report({ checkError: "No network." })]);
    const line = statusLine(s, "retail", "install", clock);
    assert.equal(line?.text, "Automatic check failed · @T1");
    assert.deepEqual(line?.problems, [{ name: "Check", message: "No network.", kind: "failed" }]);
  });

  it("offers Undo for an undoable round", () => {
    const undo = { installationId: "retail", roundAt: "T0", addons: 2 };
    const s = status("install", [report({ updated: [outcome("A", "updated")] })], {
      undoable: [undo, { installationId: "classic", roundAt: "T0", addons: 1 }],
    });
    assert.deepEqual(statusLine(s, "retail", "install", clock)?.undo, undo);
  });

  it("keeps an undoable round after a round that found nothing, or after a restart", () => {
    const undo = { installationId: "retail", roundAt: "T0", addons: 3 };
    for (const s of [
      status("install", [report()], { undoable: [undo] }),
      status(null, [], { undoable: [undo] }),
    ]) {
      const line = statusLine(s, "retail", "install", clock);
      assert.equal(line?.text, "Auto-updated 3 addons · @T0");
      assert.deepEqual(line?.undo, undo);
    }
  });
});

describe("statusLine after an Undo", () => {
  const undone = status("install", [report({ updated: [outcome("A", "updated")] })]);

  it("hides the outcome of the undone round", () => {
    assert.equal(statusLine(undone, "retail", "install", clock, "T2"), null);
  });

  it("shows a newer round again", () => {
    assert.equal(
      statusLine(undone, "retail", "install", clock, "T-old")?.text,
      "Auto-updated 1 addon · @T1",
    );
  });
});

describe("statusLine when off", () => {
  const undo = { installationId: "retail", roundAt: "T0", addons: 2 };

  it("shows nothing without an undoable round, or before the mode is read", () => {
    const s = status("install", [report({ updated: [outcome("A", "updated")] })], {
      undoable: [{ installationId: "classic", roundAt: "T0", addons: 1 }],
    });
    assert.equal(statusLine(s, "retail", "off", clock), null);
    assert.equal(statusLine(status(null, []), "retail", "off", clock), null);
    assert.equal(statusLine(s, "classic", null, clock), null);
    assert.equal(statusLine(null, "retail", "install", clock), null);
    assert.equal(statusLine(null, "retail", "off", clock), null);
  });

  it("still offers Undo for an undoable round", () => {
    for (const s of [
      status(null, [], { undoable: [undo] }),
      status("install", [report({ updated: [outcome("A", "updated")] })], { undoable: [undo] }),
    ]) {
      assert.deepEqual(statusLine(s, "retail", "off", clock), {
        text: "Auto-updated 2 addons · @T0",
        problemText: null,
        problems: [],
        undo,
        failed: false,
      });
    }
  });

  it("never shows available updates or check errors", () => {
    for (const r of [
      report({ available: [available("A")] }),
      report({ checkError: "No network." }),
      report({ errors: [{ source: null, key: null, name: "B", message: "Disk full." }] }),
      report({ skipped: [outcome("C", "skipped", "Not distributable.")] }),
    ]) {
      for (const mode of ["notify", "install"] as const) {
        assert.equal(statusLine(status(mode, [r]), "retail", "off", clock), null);
        const line = statusLine(status(mode, [r], { undoable: [undo] }), "retail", "off", clock);
        assert.equal(line?.text, "Auto-updated 2 addons · @T0");
        assert.deepEqual(line?.problems, []);
        assert.equal(line?.problemText, null);
      }
    }
  });

  it("disappears after a successful Undo", () => {
    // Undo emits the status without the undoable round; the undone round stays in `lastRound`.
    const after = status("install", [report({ updated: [outcome("A", "updated")] })]);
    assert.equal(statusLine(after, "retail", "off", clock, "T2"), null);
    assert.equal(statusLine(after, "retail", "off", clock), null);
  });
});

describe("changedInstallations", () => {
  const changed = status("install", [
    report({ updated: [outcome("A", "updated")] }),
    report({ installationId: "classic" }),
  ]);

  it("lists the installations an install round changed", () => {
    assert.deepEqual(changedInstallations(null, changed), ["retail"]);
    assert.deepEqual(changedInstallations(status(null, []), changed), ["retail"]);
  });

  it("ignores the same round seen again", () => {
    assert.deepEqual(changedInstallations(changed, changed), []);
  });

  it("ignores notify rounds", () => {
    const notify = status("notify", [report({ available: [available("A")] })]);
    assert.deepEqual(changedInstallations(null, notify), []);
  });
});

describe("staleUpdateChecks", () => {
  it("lists the installations a notify round found updates for", () => {
    const notify = status("notify", [
      report({ available: [available("A")] }),
      report({ installationId: "classic" }),
    ]);
    assert.deepEqual(staleUpdateChecks(null, notify), ["retail"]);
    assert.deepEqual(staleUpdateChecks(status(null, []), notify), ["retail"]);
  });

  it("lists the installations an install round updated or failed on", () => {
    const install = status("install", [
      report({ updated: [outcome("A", "updated")] }),
      report({
        installationId: "classic",
        errors: [{ source: null, key: null, name: "B", message: "Locked." }],
      }),
      report({ installationId: "ptr" }),
    ]);
    assert.deepEqual(staleUpdateChecks(null, install), ["retail", "classic"]);
  });

  it("ignores the same round seen again", () => {
    const notify = status("notify", [report({ available: [available("A")] })]);
    assert.deepEqual(staleUpdateChecks(notify, notify), []);
  });

  it("ignores a status without rounds", () => {
    assert.deepEqual(staleUpdateChecks(null, status(null, [])), []);
  });

  /** A round of `mode` that finished at `finishedAt`, with the given reports. */
  function roundAt(
    mode: RoundReport["mode"],
    finishedAt: string,
    reports: InstallationReport[],
  ): AutoUpdateStatus {
    return status(mode, [], {
      lastRound: { mode, startedAt: "T0", finishedAt, retrying: false, installations: reports },
    });
  }

  it("lists a later notify round with updates, after an earlier one", () => {
    const earlier = roundAt("notify", "T2", [report({ available: [available("A")] })]);
    const later = roundAt("notify", "T4", [
      report({ checkedAt: "T3", available: [available("A"), available("B")] }),
    ]);
    assert.deepEqual(staleUpdateChecks(earlier, later), ["retail"]);
  });

  it("ignores a new notify round that found no updates", () => {
    const earlier = roundAt("notify", "T2", [report({ available: [available("A")] })]);
    const later = roundAt("notify", "T4", [
      report({ checkedAt: "T3" }),
      report({ installationId: "classic", checkedAt: "T3" }),
    ]);
    assert.deepEqual(staleUpdateChecks(earlier, later), []);
    assert.deepEqual(staleUpdateChecks(null, later), []);
  });

  it("ignores a notify round whose only finding is a check error", () => {
    const failed = roundAt("notify", "T4", [report({ checkError: "No network." })]);
    assert.deepEqual(staleUpdateChecks(null, failed), []);
  });

  it("ignores the same round emitted again with a different outcome", () => {
    // After a manual check or update the backend reconciles `lastRound` and emits it again with
    // the same `finishedAt` (issue #273): the table already holds that manual result.
    const round = roundAt("notify", "T2", [
      report({ available: [available("A"), available("B")] }),
    ]);
    const checked = roundAt("notify", "T2", [
      report({ checkedAt: "T5", available: [available("B")] }),
    ]);
    const updated = roundAt("notify", "T2", [report({ checkedAt: "T6" })]);
    assert.deepEqual(staleUpdateChecks(round, checked), []);
    assert.deepEqual(staleUpdateChecks(checked, updated), []);
    const install = roundAt("install", "T2", [
      report({ available: [available("A")], updated: [outcome("A", "updated")] }),
    ]);
    const reconciled = roundAt("install", "T2", [
      report({ checkedAt: "T5", updated: [outcome("A", "updated")] }),
    ]);
    assert.deepEqual(staleUpdateChecks(install, reconciled), []);
    assert.deepEqual(changedInstallations(install, reconciled), []);
  });
});

describe("describeUndo", () => {
  it("summarizes restored, skipped and failed projects", () => {
    const outcomes: UndoOutcome[] = [
      { source: "github", key: "a", name: "A", status: "restored" },
      { source: "github", key: "b", name: "B", status: "skipped", message: "Changed." },
      { source: "github", key: "c", name: "", status: "error", message: "Locked." },
    ];
    assert.deepEqual(describeUndo(outcomes), {
      text: "Restored 1 addon · 1 failed · 1 skipped.",
      problems: [
        { name: "B", message: "Changed.", kind: "skipped" },
        { name: "c", message: "Locked.", kind: "failed" },
      ],
    });
  });
});

describe("clockTime", () => {
  it("shows the local time for today and the date otherwise", () => {
    const at = new Date(2026, 9, 27, 9, 5);
    assert.equal(clockTime(at.toISOString(), new Date(2026, 9, 27, 20, 0)), "09:05");
    assert.equal(clockTime(at.toISOString(), new Date(2026, 9, 28, 1, 0)), "Oct 27, 09:05");
  });

  it("keeps an unreadable time as it is", () => {
    assert.equal(clockTime("soon"), "soon");
  });
});
