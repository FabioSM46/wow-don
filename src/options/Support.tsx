import { openUrl } from "@tauri-apps/plugin-opener";
import { useState } from "react";
import { errorMessage } from "../api";
import { SettingRow, SettingsSection } from "./Setting";
import { SUPPORT_PAGES } from "./supportLinks";

/**
 * The "Support" section of Options: two buttons that open GitHub Sponsors and Ko-fi in the
 * browser. Nothing is asked for anywhere else in the app.
 */
export function Support() {
  const [error, setError] = useState<string | null>(null);

  function open(url: string) {
    setError(null);
    openUrl(url).catch((err) => setError(`The page could not be opened: ${errorMessage(err)}`));
  }

  return (
    <SettingsSection id="support-title" title="Support">
      <div className="settings-card">
        <SettingRow
          label="Support WoW-don"
          description="WoW-don is free. If it is useful to you, you can support its development."
        >
          {SUPPORT_PAGES.map((page) => (
            <button key={page.url} className="button secondary" onClick={() => open(page.url)}>
              {page.label}
            </button>
          ))}
        </SettingRow>
      </div>
      {error && (
        <p className="message error" role="alert">
          {error}
        </p>
      )}
    </SettingsSection>
  );
}
