// Tema dell'interfaccia (issue #92): la scelta salvata dal backend in `settings.json` diventa
// l'attributo `data-theme` su `<html>`, che `App.css` legge. Funzioni pure, senza React né I/O.

import type { Theme } from "../api";

/** Le scelte del controllo in Options, nell'ordine in cui compaiono. */
export const THEMES: readonly { id: Theme; label: string }[] = [
  { id: "system", label: "System" },
  { id: "light", label: "Light" },
  { id: "dark", label: "Dark" },
];

export function isTheme(value: string): value is Theme {
  return THEMES.some((theme) => theme.id === value);
}

/** Il valore di `data-theme`: nessun attributo per `system`, che segue il sistema operativo. */
export function themeAttribute(theme: Theme): "light" | "dark" | null {
  return theme === "system" ? null : theme;
}

/** La parte di `<html>` che serve ad applicare il tema; nei test basta un oggetto finto. */
type ThemeRoot = Pick<Element, "setAttribute" | "removeAttribute">;

export function applyTheme(root: ThemeRoot, theme: Theme): void {
  const attribute = themeAttribute(theme);
  if (attribute === null) root.removeAttribute("data-theme");
  else root.setAttribute("data-theme", attribute);
}
