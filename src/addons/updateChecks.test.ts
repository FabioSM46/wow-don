// Tests for the session cache of update checks: `pnpm test` (node --test, Node strips the types).
// The state is module-wide, so every test uses its own installation ids.

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import type { AddonUpdate } from "../api";
import {
  checkGeneration,
  claimAutoCheck,
  errorCache,
  forgetUpdateChecks,
  updatesCache,
} from "./updateChecks.ts";

const result: AddonUpdate[] = [];

describe("claimAutoCheck", () => {
  it("starts the automatic check only on the first visit", () => {
    assert.equal(claimAutoCheck("first-a"), true);
    assert.equal(claimAutoCheck("first-a"), false);
  });

  it("does not start it when a result is already cached", () => {
    updatesCache.set("cached-a", result);
    assert.equal(claimAutoCheck("cached-a"), false);
  });
});

describe("forgetUpdateChecks", () => {
  it("makes the next visit check again, as after a round with new findings", () => {
    // The table checked once, then left the installation: the next visit shows the cache.
    assert.equal(claimAutoCheck("round-a"), true);
    updatesCache.set("round-a", result);
    errorCache.set("round-a", "No network.");
    assert.equal(claimAutoCheck("round-a"), false);

    forgetUpdateChecks(["round-a"]);
    assert.equal(updatesCache.has("round-a"), false);
    assert.equal(errorCache.has("round-a"), false);
    assert.equal(claimAutoCheck("round-a"), true);
  });

  it("drops a check already in flight", () => {
    const started = checkGeneration("flight-a");
    forgetUpdateChecks(["flight-a"]);
    assert.equal(checkGeneration("flight-a"), started + 1);
  });

  it("leaves the other installations alone", () => {
    assert.equal(claimAutoCheck("other-a"), true);
    updatesCache.set("other-a", result);
    const generation = checkGeneration("other-a");
    forgetUpdateChecks(["other-b"]);
    assert.equal(updatesCache.has("other-a"), true);
    assert.equal(checkGeneration("other-a"), generation);
    assert.equal(claimAutoCheck("other-a"), false);
  });
});
