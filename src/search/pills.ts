// The state pills of the Search tab (issue #229): which `.pill` class (D42) shows the state
// of a project blocked by CurseForge. Text and hint stay those of `blockedHint`; here only
// the color is chosen. Pure functions, no React or I/O.

import {
  blockedHint,
  mirrorName,
  mirrorStep,
  type BlockedHint,
  type MirrorCheck,
} from "./mirror.ts";

/** A pill: the text, the hint and the class with the color variant. */
export type StatePill = BlockedHint & { className: string };

/**
 * The pill of a blocked project: accent with a usable GitHub mirror ("via GitHub"), warning
 * when it can only be opened on the website ("website only", D48), neutral while that is not
 * known yet ("checking GitHub…", "blocked on CurseForge"). The color signals the state, it
 * does not decorate.
 */
export function blockedPill(check: MirrorCheck | undefined): StatePill {
  return { ...blockedHint(check), className: pillClass(check) };
}

function pillClass(check: MirrorCheck | undefined): string {
  if (mirrorName(check)) return "pill accent";
  return mirrorStep(check).kind === "website" ? "pill warning" : "pill";
}
