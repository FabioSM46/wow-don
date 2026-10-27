// Tab e gallery del dettaglio di un addon: etichette, testo alternativo e navigazione tra le
// immagini. Funzioni pure, senza React né I/O.

import type { Screenshot } from "../api";

/** L'etichetta della tab Gallery con il numero di immagini, come "Gallery (3)". */
export function galleryLabel(count: number): string {
  return `Gallery (${count})`;
}

/** Il titolo di un'immagine; senza titolo "Screenshot N", contando da 1. */
export function screenshotTitle(shot: Pick<Screenshot, "title">, index: number): string {
  return shot.title.trim() || `Screenshot ${index + 1}`;
}

/** L'immagine dopo `index`, ricominciando dalla prima dopo l'ultima. */
export function nextIndex(index: number, count: number): number {
  return count > 0 ? (index + 1) % count : 0;
}

/** L'immagine prima di `index`, passando all'ultima prima della prima. */
export function previousIndex(index: number, count: number): number {
  return count > 0 ? (index - 1 + count) % count : 0;
}

/** La posizione di un'immagine, come "2 / 5". */
export function positionLabel(index: number, count: number): string {
  return `${index + 1} / ${count}`;
}

/**
 * La tab da attivare per un tasto nella barra di tab (pattern WAI-ARIA, come le tab dell'app):
 * frecce con giro, Home e Fine agli estremi; un altro tasto non sposta niente (`null`).
 */
export function tabIndexForKey(key: string, index: number, count: number): number | null {
  switch (key) {
    case "ArrowRight":
      return nextIndex(index, count);
    case "ArrowLeft":
      return previousIndex(index, count);
    case "Home":
      return 0;
    case "End":
      return Math.max(count - 1, 0);
    default:
      return null;
  }
}
