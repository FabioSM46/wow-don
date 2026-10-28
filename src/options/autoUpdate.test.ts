// Tests for the Automatic updates section texts: `pnpm test` (node --test).

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  AUTO_UPDATE_INTERVALS,
  AUTO_UPDATE_MODES,
  installationHint,
  intervalDisabled,
  intervalLabel,
  isAutoUpdateMode,
  modeHint,
  shownValue,
} from "./autoUpdate.ts";

describe("modes and intervals", () => {
  it("offers Off, Notify only and Install, with the backend values", () => {
    assert.deepEqual(
      AUTO_UPDATE_MODES.map((mode) => [mode.id, mode.label]),
      [
        ["off", "Off"],
        ["notify", "Notify only"],
        ["install", "Install"],
      ],
    );
    assert.equal(isAutoUpdateMode("install"), true);
    assert.equal(isAutoUpdateMode("always"), false);
  });

  it("offers the intervals the backend accepts", () => {
    assert.deepEqual(AUTO_UPDATE_INTERVALS, [1, 3, 6, 12, 24]);
    assert.equal(intervalLabel(1), "Every hour");
    assert.equal(intervalLabel(6), "Every 6 hours");
  });

  it("disables the interval with Off and while the settings load", () => {
    assert.equal(intervalDisabled("off"), true);
    assert.equal(intervalDisabled(null), true);
    assert.equal(intervalDisabled("notify"), false);
    assert.equal(intervalDisabled("install"), false);
  });
});

describe("hints", () => {
  it("says what each mode does", () => {
    assert.equal(modeHint("off").includes("only when you ask"), true);
    assert.equal(modeHint("notify").includes("only tells you"), true);
    assert.equal(modeHint("install").includes("installs them"), true);
    assert.equal(modeHint(null), modeHint("notify"));
  });

  it("says that Off covers every installation", () => {
    assert.equal(installationHint("off"), "Automatic updates are off for every installation.");
    assert.equal(installationHint("install").includes("Don't auto-update"), true);
  });
});

describe("shownValue", () => {
  it("shows the value being saved, then the saved one", () => {
    assert.equal(shownValue("off", "install"), "install");
    assert.equal(shownValue("off", undefined), "off");
    assert.equal(shownValue(6, 12), 12);
    assert.equal(shownValue(false, undefined), false);
  });
});
