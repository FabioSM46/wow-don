// Tests for the Support section's addresses: `pnpm test` (node --test, Node strips the types).

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { describe, it } from "node:test";

// Support.tsx imports the Tauri plugin, which does not load under node: read the source instead.
const source = readFileSync(new URL("./Support.tsx", import.meta.url), "utf8");
const funding = readFileSync(new URL("../../.github/FUNDING.yml", import.meta.url), "utf8");

describe("support addresses", () => {
  it("match the funding file", () => {
    assert.equal(funding.split("\n").includes("github: FabioSM46"), true);
    assert.equal(source.includes('"https://github.com/sponsors/FabioSM46"'), true);
    assert.equal(funding.split("\n").includes("ko_fi: fabiosm46"), true);
    assert.equal(source.includes('"https://ko-fi.com/fabiosm46"'), true);
  });
});
