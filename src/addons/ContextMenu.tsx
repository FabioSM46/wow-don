import { useEffect, useLayoutEffect, useRef, useState, type KeyboardEvent } from "react";
import type { MenuAction, MenuItem } from "./selection";

type Props = {
  /** The point where it opens, in window coordinates. */
  x: number;
  y: number;
  items: MenuItem[];
  /** The accessible name of the menu: the addon or the selection it acts on. */
  label: string;
  onChoose: (action: MenuAction) => void;
  /** `restoreFocus`: the menu closes with Esc and focus returns to the row. */
  onClose: (restoreFocus: boolean) => void;
};

/** The margin between the menu and the window edges. */
const EDGE = 8;

/**
 * A lightweight context menu (issue #149): it opens at the click point, stays inside the window
 * and closes with Esc, a click outside, scrolling, resizing or focus leaving. Items are
 * chosen with the arrows and Enter (WAI-ARIA "menu" pattern); a disabled item stays
 * reachable and gives the reason in its `title`.
 */
export function ContextMenu({ x, y, items, label, onChoose, onClose }: Props) {
  const ref = useRef<HTMLDivElement>(null);
  const itemRefs = useRef<(HTMLButtonElement | null)[]>([]);
  const [position, setPosition] = useState({ left: x, top: y });

  // Inside the window: a click near the right or bottom edge opens the menu inwards.
  useLayoutEffect(() => {
    const menu = ref.current;
    if (!menu) return;
    const { width, height } = menu.getBoundingClientRect();
    setPosition({
      left: Math.max(EDGE, Math.min(x, window.innerWidth - width - EDGE)),
      top: Math.max(EDGE, Math.min(y, window.innerHeight - height - EDGE)),
    });
  }, [x, y]);

  // Focus goes to the first enabled item on opening (from the keyboard too).
  useEffect(() => {
    const first = items.findIndex((item) => item.disabledReason === null);
    itemRefs.current[first === -1 ? 0 : first]?.focus();
    // Only on opening: items that change do not move the focus.
  }, []);

  useEffect(() => {
    const outside = (event: Event) => {
      if (!ref.current?.contains(event.target as Node)) onClose(false);
    };
    const close = () => onClose(false);
    document.addEventListener("mousedown", outside, true);
    // A right click elsewhere reopens the menu at the new point: this one closes first.
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
        // Esc closes the menu and does not also leave selection mode.
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
        // Focus leaves the menu (tab change from the keyboard, another control): it closes.
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
