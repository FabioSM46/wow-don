#!/usr/bin/env bash
# No two tracked files may differ only in letter case — in their full path, or in their name
# without the extension inside one folder.
#
# Windows and macOS (APFS) do not tell `importExport.ts` from `ImportExport.tsx` when a module
# is imported without its extension: `import "./ImportExport"` found the pure module instead of
# the component, and the v0.1.0 release failed to build its NSIS installer and .dmg. Linux tells
# them apart, and the PR CI builds the frontend on Linux only, so nothing else catches it.
# Same names in the same case (App.css and App.tsx, icon.png and icon.ico) are fine.
#
# Run: bash scripts/test/case-collisions.test.sh

set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

pass=0
fail=0

ok() { echo "  ok   — $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL — $1"; fail=$((fail + 1)); }

# collisions: reads one path per line, prints every group of paths that clash.
collisions() {
  awk '
    {
      path = $0
      full[tolower(path)] = full[tolower(path)] " " path
      nfull[tolower(path)]++
      dir = path; name = path
      if (sub(/\/[^\/]*$/, "", dir) == 0) dir = "."
      sub(/^.*\//, "", name)
      stem = name; sub(/\.[^.]*$/, "", stem)
      key = tolower(dir "/" stem)
      if (!(key in first)) first[key] = dir "/" stem
      else if (first[key] != dir "/" stem) clash[key] = 1
      stems[key] = stems[key] " " path
    }
    END {
      for (k in nfull) if (nfull[k] > 1) print "same path:" full[k]
      for (k in clash) print "same name:" stems[k]
    }
  ' | sort
}

echo "== the checker"
out=$(printf '%s\n' src/a/importExport.ts src/a/ImportExport.tsx | collisions)
[ -n "$out" ] && ok "flags importExport.ts beside ImportExport.tsx" || bad "missed a stem clash"
out=$(printf '%s\n' src/App.css src/App.tsx icons/icon.png icons/icon.ico | collisions)
[ -z "$out" ] && ok "accepts App.css beside App.tsx" || bad "false clash: $out"
out=$(printf '%s\n' docs/Readme.md docs/README.md | collisions)
[ -n "$out" ] && ok "flags paths that differ only in case" || bad "missed a path clash"
out=$(printf '%s\n' src/a/gallery.ts src/b/Gallery.tsx | collisions)
[ -z "$out" ] && ok "accepts the same name in different folders" || bad "false clash: $out"

echo "== the repository"
if ! files=$(git -C "$ROOT" ls-files); then
  bad "git ls-files failed"
else
  out=$(printf '%s\n' "$files" | collisions)
  if [ -z "$out" ]; then
    ok "no tracked files differ only in case"
  else
    bad "files that differ only in case:"
    printf '         %s\n' "$out"
  fi
fi

echo
echo "case-collisions: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
