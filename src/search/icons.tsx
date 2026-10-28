// Le due icone della tab Search (issue #229): la lente nella casella di ricerca e il segno del
// link esterno sui pulsanti che aprono CurseForge nel browser. SVG in linea, nel colore del
// testo, nascoste ai lettori di schermo: il testo accanto dice già cosa fanno.

/** La lente della casella di ricerca. */
export function SearchIcon() {
  return (
    <svg
      className="icon"
      viewBox="0 0 16 16"
      width="14"
      height="14"
      aria-hidden="true"
      focusable="false"
    >
      <circle cx="7" cy="7" r="4.5" fill="none" stroke="currentColor" strokeWidth="1.5" />
      <path d="M10.5 10.5 14 14" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
    </svg>
  );
}

/** Il link esterno: si apre nel browser, fuori dall'app. */
export function ExternalLinkIcon() {
  return (
    <svg
      className="icon"
      viewBox="0 0 16 16"
      width="14"
      height="14"
      aria-hidden="true"
      focusable="false"
    >
      <path
        d="M9 3h4v4M13 3 7.5 8.5M11.5 9.5V13h-8.5V4.5H6.5"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.5"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}
