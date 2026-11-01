// Tests for the footer notice of a newer release: `pnpm test` (node --test, Node strips the types).

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import type { AppUpdateStatus } from "./api.ts";
import { appUpdateNotice } from "./appUpdate.ts";

const RELEASE = "https://example.com/releases/tag/v0.1.2";
const update: AppUpdateStatus = {
  current: "0.1.1",
  latest: "0.1.2",
  url: RELEASE,
  updateAvailable: true,
};

describe("appUpdateNotice", () => {
  it("names the app and the new version, and links the release", () => {
    assert.deepEqual(appUpdateNotice(update, true), { text: "WoW-don 0.1.2 available", url: RELEASE });
  });

  it("never says a bare update, which in this app means addons", () => {
    const text = appUpdateNotice(update, true)?.text ?? "";
    assert.equal(text.startsWith("WoW-don "), true);
    assert.equal(/update/i.test(text), false);
  });

  it("shows nothing before a check has succeeded", () => {
    const unknown = { current: "0.1.1", latest: null, url: null, updateAvailable: false };
    assert.equal(appUpdateNotice(unknown, true), null);
    assert.equal(appUpdateNotice(null, true), null);
  });

  it("shows nothing when the known release is not newer", () => {
    assert.equal(appUpdateNotice({ ...update, updateAvailable: false }, true), null);
  });

  it("goes at once with the option off, even if the backend still knows the release", () => {
    assert.equal(appUpdateNotice(update, false), null);
  });

  it("shows nothing until the option has been read", () => {
    assert.equal(appUpdateNotice(update, null), null);
  });

  it("shows nothing without a version or a page to open", () => {
    assert.equal(appUpdateNotice({ ...update, latest: null }, true), null);
    assert.equal(appUpdateNotice({ ...update, url: null }, true), null);
  });
});
