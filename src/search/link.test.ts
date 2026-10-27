// Tests for "Add by link": `pnpm test` (node --test, Node strips the types).

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import type { UpdateOutcome } from "../api.ts";
import { NOT_DOWNLOADABLE, checkLink, linkOutcome, previewNote } from "./link.ts";

describe("checkLink", () => {
  it("trims the text and passes anything that may be a link to the backend", () => {
    assert.deepEqual(checkLink("  https://github.com/owner/repo \n"), {
      ok: true,
      url: "https://github.com/owner/repo",
    });
    // Without a scheme the backend assumes https://.
    assert.deepEqual(checkLink("tukui.org/elvui"), { ok: true, url: "tukui.org/elvui" });
  });

  it("asks for a link when the field is empty", () => {
    assert.deepEqual(checkLink(""), { ok: false, message: "Paste the link of an addon." });
    assert.deepEqual(checkLink("   "), { ok: false, message: "Paste the link of an addon." });
  });

  it("rejects text with spaces or without a host", () => {
    const invalid = { ok: false, message: "This is not a valid link." };
    assert.deepEqual(checkLink("Details damage meter"), invalid);
    assert.deepEqual(checkLink("https://github.com/owner/repo extra"), invalid);
    assert.deepEqual(checkLink("elvui"), invalid);
  });
});

describe("previewNote", () => {
  it("explains a project that cannot be downloaded, with the backend reason first", () => {
    assert.equal(
      previewNote({ downloadable: false, message: "No zip for this game: drop it in." }),
      "No zip for this game: drop it in.",
    );
    assert.equal(previewNote({ downloadable: false }), NOT_DOWNLOADABLE);
    assert.equal(previewNote({ downloadable: false, message: "  " }), NOT_DOWNLOADABLE);
    assert.equal(NOT_DOWNLOADABLE.includes("drop it into this window"), true);
  });

  it("shows a warning on a downloadable project and nothing without one", () => {
    assert.equal(previewNote({ downloadable: true, message: "Beta release." }), "Beta release.");
    assert.equal(previewNote({ downloadable: true }), null);
  });
});

describe("linkOutcome", () => {
  const base: UpdateOutcome = {
    modId: 0,
    source: "github",
    key: "owner/repo",
    name: "Repo",
    status: "updated",
  };

  it("says the project was installed, with a warning after the sentence", () => {
    assert.deepEqual(linkOutcome(base, { name: "Preview" }), {
      text: "Repo installed.",
      error: false,
      installed: true,
    });
    assert.deepEqual(linkOutcome({ ...base, message: "Records not updated." }, { name: "x" }), {
      text: "Repo installed. Records not updated.",
      error: false,
      installed: true,
    });
  });

  it("gives the reason of a skipped install in a neutral tone", () => {
    assert.deepEqual(
      linkOutcome({ ...base, status: "skipped", message: "Already installed." }, { name: "x" }),
      { text: "Repo was not installed: Already installed.", error: false, installed: false },
    );
    assert.equal(linkOutcome({ ...base, status: "skipped" }, { name: "x" }).text, "Repo was not installed.");
  });

  it("reports an error with its message", () => {
    assert.deepEqual(
      linkOutcome({ ...base, status: "error", message: "Network down." }, { name: "x" }),
      { text: "Repo could not be installed: Network down.", error: true, installed: false },
    );
    assert.equal(
      linkOutcome({ ...base, status: "error" }, { name: "x" }).text,
      "Repo could not be installed.",
    );
  });

  it("falls back to the preview name, then to the project id", () => {
    const unnamed = { ...base, name: " " };
    assert.equal(linkOutcome(unnamed, { name: "ElvUI" }).text, "ElvUI installed.");
    assert.equal(linkOutcome(unnamed, { name: "" }).text, "owner/repo installed.");
  });
});
