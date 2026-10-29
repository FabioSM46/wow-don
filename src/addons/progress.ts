// The progress of installs and updates (issue #128): the backend's `install-progress` event
// (#127), the percentage and text of the bar, the event → row matching. Pure functions,
// no React or I/O.

import type { SourceKind } from "../api";
import { projectId } from "./sources.ts";

/** The name of the Tauri event, like `progress::EVENT` in the backend. */
export const PROGRESS_EVENT = "install-progress";

/**
 * `downloaded` comes only from the import (issue #180): the zip is downloaded and waits its turn,
 * because the backend downloads several projects together and installs them one at a time.
 */
export type ProgressPhase = "checking" | "downloading" | "downloaded" | "extracting" | "installing";

/** The phases in the order a project goes through them. */
export const PHASE_ORDER: readonly ProgressPhase[] = [
  "checking",
  "downloading",
  "downloaded",
  "extracting",
  "installing",
];

/**
 * The payload of `install-progress`. `source` is `null` and `key` empty for a zip chosen by
 * the user; `received` and `total` arrive only in `downloading`, and `total` only if the server
 * declares the length. There is no final phase: progress ends when the command returns its
 * outcomes.
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

/** The key of an event: the project (`projectId`), or `zip` for a local zip. */
export function progressKey(event: InstallProgress): string {
  return event.source === null ? "zip" : projectId({ source: event.source, key: event.key });
}

/** The download percentage, from 0 to 100, or `null` when the bar is indeterminate. */
export function progressPercent(event: InstallProgress | undefined): number | null {
  if (event?.phase !== "downloading") return null;
  const { received, total } = event;
  if (received === undefined || total === undefined || !(total > 0)) return null;
  return Math.min(100, Math.max(0, Math.floor((received / total) * 100)));
}

/** The text next to the bar: the phase, with the percentage when there is one. */
export function progressLabel(event: InstallProgress | undefined, idle = "Waiting…"): string {
  if (!event) return idle;
  switch (event.phase) {
    case "checking":
      return "Checking…";
    case "downloading": {
      const percent = progressPercent(event);
      return percent === null ? "Downloading…" : `Downloading ${percent}%`;
    }
    case "downloaded":
      return "Waiting to install…";
    case "extracting":
      return "Extracting…";
    case "installing":
      return "Installing…";
  }
}

/**
 * True if the two events look the same: same phase, same name and same percentage. Such an
 * event does not change the bar and must not redraw the list.
 */
export function sameView(a: InstallProgress | undefined, b: InstallProgress): boolean {
  return (
    a !== undefined &&
    progressKey(a) === progressKey(b) &&
    a.phase === b.phase &&
    a.name === b.name &&
    progressPercent(a) === progressPercent(b)
  );
}

/**
 * Adds an event to the key → latest event map, if it belongs to the given installation. An
 * event that changes nothing visible returns the same map, so React does not redraw.
 */
export function withProgress(
  previous: Map<string, InstallProgress>,
  event: InstallProgress,
  installationId: string,
): Map<string, InstallProgress> {
  if (event.installationId !== installationId) return previous;
  const key = progressKey(event);
  if (sameView(previous.get(key), event)) return previous;
  return new Map(previous).set(key, event);
}

/**
 * How many projects of an update have already reached their turn: the backend brings them
 * all to `checking` at the start, then one at a time beyond it. It is the position in
 * "Updating 2 of 5".
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
 * The progress of the installs started from Search, per chosen project. The backend runs
 * one at a time (`InstallLock`) and delivers the dependencies too, with their id: an event
 * of a chosen project makes it the current one, and every other event goes to the current one.
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
  if (current === previous.current && sameView(previous.runs.get(current), event)) return previous;
  return { current, runs: new Map(previous.runs).set(current, event) };
}

/** Removes the progress of a finished install. */
export function withoutRun(previous: RunProgress, run: string): RunProgress {
  if (!previous.runs.has(run) && previous.current !== run) return previous;
  const runs = new Map(previous.runs);
  runs.delete(run);
  return { current: previous.current === run ? null : previous.current, runs };
}

/** The text for a Search install: with the name of the dependency in progress. */
export function runLabel(event: InstallProgress | undefined, run: string): string {
  const label = progressLabel(event, "Starting…");
  if (!event || progressKey(event) === run || !event.name) return label;
  return `${label} · ${event.name}`;
}
