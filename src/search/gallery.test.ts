// Tests for the add-on detail tabs and gallery: `pnpm test` (node --test, Node strips the types).

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  galleryLabel,
  nextIndex,
  positionLabel,
  previousIndex,
  screenshotTitle,
  tabIndexForKey,
} from "./gallery.ts";

describe("gallery labels", () => {
  it("counts the screenshots in the tab label, zero included", () => {
    assert.equal(galleryLabel(3), "Gallery (3)");
    assert.equal(galleryLabel(0), "Gallery (0)");
  });

  it("uses the title, or a numbered fallback counting from one", () => {
    assert.equal(screenshotTitle({ title: "Options panel" }, 0), "Options panel");
    assert.equal(screenshotTitle({ title: "" }, 0), "Screenshot 1");
    assert.equal(screenshotTitle({ title: "   " }, 4), "Screenshot 5");
  });

  it("shows the position counting from one", () => {
    assert.equal(positionLabel(0, 5), "1 / 5");
    assert.equal(positionLabel(4, 5), "5 / 5");
  });
});

describe("gallery navigation", () => {
  it("wraps around in both directions", () => {
    assert.equal(nextIndex(0, 3), 1);
    assert.equal(nextIndex(2, 3), 0);
    assert.equal(previousIndex(1, 3), 0);
    assert.equal(previousIndex(0, 3), 2);
  });

  it("stays on the only image", () => {
    assert.equal(nextIndex(0, 1), 0);
    assert.equal(previousIndex(0, 1), 0);
  });

  it("does not divide by zero without images", () => {
    assert.equal(nextIndex(0, 0), 0);
    assert.equal(previousIndex(0, 0), 0);
  });
});

describe("tab keys", () => {
  it("moves with the arrows and wraps", () => {
    assert.equal(tabIndexForKey("ArrowRight", 0, 2), 1);
    assert.equal(tabIndexForKey("ArrowRight", 1, 2), 0);
    assert.equal(tabIndexForKey("ArrowLeft", 0, 2), 1);
  });

  it("jumps to the ends with Home and End", () => {
    assert.equal(tabIndexForKey("Home", 1, 2), 0);
    assert.equal(tabIndexForKey("End", 0, 2), 1);
  });

  it("ignores other keys", () => {
    assert.equal(tabIndexForKey("Enter", 0, 2), null);
    assert.equal(tabIndexForKey("ArrowDown", 0, 2), null);
  });
});
