import { useEffect, useRef, useState } from "react";
import { getCurrentWebview } from "@tauri-apps/api/webview";

type Props = {
  /** The name of the installation the zip will go into, or `null` if there is none. */
  target: string | null;
  /** Called on drop with the paths of the dragged files. */
  onDrop: (paths: string[]) => void;
};

/**
 * Dragging files onto the window (D32): while a file hovers over the window it shows an
 * overlay with the target installation, on drop it passes the paths to `onDrop`. Tauri
 * delivers the paths with the webview event; the backend reads the file.
 */
export function ZipDrop({ target, onDrop }: Props) {
  const [over, setOver] = useState(false);
  // The listener is registered only once: it always reads the latest `onDrop`.
  const dropRef = useRef(onDrop);
  dropRef.current = onDrop;

  useEffect(() => {
    let unlisten: (() => void) | null = null;
    let disposed = false;
    getCurrentWebview()
      .onDragDropEvent((event) => {
        const payload = event.payload;
        if (payload.type === "enter" || payload.type === "over") {
          setOver(true);
        } else if (payload.type === "leave") {
          setOver(false);
        } else {
          setOver(false);
          dropRef.current(payload.paths);
        }
      })
      .then((stop) => {
        if (disposed) stop();
        else unlisten = stop;
      })
      // Outside Tauri (for example `vite` in the browser) there is nothing to listen to; inside
      // the app an error here means dragging does not work: a trace of it is left.
      .catch((err) => console.warn("Drag and drop is unavailable:", err));
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  if (!over) return null;
  return (
    <div className="zip-drop" aria-hidden="true">
      <p className="zip-drop-text">
        {target ? `Drop a .zip to install into ${target}` : "Add an installation in Options first"}
      </p>
    </div>
  );
}
