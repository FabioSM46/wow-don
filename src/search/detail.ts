// Detail of a catalog addon: description paragraphs, files and dependencies as they are
// shown. Pure functions, no React or I/O.

import type { CatalogCategory, Dependency, DetailFile, VersionType } from "../api";
// With the extension: `node --test` resolves imports at runtime without the bundler.
import { formatDate } from "./catalog.ts";

/**
 * The description paragraphs: the backend separates paragraphs with an empty line and the lines
 * of a list with a line break (`\r\n` too), which stays inside the paragraph. An empty (or
 * whitespace-only) description has no paragraphs.
 */
export function paragraphs(description: string): string[] {
  return description
    .split(/\r?\n[ \t]*\r?\n/)
    .map((paragraph) => paragraph.trim())
    .filter((paragraph) => paragraph !== "");
}

/** A file's release type as a label; an unknown value has none. */
export function releaseLabel(file: Pick<DetailFile, "releaseType">): string {
  switch (file.releaseType) {
    case 1:
      return "Release";
    case 2:
      return "Beta";
    default:
      return "";
  }
}

/** A file's release type and date on one line, like "Beta · Oct 24, 2026". */
export function fileMeta(file: Pick<DetailFile, "releaseType" | "date">): string {
  return [releaseLabel(file), formatDate(file.date)].filter((part) => part !== "").join(" · ");
}

/** A file's name; without a name the id remains, so the line is not empty. */
export function fileName(file: Pick<DetailFile, "id" | "name">): string {
  return file.name.trim() || `File #${file.id}`;
}

/** A dependency's name; CurseForge may not return the project, and the id remains. */
export function dependencyName(dependency: Dependency): string {
  return dependency.name.trim() || `Project #${dependency.id}`;
}

/** The categories on one line, in the backend's order; without categories "—". */
export function formatCategories(categories: CatalogCategory[]): string {
  const names = categories.map((category) => category.name).filter((name) => name !== "");
  return names.length > 0 ? names.join(", ") : "—";
}

/** The name of the detail's game version; until the list is known, none. */
export function versionName(versionTypes: VersionType[], id: number): string | null {
  return versionTypes.find((type) => type.id === id)?.name ?? null;
}
