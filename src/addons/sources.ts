// The addon sources in the Installed tab (D12, D31): a project's identity, the source name
// and the request for `update_addons`. Pure functions, no React or I/O.

import type { AddonUpdate, SourceKind } from "../api";

/** The source names, in the order the backend queries them. */
export const SOURCE_LABELS: Record<SourceKind, string> = {
  curseforge: "CurseForge",
  tukui: "Tukui",
  wowinterface: "WoWInterface",
  github: "GitHub",
};

/** A project of a source, as the backend identifies it. */
export type ProjectRef = { source: SourceKind; key: string };

/**
 * A project's identity in the frontend: the source and the id within the source. Two sources
 * can use the same id (`5108` on CurseForge and on WoWInterface), so the source is part of it.
 */
export function projectId(project: ProjectRef): string {
  return `${project.source}:${project.key}`;
}

/** CurseForge projects go in `modIds`, those of the other sources in `projects`. */
export function updateRequest(projects: ProjectRef[]): {
  modIds: number[];
  projects: ProjectRef[];
} {
  const modIds: number[] = [];
  const others: ProjectRef[] = [];
  const seen = new Set<string>();
  for (const project of projects) {
    const id = projectId(project);
    if (seen.has(id)) continue;
    seen.add(id);
    const modId = Number(project.key);
    if (project.source === "curseforge" && Number.isInteger(modId) && modId > 0) {
      modIds.push(modId);
    } else {
      others.push({ source: project.source, key: project.key });
    }
  }
  return { modIds, projects: others };
}

/** The sources that appear in the check, in the order of `SOURCE_LABELS`. */
export function sourcesIn(updates?: Map<string, AddonUpdate>): SourceKind[] {
  if (!updates) return [];
  const present = new Set<SourceKind>();
  for (const update of updates.values()) {
    if (update.project) present.add(update.project.source);
  }
  return (Object.keys(SOURCE_LABELS) as SourceKind[]).filter((source) => present.has(source));
}
