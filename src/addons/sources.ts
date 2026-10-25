// Le fonti degli addon nella tab Installed (D12, D31): l'identità di un progetto, il nome della
// fonte e la richiesta per `update_addons`. Funzioni pure, senza React né I/O.

import type { AddonUpdate, SourceKind } from "../api";

/** I nomi delle fonti, nell'ordine in cui il backend le interroga. */
export const SOURCE_LABELS: Record<SourceKind, string> = {
  curseforge: "CurseForge",
  tukui: "Tukui",
  wowinterface: "WoWInterface",
  github: "GitHub",
};

/** Un progetto di una fonte, come lo identifica il backend. */
export type ProjectRef = { source: SourceKind; key: string };

/**
 * L'identità di un progetto nel frontend: la fonte e l'id nella fonte. Due fonti possono usare
 * lo stesso id (`5108` su CurseForge e su WoWInterface), quindi la fonte ne fa parte.
 */
export function projectId(project: ProjectRef): string {
  return `${project.source}:${project.key}`;
}

/** I progetti CurseForge vanno in `modIds`, quelli delle altre fonti in `projects`. */
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

/** Le fonti che compaiono nel controllo, nell'ordine di `SOURCE_LABELS`. */
export function sourcesIn(updates?: Map<string, AddonUpdate>): SourceKind[] {
  if (!updates) return [];
  const present = new Set<SourceKind>();
  for (const update of updates.values()) {
    if (update.project) present.add(update.project.source);
  }
  return (Object.keys(SOURCE_LABELS) as SourceKind[]).filter((source) => present.has(source));
}
