#!/usr/bin/env bash
# No two tracked files may differ only in letter case — in their full path, or in their name
# without the extension inside one folder.
#
# Windows and macOS (APFS) do not tell `importExport.ts` from `ImportExport.tsx` when a module
# is imported without its extension: `import "./ImportExport"` found the pure module instead of
# the component, and the v0.1.0 release failed to build its NSIS installer and .dmg. Linux tells
# them apart, and the PR CI builds the frontend on Linux only, so nothing else catches it.
# Same names in the same case (App.css and App.tsx, icon.png and icon.ico) are fine. Two
# folders that differ only in case (src/Utils, src/utils) are one folder there, so they clash too.
#
# Paths come unquoted (core.quotePath=false), so non-ASCII names are compared as text.
#
# Run: bash scripts/test/case-collisions.test.sh

set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

pass=0
fail=0

ok() { echo "  ok   — $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL — $1"; fail=$((fail + 1)); }

# collisions: reads one path per line, prints every group of paths that clash. Python's
# casefold() folds non-ASCII names the way the file systems do, whatever awk or locale is set.
collisions() {
  python3 -I -c '
import sys
from collections import defaultdict
paths = [p for p in sys.stdin.read().split("\n") if p]
groups = defaultdict(lambda: defaultdict(set))
for p in paths:
    parts = p.split("/")
    groups["same path"][p.casefold()].add(p)
    stem = parts[-1].rsplit(".", 1)[0] if "." in parts[-1][1:] else parts[-1]
    groups["same name"]["/".join(parts[:-1] + [stem]).casefold()].add("/".join(parts[:-1] + [stem]))
    for i in range(1, len(parts)):
        d = "/".join(parts[:i])
        groups["same folder"][d.casefold()].add(d)
out = []
for kind, keys in groups.items():
    for spellings in keys.values():
        if len(spellings) > 1:
            out.append(kind + ": " + " ".join(sorted(spellings)))
print("\n".join(sorted(out)))
'
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
out=$(printf '%s\n' src/Utils/a.ts src/utils/b.ts | collisions)
[ -n "$out" ] && ok "flags folders that differ only in case" || bad "missed a folder clash"
out=$(printf '%s\n' src/Ürgh.ts src/ürgh.tsx | collisions)
[ -n "$out" ] && ok "folds non-ASCII names" || bad "missed a non-ASCII clash"

echo "== the repository"
if ! files=$(git -C "$ROOT" -c core.quotePath=false ls-files); then
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
