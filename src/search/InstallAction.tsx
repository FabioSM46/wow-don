import type { CatalogAddon } from "../api";
import { installLabel, type InstallSummary } from "./install";
import { blockedHint, isBlocked, mirrorStep, unavailableText, type MirrorCheck } from "./mirror";
import { ProgressBar } from "../addons/ProgressBar";
import { ExternalLinkIcon } from "./icons";

/**
 * Un'installazione avviata dalla tab Search: in corso, finita con gli esiti o rifiutata. Per un
 * progetto bloccato da CurseForge senza un mirror GitHub utilizzabile (issue #165) è
 * `unavailable`, con il motivo e la pagina del progetto.
 */
export type InstallRun =
  | { status: "running" }
  | { status: "done"; summary: InstallSummary }
  | { status: "failed"; message: string }
  | { status: "unavailable"; name: string; reason: string; url: string | null };

type ActionProps = {
  addon: CatalogAddon;
  run: InstallRun | undefined;
  /** La barra di un'installazione in corso: percentuale (o `null`) e fase. */
  progress?: { value: number | null; label: string };
  /** `row`: piccolo, nella riga dei risultati; `detail`: nell'intestazione del dettaglio. */
  variant: "row" | "detail";
  /** Il controllo del mirror GitHub di un progetto bloccato, se è stato chiesto. */
  mirror?: MirrorCheck;
  onInstall: (addon: CatalogAddon) => void;
  onOpenPage: (url: string) => void;
};

/**
 * L'azione di installazione di un addon: Install, la barra di avanzamento, Checking… o il link
 * al sito. Nella riga sta in un'ultima colonna sempre visibile (issue #229); per un addon già
 * installato la riga non ha azioni: lo dice la pastiglia "Installed" sotto il nome.
 */
export function InstallAction(props: ActionProps) {
  if (props.variant === "detail") return <ActionContent {...props} />;
  return (
    <span className="row-actions">
      <ActionContent {...props} />
    </span>
  );
}

function isFailed(run: InstallRun | undefined): boolean {
  return run?.status === "failed" || (run?.status === "done" && run.summary.error);
}

function ActionContent({
  addon,
  run,
  progress,
  variant,
  mirror,
  onInstall,
  onOpenPage,
}: ActionProps) {
  const row = variant === "row";
  // Il segno "Installed" è già sotto il nome (riga) o accanto al nome (dettaglio).
  if (addon.installed) return null;
  const running = run?.status === "running";
  if (running) {
    return (
      <ProgressBar
        value={progress?.value ?? null}
        label={progress?.label ?? "Starting…"}
        name={addon.name}
      />
    );
  }
  // Un progetto bloccato si installa dal mirror GitHub: prima il controllo, poi Install; senza
  // un mirror utilizzabile (o senza repository GitHub) resta il link al sito (issue #165, #223).
  // Nel dettaglio il link a CurseForge è uno solo, nell'intestazione: qui non si ripete.
  const blocked = isBlocked(addon);
  const step = blocked ? mirrorStep(mirror) : null;
  if (step?.kind === "website") {
    if (!row || !addon.url) return null;
    const url = addon.url;
    return (
      <button
        className="button ghost small"
        title={unavailableText(step.reason)}
        onClick={(event) => {
          // L'azione non sceglie la riga.
          event.stopPropagation();
          onOpenPage(url);
        }}
      >
        <ExternalLinkIcon />
        Open on CurseForge
      </button>
    );
  }
  if (step?.kind === "wait") {
    return (
      <button className={row ? "button secondary small" : "button"} disabled aria-busy="true">
        {row ? "Checking…" : "Checking GitHub…"}
      </button>
    );
  }
  const failed = isFailed(run);
  return (
    <>
      {row && failed && (
        <span className="pill danger" title={failureText(run)}>
          Failed
        </span>
      )}
      <button
        className={row ? "button soft small" : "button"}
        title={blocked ? blockedHint(mirror).title : undefined}
        onClick={(event) => {
          event.stopPropagation();
          onInstall(addon);
        }}
      >
        {installLabel(addon)}
      </button>
    </>
  );
}

function failureText(run: InstallRun | undefined): string | undefined {
  if (run?.status === "failed") return run.message;
  if (run?.status === "done") return run.summary.headline;
  return undefined;
}

/**
 * L'esito di un'installazione in linea: la frase sul progetto e una riga per dipendenza; per un
 * progetto senza un mirror utilizzabile il motivo e il link alla pagina.
 */
export function InstallResult({
  run,
  onOpenPage,
}: {
  run: InstallRun;
  onOpenPage: (url: string) => void;
}) {
  if (run.status === "running") return null;
  if (run.status === "unavailable") {
    const url = run.url;
    return (
      <p className="message muted" role="status">
        {run.name} was not installed. {unavailableText(run.reason)}{" "}
        {url && (
          <button className="link inline" onClick={() => onOpenPage(url)}>
            Open on CurseForge
          </button>
        )}
      </p>
    );
  }
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
