// Tests for the install outcome summary: `pnpm test` (node --test, Node strips the types).

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import type { InstallOutcome } from "../api.ts";
import { installedIds, summarizeInstall, withInstalled } from "./install.ts";

const file = { id: 9, name: "Foo-1.0.zip", date: "2026-10-01T00:00:00Z" };

describe("install summary", () => {
  it("names the project and counts the dependencies installed with it", () => {
    const outcomes: InstallOutcome[] = [
      { modId: 1, name: "Foo", status: "installed", file },
      { modId: 2, name: "Lib", status: "installed", file, dependencyOf: 1 },
      { modId: 3, name: "Core", status: "installed", file, dependencyOf: 2 },
    ];
    const summary = summarizeInstall(outcomes, { id: 1 });
    assert.equal(summary.headline, "Foo installed with 2 dependencies.");
    assert.equal(summary.error, false);
    assert.deepEqual(summary.lines, [
      { modId: 2, text: "Lib (required by Foo): installed", error: false },
      { modId: 3, text: "Core (required by Lib): installed", error: false },
    ]);
  });

  it("uses the singular for one dependency and nothing for none", () => {
    assert.equal(
      summarizeInstall([
        { modId: 1, name: "Foo", status: "installed", file },
        { modId: 2, name: "Lib", status: "installed", file, dependencyOf: 1 },
      ], { id: 1 }).headline,
      "Foo installed with 1 dependency.",
    );
    assert.equal(
      summarizeInstall([{ modId: 1, name: "Foo", status: "installed", file }], { id: 1 }).headline,
      "Foo installed.",
    );
  });

  it("keeps the warning of a successful install after the sentence", () => {
    const summary = summarizeInstall([
      { modId: 1, name: "Foo", status: "installed", file, message: "Folder Bar was replaced." },
    ], { id: 1 });
    assert.equal(summary.headline, "Foo installed. Folder Bar was replaced.");
  });

  it("explains skipped and failed dependencies, failures as errors", () => {
    const summary = summarizeInstall([
      { modId: 1, name: "Foo", status: "installed", file },
      { modId: 2, name: "Lib", status: "skipped", message: "Already installed.", dependencyOf: 1 },
      { modId: 3, name: "", status: "error", message: "Download failed.", dependencyOf: 1 },
    ], { id: 1 });
    assert.equal(summary.headline, "Foo installed.");
    assert.equal(summary.error, false);
    assert.deepEqual(summary.lines, [
      { modId: 2, text: "Lib (required by Foo): skipped — Already installed.", error: false },
      { modId: 3, text: "Project #3 (required by Foo): error — Download failed.", error: true },
    ]);
  });

  it("gives the reason when the project itself is skipped or fails", () => {
    const skipped = summarizeInstall([
      { modId: 1, name: "Foo", status: "skipped", message: "Already installed." },
    ], { id: 1 });
    assert.equal(skipped.headline, "Foo was not installed: Already installed.");
    assert.equal(skipped.error, false);

    const failed = summarizeInstall([
      { modId: 1, name: "Foo", status: "error", message: "No file for this game version." },
    ], { id: 1 });
    assert.equal(failed.headline, "Foo could not be installed: No file for this game version.");
    assert.equal(failed.error, true);

    assert.equal(
      summarizeInstall([{ modId: 1, name: "Foo", status: "error" }], { id: 1 }).headline,
      "Foo could not be installed.",
    );
  });

  it("falls back to the search name, then to the id, when the outcome has no name", () => {
    const outcomes: InstallOutcome[] = [
      { modId: 1, name: "", status: "error", message: "Boom." },
    ];
    assert.equal(
      summarizeInstall(outcomes, { id: 1, name: "Foo" }).headline,
      "Foo could not be installed: Boom.",
    );
    assert.equal(
      summarizeInstall(outcomes, { id: 1 }).headline,
      "Project #1 could not be installed: Boom.",
    );
  });

  it("names a parent that has no outcome by its id", () => {
    const summary = summarizeInstall([
      { modId: 1, name: "Foo", status: "installed", file },
      { modId: 2, name: "Lib", status: "installed", file, dependencyOf: 7 },
    ], { id: 1 });
    assert.equal(summary.lines[0].text, "Lib (required by Project #7): installed");
  });

  it("finds the requested project by id, not by position", () => {
    const summary = summarizeInstall(
      [
        { modId: 2, name: "Lib", status: "installed", file, dependencyOf: 1 },
        { modId: 1, name: "Foo", status: "installed", file },
      ],
      { id: 1 },
    );
    assert.equal(summary.headline, "Foo installed with 1 dependency.");
    assert.deepEqual(summary.lines.map((line) => line.modId), [2]);
  });

  it("keeps the dependency lines when no outcome is the requested project", () => {
    const summary = summarizeInstall(
      [{ modId: 2, name: "Lib", status: "error", message: "Boom.", dependencyOf: 1 }],
      { id: 1, name: "Foo" },
    );
    assert.equal(summary.headline, "Nothing was installed.");
    assert.deepEqual(summary.lines, [
      { modId: 2, text: "Lib (required by Project #1): error — Boom.", error: true },
    ]);
  });

  it("says that nothing was installed for an empty list", () => {
    assert.deepEqual(summarizeInstall([], { id: 1 }), {
      headline: "Nothing was installed.",
      error: false,
      lines: [],
    });
  });
});

describe("installed state", () => {
  it("collects only the projects that were installed", () => {
    assert.deepEqual(
      installedIds([
        { modId: 1, name: "Foo", status: "installed", file },
        { modId: 2, name: "Lib", status: "skipped", dependencyOf: 1 },
        { modId: 3, name: "Core", status: "error", dependencyOf: 1 },
        { modId: 4, name: "Ui", status: "installed", file, dependencyOf: 1 },
      ]),
      [1, 4],
    );
  });

  it("marks an add-on installed after the search response, keeping the same object otherwise", () => {
    const installed = new Set([1]);
    const fresh = { id: 1, installed: false, name: "Foo" };
    assert.deepEqual(withInstalled(fresh, installed), { id: 1, installed: true, name: "Foo" });
    assert.equal(fresh.installed, false);

    const already = { id: 1, installed: true };
    assert.equal(withInstalled(already, installed), already);
    const other = { id: 2, installed: false };
    assert.equal(withInstalled(other, installed), other);
  });
});
