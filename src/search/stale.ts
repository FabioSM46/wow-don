// La tab Search dopo un cambio degli addon fatto altrove (rimozione, reinstallazione, zip,
// importazione): quali installazioni hanno risultati da rileggere e quale stato della sessione
// non vale più. Funzioni pure, senza React né I/O.

/** Installazioni e progetti installati valgono per installazione: la chiave le tiene insieme. */
export function installKey(installationId: string, modId: number): string {
  return `${installationId}:${modId}`;
}

/** L'installazione della chiave `installKey`: tutto quello che precede l'ultimo `:`. */
function installationOf(key: string): string {
  return key.slice(0, key.lastIndexOf(":"));
}

/** `stale` con l'installazione `id`; lo stesso insieme se c'era già. */
export function markStale(stale: ReadonlySet<string>, id: string): ReadonlySet<string> {
  return stale.has(id) ? stale : new Set(stale).add(id);
}

/** `stale` senza le installazioni `done`; lo stesso insieme se non ne toglie nessuna. */
export function clearStale(
  stale: ReadonlySet<string>,
  done: ReadonlySet<string>,
): ReadonlySet<string> {
  if (![...done].some((id) => stale.has(id))) return stale;
  return new Set([...stale].filter((id) => !done.has(id)));
}

/**
 * I progetti installati nella sessione, senza quelli delle installazioni `stale`: per quelle
 * il segno "installato" torna a darlo la ricerca riletta. Lo stesso insieme se non cambia.
 */
export function forgetInstalled(
  keys: Set<string>,
  stale: ReadonlySet<string>,
): Set<string> {
  const next = new Set([...keys].filter((key) => !stale.has(installationOf(key))));
  return next.size === keys.size ? keys : next;
}

/**
 * Le installazioni avviate nella sessione, senza quelle finite nelle installazioni `stale`: un
 * esito di prima non descrive più gli addon sul disco. Quelle in corso restano. La stessa mappa
 * se non cambia.
 */
export function forgetFinishedRuns<R extends { status: string }>(
  runs: Map<string, R>,
  stale: ReadonlySet<string>,
): Map<string, R> {
  const next = new Map(
    [...runs].filter(([key, run]) => run.status === "running" || !stale.has(installationOf(key))),
  );
  return next.size === runs.size ? runs : next;
}
