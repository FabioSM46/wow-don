// I testi della sezione "System" di Options (issue #160), separati dal componente per i test.

/** La spiegazione di "Close to tray" disabilitata: senza icona la X chiude l'app. */
export const TRAY_UNAVAILABLE = "The system tray isn't available on this desktop.";

/**
 * Il suggerimento sotto "Close to tray". `trayAvailable` è `null` finché le impostazioni non
 * sono lette: vale il testo normale, la casella è comunque disabilitata.
 */
export function closeToTrayHint(trayAvailable: boolean | null): string {
  if (trayAvailable === false) return TRAY_UNAVAILABLE;
  return "Closing the window keeps WoW-don running in the tray. Quit from the tray icon's menu.";
}

/**
 * Il suggerimento sotto "Start with the system": dice cosa succede all'accesso con le altre
 * impostazioni di adesso. Il backend parte nascosto solo con "Close to tray" attiva e la tray
 * disponibile; altrimenti apre la finestra.
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
