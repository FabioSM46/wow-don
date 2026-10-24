// Dettaglio di un addon del catalogo: paragrafi della descrizione, file e dipendenze come si
// mostrano. Funzioni pure, senza React né I/O.

import type { CatalogCategory, Dependency, DetailFile, VersionType } from "../api";
// Con l'estensione: `node --test` risolve gli import a runtime senza il bundler.
import { formatDate } from "./catalog.ts";

/**
 * I paragrafi della descrizione: il backend separa i paragrafi con una riga vuota e le righe
 * di un elenco con un a capo, che resta dentro il paragrafo. Una descrizione vuota (o di soli
 * spazi) non ha paragrafi.
 */
export function paragraphs(description: string): string[] {
  return description
    .split(/\n[ \t]*\n/)
    .map((paragraph) => paragraph.trim())
    .filter((paragraph) => paragraph !== "");
}

/** Il tipo di rilascio di un file come etichetta; un valore sconosciuto non ne ha. */
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

/** Tipo di rilascio e data di un file in una riga, come "Beta · Oct 24, 2026". */
export function fileMeta(file: Pick<DetailFile, "releaseType" | "date">): string {
  return [releaseLabel(file), formatDate(file.date)].filter((part) => part !== "").join(" · ");
}

/** Il nome di un file; senza nome resta l'id, così la riga non è vuota. */
export function fileName(file: Pick<DetailFile, "id" | "name">): string {
  return file.name.trim() || `File #${file.id}`;
}

/** Il nome di una dipendenza; CurseForge può non restituire il progetto, e resta l'id. */
export function dependencyName(dependency: Dependency): string {
  return dependency.name.trim() || `Project #${dependency.id}`;
}

/** Le categorie in una riga, nell'ordine del backend; senza categorie "—". */
export function formatCategories(categories: CatalogCategory[]): string {
  const names = categories.map((category) => category.name).filter((name) => name !== "");
  return names.length > 0 ? names.join(", ") : "—";
}

/** Il nome della versione del gioco del dettaglio; finché l'elenco non è noto, nessuno. */
export function versionName(versionTypes: VersionType[], id: number): string | null {
  return versionTypes.find((type) => type.id === id)?.name ?? null;
}
