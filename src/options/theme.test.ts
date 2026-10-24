// Tests for the theme helpers: `pnpm test` (node --test, Node strips the types).

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { applyTheme, isTheme, THEMES, themeAttribute } from "./theme.ts";

/** Un `<html>` finto che ricorda solo gli attributi. */
function fakeRoot() {
  const attributes = new Map<string, string>();
  return {
    attributes,
    setAttribute: (name: string, value: string) => void attributes.set(name, value),
    removeAttribute: (name: string) => void attributes.delete(name),
  };
}

describe("theme", () => {
  it("offers system, light and dark in this order", () => {
    assert.deepEqual(
      THEMES.map((theme) => theme.label),
      ["System", "Light", "Dark"],
    );
  });

  it("recognizes only the three themes", () => {
    for (const known of ["system", "light", "dark"]) assert.equal(isTheme(known), true);
    for (const unknown of ["Dark", "", "sepia"]) assert.equal(isTheme(unknown), false);
  });

  it("maps system to no attribute and the forced themes to themselves", () => {
    assert.equal(themeAttribute("system"), null);
    assert.equal(themeAttribute("light"), "light");
    assert.equal(themeAttribute("dark"), "dark");
  });

  it("sets data-theme for a forced theme and removes it for system", () => {
    const root = fakeRoot();
    applyTheme(root, "dark");
    assert.equal(root.attributes.get("data-theme"), "dark");
    applyTheme(root, "light");
    assert.equal(root.attributes.get("data-theme"), "light");
    applyTheme(root, "system");
    assert.equal(root.attributes.has("data-theme"), false);
  });
});
