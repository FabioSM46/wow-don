// Installazione dalla tab Search: il riepilogo degli esiti di `install_addon` e lo stato
// "installato" che le righe e il dettaglio mostrano dopo un'installazione. Funzioni pure,
// senza React né I/O.

import type { CatalogAddon, InstallOutcome } from "../api";

/** Una riga del riepilogo: `error` si mostra come errore, il resto in tono neutro. */
export type SummaryLine = { modId: number; text: string; error: boolean };

/** Il riepilogo di un'installazione: una frase sul progetto scelto e una riga per dipendenza. */
export type InstallSummary = { headline: string; error: boolean; lines: SummaryLine[] };

/** Il nome di un esito: il backend può lasciarlo vuoto, e allora resta l'id. */
function outcomeName(outcome: InstallOutcome, fallback?: string): string {
  return outcome.name.trim() || fallback?.trim() || `Project #${outcome.modId}`;
}

/**
 * Il riepilogo degli esiti di `install_addon` per il progetto `requested` (id e nome che gli dà
 * la ricerca, per quando l'esito non ne ha uno): il backend mette prima il suo esito e poi
 * quelli delle dipendenze, in ampiezza e con `dependencyOf`. L'esito del progetto si cerca per
 * id, così la frase non descrive mai una dipendenza.
 */
export function summarizeInstall(
  outcomes: InstallOutcome[],
  requested: { id: number; name?: string },
): InstallSummary {
  const main =
    outcomes.find((o) => o.modId === requested.id && o.dependencyOf === undefined) ??
    outcomes.find((o) => o.dependencyOf === undefined);
  const dependencies = outcomes.filter((o) => o !== main);

  const names = new Map<number, string>();
  for (const outcome of dependencies) names.set(outcome.modId, outcomeName(outcome));
  if (main) {
    const fallbackName = main.modId === requested.id ? requested.name : undefined;
    names.set(main.modId, outcomeName(main, fallbackName));
  }
  const nameOf = (modId: number) => names.get(modId) ?? `Project #${modId}`;

  const lines = dependencies.map((outcome): SummaryLine => {
    const parent = outcome.dependencyOf;
    const name = nameOf(outcome.modId);
    const label = parent === undefined ? name : `${name} (required by ${nameOf(parent)})`;
    const state =
      outcome.status === "installed"
        ? "installed"
        : outcome.status === "skipped"
          ? "skipped"
          : "error";
    return {
      modId: outcome.modId,
      text: `${label}: ${state}${outcome.message ? ` — ${outcome.message}` : ""}`,
      error: outcome.status === "error",
    };
  });

  // Senza l'esito del progetto (una risposta che il backend non dà) restano le righe.
  if (!main) return { headline: "Nothing was installed.", error: false, lines };

  const mainName = nameOf(main.modId);
  const reason = main.message ? `: ${main.message}` : ".";
  let headline: string;
  switch (main.status) {
    case "installed": {
      const installed = dependencies.filter((o) => o.status === "installed").length;
      const withDeps =
        installed > 0 ? ` with ${installed} ${installed === 1 ? "dependency" : "dependencies"}` : "";
      // Su un'installazione riuscita il messaggio è un avviso: segue la frase.
      headline = `${mainName} installed${withDeps}.${main.message ? ` ${main.message}` : ""}`;
      break;
    }
    case "skipped":
      headline = `${mainName} was not installed${reason}`;
      break;
    default:
      headline = `${mainName} could not be installed${reason}`;
  }

  return { headline, error: main.status === "error", lines };
}

/** Gli id dei progetti che l'installazione ha messo nell'installazione. */
export function installedIds(outcomes: InstallOutcome[]): number[] {
  return outcomes.filter((o) => o.status === "installed").map((o) => o.modId);
}

/**
 * L'addon con il segno "installato" aggiornato: la ricerca lo calcola a ogni risposta, ma un
 * progetto installato dopo la risposta è in `installed`. Lo stesso oggetto se non cambia niente.
 */
export function withInstalled<T extends Pick<CatalogAddon, "id" | "installed">>(
  addon: T,
  installed: ReadonlySet<number>,
): T {
  return !addon.installed && installed.has(addon.id) ? { ...addon, installed: true } : addon;
}
