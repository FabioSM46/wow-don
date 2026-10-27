// Il risultato di `refresh_addons` (issue #126): la lista arriva sempre, gli stati solo se il
// controllo è riuscito. Funzioni pure, senza React né I/O.

import type { AddonUpdate, Refresh } from "../api";

/** Gli stati di aggiornamento di un `refresh_addons`, oppure il motivo per cui mancano. */
export type RefreshCheck = { error: null; updates: AddonUpdate[] } | { error: string };

export function refreshCheck(result: Refresh): RefreshCheck {
  // Un messaggio vuoto non è un motivo da mostrare: vale come errore generico.
  if (result.checkError !== null) {
    return { error: result.checkError.trim() || "The update check failed." };
  }
  return { error: null, updates: result.updates };
}
