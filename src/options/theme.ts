// Interface theme (issue #92): the choice the backend saves in `settings.json` becomes the
// `data-theme` attribute on `<html>`, which `App.css` reads. Pure functions, no React or I/O.

import type { Theme } from "../api";

/** The choices of the control in Options, in the order they appear. */
export const THEMES: readonly { id: Theme; label: string }[] = [
  { id: "system", label: "System" },
  { id: "light", label: "Light" },
  { id: "dark", label: "Dark" },
];

export function isTheme(value: string): value is Theme {
  return THEMES.some((theme) => theme.id === value);
}

/** The value of `data-theme`: no attribute for `system`, which follows the operating system. */
export function themeAttribute(theme: Theme): "light" | "dark" | null {
  return theme === "system" ? null : theme;
}

/** The part of `<html>` needed to apply the theme; in tests a fake object is enough. */
type ThemeRoot = Pick<Element, "setAttribute" | "removeAttribute">;

export function applyTheme(root: ThemeRoot, theme: Theme): void {
  const attribute = themeAttribute(theme);
  if (attribute === null) root.removeAttribute("data-theme");
  else root.setAttribute("data-theme", attribute);
}
