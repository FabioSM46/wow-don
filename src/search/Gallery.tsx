import { useCallback, useEffect, useRef, useState } from "react";
import type { MouseEvent } from "react";
import { createPortal } from "react-dom";
import type { Screenshot } from "../api";
import { nextIndex, positionLabel, previousIndex, screenshotTitle } from "./screenshots";

type Props = {
  screenshots: Screenshot[];
  /** The gallery is visible: only then do arrows and Esc act on the open image. */
  active: boolean;
};

/** What Tab can reach inside the viewer, to keep the focus there while it is open. */
const FOCUSABLE = "button:not(:disabled), [href], [tabindex]:not([tabindex='-1'])";

/**
 * The thumbnails in a grid; a click opens the large image in a viewer that covers the window
 * (issue #340), with title and description as text. The viewer is rendered in
 * `document.body`: inside `.search-page` a `position: fixed` element would be sized by the
 * Search area, not by the window. Images load from the CDN with `<img src>`: no `fetch` from
 * the frontend.
 */
export function Gallery({ screenshots, active }: Props) {
  const [open, setOpen] = useState<number | null>(null);
  // The images that failed to load, by position: "t" the thumbnail, "f" the large one.
  const [broken, setBroken] = useState<ReadonlySet<string>>(() => new Set());
  const thumbRefs = useRef<(HTMLButtonElement | null)[]>([]);
  const closeRef = useRef<HTMLButtonElement>(null);
  const viewerRef = useRef<HTMLDivElement>(null);
  // Where the last press in the viewer started: a click is dispatched on the common ancestor
  // of press and release, so a text selection released over the backdrop must not close it.
  const pressTarget = useRef<EventTarget | null>(null);
  const lastOpen = useRef<number | null>(null);
  const count = screenshots.length;
  const isOpen = open !== null;

  const markBroken = useCallback((key: string) => {
    setBroken((current) => (current.has(key) ? current : new Set(current).add(key)));
  }, []);

  // Focus goes to the Close button when the view opens and returns to the thumbnail of the last
  // image viewed when it closes.
  useEffect(() => {
    if (isOpen) {
      closeRef.current?.focus({ preventScroll: true });
    } else if (lastOpen.current !== null) {
      thumbRefs.current[lastOpen.current]?.focus({ preventScroll: true });
    }
  }, [isOpen]);

  useEffect(() => {
    lastOpen.current = open;
  }, [open]);

  // A gallery that is no longer visible closes its viewer: it would otherwise cover the window
  // over a tab or a screen that is not the gallery.
  useEffect(() => {
    if (!active) setOpen(null);
  }, [active]);

  // Esc closes the view before the detail goes back to the results: the listener is in capture
  // on the window, so it comes before the detail's one, which skips an event already
  // handled (`defaultPrevented`). The arrows do not act inside a tab bar or a field.
  useEffect(() => {
    if (!active || !isOpen) return;
    function onKeyDown(event: KeyboardEvent) {
      if (event.defaultPrevented) return;
      if (event.key === "Escape") {
        event.preventDefault();
        setOpen(null);
        return;
      }
      if (event.key === "Tab") {
        trapFocus(event, viewerRef.current);
        return;
      }
      if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return;
      const target = event.target;
      if (
        target instanceof Element &&
        target.closest('[role="tablist"], input, select, textarea')
      ) {
        return;
      }
      event.preventDefault();
      const step = event.key === "ArrowRight" ? nextIndex : previousIndex;
      setOpen((index) => (index === null ? null : step(index, count)));
    }
    window.addEventListener("keydown", onKeyDown, true);
    return () => window.removeEventListener("keydown", onKeyDown, true);
  }, [active, isOpen, count]);

  if (count === 0) {
    return <p className="empty">This addon has no screenshots.</p>;
  }

  // Only a click that starts and ends on the empty space around the image closes the viewer,
  // not one on the image, the arrows or the caption, nor a drag that ends outside them.
  function closeOnBackdrop(event: MouseEvent<HTMLElement>) {
    if (event.target === event.currentTarget && pressTarget.current === event.currentTarget) {
      setOpen(null);
    }
  }

  let viewer = null;
  if (open !== null && active) {
    const shot = screenshots[open];
    const title = screenshotTitle(shot, open);
    const description = shot.description.trim();
    viewer = createPortal(
      <div
        ref={viewerRef}
        className="gallery-viewer"
        role="dialog"
        aria-modal="true"
        aria-label={title}
        onMouseDown={(event) => {
          pressTarget.current = event.target;
        }}
        onClick={closeOnBackdrop}
      >
        <div className="gallery-viewer-bar">
          <span className="muted">{positionLabel(open, count)}</span>
          <button ref={closeRef} className="link subtle" onClick={() => setOpen(null)}>
            Close
          </button>
        </div>
        <div className="gallery-stage" onClick={closeOnBackdrop}>
          <button
            className="gallery-nav"
            aria-label="Previous image"
            disabled={count < 2}
            onClick={() => setOpen(previousIndex(open, count))}
          >
            ‹
          </button>
          <div className="gallery-frame" onClick={closeOnBackdrop}>
            {broken.has(`f${open}`) ? (
              <span className="gallery-placeholder gallery-placeholder-large">{title}</span>
            ) : (
              <img
                key={open}
                className="gallery-image"
                src={shot.url}
                alt={title}
                onError={() => markBroken(`f${open}`)}
              />
            )}
          </div>
          <button
            className="gallery-nav"
            aria-label="Next image"
            disabled={count < 2}
            onClick={() => setOpen(nextIndex(open, count))}
          >
            ›
          </button>
        </div>
        {/* Focusable, so that a long description can be scrolled with the keyboard. */}
        <div className="gallery-caption" tabIndex={0} role="region" aria-label="Description">
          <p className="gallery-title">{title}</p>
          {/* Plain text: React shows it as text, never as HTML. */}
          {description && <p className="gallery-description">{description}</p>}
        </div>
      </div>,
      document.body,
    );
  }

  return (
    <>
      <ul className="gallery-grid">
        {screenshots.map((shot, index) => {
          const title = screenshotTitle(shot, index);
          return (
            <li key={index}>
              <button
                ref={(element) => {
                  thumbRefs.current[index] = element;
                }}
                className="gallery-thumb"
                title={title}
                onClick={() => setOpen(index)}
              >
                {broken.has(`t${index}`) ? (
                  <span className="gallery-placeholder">{title}</span>
                ) : (
                  <img
                    src={shot.thumbnailUrl}
                    alt={title}
                    loading="lazy"
                    onError={() => markBroken(`t${index}`)}
                  />
                )}
              </button>
            </li>
          );
        })}
      </ul>
      {viewer}
    </>
  );
}

/** Keeps Tab and Shift+Tab inside `container`, wrapping from the last element to the first. */
function trapFocus(event: KeyboardEvent, container: HTMLElement | null) {
  if (!container) return;
  const items = Array.from(container.querySelectorAll<HTMLElement>(FOCUSABLE));
  if (items.length === 0) return;
  const first = items[0];
  const last = items[items.length - 1];
  const current = document.activeElement;
  const inside = current instanceof Node && container.contains(current);
  if (event.shiftKey && (!inside || current === first)) {
    event.preventDefault();
    last.focus();
  } else if (!event.shiftKey && (!inside || current === last)) {
    event.preventDefault();
    first.focus();
  }
}
