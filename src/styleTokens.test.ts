// Tests for the type scale of D42 (issue #228): every font size and weight in App.css is a
// token. `pnpm test` (node --test, Node strips the types).

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { describe, it } from "node:test";

/** App.css without comments, so a value quoted in a comment is not read as a declaration. */
const css = readFileSync(new URL("./App.css", import.meta.url), "utf8").replace(
  /\/\*[\s\S]*?\*\//g,
  "",
);

/** The values of every `property: value` declaration with this exact property name. */
function values(property: string): string[] {
  const pattern = new RegExp(`(?<![-\\w])${property}\\s*:\\s*([^;}]+)`, "g");
  return [...css.matchAll(pattern)].map((match) => match[1].trim());
}

/** The value a custom property gets in the first `:root` block. */
function rootToken(name: string): string | undefined {
  const root = css.slice(css.indexOf(":root {"), css.indexOf("}", css.indexOf(":root {")));
  return new RegExp(`${name}\\s*:\\s*([^;]+);`).exec(root)?.[1].trim();
}

describe("App.css type scale", () => {
  it("defines the scale tokens with the D42 values", () => {
    const expected: Record<string, string> = {
      "--text-xs": "12px",
      "--text-base": "14px",
      "--text-lg": "16px",
      "--text-xl": "20px",
      "--weight-regular": "400",
      "--weight-strong": "600",
      "--control-height": "32px",
      "--control-height-sm": "28px",
      "--radius": "8px",
      "--radius-lg": "10px",
      "--space-1": "4px",
      "--space-2": "8px",
      "--space-3": "12px",
      "--space-4": "16px",
      "--space-5": "24px",
      "--space-6": "32px",
    };
    for (const [name, value] of Object.entries(expected)) {
      assert.equal(rootToken(name), value, name);
    }
  });

  it("uses only the four size tokens or inherit for font-size", () => {
    const allowed = new Set([
      "var(--text-xs)",
      "var(--text-base)",
      "var(--text-lg)",
      "var(--text-xl)",
      "inherit",
    ]);
    const sizes = values("font-size");
    assert.equal(sizes.length > 0, true, "no font-size found");
    assert.deepEqual(
      sizes.filter((size) => !allowed.has(size)),
      [],
    );
  });

  it("uses only the two weight tokens or inherit for font-weight", () => {
    const allowed = new Set(["var(--weight-regular)", "var(--weight-strong)", "inherit"]);
    assert.deepEqual(
      values("font-weight").filter((weight) => !allowed.has(weight)),
      [],
    );
  });

  it("sets no size through the font shorthand", () => {
    assert.deepEqual(
      values("font").filter((font) => font !== "inherit"),
      [],
    );
  });
});
