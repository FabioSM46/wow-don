// "Add by link" in the Search tab (issue #110): the minimal validation of the pasted text and
// the text of the `install_from_link` outcome. The backend recognises the real link
// (`resolve_link`): here only what cannot be a link is discarded. Pure functions, no React or I/O.

import type { LinkPreview, UpdateOutcome } from "../api";

/** The link to pass to `resolve_link`, or the reason the text cannot be one. */
export type LinkInput = { ok: true; url: string } | { ok: false; message: string };

/**
 * The pasted text, without surrounding spaces. Empty, with spaces in the middle or without a dot
 * (no host can lack one) is not a link: the answer comes without calling the backend. Everything
 * else is judged by `resolve_link`, which knows which sources and which pages it recognises.
 */
export function checkLink(text: string): LinkInput {
  const url = text.trim();
  if (url === "") return { ok: false, message: "Paste the link of an addon." };
  if (/\s/.test(url) || !url.includes(".")) {
    return { ok: false, message: "This is not a valid link." };
  }
  return { ok: true, url };
}

/** When the preview cannot be installed and the backend does not say why. */
export const NOT_DOWNLOADABLE =
  "WoW-don cannot download this addon for this game version: download the zip from its page, then drop it into this window.";

/** The explanation under the preview: the reason for a non-downloadable addon, or a warning. */
export function previewNote(preview: Pick<LinkPreview, "downloadable" | "message">): string | null {
  const message = preview.message?.trim() ?? "";
  if (!preview.downloadable) return message || NOT_DOWNLOADABLE;
  return message || null;
}

/** The outcome of an install from a link: `error` shows as an error, the rest in a neutral tone. */
export type LinkResult = { text: string; error: boolean; installed: boolean };

/**
 * The sentence on the outcome of `install_from_link`, which has the shape of `UpdateOutcome`:
 * `updated` means installed. The outcome's name can be empty, and then the preview's applies.
 */
export function linkOutcome(outcome: UpdateOutcome, preview: Pick<LinkPreview, "name">): LinkResult {
  const name = outcome.name.trim() || preview.name.trim() || outcome.key;
  const message = outcome.message?.trim();
  switch (outcome.status) {
    case "updated":
      // On a successful install the message is a warning: it follows the sentence.
      return {
        text: `${name} installed.${message ? ` ${message}` : ""}`,
        error: false,
        installed: true,
      };
    case "skipped":
      return {
        text: `${name} was not installed${message ? `: ${message}` : "."}`,
        error: false,
        installed: false,
      };
    default:
      return {
        text: `${name} could not be installed${message ? `: ${message}` : "."}`,
        error: true,
        installed: false,
      };
  }
}
