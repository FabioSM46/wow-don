// The addresses of the Options → Support buttons, without the Tauri plugin so the tests can
// import them.

/** The same accounts as `.github/FUNDING.yml` (`github`, `ko_fi`). */
export const SUPPORT_LINKS = {
  github: "https://github.com/sponsors/FabioSM46",
  koFi: "https://ko-fi.com/fabiosm46",
} as const;

/** The buttons, in order. */
export const SUPPORT_PAGES = [
  { label: "GitHub Sponsors", url: SUPPORT_LINKS.github },
  { label: "Ko-fi", url: SUPPORT_LINKS.koFi },
] as const;
