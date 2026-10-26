// Tests for the Search state that a change made elsewhere makes stale: `pnpm test`.

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  clearStale,
  forgetFinishedRuns,
  forgetInstalled,
  installKey,
  markStale,
} from "./stale.ts";

describe("stale installations", () => {
  it("adds an installation once and keeps the same set when it is already there", () => {
    const empty: ReadonlySet<string> = new Set();
    const one = markStale(empty, "a");
    assert.deepEqual([...one], ["a"]);
    assert.equal(empty.size, 0);
    assert.equal(markStale(one, "a"), one);
  });

  it("clears only the installations that were refreshed", () => {
    const stale = new Set(["a", "b"]);
    assert.deepEqual([...clearStale(stale, new Set(["a"]))], ["b"]);
    assert.equal(clearStale(stale, new Set(["c"])), stale);
  });
});

describe("forgetting session state", () => {
  it("drops the projects installed in the stale installations only", () => {
    const keys = new Set([installKey("a", 1), installKey("a", 2), installKey("b", 1)]);
    assert.deepEqual([...forgetInstalled(keys, new Set(["a"]))], [installKey("b", 1)]);
    assert.equal(forgetInstalled(keys, new Set(["c"])), keys);
  });

  it("does not mistake an installation whose id is a prefix of another", () => {
    const keys = new Set([installKey("a", 1), installKey("ab", 1)]);
    assert.deepEqual([...forgetInstalled(keys, new Set(["a"]))], [installKey("ab", 1)]);
  });

  it("drops finished runs of the stale installations and keeps running ones", () => {
    const runs = new Map<string, { status: "running" | "done" | "failed" }>([
      [installKey("a", 1), { status: "done" }],
      [installKey("a", 2), { status: "running" }],
      [installKey("a", 3), { status: "failed" }],
      [installKey("b", 1), { status: "done" }],
    ]);
    assert.deepEqual(
      [...forgetFinishedRuns(runs, new Set(["a"])).keys()],
      [installKey("a", 2), installKey("b", 1)],
    );
    assert.equal(forgetFinishedRuns(runs, new Set(["b", "c"])).size, 3);
    assert.equal(forgetFinishedRuns(runs, new Set(["c"])), runs);
  });
});
