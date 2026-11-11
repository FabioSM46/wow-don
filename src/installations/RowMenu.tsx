import { useEffect, useRef, useState, type KeyboardEvent, type RefObject } from "react";

export type RowMenuItem = {
  label: string;
  /** A destructive action, in the danger color. */
  danger?: boolean;
  /** An unavailable item stays reachable, with the reason in its `title`. */
  disabledReason?: string | null;
  /** What the item does, in its `title` while it is available. */
  hint?: string;
  /** A checkable item (`menuitemcheckbox`): `true` or `false` is its state, shown by a mark. */
  checked?: boolean;
  onSelect: () => void;
};

type Props = {
  /** The accessible name of the button and of the menu, for example "More actions for Retail". */
  label: string;
  items: RowMenuItem[];
  /** The "⋯" button: whoever uses the menu brings focus back to it when the chosen action ends. */
  triggerRef?: RefObject<HTMLButtonElement | null>;
  /** The classes of the "⋯" button; a small ghost button when omitted, as in an Options row. */
  triggerClassName?: string;
};

/**
 * The "⋯" menu of an Options row (issue #231) and of the Installed header (issue #314): the
 * rare actions, out of view until they are needed. It opens below the button and closes with Esc, Tab (which moves on
 * to the next or previous control), a click outside or the window losing focus; items are
 * chosen with the arrows and Enter (pattern
 * WAI-ARIA "menu button").
 */
export function RowMenu({
  label,
  items,
  triggerRef,
  triggerClassName = "button ghost small row-menu-trigger",
}: Props) {
  const [open, setOpen] = useState(false);
  const ownTrigger = useRef<HTMLButtonElement | null>(null);
  const trigger = triggerRef ?? ownTrigger;
  const menuRef = useRef<HTMLDivElement>(null);
  const itemRefs = useRef<(HTMLButtonElement | null)[]>([]);

  useEffect(() => {
    if (!open) return;
    const first = items.findIndex((item) => !item.disabledReason);
    itemRefs.current[first === -1 ? 0 : first]?.focus();
    const outside = (event: Event) => {
      const target = event.target as Node;
      if (!menuRef.current?.contains(target) && !trigger.current?.contains(target)) {
        setOpen(false);
      }
    };
    const close = () => setOpen(false);
    document.addEventListener("mousedown", outside, true);
    window.addEventListener("blur", close);
    return () => {
      document.removeEventListener("mousedown", outside, true);
      window.removeEventListener("blur", close);
    };
    // Only on opening: items that change do not move the focus.
  }, [open]);

  function closeAndFocus() {
    setOpen(false);
    trigger.current?.focus();
  }

  function onMenuKeyDown(event: KeyboardEvent<HTMLDivElement>) {
    const buttons = itemRefs.current.filter((b): b is HTMLButtonElement => b !== null);
    const index = buttons.indexOf(document.activeElement as HTMLButtonElement);
    let next: number;
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
        event.stopPropagation();
        closeAndFocus();
        return;
      case "Tab":
        // No `preventDefault`: focus goes to the "⋯" button and the browser continues the
        // sequence from there, forward with Tab and backward with Shift+Tab (WAI-ARIA pattern).
        closeAndFocus();
        return;
      default:
        return;
    }
    event.preventDefault();
    buttons[next]?.focus();
  }

  return (
    <span className="row-menu">
      <button
        ref={trigger}
        type="button"
        className={triggerClassName}
        aria-label={label}
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={() => setOpen((value) => !value)}
        onKeyDown={(event) => {
          if (event.key === "ArrowDown" && !open) {
            event.preventDefault();
            setOpen(true);
          }
        }}
      >
        ⋯
      </button>
      {open && (
        <div ref={menuRef} className="row-menu-list" role="menu" aria-label={label} onKeyDown={onMenuKeyDown}>
          {items.map((item, index) => (
            <button
              key={item.label}
              ref={(element) => {
                itemRefs.current[index] = element;
              }}
              type="button"
              className={item.danger ? "row-menu-item danger" : "row-menu-item"}
              role={item.checked === undefined ? "menuitem" : "menuitemcheckbox"}
              aria-checked={item.checked}
              tabIndex={-1}
              aria-disabled={Boolean(item.disabledReason)}
              title={item.disabledReason ?? item.hint}
              onClick={() => {
                if (item.disabledReason) return;
                setOpen(false);
                item.onSelect();
              }}
            >
              {item.label}
            </button>
          ))}
        </div>
      )}
    </span>
  );
}
