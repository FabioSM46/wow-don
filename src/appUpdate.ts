// The notice of a newer WoW-don release in the footer (D46, issue #260): whether it shows and what
// it says. Pure functions, no React or I/O. The check itself lives in the backend.

import type { AppUpdateStatus } from "./api";

/** The name of the Tauri event, like `app_update::EVENT` in the backend. */
export const APP_UPDATE_EVENT = "app-update";

export type AppUpdateNotice = { text: string; url: string };

/**
 * The notice, or `null` for a footer with the version alone. `enabled` is the saved
 * `checkAppUpdates` (`null` until it is read): turned off, the backend forgets the release only
 * after a check in flight ends, while the notice must go at once. The text names the app because
 * the same window talks about addon updates all the time (D41).
 */
export function appUpdateNotice(
  status: AppUpdateStatus | null,
  enabled: boolean | null,
): AppUpdateNotice | null {
  if (enabled !== true || !status?.updateAvailable) return null;
  if (status.latest === null || status.url === null) return null;
  return { text: `WoW-don v${status.latest} available`, url: status.url };
}
