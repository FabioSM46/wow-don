import {
  createContext,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from "react";
import { createPortal } from "react-dom";

/** Where the messages below a provider are drawn, and whether they show now. */
type Target = { element: HTMLElement | null; hidden: boolean };

const StatusAreaContext = createContext<Target>({ element: null, hidden: false });

type ProviderProps = {
  /** The element of the area, from `StatusArea`; `null` until it is mounted. */
  element: HTMLElement | null;
  /**
   * The messages below belong to a tab that is not in view (the tabs stay mounted): they keep
   * their place in the area and are hidden until the tab comes back.
   */
  hidden?: boolean;
  children: ReactNode;
};

export function StatusAreaProvider({ element, hidden = false, children }: ProviderProps) {
  const target = useMemo(() => ({ element, hidden }), [element, hidden]);
  return <StatusAreaContext.Provider value={target}>{children}</StatusAreaContext.Provider>;
}

/** The custom property that carries the height of the area to the scrolling regions. */
const HEIGHT_PROPERTY = "--status-area-height";

type AreaProps = {
  /** Receives the element the messages are drawn in, for `StatusAreaProvider`. */
  onElement: (element: HTMLElement | null) => void;
};

/**
 * The status area (D48, issue #315): one box anchored to the bottom of the content, above the
 * footer, where errors, progress and results appear without moving what is above them. It is
 * laid over the content, so its height is published as `--status-area-height` on the parent
 * element: the scrolling regions pad their end by that much and their last row can always be
 * scrolled clear of the area. With nothing to show, the area is not displayed and the height
 * is zero.
 */
export function StatusArea({ onElement }: AreaProps) {
  const [element, setElement] = useState<HTMLDivElement | null>(null);

  useEffect(() => {
    onElement(element);
  }, [element, onElement]);

  useEffect(() => {
    const host = element?.parentElement?.parentElement;
    if (!element || !host) return;
    const publish = () => host.style.setProperty(HEIGHT_PROPERTY, `${element.offsetHeight}px`);
    publish();
    const observer = new ResizeObserver(publish);
    observer.observe(element);
    return () => {
      observer.disconnect();
      host.style.removeProperty(HEIGHT_PROPERTY);
    };
  }, [element]);

  return (
    <div className="status-anchor">
      <div ref={setElement} className="status-area" role="region" aria-label="Messages" />
    </div>
  );
}

type MessageProps = {
  /** An error: in the danger color and announced as an alert. */
  error?: boolean;
  /** Work in progress: it goes away by itself when the work ends, so it has no "Dismiss". */
  progress?: boolean;
  /**
   * How the message is announced. By default an error is an alert, progress is not a live
   * region (the bar inside it describes itself) and anything else is a status.
   */
  role?: "alert" | "status";
  /** Clears the owner's state behind the message; without it "Dismiss" only hides it. */
  onDismiss?: () => void;
  children: ReactNode;
};

/**
 * One message of the status area. The owner keeps the state and renders this where it used to
 * render the message; the content is drawn in the shared area instead. Each message is its own
 * portal, and a portal that mounts is appended to the area: the messages stack in the order
 * they appeared, newest last, whoever owns them. Nothing leaves on a timer: a message stays
 * until its owner removes it or the user dismisses it.
 */
export function StatusMessage({ error, progress, role, onDismiss, children }: MessageProps) {
  const { element: area, hidden } = useContext(StatusAreaContext);
  const [dismissed, setDismissed] = useState(false);
  const mounted = area !== null && !dismissed;

  // The newest message is the last one: an area already scrolling moves to show it.
  useEffect(() => {
    if (mounted) area.scrollTop = area.scrollHeight;
  }, [mounted, area]);

  if (!mounted) return null;
  return createPortal(
    <div
      className={error ? "status-message error" : "status-message"}
      role={role ?? (error ? "alert" : progress ? undefined : "status")}
      hidden={hidden}
    >
      <div className="status-message-body">{children}</div>
      {!progress && (
        <button
          className="link subtle"
          onClick={() => {
            setDismissed(true);
            onDismiss?.();
          }}
        >
          Dismiss
        </button>
      )}
    </div>,
    area,
  );
}
