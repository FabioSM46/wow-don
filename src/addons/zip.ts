// Installing a manually downloaded zip (D32): which dropped file is accepted and the text of
// the `install_zip` outcome. Pure functions, no React or I/O.

import type { LocalInstall } from "../api";

/** The file name at the end of a path, with `/` or `\` as separator. */
export function fileName(path: string): string {
  const parts = path.split(/[\\/]/);
  return parts[parts.length - 1] || path;
}

/** Whether the path ends in `.zip`, ignoring case. */
export function isZipPath(path: string): boolean {
  return fileName(path).toLowerCase().endsWith(".zip");
}

/**
 * The file to install among the dropped ones, or the reason nothing is installed.
 * The backend is not called until this returns a path.
 */
export function pickZip(
  paths: string[],
  hasInstallation: boolean,
): { path: string } | { problem: string } {
  if (!hasInstallation) {
    return { problem: "Add an installation in Options before installing a zip." };
  }
  if (paths.length === 0) return { problem: "Drop a .zip file to install it." };
  if (paths.length > 1) return { problem: "Drop one .zip file at a time." };
  if (!isZipPath(paths[0])) {
    return { problem: `${fileName(paths[0])} is not a .zip file: only zips can be installed.` };
  }
  return { path: paths[0] };
}

function folderList(folders: string[]): string {
  return folders.join(", ");
}

/** The sentence for the `install_zip` outcome of zip `zipName` in installation `target`. */
export function describeZipInstall(result: LocalInstall, zipName: string, target: string): string {
  const count = result.installed.length;
  const what = count === 1 ? "1 folder" : `${count} folders`;
  let text = `Installed ${zipName} into ${target}: ${what} (${folderList(result.installed)}).`;
  if (result.replaced.length > 0) text += ` Replaced: ${folderList(result.replaced)}.`;
  if (result.warning) text += ` ${result.warning}`;
  return text;
}
