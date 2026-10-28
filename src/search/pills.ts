// Le pastiglie di stato della tab Search (issue #229): quale classe `.pill` (D42) mostra lo stato
// di un progetto bloccato da CurseForge. Testo e suggerimento restano quelli di `blockedHint`;
// qui si sceglie solo il colore. Funzioni pure, senza React né I/O.

import { blockedHint, mirrorName, type BlockedHint, type MirrorCheck } from "./mirror.ts";

/** Una pastiglia: il testo, il suggerimento e la classe con la variante di colore. */
export type StatePill = BlockedHint & { className: string };

/**
 * La pastiglia di un progetto bloccato: d'accento con un mirror GitHub utilizzabile ("via
 * GitHub"), neutra altrimenti ("website only", "checking GitHub…", "blocked on CurseForge"). Il
 * colore segnala lo stato che lascia installare, non abbellisce.
 */
export function blockedPill(check: MirrorCheck | undefined): StatePill {
  const hint = blockedHint(check);
  return { ...hint, className: mirrorName(check) ? "pill accent" : "pill" };
}
