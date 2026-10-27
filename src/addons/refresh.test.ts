// Tests for the refresh_addons result: `pnpm test` (node --test, Node strips the types).

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import type { AddonUpdate, Refresh } from "../api";
import { refreshCheck } from "./refresh.ts";

const update = { folder: "Alpha" } as AddonUpdate;

describe("refreshCheck", () => {
  it("returns the states of a successful check", () => {
    const result: Refresh = { addons: [], updates: [update], checkError: null };
    assert.deepEqual(refreshCheck(result), { error: null, updates: [update] });
  });

  it("returns the reason of a failed check, ignoring any states", () => {
    const result: Refresh = { addons: [], updates: [update], checkError: "network down" };
    assert.deepEqual(refreshCheck(result), { error: "network down" });
  });

  it("gives a blank reason a generic message", () => {
    const result: Refresh = { addons: [], updates: [], checkError: "  " };
    assert.deepEqual(refreshCheck(result), { error: "The update check failed." });
  });
});
