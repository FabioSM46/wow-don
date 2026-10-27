// "Add by link" nella tab Search (issue #110): la validazione minima del testo incollato e il
// testo dell'esito di `install_from_link`. Il link vero lo riconosce il backend (`resolve_link`):
// qui si scarta solo quello che non può essere un link. Funzioni pure, senza React né I/O.

import type { LinkPreview, UpdateOutcome } from "../api";

/** Il link da passare a `resolve_link`, o il motivo per cui il testo non può esserlo. */
export type LinkInput = { ok: true; url: string } | { ok: false; message: string };

/**
 * Il testo incollato, senza spazi ai lati. Vuoto, con spazi in mezzo o senza un punto (nessun
 * host può mancarne) non è un link: si risponde senza chiamare il backend. Tutto il resto lo
 * giudica `resolve_link`, che sa quali fonti e quali pagine riconosce.
 */
export function checkLink(text: string): LinkInput {
  const url = text.trim();
  if (url === "") return { ok: false, message: "Paste the link of an addon." };
  if (/\s/.test(url) || !url.includes(".")) {
    return { ok: false, message: "This is not a valid link." };
  }
  return { ok: true, url };
}

/** Quando l'anteprima non si può installare e il backend non dice perché. */
export const NOT_DOWNLOADABLE =
  "WoW-don cannot download this addon for this game version: download the zip from its page, then drop it into this window.";

/** La spiegazione sotto l'anteprima: il motivo di un addon non scaricabile, o un avviso. */
export function previewNote(preview: Pick<LinkPreview, "downloadable" | "message">): string | null {
  const message = preview.message?.trim() ?? "";
  if (!preview.downloadable) return message || NOT_DOWNLOADABLE;
  return message || null;
}

/** L'esito di un'installazione da link: `error` si mostra come errore, il resto in tono neutro. */
export type LinkResult = { text: string; error: boolean; installed: boolean };

/**
 * La frase sull'esito di `install_from_link`, che ha la forma di `UpdateOutcome`: `updated`
 * vuol dire installato. Il nome dell'esito può essere vuoto, e allora vale quello dell'anteprima.
 */
export function linkOutcome(outcome: UpdateOutcome, preview: Pick<LinkPreview, "name">): LinkResult {
  const name = outcome.name.trim() || preview.name.trim() || outcome.key;
  const message = outcome.message?.trim();
  switch (outcome.status) {
    case "updated":
      // Su un'installazione riuscita il messaggio è un avviso: segue la frase.
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
