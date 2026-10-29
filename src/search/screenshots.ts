// Tabs and gallery of an addon's detail: labels, alternative text and navigation between
// images. Pure functions, no React or I/O.

import type { Screenshot } from "../api";

/** The label of the Gallery tab with the number of images, like "Gallery (3)". */
export function galleryLabel(count: number): string {
  return `Gallery (${count})`;
}

/** An image's title; without a title "Screenshot N", counting from 1. */
export function screenshotTitle(shot: Pick<Screenshot, "title">, index: number): string {
  return shot.title.trim() || `Screenshot ${index + 1}`;
}

/** The image after `index`, starting over from the first after the last. */
export function nextIndex(index: number, count: number): number {
  return count > 0 ? (index + 1) % count : 0;
}

/** The image before `index`, going to the last before the first. */
export function previousIndex(index: number, count: number): number {
  return count > 0 ? (index - 1 + count) % count : 0;
}

/** The position of an image, like "2 / 5". */
export function positionLabel(index: number, count: number): string {
  return `${index + 1} / ${count}`;
}

/**
 * The tab to activate for a key in the tab bar (WAI-ARIA pattern, like the app's tabs):
 * arrows that wrap around, Home and End at the ends; any other key moves nothing (`null`).
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
