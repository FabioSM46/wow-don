import type { CatalogAddon } from "../api";
import { installLabel, type InstallSummary } from "./install";
import { blockedHint, isBlocked, mirrorStep, unavailableText, type MirrorCheck } from "./mirror";
import { ProgressBar } from "../addons/ProgressBar";
import { ExternalLinkIcon } from "./icons";

/**
 * An install started from the Search tab: in progress, finished with the outcomes or rejected.
 * For a project blocked by CurseForge without a usable GitHub mirror (issue #165) it is
 * `unavailable`, with the reason and the project page.
 */
export type InstallRun =
  | { status: "running" }
  | { status: "done"; summary: InstallSummary }
  | { status: "failed"; message: string }
  | { status: "unavailable"; name: string; reason: string; url: string | null };

type ActionProps = {
  addon: CatalogAddon;
  run: InstallRun | undefined;
  /** The bar of an install in progress: percentage (or `null`) and phase. */
  progress?: { value: number | null; label: string };
  /** `row`: small, in the results row; `detail`: in the detail header. */
  variant: "row" | "detail";
  /** The GitHub mirror check of a blocked project, if it was requested. */
  mirror?: MirrorCheck;
  onInstall: (addon: CatalogAddon) => void;
  onOpenPage: (url: string) => void;
};

/**
 * An addon's install action: Install, the progress bar, Checking… or the link to the
 * website. In the row it sits in a last column that is always visible (issue #229); for an
 * addon already installed the row has no actions: the "Installed" pill under the name says so.
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
  // The "Installed" mark is already under the name (row) or next to the name (detail).
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
  // A blocked project installs from the GitHub mirror: first the check, then Install; without
  // a usable mirror (or without a GitHub repository) the link to the website remains
  // (issue #165, #223).
  // In the detail there is only one link to CurseForge, in the header: it is not repeated here.
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
          // The action does not select the row.
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
 * The inline outcome of an install: the sentence on the project and one line per dependency;
 * for a project without a usable mirror, the reason and the link to the page.
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
