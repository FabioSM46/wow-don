// The texts and states of the Options "Automatic addon updates" section (D41, issue #204), kept
// apart from the component for tests. Pure functions, no React or I/O.

import type { AutoUpdateMode } from "../api";

/** The modes, in the order they appear in the control. */
export const AUTO_UPDATE_MODES: readonly { id: AutoUpdateMode; label: string }[] = [
  { id: "off", label: "Off" },
  { id: "notify", label: "Notify only" },
  { id: "install", label: "Install" },
];

/** The intervals the backend accepts, in hours (`AUTO_UPDATE_INTERVALS_HOURS`). */
export const AUTO_UPDATE_INTERVALS: readonly number[] = [1, 3, 6, 12, 24];

export function isAutoUpdateMode(value: string): value is AutoUpdateMode {
  return AUTO_UPDATE_MODES.some((mode) => mode.id === value);
}

/** The text of an interval in the control: "Every hour", "Every 6 hours". */
export function intervalLabel(hours: number): string {
  return hours === 1 ? "Every hour" : `Every ${hours} hours`;
}

/** The interval does not matter with automatic updates off, and the control is disabled. */
export function intervalDisabled(mode: AutoUpdateMode | null): boolean {
  return mode === null || mode === "off";
}

/** The hint under the mode: it says what the current choice does. */
export function modeHint(mode: AutoUpdateMode | null): string {
  switch (mode) {
    case "off":
      return "WoW-don checks for addon updates only when you ask.";
    case "install":
      return "WoW-don checks for addon updates in the background and installs them.";
    default:
      return "WoW-don checks for addon updates in the background and only tells you what it found.";
  }
}

/**
 * The hint under an installation's switch. With the "Off" mode no installation updates
 * on its own, whatever the switch.
 */
export function installationHint(mode: AutoUpdateMode | null): string {
  return mode === "off"
    ? "Automatic addon updates are off for every installation."
    : "To keep a single addon as it is, right-click it in Installed and choose Don't update.";
}

/**
 * The value a control shows: the one being saved until the backend answers, then the saved
 * one. A failed save removes `pending` and the control goes back to the saved value.
 */
export function shownValue<T>(saved: T, pending: T | undefined): T {
  return pending === undefined ? saved : pending;
}

/** The label of the switch for system notifications after automatic rounds (issue #299). */
export const NOTIFICATIONS_LABEL = "System notifications";

/**
 * The description under the notifications switch. With the "Off" mode there are no automatic
 * checks, so nothing to notify: the description says why the switch is disabled.
 */
export function notificationsHint(mode: AutoUpdateMode | null): string {
  return mode === "off"
    ? "No automatic checks run while automatic addon updates are off."
    : "Notifies you when an automatic check finds or installs updates.";
}

/**
 * The notifications switch is disabled while the settings load and with the "Off" mode; it
 * keeps its saved value either way.
 */
export function notificationsDisabled(mode: AutoUpdateMode | null): boolean {
  return mode === null || mode === "off";
}
