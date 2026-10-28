import { useEffect, useRef, useState, type KeyboardEvent, type RefObject } from "react";

export type RowMenuItem = {
  label: string;
  /** Un'azione distruttiva, nel colore di pericolo. */
  danger?: boolean;
  /** Una voce non disponibile resta raggiungibile, con il motivo nel `title`. */
  disabledReason?: string | null;
  onSelect: () => void;
};

type Props = {
  /** Il nome accessibile del pulsante e del menu, per esempio "More actions for Retail". */
  label: string;
  items: RowMenuItem[];
  /** Il pulsante "⋯": chi usa il menu ci riporta il focus quando l'azione scelta finisce. */
  triggerRef?: RefObject<HTMLButtonElement | null>;
};

/**
 * Il menu "⋯" di una riga di Options (issue #231): le azioni rare di un'installazione, fuori
 * dalla vista finché non servono. Si apre sotto il pulsante e si chiude con Esc, Tab (che
 * prosegue verso il controllo seguente o precedente), un clic fuori o la finestra che perde il
 * focus; le voci si scelgono con le frecce e Invio (pattern
 * WAI-ARIA "menu button").
 */
export function RowMenu({ label, items, triggerRef }: Props) {
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
    // Solo all'apertura: le voci che cambiano non spostano il focus.
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
        // Senza `preventDefault`: il focus passa al pulsante "⋯" e il browser continua la
        // sequenza da lì, in avanti con Tab e indietro con Maiusc+Tab (pattern WAI-ARIA).
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
        className="button ghost small row-menu-trigger"
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
              role="menuitem"
              tabIndex={-1}
              aria-disabled={Boolean(item.disabledReason)}
              title={item.disabledReason ?? undefined}
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
