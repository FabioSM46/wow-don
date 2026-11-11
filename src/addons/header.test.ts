// Tests for the header of the Installed tab (issue #314): `pnpm test` (node --test, Node strips
// the types).

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { primaryAction, showOptions, statusParts, updateAllLabel } from "./header.ts";

describe("primaryAction", () => {
  const idle = { selecting: false, running: false, updatable: 0 };

  it("is the check when nothing is to update", () => {
    assert.equal(primaryAction(idle), "check");
  });

  it("is Update all while updates are available", () => {
    assert.equal(primaryAction({ ...idle, updatable: 3 }), "updateAll");
  });

  it("stays Update all during a run, where its progress is shown", () => {
    assert.equal(primaryAction({ ...idle, running: true }), "updateAll");
    assert.equal(primaryAction({ ...idle, running: true, updatable: 2 }), "updateAll");
  });

  it("gives way to the bulk actions while a selection exists", () => {
    assert.equal(primaryAction({ ...idle, selecting: true }), "selection");
    assert.equal(primaryAction({ selecting: true, running: true, updatable: 3 }), "selection");
  });
});

describe("updateAllLabel", () => {
  const idle = {
    running: false,
    reinstalling: false,
    position: 0,
    total: 0,
    updatable: 3,
    heldUnknown: false,
  };

  it("counts the projects it sends", () => {
    assert.equal(updateAllLabel(idle), "Update all (3)");
  });

  it("shows no count while the held addons are unknown", () => {
    assert.equal(updateAllLabel({ ...idle, heldUnknown: true }), "Update all");
  });

  it("shows the progress of a run", () => {
    const run = { ...idle, running: true, total: 5 };
    assert.equal(updateAllLabel(run), "Updating…");
    assert.equal(updateAllLabel({ ...run, position: 2 }), "Updating 2 of 5");
    assert.equal(updateAllLabel({ ...run, reinstalling: true }), "Reinstalling…");
    assert.equal(updateAllLabel({ ...run, reinstalling: true, position: 5 }), "Reinstalling 5 of 5");
    // The run wins over the unknown held addons: the button is locked either way.
    assert.equal(updateAllLabel({ ...run, heldUnknown: true }), "Updating…");
  });
});

describe("showOptions", () => {
  const counts = { all: 120, updates: 3, errors: 1 };

  it("gives the three segments with their counts", () => {
    assert.deepEqual(showOptions(counts, true), [
      { id: "all", label: "All", count: 120, disabled: false },
      { id: "updates", label: "Updates", count: 3, disabled: false },
      { id: "errors", label: "Errors", count: 1, disabled: false },
    ]);
  });

  it("disables Updates, without a count, until a check result exists", () => {
    const [all, updates, errors] = showOptions({ ...counts, updates: 0 }, false);
    assert.deepEqual(updates, { id: "updates", label: "Updates", count: null, disabled: true });
    assert.equal(all.disabled, false);
    assert.equal(errors.disabled, false);
  });
});

describe("statusParts", () => {
  const state = { total: 120, visible: 120, toUpdate: 3, checked: "14:05" };

  it("reads addons, updates and the time of the check", () => {
    assert.deepEqual(statusParts(state), ["120 addons", "3 to update", "checked 14:05"]);
  });

  it("says how many the filters leave", () => {
    assert.equal(statusParts({ ...state, visible: 12 })[0], "12 of 120 addons");
    assert.equal(statusParts({ ...state, visible: 0 })[0], "0 of 120 addons");
  });

  it("uses the singular for one addon", () => {
    assert.equal(statusParts({ ...state, total: 1, visible: 1 })[0], "1 addon");
  });

  it("leaves out an update count that is zero or unknown", () => {
    assert.deepEqual(statusParts({ ...state, toUpdate: 0 }), ["120 addons", "checked 14:05"]);
    assert.deepEqual(statusParts({ ...state, toUpdate: null }), ["120 addons", "checked 14:05"]);
  });

  it("leaves out the time before the first check", () => {
    assert.deepEqual(statusParts({ ...state, toUpdate: null, checked: null }), ["120 addons"]);
  });
});
