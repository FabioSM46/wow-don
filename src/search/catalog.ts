// Ricerca nel catalogo: parametri, pagine, risposte fuori ordine e formattazione. Funzioni
// pure, senza React né I/O.

import type {
  CatalogAddon,
  CatalogCategory,
  CatalogPage,
  CatalogSearch,
  CatalogSort,
  SortOrder,
} from "../api";

/** Pausa di scrittura dopo cui la ricerca parte da sola. */
export const SEARCH_DELAY_MS = 300;

/** Oltre questo indice l'API di CurseForge non restituisce risultati (vedi `catalog.rs`). */
export const MAX_RESULTS = 10_000;

/** Gli ordinamenti, nell'ordine del menu, con la direzione con cui partono. */
export const SORTS: readonly { id: CatalogSort; label: string; order: SortOrder }[] = [
  { id: "popularity", label: "Popularity", order: "desc" },
  { id: "lastUpdated", label: "Last updated", order: "desc" },
  { id: "name", label: "Name", order: "asc" },
  { id: "totalDownloads", label: "Downloads", order: "desc" },
];

export const defaultSearch: CatalogSearch = {
  query: "",
  categoryId: null,
  versionType: null,
  sort: "popularity",
  sortOrder: "desc",
};

/** Cambia ordinamento ripartendo dalla sua direzione naturale (A→Z per il nome, il più alto
 *  prima per gli altri). */
export function withSort<T extends Pick<CatalogSearch, "sort" | "sortOrder">>(
  search: T,
  sort: CatalogSort,
): T {
  const order = SORTS.find((s) => s.id === sort)?.order ?? "desc";
  return { ...search, sort, sortOrder: order };
}

/** Una voce del menu delle categorie; `depth` 1 è una sottocategoria. */
export type CategoryOption = { id: number; name: string; depth: 0 | 1 };

/**
 * Le categorie per il menu: ogni categoria principale seguita dalle sue sottocategorie,
 * nell'ordine (per nome) in cui arrivano dal backend. Una sottocategoria il cui genitore
 * manca resta tra le principali. Il menu ha due livelli: una categoria più profonda (o in un
 * ciclo di genitori) non si perde, compare in fondo tra le principali.
 */
export function categoryOptions(categories: CatalogCategory[]): CategoryOption[] {
  const ids = new Set(categories.map((category) => category.id));
  const isChild = (category: CatalogCategory) =>
    category.parentCategoryId !== null &&
    category.parentCategoryId !== category.id &&
    ids.has(category.parentCategoryId);
  const options: CategoryOption[] = [];
  const emitted = new Set<number>();
  const push = (category: CatalogCategory, depth: 0 | 1) => {
    if (emitted.has(category.id)) return;
    emitted.add(category.id);
    options.push({ id: category.id, name: category.name, depth });
  };
  for (const parent of categories.filter((category) => !isChild(category))) {
    push(parent, 0);
    for (const child of categories) {
      if (isChild(child) && child.parentCategoryId === parent.id) push(child, 1);
    }
  }
  // Rete di sicurezza: ogni categoria compare almeno una volta, anche quelle che i due
  // livelli qui sopra non raggiungono.
  for (const category of categories) push(category, 0);
  return options;
}

/** Il testo della casella come lo riceve il backend: senza spazi ai bordi, così "abc " e
 *  "abc" non fanno due ricerche. */
export function normalizeQuery(text: string): string {
  return text.trim();
}

/** Lo stato dei risultati: le pagine caricate finora, senza duplicati. */
export type Results = {
  addons: CatalogAddon[];
  /** L'indice da chiedere per la pagina successiva. */
  nextIndex: number;
  totalCount: number;
  /** L'ultima pagina era vuota: non c'è altro da chiedere anche se `totalCount` dice di più. */
  exhausted: boolean;
};

/** I risultati della prima pagina di una ricerca nuova. */
export function firstPage(page: CatalogPage): Results {
  return appendPage({ addons: [], nextIndex: 0, totalCount: 0, exhausted: false }, page);
}

/**
 * Accoda una pagina. Tra una pagina e l'altra il catalogo può cambiare ordine: un addon già
 * mostrato che ricompare nella pagina dopo non si ripete.
 */
export function appendPage(results: Results, page: CatalogPage): Results {
  const seen = new Set(results.addons.map((addon) => addon.id));
  const fresh = page.addons.filter((addon) => {
    if (seen.has(addon.id)) return false;
    seen.add(addon.id);
    return true;
  });
  return {
    addons: [...results.addons, ...fresh],
    nextIndex: page.index + page.pageSize,
    totalCount: page.totalCount,
    exhausted: page.addons.length === 0,
  };
}

/** Se "Load more" ha ancora qualcosa da chiedere. */
export function hasMore(results: Results): boolean {
  return (
    !results.exhausted &&
    results.nextIndex < results.totalCount &&
    results.nextIndex < MAX_RESULTS
  );
}

/**
 * Numera le richieste: solo la risposta dell'ultima partita si applica, così una risposta
 * lenta di una ricerca vecchia non copre quella più recente.
 */
export class RequestTracker {
  private latest = 0;

  /** Il numero di una nuova richiesta, che rende superate tutte le precedenti. */
  next(): number {
    this.latest += 1;
    return this.latest;
  }

  isLatest(id: number): boolean {
    return id === this.latest;
  }
}

const compactFormat = new Intl.NumberFormat("en", {
  notation: "compact",
  maximumFractionDigits: 1,
});
const fullFormat = new Intl.NumberFormat("en");
// In UTC, come le date di CurseForge: il giorno mostrato non cambia con il fuso della macchina.
const dateFormat = new Intl.DateTimeFormat("en", { dateStyle: "medium", timeZone: "UTC" });

/** Download in forma breve (`1.2M`); il numero intero va nel suggerimento. */
export function formatDownloads(count: number): string {
  return compactFormat.format(count);
}

export function formatCount(count: number): string {
  return fullFormat.format(count);
}

/** Una data ISO 8601 come "Oct 24, 2026"; una data mancante o non valida mostra "—". */
export function formatDate(iso: string): string {
  if (!iso) return "—";
  const date = new Date(iso);
  return Number.isNaN(date.getTime()) ? "—" : dateFormat.format(date);
}

/** Gli autori in una riga; senza autori "—". */
export function formatAuthors(authors: string[]): string {
  return authors.length > 0 ? authors.join(", ") : "—";
}

/** La frase sopra la tabella: quanti risultati sono mostrati su quanti. */
export function resultCount(results: Results): string {
  const shown = results.addons.length;
  const total = results.totalCount;
  if (total === 0) return "No results";
  if (shown >= total) return `${formatCount(total)} ${total === 1 ? "result" : "results"}`;
  return `${formatCount(shown)} of ${formatCount(total)} results`;
}
