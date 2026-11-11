// Tests for the Version column of the Installed tab (issue #313): `pnpm test` (node --test).

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import type { AddonUpdate } from "../api";
import { NO_VERSION, versionCell, versionText } from "./versionColumn.ts";

const base = {
  folder: "Questie",
  source: "curseforge" as const,
  project: null,
  installedFile: null,
  versionOverride: null,
  projectFolders: [],
  githubMirror: null,
  githubMirrorReason: null,
};

const file = (name: string) => ({ id: 1, name, date: "2026-01-01T00:00:00Z" });

const available = (name: string): AddonUpdate => ({
  ...base,
  status: "updateAvailable",
  latest: file(name),
});

describe("versionCell", () => {
  it("shows the installed version alone before the check", () => {
    assert.deepEqual(versionCell("1.2.0"), { installed: "1.2.0", latest: null });
    assert.equal(versionText(versionCell("1.2.0")), "1.2.0");
  });

  it("shows installed → latest when an update is available", () => {
    const cell = versionCell("1.2.0", available("1.3.0"));
    assert.deepEqual(cell, { installed: "1.2.0", latest: "1.3.0" });
    assert.equal(versionText(cell), "1.2.0 → 1.3.0");
  });

  it("shows a dash for a missing or blank version", () => {
    assert.equal(versionCell(null).installed, NO_VERSION);
    assert.equal(versionCell("   ").installed, NO_VERSION);
    assert.equal(versionText(versionCell(null)), "—");
  });

  it("keeps the latest version next to a missing installed one", () => {
    assert.equal(versionText(versionCell(null, available("1.3.0"))), "— → 1.3.0");
  });

  it("shows no latest version for the statuses without an update", () => {
    const others: AddonUpdate[] = [
      { ...base, status: "upToDate" },
      { ...base, status: "notMatched" },
      { ...base, status: "error", message: "offline" },
      // A newer beta is not an update the app installs with the option turned off.
      { ...base, status: "betaOnly", latest: file("1.3.0-beta1") },
    ];
    for (const update of others) {
      assert.deepEqual(
        versionCell("1.2.0", update),
        { installed: "1.2.0", latest: null },
        update.status,
      );
    }
  });

  it("ignores a latest file without a name", () => {
    assert.equal(versionCell("1.2.0", available("  ")).latest, null);
  });

  it("trims both versions", () => {
    assert.deepEqual(versionCell(" 1.2.0 ", available(" 1.3.0 ")), {
      installed: "1.2.0",
      latest: "1.3.0",
    });
  });
});
