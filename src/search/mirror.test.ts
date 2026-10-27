// Tests for the GitHub mirror in the Search tab (issue #165): `pnpm test` (node --test).

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  blockedHint,
  detailNote,
  mirrorName,
  mirrorStep,
  unavailableText,
  type MirrorCheck,
} from "./mirror.ts";

const available: MirrorCheck = {
  status: "done",
  result: { available: true, repo: "owner/alpha", tag: "v2", reason: null },
};
const different: MirrorCheck = {
  status: "done",
  result: {
    available: false,
    repo: null,
    tag: null,
    reason: "The latest GitHub release (v1) is not the CurseForge file (Alpha-2.zip).",
  },
};
const limited: MirrorCheck = {
  status: "done",
  result: {
    available: false,
    repo: null,
    tag: null,
    reason: "GitHub's limit of 60 requests an hour has been reached: try again after 14:32 UTC.",
  },
};
const notBlocked: MirrorCheck = {
  status: "done",
  result: { available: false, repo: null, tag: null, reason: null },
};
const failed: MirrorCheck = { status: "failed", message: "CurseForge did not answer." };
const checking: MirrorCheck = { status: "checking" };

describe("mirrorStep", () => {
  it("checks first, then waits for the answer", () => {
    assert.deepEqual(mirrorStep(undefined), { kind: "check" });
    assert.deepEqual(mirrorStep(failed), { kind: "check" });
    assert.deepEqual(mirrorStep(checking), { kind: "wait" });
  });

  it("installs with a mirror, or when CurseForge no longer blocks the addon", () => {
    assert.deepEqual(mirrorStep(available), { kind: "install" });
    assert.deepEqual(mirrorStep(notBlocked), { kind: "install" });
  });

  it("sends to the website with the reason otherwise", () => {
    assert.deepEqual(mirrorStep(different), {
      kind: "website",
      reason: "The latest GitHub release (v1) is not the CurseForge file (Alpha-2.zip).",
    });
    assert.equal(mirrorStep(limited).kind, "website");
  });
});

describe("labels", () => {
  it("names the mirror only when it is available", () => {
    assert.equal(mirrorName(available), "owner/alpha v2");
    assert.equal(mirrorName(different), null);
    assert.equal(mirrorName(undefined), null);
  });

  it("labels the row: blocked before the check, then via GitHub or website only", () => {
    assert.equal(blockedHint(undefined).label, "blocked on CurseForge");
    assert.equal(blockedHint(checking).label, "blocked on CurseForge");
    assert.equal(blockedHint(failed).label, "blocked on CurseForge");
    assert.equal(blockedHint(available).label, "via GitHub");
    assert.equal(/^owner\/alpha v2\n/.test(blockedHint(available).title), true);
    assert.deepEqual(blockedHint(limited), {
      label: "website only",
      title: unavailableText(
        "GitHub's limit of 60 requests an hour has been reached: try again after 14:32 UTC.",
      ),
    });
  });

  it("explains each state in the detail", () => {
    assert.equal(/from GitHub \(owner\/alpha v2\)\.$/.test(detailNote(available)), true);
    assert.equal(/is not the CurseForge file .* Get it from the CurseForge website\.$/.test(detailNote(different)), true);
    assert.equal(/checking GitHub/.test(detailNote(checking)), true);
    assert.equal(/GitHub could not be checked: CurseForge did not answer\.$/.test(detailNote(failed)), true);
    assert.equal(/looks for the same release on GitHub/.test(detailNote(undefined)), true);
  });
});
