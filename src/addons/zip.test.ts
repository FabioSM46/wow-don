// Tests for the local zip helpers: `pnpm test` (node --test, Node strips the types).

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { describeZipInstall, fileName, isZipPath, pickZip } from "./zip.ts";

describe("fileName", () => {
  it("takes the last component with either separator", () => {
    assert.equal(fileName("/data/downloads/Details.zip"), "Details.zip");
    assert.equal(fileName("D:\\Downloads\\Details.zip"), "Details.zip");
    assert.equal(fileName("Details.zip"), "Details.zip");
  });
});

describe("isZipPath", () => {
  it("accepts .zip in any letter case", () => {
    assert.equal(isZipPath("/tmp/a/Details.ZIP"), true);
    assert.equal(isZipPath("C:\\x\\pack.Zip"), true);
  });

  it("rejects other files and folders named like zips", () => {
    assert.equal(isZipPath("/tmp/a/Details.rar"), false);
    assert.equal(isZipPath("/tmp/a.zip/readme"), false);
    assert.equal(isZipPath("/tmp/zip"), false);
  });
});

describe("pickZip", () => {
  it("returns the single dropped zip", () => {
    assert.deepEqual(pickZip(["/d/Details.zip"], true), { path: "/d/Details.zip" });
  });

  it("explains every rejected drop", () => {
    assert.deepEqual(pickZip(["/d/Details.zip"], false), {
      problem: "Add an installation in Options before installing a zip.",
    });
    assert.deepEqual(pickZip([], true), { problem: "Drop a .zip file to install it." });
    assert.deepEqual(pickZip(["/d/a.zip", "/d/b.zip"], true), {
      problem: "Drop one .zip file at a time.",
    });
    assert.deepEqual(pickZip(["/d/notes.txt"], true), {
      problem: "notes.txt is not a .zip file: only zips can be installed.",
    });
  });
});

describe("describeZipInstall", () => {
  it("lists the installed folders", () => {
    assert.equal(
      describeZipInstall({ installed: ["Details"], replaced: [] }, "Details.zip", "Retail"),
      "Installed Details.zip into Retail: 1 folder (Details).",
    );
  });

  it("adds the replaced folders", () => {
    assert.equal(
      describeZipInstall(
        { installed: ["Pack", "Pack_Options"], replaced: ["pack"] },
        "Pack.zip",
        "Classic",
      ),
      "Installed Pack.zip into Classic: 2 folders (Pack, Pack_Options). Replaced: pack.",
    );
  });
});
