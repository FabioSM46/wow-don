import { useEffect, useRef, useState } from "react";
import { getCurrentWebview } from "@tauri-apps/api/webview";

type Props = {
  /** Il nome dell'installazione in cui finirà lo zip, o `null` se non ce n'è una. */
  target: string | null;
  /** Chiamata al rilascio con i percorsi dei file trascinati. */
  onDrop: (paths: string[]) => void;
};

/**
 * Il trascinamento di file sulla finestra (D32): mentre un file passa sopra la finestra mostra
 * uno strato con l'installazione di destinazione, al rilascio passa i percorsi a `onDrop`. Tauri
 * consegna i percorsi con l'evento della webview; il file lo legge il backend.
 */
export function ZipDrop({ target, onDrop }: Props) {
  const [over, setOver] = useState(false);
  // Il listener si registra una volta sola: legge sempre l'ultimo `onDrop`.
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
      // Fuori da Tauri (per esempio `vite` nel browser) non c'è niente da ascoltare; dentro
      // l'app un errore qui vuol dire che il trascinamento non funziona: se ne lascia traccia.
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
