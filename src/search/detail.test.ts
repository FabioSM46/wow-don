// Tests for the add-on detail formatting: `pnpm test` (node --test, Node strips the types).

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  dependencyName,
  fileMeta,
  fileName,
  formatCategories,
  paragraphs,
  releaseLabel,
  versionName,
} from "./detail.ts";

describe("description paragraphs", () => {
  it("splits on blank lines and keeps single line breaks inside a paragraph", () => {
    assert.deepEqual(paragraphs("Intro line.\n\nFeatures:\n- one\n- two\n\nEnd."), [
      "Intro line.",
      "Features:\n- one\n- two",
      "End.",
    ]);
  });

  it("treats a line of spaces as a blank line and drops empty paragraphs", () => {
    assert.deepEqual(paragraphs("  First \n  \t \n\n\nSecond  "), ["First", "Second"]);
  });

  it("has no paragraphs for an empty description", () => {
    assert.deepEqual(paragraphs(""), []);
    assert.deepEqual(paragraphs(" \n\n \n"), []);
  });

  it("keeps markup characters as plain text", () => {
    assert.deepEqual(paragraphs("<img src=x onerror=alert(1)> & <3"), [
      "<img src=x onerror=alert(1)> & <3",
    ]);
  });
});

describe("files", () => {
  it("labels release and beta files", () => {
    assert.equal(releaseLabel({ releaseType: 1 }), "Release");
    assert.equal(releaseLabel({ releaseType: 2 }), "Beta");
    assert.equal(releaseLabel({ releaseType: 3 }), "");
  });

  it("puts release type and UTC date on one line", () => {
    assert.equal(fileMeta({ releaseType: 2, date: "2026-10-24T23:30:00Z" }), "Beta · Oct 24, 2026");
    assert.equal(fileMeta({ releaseType: 9, date: "2026-10-24T00:00:00Z" }), "Oct 24, 2026");
    assert.equal(fileMeta({ releaseType: 1, date: "" }), "Release · —");
  });

  it("falls back to the id when a file has no name", () => {
    assert.equal(fileName({ id: 7, name: "DBM 11.0.2" }), "DBM 11.0.2");
    assert.equal(fileName({ id: 7, name: "  " }), "File #7");
  });
});

describe("dependencies and metadata", () => {
  it("falls back to the project id when CurseForge returns no name", () => {
    assert.equal(dependencyName({ id: 3358, name: "Ace3" }), "Ace3");
    assert.equal(dependencyName({ id: 3358, name: "" }), "Project #3358");
  });

  it("joins category names", () => {
    assert.equal(
      formatCategories([
        { id: 1, name: "Bags", parentCategoryId: null },
        { id: 2, name: "Combat", parentCategoryId: null },
      ]),
      "Bags, Combat",
    );
    assert.equal(formatCategories([]), "—");
  });

  it("names the game version of the detail once the list is known", () => {
    const types = [
      { id: 517, name: "Retail" },
      { id: 67408, name: "Classic" },
    ];
    assert.equal(versionName(types, 67408), "Classic");
    assert.equal(versionName(types, 1), null);
    assert.equal(versionName([], 517), null);
  });
});
