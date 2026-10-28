// Tests for the GitHub mirror in the Search tab (issue #165): `pnpm test` (node --test).

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  NO_REPOSITORY,
  blockedHint,
  detailNote,
  mirrorCheckFor,
  mirrorName,
  mirrorStep,
  mirrorsToCheck,
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
const checkingForInstall: MirrorCheck = { status: "checking", forInstall: true };

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
    assert.equal(blockedHint(checking).label, "checking GitHub…");
    assert.equal(blockedHint(checkingForInstall).label, "checking GitHub…");
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

type Project = {
  id: number;
  allowModDistribution: boolean;
  githubRepo: string | null;
  installed: boolean;
};

const project = (id: number, fields: Partial<Project> = {}): Project => ({
  id,
  allowModDistribution: false,
  githubRepo: "owner/alpha",
  installed: false,
  ...fields,
});

describe("mirrorCheckFor (issue #223)", () => {
  const noRepo = project(1, { githubRepo: null });

  it("sends a blocked project without a GitHub repository to the website, with no check", () => {
    const check = mirrorCheckFor(noRepo, undefined);
    assert.deepEqual(mirrorStep(check), { kind: "website", reason: NO_REPOSITORY });
    assert.deepEqual(blockedHint(check), {
      label: "website only",
      title: unavailableText(NO_REPOSITORY),
    });
    assert.equal(
      /does not link a GitHub repository\. Get it from the CurseForge website\.$/.test(
        detailNote(check),
      ),
      true,
    );
    // Whatever the session holds, without a repository there is no mirror.
    assert.equal(mirrorStep(mirrorCheckFor(noRepo, failed)).kind, "website");
  });

  it("keeps the session check for a blocked project with a repository", () => {
    const withRepo = project(2);
    assert.equal(mirrorCheckFor(withRepo, undefined), undefined);
    for (const check of [checking, available, different, failed]) {
      assert.equal(mirrorCheckFor(withRepo, check), check);
    }
    assert.equal(blockedHint(mirrorCheckFor(withRepo, checking)).label, "checking GitHub…");
    assert.equal(blockedHint(mirrorCheckFor(withRepo, available)).label, "via GitHub");
    assert.equal(blockedHint(mirrorCheckFor(withRepo, different)).label, "website only");
    // A failed check leaves the row as before: Install, which tries again.
    assert.equal(blockedHint(mirrorCheckFor(withRepo, failed)).label, "blocked on CurseForge");
    assert.deepEqual(mirrorStep(mirrorCheckFor(withRepo, failed)), { kind: "check" });
  });

  it("does not touch projects that CurseForge lets download", () => {
    const free = project(3, { allowModDistribution: true, githubRepo: null });
    assert.equal(mirrorCheckFor(free, undefined), undefined);
    assert.equal(mirrorCheckFor(free, available), available);
  });
});

describe("mirrorsToCheck (issue #223)", () => {
  const ids = (addons: Project[]) => addons.map((a) => a.id);
  const none = () => false;

  it("checks only blocked, not installed projects with a GitHub repository", () => {
    const addons = [
      project(1),
      project(2, { githubRepo: null }),
      project(3, { allowModDistribution: true }),
      project(4, { installed: true }),
      project(5),
    ];
    assert.deepEqual(ids(mirrorsToCheck(addons, none, 4)), [1, 5]);
  });

  it("skips projects with an outcome or a check in progress, failures included", () => {
    const addons = [project(1), project(2), project(3)];
    assert.deepEqual(ids(mirrorsToCheck(addons, (a) => a.id !== 3, 4)), [3]);
  });

  it("starts at most the free slots, in list order", () => {
    const addons = [1, 2, 3, 4, 5, 6].map((id) => project(id));
    assert.deepEqual(ids(mirrorsToCheck(addons, none, 4)), [1, 2, 3, 4]);
    assert.deepEqual(ids(mirrorsToCheck(addons, none, 1)), [1]);
    assert.deepEqual(mirrorsToCheck(addons, none, 0), []);
  });
});
