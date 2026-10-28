// Il mirror GitHub dei progetti CurseForge bloccati nella tab Search (D34, issue #165): lo stato
// del controllo (`github_mirror`), quello che Install fa e i testi della riga e del dettaglio.
// Dalla issue #223 il controllo parte da solo per i bloccati visibili con un repository GitHub,
// e un bloccato senza repository va al sito senza nessuna richiesta.
// Funzioni pure, senza React né I/O.

import type { CatalogAddon, MirrorAvailability } from "../api";

/**
 * Il controllo del mirror di un progetto in un'installazione. Un esito `done` vale per la
 * sessione (un secondo clic non rifà la richiesta); un errore no: il clic successivo riprova.
 */
export type MirrorCheck =
  // `forInstall`: chiesto da Install, che aspetta l'esito; senza, è il controllo automatico.
  | { status: "checking"; forInstall?: boolean }
  | { status: "done"; result: MirrorAvailability }
  | { status: "failed"; message: string };

/** CurseForge non lascia scaricare il progetto: Install passa dal controllo del mirror. */
export function isBlocked(addon: Pick<CatalogAddon, "allowModDistribution">): boolean {
  return !addon.allowModDistribution;
}

/** Il motivo, uguale a quello di `github_mirror`, di un bloccato senza repository GitHub. */
export const NO_REPOSITORY = "The CurseForge project does not link a GitHub repository.";

/**
 * L'esito che vale per `addon`: un progetto bloccato senza `githubRepo` non ha un mirror
 * possibile, quindi è già "website only" senza chiedere niente; per gli altri il controllo
 * della sessione (o `undefined`).
 */
export function mirrorCheckFor(
  addon: Pick<CatalogAddon, "allowModDistribution" | "githubRepo">,
  check: MirrorCheck | undefined,
): MirrorCheck | undefined {
  if (!isBlocked(addon) || addon.githubRepo !== null) return check;
  return {
    status: "done",
    result: { available: false, repo: null, tag: null, reason: NO_REPOSITORY },
  };
}

/** Quanti controlli automatici del mirror alla volta: ognuno legge anche il progetto da
 *  CurseForge. */
export const MIRROR_CHECKS_AT_ONCE = 4;

/**
 * I progetti visibili da controllare da soli, nell'ordine della lista: bloccati, con un
 * repository GitHub, non installati e senza un esito o un controllo già in corso (`known`). Un
 * controllo fallito resta noto: riprova solo il clic su Install.
 */
export function mirrorsToCheck<
  T extends Pick<CatalogAddon, "allowModDistribution" | "githubRepo" | "installed">,
>(addons: readonly T[], known: (addon: T) => boolean, limit: number): T[] {
  const pending: T[] = [];
  for (const addon of addons) {
    if (pending.length >= limit) break;
    if (!isBlocked(addon) || addon.githubRepo === null || addon.installed) continue;
    if (!known(addon)) pending.push(addon);
  }
  return pending;
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
 * motivo, "checking GitHub…" durante il controllo, e prima del controllo (o dopo un errore)
 * un'etichetta che dice che Install guarderà su GitHub.
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
  if (step.kind === "wait") {
    return {
      label: "checking GitHub…",
      title: `${BLOCKED}: checking GitHub for the same release…`,
    };
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
