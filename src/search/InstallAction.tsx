import type { CatalogAddon } from "../api";
import type { InstallSummary } from "./install";

/** Un'installazione avviata dalla tab Search: in corso, finita con gli esiti o rifiutata. */
export type InstallRun =
  | { status: "running" }
  | { status: "done"; summary: InstallSummary }
  | { status: "failed"; message: string };

type ActionProps = {
  addon: CatalogAddon;
  run: InstallRun | undefined;
  /** `row`: piccolo, nella riga dei risultati; `detail`: nell'intestazione del dettaglio. */
  variant: "row" | "detail";
  onInstall: (addon: CatalogAddon) => void;
  onOpenPage: (url: string) => void;
};

const BLOCKED_REASON =
  "The author does not allow downloads from other apps: install it from the CurseForge website.";

/**
 * L'azione di installazione di un addon: Install, Installing…, Installed o il link al sito.
 * Nella riga compare al passaggio del mouse o del focus, e resta in vista durante
 * l'installazione e dopo un errore.
 */
export function InstallAction(props: ActionProps) {
  if (props.variant === "detail") return <ActionContent {...props} />;
  const status = props.run?.status;
  const pinned = !props.addon.installed && (status === "running" || isFailed(props.run));
  return (
    <span className={pinned ? "row-actions pinned" : "row-actions"}>
      <ActionContent {...props} />
    </span>
  );
}

function isFailed(run: InstallRun | undefined): boolean {
  return run?.status === "failed" || (run?.status === "done" && run.summary.error);
}

function ActionContent({ addon, run, variant, onInstall, onOpenPage }: ActionProps) {
  const small = variant === "row" ? " small" : "";
  if (addon.installed) {
    // Nel dettaglio il segno "Installed" è già accanto al nome.
    return variant === "row" ? <span className="installed-mark">Installed</span> : null;
  }
  if (!addon.allowModDistribution) {
    if (!addon.url) return null;
    const url = addon.url;
    return (
      <button
        className={`button secondary${small}`}
        title={BLOCKED_REASON}
        onClick={(event) => {
          // L'azione non sceglie la riga.
          event.stopPropagation();
          onOpenPage(url);
        }}
      >
        Install from CurseForge
      </button>
    );
  }
  const running = run?.status === "running";
  const failed = isFailed(run);
  return (
    <>
      {variant === "row" && failed && (
        <span className="error install-failed" title={failureText(run)}>
          Failed
        </span>
      )}
      <button
        className={variant === "row" ? "button secondary small" : "button"}
        disabled={running}
        aria-busy={running}
        onClick={(event) => {
          event.stopPropagation();
          onInstall(addon);
        }}
      >
        {running ? "Installing…" : "Install"}
      </button>
    </>
  );
}

function failureText(run: InstallRun | undefined): string | undefined {
  if (run?.status === "failed") return run.message;
  if (run?.status === "done") return run.summary.headline;
  return undefined;
}

/** L'esito di un'installazione in linea: la frase sul progetto e una riga per dipendenza. */
export function InstallResult({ run }: { run: InstallRun }) {
  if (run.status === "running") return null;
  if (run.status === "failed") {
    return (
      <p className="message error" role="alert">
        Install failed: {run.message}
      </p>
    );
  }
  const { summary } = run;
  return (
    <div className="message install-summary" role="status">
      <p className={summary.error ? "error" : undefined}>{summary.headline}</p>
      {summary.lines.length > 0 && (
        <ul>
          {summary.lines.map((line) => (
            <li key={line.modId} className={line.error ? "error" : "muted"}>
              {line.text}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
