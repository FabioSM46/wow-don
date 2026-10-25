// Installazione di uno zip scaricato a mano (D32): quale file trascinato si accetta e il testo
// dell'esito di `install_zip`. Funzioni pure, senza React né I/O.

import type { LocalInstall } from "../api";

/** Il nome del file alla fine di un percorso, con `/` o `\` come separatore. */
export function fileName(path: string): string {
  const parts = path.split(/[\\/]/);
  return parts[parts.length - 1] || path;
}

/** Se il percorso termina in `.zip`, senza distinguere maiuscole e minuscole. */
export function isZipPath(path: string): boolean {
  return fileName(path).toLowerCase().endsWith(".zip");
}

/**
 * Il file da installare tra quelli trascinati, oppure il motivo per cui non si installa niente.
 * Il backend non si chiama finché questo non restituisce un percorso.
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

/** La frase dell'esito di `install_zip` per lo zip `zipName` nell'installazione `target`. */
export function describeZipInstall(result: LocalInstall, zipName: string, target: string): string {
  const count = result.installed.length;
  const what = count === 1 ? "1 folder" : `${count} folders`;
  let text = `Installed ${zipName} into ${target}: ${what} (${folderList(result.installed)}).`;
  if (result.replaced.length > 0) text += ` Replaced: ${folderList(result.replaced)}.`;
  return text;
}
