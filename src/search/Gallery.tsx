import { useCallback, useEffect, useRef, useState } from "react";
import type { Screenshot } from "../api";
import { nextIndex, positionLabel, previousIndex, screenshotTitle } from "./gallery";

type Props = {
  screenshots: Screenshot[];
  /** La gallery è visibile: solo allora frecce ed Esc agiscono sull'immagine aperta. */
  active: boolean;
};

/**
 * Le miniature in una griglia; un clic apre l'immagine grande al posto della griglia, con
 * titolo e descrizione come testo. Le immagini si caricano dal CDN con `<img src>`: nessun
 * `fetch` dal frontend.
 */
export function Gallery({ screenshots, active }: Props) {
  const [open, setOpen] = useState<number | null>(null);
  // Le immagini che non si sono caricate, per posizione: "t" la miniatura, "f" quella grande.
  const [broken, setBroken] = useState<ReadonlySet<string>>(() => new Set());
  const thumbRefs = useRef<(HTMLButtonElement | null)[]>([]);
  const closeRef = useRef<HTMLButtonElement>(null);
  const lastOpen = useRef<number | null>(null);
  const count = screenshots.length;
  const isOpen = open !== null;

  const markBroken = useCallback((key: string) => {
    setBroken((current) => (current.has(key) ? current : new Set(current).add(key)));
  }, []);

  // Il focus va sul pulsante Close quando la vista si apre e torna sulla miniatura dell'ultima
  // immagine vista quando si chiude.
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

  // Esc chiude la vista prima che il dettaglio torni ai risultati: l'ascolto è in cattura
  // sulla finestra, quindi arriva prima di quello del dettaglio, che salta un evento già
  // gestito (`defaultPrevented`). Le frecce non agiscono dentro una barra di tab o un campo.
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
          {/* Testo semplice: React lo mostra come testo, mai come HTML. */}
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
