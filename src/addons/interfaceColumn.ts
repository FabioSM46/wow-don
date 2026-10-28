// La colonna Interface della tab Installed (issue #230): una sola versione e il resto come
// `+N`, così un addon che dichiara molte versioni non manda a capo la riga. Funzioni pure,
// senza React né I/O.

/** Quello che la cella mostra: la versione scelta, quante altre ce ne sono e l'elenco intero. */
export type InterfaceSummary = { shown: number; more: number; title: string };

/**
 * Gli intervalli di `## Interface` di ogni flavor, `[da, fino a escluso)`. È la stessa tabella
 * di `fits` in `src-tauri/src/sources/tukui.rs`: `interfaceColumn.test.ts` legge quel file e
 * fallisce se le due tabelle divergono.
 */
export const FLAVOR_RANGES: Readonly<Record<string, readonly [number, number]>> = {
  retail: [100_000, Number.POSITIVE_INFINITY],
  classic_era: [11_000, 11_600],
  classic_beta: [16_000, 17_000],
  forever: [16_000, 17_000],
  anniversary: [20_000, 30_000],
  classic: [30_000, 100_000],
};

/**
 * Se un numero di `## Interface` appartiene al gioco del flavor dell'installazione. Si accetta
 * anche il nome della cartella (`_classic_era_`) e le maiuscole non contano. Un flavor
 * sconosciuto non ha versioni.
 */
export function fitsFlavor(flavor: string, version: number): boolean {
  const key = flavor.replace(/^_+|_+$/g, "").toLowerCase();
  const range = Object.prototype.hasOwnProperty.call(FLAVOR_RANGES, key)
    ? FLAVOR_RANGES[key]
    : undefined;
  return range !== undefined && version >= range[0] && version < range[1];
}

/**
 * La versione da mostrare è la prima che appartiene al flavor dell'installazione, altrimenti
 * la prima dichiarata; le altre diventano `+N`, e `title` le elenca tutte nell'ordine del
 * `.toc`. Senza versioni non c'è niente da mostrare (`null`).
 */
export function interfaceSummary(versions: number[], flavor: string): InterfaceSummary | null {
  if (versions.length === 0) return null;
  const shown = versions.find((version) => fitsFlavor(flavor, version)) ?? versions[0];
  return { shown, more: versions.length - 1, title: versions.join(", ") };
}
