// I testi e gli stati della sezione "Automatic updates" di Options (D41, issue #204), separati
// dal componente per i test. Funzioni pure, senza React né I/O.

import type { AutoUpdateMode } from "../api";

/** Le modalità, nell'ordine in cui compaiono nel controllo. */
export const AUTO_UPDATE_MODES: readonly { id: AutoUpdateMode; label: string }[] = [
  { id: "off", label: "Off" },
  { id: "notify", label: "Notify only" },
  { id: "install", label: "Install" },
];

/** Gli intervalli che il backend accetta, in ore (`AUTO_UPDATE_INTERVALS_HOURS`). */
export const AUTO_UPDATE_INTERVALS: readonly number[] = [1, 3, 6, 12, 24];

export function isAutoUpdateMode(value: string): value is AutoUpdateMode {
  return AUTO_UPDATE_MODES.some((mode) => mode.id === value);
}

/** Il testo di un intervallo nel controllo: "Every hour", "Every 6 hours". */
export function intervalLabel(hours: number): string {
  return hours === 1 ? "Every hour" : `Every ${hours} hours`;
}

/** L'intervallo non conta con l'aggiornamento automatico spento, e il controllo è disattivato. */
export function intervalDisabled(mode: AutoUpdateMode | null): boolean {
  return mode === null || mode === "off";
}

/** Il suggerimento sotto la modalità: dice cosa fa la scelta di adesso. */
export function modeHint(mode: AutoUpdateMode | null): string {
  switch (mode) {
    case "off":
      return "WoW-don checks for updates only when you ask.";
    case "install":
      return "WoW-don checks for updates in the background and installs them.";
    default:
      return "WoW-don checks for updates in the background and only tells you what it found.";
  }
}

/**
 * Il suggerimento sotto l'interruttore di un'installazione. Con la modalità "Off" nessuna
 * installazione si aggiorna da sola, qualunque sia l'interruttore.
 */
export function installationHint(mode: AutoUpdateMode | null): string {
  return mode === "off"
    ? "Automatic updates are off for every installation."
    : "To skip a single addon, right-click it in Installed and choose Don't auto-update.";
}

/**
 * Il valore mostrato da un controllo: quello in salvataggio finché il backend non risponde,
 * poi quello salvato. Un salvataggio fallito toglie `pending` e il controllo torna al salvato.
 */
export function shownValue<T>(saved: T, pending: T | undefined): T {
  return pending === undefined ? saved : pending;
}
