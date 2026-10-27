// Tests for the source helpers: `pnpm test` (node --test, Node strips the types).

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import type { AddonUpdate, SourceKind } from "../api";
import { projectId, sourcesIn, updateRequest } from "./sources.ts";

describe("projectId", () => {
  it("keeps sources apart even with the same key", () => {
    assert.equal(projectId({ source: "curseforge", key: "5108" }), "curseforge:5108");
    assert.equal(projectId({ source: "wowinterface", key: "5108" }), "wowinterface:5108");
  });
});

describe("updateRequest", () => {
  it("splits CurseForge ids from the other sources without repeats", () => {
    assert.deepEqual(
      updateRequest([
        { source: "curseforge", key: "61284" },
        { source: "tukui", key: "-2" },
        { source: "curseforge", key: "61284" },
        { source: "github", key: "owner/repo" },
      ]),
      {
        modIds: [61284],
        projects: [
          { source: "tukui", key: "-2" },
          { source: "github", key: "owner/repo" },
        ],
      },
    );
  });

  it("sends a CurseForge key that is not a number as a project", () => {
    assert.deepEqual(updateRequest([{ source: "curseforge", key: "x" }]), {
      modIds: [],
      projects: [{ source: "curseforge", key: "x" }],
    });
  });
});

function matched(folder: string, source: SourceKind): AddonUpdate {
  return {
    folder,
    source,
    project: { id: 0, source, key: folder, name: folder, url: null, allowModDistribution: true },
    installedFile: null,
    versionOverride: null,
    projectFolders: [folder],
    githubMirror: null,
    githubMirrorReason: null,
    status: "upToDate",
  };
}

describe("sourcesIn", () => {
  it("lists the sources of matched folders in a fixed order", () => {
    const updates = new Map<string, AddonUpdate>([
      ["A", matched("A", "github")],
      ["B", matched("B", "curseforge")],
      ["C", matched("C", "github")],
    ]);
    assert.deepEqual(sourcesIn(updates), ["curseforge", "github"]);
    assert.deepEqual(sourcesIn(undefined), []);
  });
});
