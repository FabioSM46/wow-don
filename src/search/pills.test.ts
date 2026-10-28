// Tests for the state pills of the Search tab (issue #229): `pnpm test` (node --test).

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { blockedHint, type MirrorCheck } from "./mirror.ts";
import { blockedPill } from "./pills.ts";

const available: MirrorCheck = {
  status: "done",
  result: { available: true, repo: "owner/alpha", tag: "v2", reason: null },
};
const different: MirrorCheck = {
  status: "done",
  result: { available: false, repo: null, tag: null, reason: "Not the same file." },
};
const failed: MirrorCheck = { status: "failed", message: "CurseForge did not answer." };

describe("blockedPill", () => {
  it("is an accent pill when Install uses a GitHub release", () => {
    const pill = blockedPill(available);
    assert.equal(pill.label, "via GitHub");
    assert.equal(pill.className, "pill accent");
  });

  it("is a neutral pill for website only, checking and not yet checked", () => {
    const cases: [MirrorCheck | undefined, string][] = [
      [different, "website only"],
      [{ status: "checking" }, "checking GitHub…"],
      [undefined, "blocked on CurseForge"],
      [failed, "blocked on CurseForge"],
    ];
    for (const [check, label] of cases) {
      const pill = blockedPill(check);
      assert.equal(pill.label, label);
      assert.equal(pill.className, "pill");
    }
  });

  it("keeps the label and the title of blockedHint", () => {
    for (const check of [available, different, failed, undefined]) {
      const { label, title } = blockedPill(check);
      assert.deepEqual({ label, title }, blockedHint(check));
    }
  });
});
