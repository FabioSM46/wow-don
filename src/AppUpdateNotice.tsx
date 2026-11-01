import { useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { errorMessage } from "./api";
import type { AppUpdateNotice as Notice } from "./appUpdate";

/**
 * The footer's link to the page of a newer release (D46, issue #260). Mounted only while there
 * is a notice and keyed by its URL, so a failure to open the page goes with that page.
 */
export function AppUpdateNotice({ notice }: { notice: Notice }) {
  const [error, setError] = useState<string | null>(null);

  function openRelease() {
    setError(null);
    openUrl(notice.url).catch((err) =>
      setError(`The page could not be opened: ${errorMessage(err)}`),
    );
  }

  return (
    <>
      <button type="button" className="link inline" onClick={openRelease}>
        {notice.text}
      </button>
      {error && (
        <span className="error" role="alert">
          {error}
        </span>
      )}
    </>
  );
}
