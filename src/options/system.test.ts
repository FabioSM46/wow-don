// Tests for the System section texts: `pnpm test` (node --test, Node strips the types).

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { autostartHint, closeToTrayHint, TRAY_UNAVAILABLE } from "./system.ts";

describe("closeToTrayHint", () => {
  it("explains how to quit when the tray is available", () => {
    assert.equal(closeToTrayHint(true).includes("Quit from the tray icon's menu"), true);
  });

  it("says the tray is missing when it isn't available", () => {
    assert.equal(closeToTrayHint(false), TRAY_UNAVAILABLE);
    assert.equal(TRAY_UNAVAILABLE, "The system tray isn't available on this desktop.");
  });

  it("uses the normal text while the settings are loading", () => {
    assert.equal(closeToTrayHint(null), closeToTrayHint(true));
  });
});

describe("autostartHint", () => {
  it("starts hidden only with Close to tray on and the tray available", () => {
    assert.equal(autostartHint(true, true).includes("starts hidden in the tray"), true);
    assert.equal(autostartHint(true, true).includes("opens its window"), false);
  });

  it("opens the window with Close to tray off", () => {
    assert.equal(autostartHint(false, true).includes("opens its window"), true);
    assert.equal(autostartHint(false, true).includes("With Close to tray on"), true);
  });

  it("opens the window when the tray isn't available, even with Close to tray on", () => {
    for (const closeToTray of [true, false]) {
      const hint = autostartHint(closeToTray, false);
      assert.equal(hint.includes("opens its window"), true);
      assert.equal(hint.includes("tray isn't available"), true);
    }
  });

  it("does not promise a hidden start while the settings are loading", () => {
    assert.equal(autostartHint(null, null).includes("opens its window"), true);
  });
});
