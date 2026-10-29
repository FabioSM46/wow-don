// The Interface column of the Installed tab (issue #230): a single version and the rest as
// `+N`, so an addon that declares many versions does not wrap the row. Pure functions,
// no React or I/O.

/** What the cell shows: the chosen version, how many others there are and the whole list. */
export type InterfaceSummary = { shown: number; more: number; title: string };

/**
 * The `## Interface` ranges of each flavor, `[from, up to exclusive)`. It is the same table as
 * `fits` in `src-tauri/src/sources/tukui.rs`: `interfaceColumn.test.ts` reads that file and
 * fails if the two tables diverge.
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
 * Whether an `## Interface` number belongs to the game of the installation's flavor. The folder
 * name (`_classic_era_`) is accepted too and case does not matter. An unknown flavor has no
 * versions.
 */
export function fitsFlavor(flavor: string, version: number): boolean {
  const key = flavor.replace(/^_+|_+$/g, "").toLowerCase();
  const range = Object.prototype.hasOwnProperty.call(FLAVOR_RANGES, key)
    ? FLAVOR_RANGES[key]
    : undefined;
  return range !== undefined && version >= range[0] && version < range[1];
}

/**
 * The version to show is the first that belongs to the installation's flavor, otherwise the
 * first declared; the others become `+N`, and `title` lists them all in the order of the
 * `.toc`. Without versions there is nothing to show (`null`).
 */
export function interfaceSummary(versions: number[], flavor: string): InterfaceSummary | null {
  if (versions.length === 0) return null;
  const shown = versions.find((version) => fitsFlavor(flavor, version)) ?? versions[0];
  return { shown, more: versions.length - 1, title: versions.join(", ") };
}
