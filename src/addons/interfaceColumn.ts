// La colonna Interface della tab Installed (issue #230): una sola versione e il resto come
// `+N`, così un addon che dichiara molte versioni non manda a capo la riga. Funzioni pure,
// senza React né I/O.

/** Quello che la cella mostra: la versione scelta, quante altre ce ne sono e l'elenco intero. */
export type InterfaceSummary = { shown: number; more: number; title: string };

/**
 * Se un numero di `## Interface` appartiene al gioco del flavor dell'installazione. La stessa
 * tabella di `fits` in `src-tauri/src/sources/tukui.rs`; si accetta anche il nome della
 * cartella (`_classic_era_`) e le maiuscole non contano. Un flavor sconosciuto non ha versioni.
 */
export function fitsFlavor(flavor: string, version: number): boolean {
  switch (flavor.replace(/^_+|_+$/g, "").toLowerCase()) {
    case "retail":
      return version >= 100_000;
    case "classic_era":
      return version >= 11_000 && version < 11_600;
    case "classic_beta":
    case "forever":
      return version >= 16_000 && version < 17_000;
    case "anniversary":
      return version >= 20_000 && version < 30_000;
    case "classic":
      return version >= 30_000 && version < 100_000;
    default:
      return false;
  }
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
