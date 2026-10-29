// The texts of the Options "System" section (issue #160), kept apart from the component for tests.

/** The explanation of a disabled "Close to tray": without an icon the X closes the app. */
export const TRAY_UNAVAILABLE = "The system tray isn't available on this desktop.";

/**
 * The hint under "Close to tray". `trayAvailable` is `null` until the settings have been
 * read: the normal text applies, the checkbox is disabled anyway.
 */
export function closeToTrayHint(trayAvailable: boolean | null): string {
  if (trayAvailable === false) return TRAY_UNAVAILABLE;
  return "Closing the window keeps WoW-don running in the tray. Quit from the tray icon's menu.";
}

/**
 * The hint under "Start with the system": it says what happens at login with the other
 * current settings. The backend starts hidden only with "Close to tray" on and the tray
 * available; otherwise it opens the window.
 */
export function autostartHint(closeToTray: boolean | null, trayAvailable: boolean | null): string {
  if (closeToTray === true && trayAvailable === true) {
    return "With Close to tray on, WoW-don starts hidden in the tray.";
  }
  if (trayAvailable === false) {
    return "WoW-don opens its window when you log in: the system tray isn't available.";
  }
  return "WoW-don opens its window when you log in. With Close to tray on, it starts hidden in the tray.";
}
