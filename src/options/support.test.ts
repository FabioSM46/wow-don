// Tests for the Support section's addresses: `pnpm test` (node --test, Node strips the types).

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { describe, it } from "node:test";

import { SUPPORT_LINKS, SUPPORT_PAGES } from "./supportLinks.ts";

/** The `key: value` lines of `.github/FUNDING.yml`. */
const funding: Record<string, string> = Object.fromEntries(
  readFileSync(new URL("../../.github/FUNDING.yml", import.meta.url), "utf8")
    .split("\n")
    .filter((line) => line.includes(":"))
    .map((line) => {
      const colon = line.indexOf(":");
      return [line.slice(0, colon).trim(), line.slice(colon + 1).trim()];
    }),
);

describe("support links", () => {
  it("are derived from the funding file", () => {
    assert.equal(funding.github, "FabioSM46");
    assert.equal(SUPPORT_LINKS.github, `https://github.com/sponsors/${funding.github}`);
    assert.equal(funding.ko_fi, "fabiosm46");
    assert.equal(SUPPORT_LINKS.koFi, `https://ko-fi.com/${funding.ko_fi}`);
  });

  it("pair each button with its own page", () => {
    assert.deepEqual(
      SUPPORT_PAGES.map((page) => [page.label, page.url]),
      [
        ["GitHub Sponsors", SUPPORT_LINKS.github],
        ["Ko-fi", SUPPORT_LINKS.koFi],
      ],
    );
  });
});
