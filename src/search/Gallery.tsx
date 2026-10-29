import { useCallback, useEffect, useRef, useState } from "react";
import type { Screenshot } from "../api";
import { nextIndex, positionLabel, previousIndex, screenshotTitle } from "./screenshots";

type Props = {
  screenshots: Screenshot[];
  /** The gallery is visible: only then do arrows and Esc act on the open image. */
  active: boolean;
};

/**
 * The thumbnails in a grid; a click opens the large image in place of the grid, with
 * title and description as text. Images load from the CDN with `<img src>`: no
 * `fetch` from the frontend.
 */
export function Gallery({ screenshots, active }: Props) {
  const [open, setOpen] = useState<number | null>(null);
  // The images that failed to load, by position: "t" the thumbnail, "f" the large one.
  const [broken, setBroken] = useState<ReadonlySet<string>>(() => new Set());
  const thumbRefs = useRef<(HTMLButtonElement | null)[]>([]);
  const closeRef = useRef<HTMLButtonElement>(null);
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

  if (open !== null) {
    const shot = screenshots[open];
    const title = screenshotTitle(shot, open);
    const description = shot.description.trim();
    return (
      <figure className="gallery-viewer" aria-label={title}>
        <div className="gallery-viewer-bar">
          <span className="muted">{positionLabel(open, count)}</span>
          <button ref={closeRef} className="link subtle" onClick={() => setOpen(null)}>
            Close
          </button>
        </div>
        <div className="gallery-stage">
          <button
            className="gallery-nav"
            aria-label="Previous image"
            disabled={count < 2}
            onClick={() => setOpen(previousIndex(open, count))}
          >
            ‹
          </button>
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
          <button
            className="gallery-nav"
            aria-label="Next image"
            disabled={count < 2}
            onClick={() => setOpen(nextIndex(open, count))}
          >
            ›
          </button>
        </div>
        <figcaption className="gallery-caption">
          <p className="gallery-title">{title}</p>
          {/* Plain text: React shows it as text, never as HTML. */}
          {description && <p className="gallery-description">{description}</p>}
        </figcaption>
      </figure>
    );
  }

  return (
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
  );
}
