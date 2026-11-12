// Tests for the GitHub mirror in the Installed tab (issue #165): `pnpm test` (node --test).

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import type { AddonUpdate } from "../api";
import {
  WEBSITE_ONLY_TITLE,
  blockedHint,
  isUpdatable,
  reinstallBlockedReason,
  websiteUpdate,
} from "./mirror.ts";

const latest = { id: 2, name: "Alpha-2.zip", date: "2026-10-01T00:00:00Z" };

function update(
  allowModDistribution: boolean,
  status: "updateAvailable" | "upToDate",
  mirror: Partial<Pick<AddonUpdate, "githubMirror" | "githubMirrorReason">> = {},
): AddonUpdate {
  return {
    folder: "Alpha",
    source: "curseforge",
    project: { id: 1, source: "curseforge", key: "1", name: "Alpha", url: null, allowModDistribution },
    installedFile: { id: 1, name: "Alpha-1.zip", date: "2026-09-01T00:00:00Z" },
    versionOverride: null,
    projectFolders: ["Alpha"],
    githubMirror: null,
    githubMirrorReason: null,
    ...mirror,
    ...(status === "updateAvailable" ? { status, latest } : { status }),
  } as AddonUpdate;
}

const withMirror = update(false, "updateAvailable", {
  githubMirror: { repo: "owner/alpha", tag: "v2" },
});
const different = update(false, "updateAvailable", {
  githubMirrorReason: "The latest GitHub release (v1) is not the CurseForge file (Alpha-2.zip).",
});
const limited = update(false, "updateAvailable", {
  githubMirrorReason: "GitHub's limit of 60 requests an hour has been reached: try again after 14:32 UTC.",
});

describe("blockedHint", () => {
  it("has no label for a project CurseForge lets the app download", () => {
    assert.equal(blockedHint(update(true, "updateAvailable")), null);
    assert.equal(blockedHint(undefined), null);
  });

  it("says via GitHub with owner/repo tag when the check found a mirror", () => {
    const hint = blockedHint(withMirror);
    assert.equal(hint?.label, "via GitHub");
    assert.equal(/^owner\/alpha v2\n/.test(hint!.title), true);
  });

  it("stays website only, with the mirror reason when there is one", () => {
    assert.deepEqual(blockedHint(update(false, "upToDate")), {
      label: "website only",
      title: WEBSITE_ONLY_TITLE,
    });
    const hint = blockedHint(different);
    assert.equal(hint?.label, "website only");
    assert.equal(
      hint?.title,
      `${WEBSITE_ONLY_TITLE}\nGitHub: The latest GitHub release (v1) is not the CurseForge file (Alpha-2.zip).`,
    );
    assert.equal(/limit of 60 requests/.test(blockedHint(limited)!.title), true);
  });
});

describe("isUpdatable", () => {
  it("updates downloadable projects and blocked ones with a mirror", () => {
    assert.equal(isUpdatable(update(true, "updateAvailable")), true);
    assert.equal(isUpdatable(withMirror), true);
  });

  it("does not update blocked projects without a mirror, or projects already up to date", () => {
    assert.equal(isUpdatable(different), false);
    assert.equal(isUpdatable(update(false, "updateAvailable")), false);
    assert.equal(isUpdatable(update(true, "upToDate")), false);
  });
});

describe("reinstallBlockedReason", () => {
  it("allows reinstalling downloadable projects and blocked ones the backend can try", () => {
    assert.equal(reinstallBlockedReason(update(true, "updateAvailable")), null);
    assert.equal(reinstallBlockedReason(withMirror), null);
    // Up to date: the backend looks for the mirror when reinstalling.
    assert.equal(reinstallBlockedReason(update(false, "upToDate")), null);
    // No mirror computed for the update: the backend still tries it.
    assert.equal(reinstallBlockedReason(update(false, "updateAvailable")), null);
  });

  it("refuses a blocked update whose mirror the check already ruled out, with the reason", () => {
    assert.equal(
      reinstallBlockedReason(different),
      "CurseForge blocks third-party downloads and the GitHub copy cannot be used: The latest GitHub release (v1) is not the CurseForge file (Alpha-2.zip).",
    );
    assert.equal(/limit of 60 requests/.test(reinstallBlockedReason(limited)!), true);
  });
});

describe("websiteUpdate", () => {
  const url = "https://www.curseforge.com/wow/addons/alpha";
  const withUrl = (u: AddonUpdate): AddonUpdate => ({ ...u, project: { ...u.project!, url } });

  it("opens CurseForge for a blocked update without a mirror", () => {
    assert.deepEqual(websiteUpdate(withUrl(update(false, "updateAvailable"))), {
      label: "Get Alpha-2.zip on CurseForge",
      title:
        "The author doesn't allow downloads from other apps. Get Alpha-2.zip on CurseForge, then drop the zip into this window.",
      url,
    });
  });

  it("adds the mirror's reason on its own line", () => {
    const action = websiteUpdate(withUrl(different));
    assert.equal(
      action?.title.endsWith(
        "\nGitHub: The latest GitHub release (v1) is not the CurseForge file (Alpha-2.zip).",
      ),
      true,
    );
  });

  it("is null without a page, with a mirror, when downloadable or up to date", () => {
    assert.equal(websiteUpdate(update(false, "updateAvailable")), null);
    assert.equal(websiteUpdate(withUrl(withMirror)), null);
    assert.equal(websiteUpdate(withUrl(update(true, "updateAvailable"))), null);
    assert.equal(websiteUpdate(withUrl(update(false, "upToDate"))), null);
    assert.equal(websiteUpdate(undefined), null);
  });

  it("is null for a project of another source", () => {
    const other = withUrl(update(false, "updateAvailable"));
    const project = { ...other.project!, source: "wowinterface" as const };
    assert.equal(websiteUpdate({ ...other, project }), null);
  });
});
