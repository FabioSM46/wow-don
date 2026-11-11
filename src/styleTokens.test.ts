// Tests for the type scale of D42 (issue #228): every font size and weight in App.css is a
// token; and for the warning and table density tokens of D48 (issue #311). `pnpm test`
// (node --test, Node strips the types).

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
      "--cell-pad-x": "14px",
      "--cell-pad-y": "8px",
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

/** The body of the first rule whose selector is exactly this one. */
function ruleBody(selector: string): string {
  const start = css.indexOf(`${selector} {`);
  assert.equal(start !== -1, true, `no rule for ${selector}`);
  const open = start + selector.length + 1;
  return css.slice(open + 1, css.indexOf("}", open));
}

/** The declarations of a rule body, one per entry, without the surrounding whitespace. */
function declarations(body: string): string[] {
  return body
    .split(";")
    .map((declaration) => declaration.trim())
    .filter((declaration) => declaration !== "");
}

type Rgb = [number, number, number];

/** `#rrggbb` as three channels. */
function hex(value: string): Rgb {
  const match = /^#([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})$/i.exec(value);
  if (match === null) {
    throw new Error(`not a #rrggbb color: ${value}`);
  }
  return [parseInt(match[1], 16), parseInt(match[2], 16), parseInt(match[3], 16)];
}

/** `rgb(r g b / a%)` as three channels and an alpha between 0 and 1. */
function translucent(value: string): { color: Rgb; alpha: number } {
  const match = /^rgb\((\d+) (\d+) (\d+) \/ (\d+)%\)$/.exec(value);
  if (match === null) {
    throw new Error(`not an rgb(r g b / a%) color: ${value}`);
  }
  return {
    color: [Number(match[1]), Number(match[2]), Number(match[3])],
    alpha: Number(match[4]) / 100,
  };
}

/** WCAG relative luminance of an sRGB color. */
function luminance(color: Rgb): number {
  const [r, g, b] = color.map((channel) => {
    const c = channel / 255;
    return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

/** WCAG contrast ratio of `text` on the soft background `soft` laid over `surface`. */
function contrastOnSoft(text: string, soft: string, surface: string): number {
  const { color, alpha } = translucent(soft);
  const under = hex(surface);
  const background = color.map(
    (channel, index) => channel * alpha + under[index] * (1 - alpha),
  ) as Rgb;
  const [light, dark] = [luminance(hex(text)), luminance(background)].sort((a, b) => b - a);
  return (light + 0.05) / (dark + 0.05);
}

/** Reads a `:root` token that the test needs, failing with its name when it is missing. */
function requiredToken(name: string): string {
  const value = rootToken(name);
  if (value === undefined) {
    throw new Error(`${name} is not defined in :root`);
  }
  return value;
}

describe("App.css warning and table density tokens (D48)", () => {
  const warningTokens = ["--warning-soft", "--warning-text"];

  it("defines the warning tokens with their dark twins in :root", () => {
    const expected: Record<string, string> = {
      "--warning-soft": "rgb(180 110 0 / 12%)",
      "--warning-text": "#7a4a00",
      "--dark-warning-soft": "rgb(245 184 74 / 14%)",
      "--dark-warning-text": "#f5c46b",
    };
    for (const [name, value] of Object.entries(expected)) {
      assert.equal(rootToken(name), value, name);
    }
  });

  it("assigns the dark twins in both dark-theme blocks, which stay identical", () => {
    const system = declarations(ruleBody(':root:not([data-theme="light"])'));
    const forced = declarations(ruleBody(':root[data-theme="dark"]'));
    assert.deepEqual(system, forced);
    for (const name of warningTokens) {
      const assignment = `${name}: var(--dark-${name.slice(2)})`;
      assert.equal(system.includes(assignment), true, assignment);
    }
  });

  it("gives every dark value a light token and an assignment in the dark blocks", () => {
    const root = declarations(ruleBody(":root"));
    const dark = root
      .map((declaration) => /^--dark-([\w-]+)\s*:/.exec(declaration)?.[1])
      .filter((name) => name !== undefined);
    assert.equal(dark.length > 0, true, "no --dark-* token found");
    const assigned = declarations(ruleBody(':root[data-theme="dark"]'));
    for (const name of dark) {
      assert.equal(rootToken(`--${name}`) !== undefined, true, `--${name} has no light value`);
      const assignment = `--${name}: var(--dark-${name})`;
      assert.equal(assigned.includes(assignment), true, assignment);
    }
  });

  it("keeps the warning text at 4.5:1 or more on its soft background over --surface", () => {
    const light = contrastOnSoft(
      requiredToken("--warning-text"),
      requiredToken("--warning-soft"),
      requiredToken("--surface"),
    );
    const dark = contrastOnSoft(
      requiredToken("--dark-warning-text"),
      requiredToken("--dark-warning-soft"),
      requiredToken("--dark-surface"),
    );
    assert.equal(light >= 4.5, true, `light contrast ${light.toFixed(2)}:1`);
    assert.equal(dark >= 4.5, true, `dark contrast ${dark.toFixed(2)}:1`);
  });

  it("uses the warning tokens only in the .pill.warning variant", () => {
    assert.deepEqual(declarations(ruleBody(".pill.warning")), [
      "background: var(--warning-soft)",
      "color: var(--warning-text)",
    ]);
    for (const name of warningTokens) {
      const uses = css.split(`var(${name})`).length - 1;
      assert.equal(uses, 1, `${name} is a state color of .pill.warning only`);
    }
  });

  it("pads the table cells with the density tokens", () => {
    const cell = declarations(ruleBody(".addon-table th,\n.addon-table td"));
    assert.equal(cell.includes("padding: var(--cell-pad-y) var(--cell-pad-x)"), true);
  });

  it("narrows the table by overriding the token, not by writing a padding", () => {
    const start = css.indexOf("@container (max-width: 960px)");
    assert.equal(start !== -1, true, "no narrow-container rule");
    const narrow = css.slice(start, css.indexOf("\n}\n", start));
    assert.equal(/\.addon-table \{\s*--cell-pad-x: 7px;\s*\}/.test(narrow), true, narrow);
    assert.equal(/(?<![-\w])padding/.test(narrow), false, narrow);
  });
});
