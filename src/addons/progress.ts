// L'avanzamento di installazioni e aggiornamenti (issue #128): l'evento `install-progress` del
// backend (#127), la percentuale e il testo della barra, l'abbinamento evento → riga. Funzioni
// pure, senza React né I/O.

import type { SourceKind } from "../api";
import { projectId } from "./sources.ts";

/** Il nome dell'evento Tauri, come `progress::EVENT` nel backend. */
export const PROGRESS_EVENT = "install-progress";

export type ProgressPhase = "checking" | "downloading" | "extracting" | "installing";

/**
 * Il payload di `install-progress`. `source` è `null` e `key` vuoto per uno zip scelto
 * dall'utente; `received` e `total` arrivano solo in `downloading`, e `total` solo se il server
 * dichiara la lunghezza. Non c'è una fase finale: l'avanzamento finisce quando il comando
 * restituisce i suoi esiti.
 */
export type InstallProgress = {
  installationId: string;
  source: SourceKind | null;
  key: string;
  name: string;
  phase: ProgressPhase;
  received?: number;
  total?: number;
};

/** La chiave di un evento: il progetto (`projectId`), o `zip` per uno zip locale. */
export function progressKey(event: InstallProgress): string {
  return event.source === null ? "zip" : projectId({ source: event.source, key: event.key });
}

/** La percentuale del download, da 0 a 100, o `null` quando la barra è indeterminata. */
export function progressPercent(event: InstallProgress | undefined): number | null {
  if (event?.phase !== "downloading") return null;
  const { received, total } = event;
  if (received === undefined || total === undefined || !(total > 0)) return null;
  return Math.min(100, Math.max(0, Math.floor((received / total) * 100)));
}

/** Il testo accanto alla barra: la fase, con la percentuale quando c'è. */
export function progressLabel(event: InstallProgress | undefined, idle = "Waiting…"): string {
  if (!event) return idle;
  switch (event.phase) {
    case "checking":
      return "Checking…";
    case "downloading": {
      const percent = progressPercent(event);
      return percent === null ? "Downloading…" : `Downloading ${percent}%`;
    }
    case "extracting":
      return "Extracting…";
    case "installing":
      return "Installing…";
  }
}

/** Aggiunge un evento alla mappa chiave → ultimo evento, se è dell'installazione indicata. */
export function withProgress(
  previous: Map<string, InstallProgress>,
  event: InstallProgress,
  installationId: string,
): Map<string, InstallProgress> {
  if (event.installationId !== installationId) return previous;
  return new Map(previous).set(progressKey(event), event);
}

/**
 * Quanti progetti di un aggiornamento sono già arrivati al proprio turno: il backend li porta
 * tutti in `checking` all'inizio, poi uno alla volta oltre. È la posizione di "Updating 2 of 5".
 */
export function progressPosition(
  progress: Map<string, InstallProgress>,
  projects: Iterable<string>,
): number {
  let started = 0;
  for (const id of projects) {
    const phase = progress.get(id)?.phase;
    if (phase !== undefined && phase !== "checking") started += 1;
  }
  return started;
}

/**
 * L'avanzamento delle installazioni avviate dalla Search, per progetto scelto. Il backend ne
 * esegue una alla volta (`InstallLock`) e consegna anche le dipendenze, con il loro id: un
 * evento di un progetto scelto rende quello il corrente, e ogni altro evento va al corrente.
 */
export type RunProgress = {
  current: string | null;
  runs: Map<string, InstallProgress>;
};

export const noRunProgress: RunProgress = { current: null, runs: new Map() };

export function withRunProgress(
  previous: RunProgress,
  event: InstallProgress,
  installationId: string,
  running: ReadonlySet<string>,
): RunProgress {
  if (event.installationId !== installationId) return previous;
  const key = progressKey(event);
  const current = running.has(key)
    ? key
    : previous.current !== null && running.has(previous.current)
      ? previous.current
      : null;
  if (current === null) return previous;
  return { current, runs: new Map(previous.runs).set(current, event) };
}

/** Toglie l'avanzamento di un'installazione finita. */
export function withoutRun(previous: RunProgress, run: string): RunProgress {
  if (!previous.runs.has(run) && previous.current !== run) return previous;
  const runs = new Map(previous.runs);
  runs.delete(run);
  return { current: previous.current === run ? null : previous.current, runs };
}

/** Il testo per un'installazione della Search: con il nome della dipendenza in corso. */
export function runLabel(event: InstallProgress | undefined, run: string): string {
  const label = progressLabel(event, "Starting…");
  if (!event || progressKey(event) === run || !event.name) return label;
  return `${label} · ${event.name}`;
}
