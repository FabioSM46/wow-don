import { useEffect, useLayoutEffect, useRef, useState, type KeyboardEvent } from "react";
import type { MenuAction, MenuItem } from "./selection";

type Props = {
  /** Il punto in cui si apre, in coordinate della finestra. */
  x: number;
  y: number;
  items: MenuItem[];
  /** Il nome accessibile del menu: l'addon o la selezione su cui agisce. */
  label: string;
  onChoose: (action: MenuAction) => void;
  /** `restoreFocus`: il menu si chiude con Esc e il focus torna alla riga. */
  onClose: (restoreFocus: boolean) => void;
};

/** Il margine tra il menu e i bordi della finestra. */
const EDGE = 8;

/**
 * Un menu contestuale leggero (issue #149): si apre nel punto del clic, resta dentro la finestra
 * e si chiude con Esc, un clic fuori, lo scroll, il ridimensionamento o il focus che esce. Le voci
 * si scelgono con le frecce e Invio (pattern WAI-ARIA "menu"); una voce disattivata resta
 * raggiungibile e dice il motivo nel `title`.
 */
export function ContextMenu({ x, y, items, label, onChoose, onClose }: Props) {
  const ref = useRef<HTMLDivElement>(null);
  const itemRefs = useRef<(HTMLButtonElement | null)[]>([]);
  const [position, setPosition] = useState({ left: x, top: y });

  // Dentro la finestra: un clic vicino al bordo destro o in basso apre il menu verso l'interno.
  useLayoutEffect(() => {
    const menu = ref.current;
    if (!menu) return;
    const { width, height } = menu.getBoundingClientRect();
    setPosition({
      left: Math.max(EDGE, Math.min(x, window.innerWidth - width - EDGE)),
      top: Math.max(EDGE, Math.min(y, window.innerHeight - height - EDGE)),
    });
  }, [x, y]);

  // Il focus va sulla prima voce attiva all'apertura (anche da tastiera).
  useEffect(() => {
    const first = items.findIndex((item) => item.disabledReason === null);
    itemRefs.current[first === -1 ? 0 : first]?.focus();
    // Solo all'apertura: le voci che cambiano non spostano il focus.
  }, []);

  useEffect(() => {
    const outside = (event: Event) => {
      if (!ref.current?.contains(event.target as Node)) onClose(false);
    };
    const close = () => onClose(false);
    document.addEventListener("mousedown", outside, true);
    // Il clic destro altrove riapre il menu nel nuovo punto: questo si chiude prima.
    document.addEventListener("contextmenu", outside, true);
    window.addEventListener("scroll", close, true);
    window.addEventListener("resize", close);
    window.addEventListener("blur", close);
    return () => {
      document.removeEventListener("mousedown", outside, true);
      document.removeEventListener("contextmenu", outside, true);
      window.removeEventListener("scroll", close, true);
      window.removeEventListener("resize", close);
      window.removeEventListener("blur", close);
    };
  }, [onClose]);

  function onKeyDown(event: KeyboardEvent<HTMLDivElement>) {
    const buttons = itemRefs.current.filter((b): b is HTMLButtonElement => b !== null);
    const index = buttons.indexOf(document.activeElement as HTMLButtonElement);
    let next: number | null = null;
    switch (event.key) {
      case "ArrowDown":
        next = (index + 1) % buttons.length;
        break;
      case "ArrowUp":
        next = (index - 1 + buttons.length) % buttons.length;
        break;
      case "Home":
        next = 0;
        break;
      case "End":
        next = buttons.length - 1;
        break;
      case "Escape":
        event.preventDefault();
        // Esc chiude il menu e non esce anche dalla modalità selezione.
        event.stopPropagation();
        onClose(true);
        return;
      case "Tab":
        event.preventDefault();
        onClose(true);
        return;
      default:
        return;
    }
    event.preventDefault();
    buttons[next]?.focus();
  }

  return (
    <div
      ref={ref}
      className="context-menu"
      role="menu"
      aria-label={label}
      style={{ left: position.left, top: position.top }}
      onKeyDown={onKeyDown}
      onBlur={(event) => {
        // Il focus esce dal menu (cambio di tab da tastiera, un altro controllo): si chiude.
        if (event.relatedTarget && !ref.current?.contains(event.relatedTarget as Node)) {
          onClose(false);
        }
      }}
      onContextMenu={(event) => event.preventDefault()}
    >
      {items.map((item, index) => (
        <button
          key={item.action}
          ref={(element) => {
            itemRefs.current[index] = element;
          }}
          className={item.action === "remove" ? "context-item danger" : "context-item"}
          role="menuitem"
          tabIndex={-1}
          aria-disabled={item.disabledReason !== null}
          title={item.disabledReason ?? undefined}
          onClick={() => {
            if (item.disabledReason === null) onChoose(item.action);
          }}
        >
          {item.label}
        </button>
      ))}
    </div>
  );
}
