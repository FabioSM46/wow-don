// Il mirror GitHub dei progetti CurseForge bloccati nella tab Installed (D34, issue #165):
// l'etichetta accanto al progetto, se Update è abilitato e se Reinstall può servire. Funzioni
// pure, senza React né I/O.

import type { AddonUpdate } from "../api";

/** Il testo di un progetto bloccato senza un mirror utilizzabile. */
export const WEBSITE_ONLY_TITLE =
  "The author does not allow downloads from third-party apps: download it from the website, then drop the zip into this window.";

/** L'etichetta accanto al nome del progetto: il testo e il suo suggerimento. */
export type BlockedHint = { label: string; title: string };

/** Il mirror utilizzabile dell'aggiornamento, se il controllo ne ha trovato uno. */
function mirrorOf(update: AddonUpdate) {
  return update.project && !update.project.allowModDistribution
    ? (update.githubMirror ?? null)
    : null;
}

/**
 * L'etichetta di un progetto che CurseForge non lascia scaricare (D13): "via GitHub" con il
 * mirror trovato dal controllo, altrimenti "website only" con il motivo del mirror, se c'è.
 * `null` per un progetto scaricabile o senza progetto.
 */
export function blockedHint(update: AddonUpdate | undefined): BlockedHint | null {
  const project = update?.project;
  if (!update || !project || project.allowModDistribution) return null;
  const mirror = mirrorOf(update);
  if (mirror) {
    return {
      label: "via GitHub",
      title: `${mirror.repo} ${mirror.tag}\nCurseForge blocks third-party downloads: Update installs this GitHub release.`,
    };
  }
  const reason = update.githubMirrorReason?.trim();
  return {
    label: "website only",
    title: reason ? `${WEBSITE_ONLY_TITLE}\nGitHub: ${reason}` : WEBSITE_ONLY_TITLE,
  };
}

/**
 * Un aggiornamento che l'app può installare da sola: un progetto scaricabile, oppure uno
 * bloccato per cui il controllo ha trovato il mirror GitHub (issue #163).
 */
export function isUpdatable(update: AddonUpdate): boolean {
  if (update.status !== "updateAvailable" || !update.project) return false;
  return update.project.allowModDistribution || mirrorOf(update) !== null;
}

/**
 * Perché Reinstall non può servire per questo progetto; `null` se il backend può provarci.
 *
 * Un progetto bloccato si reinstalla solo dal mirror, che il backend cerca al momento della
 * reinstallazione (`update_project` in `updates.rs`): per un progetto aggiornato il controllo
 * non lo calcola, quindi Reinstall resta possibile. Con un aggiornamento disponibile il backend
 * reinstalla quel file, e il controllo ha già detto che il suo mirror non si può usare
 * (`githubMirrorReason`): il backend lo salterebbe, e Reinstall non si offre.
 */
export function reinstallBlockedReason(update: AddonUpdate | undefined): string | null {
  const project = update?.project;
  if (!update || !project || project.allowModDistribution) return null;
  if (update.status !== "updateAvailable" || mirrorOf(update)) return null;
  const reason = update.githubMirrorReason?.trim();
  if (!reason) return null;
  return `CurseForge blocks third-party downloads and the GitHub copy cannot be used: ${reason}`;
}
