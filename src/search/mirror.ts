// Il mirror GitHub dei progetti CurseForge bloccati nella tab Search (D34, issue #165): lo stato
// del controllo (`github_mirror`), quello che Install fa e i testi della riga e del dettaglio.
// Funzioni pure, senza React né I/O.

import type { CatalogAddon, MirrorAvailability } from "../api";

/**
 * Il controllo del mirror di un progetto in un'installazione. Un esito `done` vale per la
 * sessione (un secondo clic non rifà la richiesta); un errore no: il clic successivo riprova.
 */
export type MirrorCheck =
  | { status: "checking" }
  | { status: "done"; result: MirrorAvailability }
  | { status: "failed"; message: string };

/** CurseForge non lascia scaricare il progetto: Install passa dal controllo del mirror. */
export function isBlocked(addon: Pick<CatalogAddon, "allowModDistribution">): boolean {
  return !addon.allowModDistribution;
}

/**
 * Cosa fa Install con l'esito del controllo:
 * - `install`: `install_addon` (dal mirror, o da CurseForge se il progetto non è più bloccato);
 * - `website`: niente da installare, si mostra il motivo e il link alla pagina;
 * - `check`: manca l'esito (mai chiesto, o fallito): prima il controllo;
 * - `wait`: il controllo è in corso.
 */
export type MirrorStep =
  | { kind: "install" }
  | { kind: "website"; reason: string }
  | { kind: "check" }
  | { kind: "wait" };

export function mirrorStep(check: MirrorCheck | undefined): MirrorStep {
  if (!check || check.status === "failed") return { kind: "check" };
  if (check.status === "checking") return { kind: "wait" };
  const { result } = check;
  // `reason` `null` senza mirror: CurseForge ora lascia scaricare il progetto.
  if (result.available || result.reason === null) return { kind: "install" };
  return { kind: "website", reason: result.reason };
}

/** `owner/repo tag` del mirror disponibile, oppure `null`. */
export function mirrorName(check: MirrorCheck | undefined): string | null {
  if (check?.status !== "done" || !check.result.available) return null;
  const { repo, tag } = check.result;
  return [repo, tag].filter(Boolean).join(" ") || null;
}

/** L'etichetta accanto al nome di un progetto bloccato, con il suo suggerimento. */
export type BlockedHint = { label: string; title: string };

const BLOCKED = "The author does not allow downloads from other apps";

/**
 * L'etichetta della riga e del dettaglio: "via GitHub" con il mirror, "website only" con il
 * motivo, e prima del controllo un'etichetta che dice che Install guarderà su GitHub.
 */
export function blockedHint(check: MirrorCheck | undefined): BlockedHint {
  const name = mirrorName(check);
  if (name) {
    return {
      label: "via GitHub",
      title: `${name}\nCurseForge blocks third-party downloads: Install uses this GitHub release.`,
    };
  }
  const step = mirrorStep(check);
  if (step.kind === "website") {
    return { label: "website only", title: unavailableText(step.reason) };
  }
  return {
    label: "blocked on CurseForge",
    title: `${BLOCKED}: Install looks for the same release on GitHub first.`,
  };
}

/** Perché il progetto si installa solo dal sito: il motivo del mirror dopo la regola D13. */
export function unavailableText(reason: string): string {
  return `${BLOCKED} and the GitHub copy cannot be used: ${reason}`;
}

/** La frase del dettaglio sotto il nome, per un progetto bloccato non installato. */
export function detailNote(check: MirrorCheck | undefined): string {
  const name = mirrorName(check);
  if (name) return `${BLOCKED}: Install uses the same release from GitHub (${name}).`;
  const step = mirrorStep(check);
  switch (step.kind) {
    case "website":
      return `${unavailableText(step.reason)} Get it from the CurseForge website.`;
    case "wait":
      return `${BLOCKED}: checking GitHub for the same release…`;
    case "install":
      return "CurseForge now allows downloads of this addon.";
    case "check":
      return check?.status === "failed"
        ? `${BLOCKED}, and GitHub could not be checked: ${check.message}`
        : `${BLOCKED}: Install looks for the same release on GitHub first.`;
  }
}
