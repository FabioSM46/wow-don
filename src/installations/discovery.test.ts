// Tests for the discovered-installations helpers: `pnpm test` (node --test, Node strips the types).

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import type { DiscoveredInstallation } from "../api.ts";
import { flavorFolder, pendingDiscoveries } from "./discovery.ts";

// Synthetic paths: they exist on no machine.
const retail: DiscoveredInstallation = {
  path: "/synthetic/games/World of Warcraft/_retail_",
  flavor: "retail",
  name: "Retail",
  origin: "Program Files (x86)",
};
const classic: DiscoveredInstallation = {
  path: "/synthetic/games/World of Warcraft/_classic_era_",
  flavor: "classic_era",
  name: "Classic Era",
  origin: "Windows registry",
};

describe("pendingDiscoveries", () => {
  it("keeps every result when nothing has been added", () => {
    assert.deepEqual(pendingDiscoveries([retail, classic], []), [retail, classic]);
  });

  it("drops results already in the installations list, keeping the order", () => {
    assert.deepEqual(pendingDiscoveries([retail, classic], [{ path: retail.path }]), [classic]);
  });

  it("drops results added from the section even before the list is reloaded", () => {
    assert.deepEqual(pendingDiscoveries([retail, classic], [], new Set([classic.path])), [retail]);
  });

  it("returns an empty list for an empty scan", () => {
    assert.deepEqual(pendingDiscoveries([], [{ path: retail.path }]), []);
  });
});

describe("flavorFolder", () => {
  it("wraps the flavor in underscores", () => {
    assert.equal(flavorFolder("retail"), "_retail_");
    assert.equal(flavorFolder("classic_era"), "_classic_era_");
  });
});
