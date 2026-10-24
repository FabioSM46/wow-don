// Installazioni trovate sul computer (issue #89): quali righe mostrare nella sezione "Found on
// this computer" di Options. Funzioni pure, senza React né I/O.

import type { DiscoveredInstallation, Installation } from "../api";

/**
 * Le installazioni trovate ancora da proporre: senza quelle già nella lista delle installazioni
 * (aggiunte da qui o con "Add installation" dopo la ricerca) e senza quelle in `added`, le righe
 * aggiunte da questa sezione. Il backend esclude già quelle note al momento della ricerca; il
 * filtro qui copre quelle aggiunte dopo. I percorsi arrivano entrambi dal backend, dalla stessa
 * validazione, quindi si confrontano così come sono.
 */
export function pendingDiscoveries(
  found: readonly DiscoveredInstallation[],
  installations: readonly Pick<Installation, "path">[],
  added: ReadonlySet<string> = new Set(),
): DiscoveredInstallation[] {
  const known = new Set(installations.map((installation) => installation.path));
  return found.filter((item) => !known.has(item.path) && !added.has(item.path));
}

/** Il nome della cartella del gioco per un flavor, per esempio `_classic_era_`. */
export function flavorFolder(flavor: string): string {
  return `_${flavor}_`;
}
