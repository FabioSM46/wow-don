// The state pills of the Search tab (issue #229): which `.pill` class (D42) shows the state
// of a project blocked by CurseForge. Text and hint stay those of `blockedHint`; here only
// the color is chosen. Pure functions, no React or I/O.

import { blockedHint, mirrorName, type BlockedHint, type MirrorCheck } from "./mirror.ts";

/** A pill: the text, the hint and the class with the color variant. */
export type StatePill = BlockedHint & { className: string };

/**
 * The pill of a blocked project: accent with a usable GitHub mirror ("via GitHub"), neutral
 * otherwise ("website only", "checking GitHub…", "blocked on CurseForge"). The color
 * signals the state that allows installing, it does not decorate.
 */
export function blockedPill(check: MirrorCheck | undefined): StatePill {
  const hint = blockedHint(check);
  return { ...hint, className: mirrorName(check) ? "pill accent" : "pill" };
}
