#!/usr/bin/env bash
# Pin scripts/third-party-licenses.py (issue #194): which packages reach
# THIRD-PARTY-LICENSES, in which order, and that the file never names the machine that
# built it — executed against stubbed `cargo` and `pnpm`, not read.
#
# The stubs stand in for `cargo metadata` and `pnpm licenses list`. The fake graph has a
# normal dependency (alpha → epsilon), a dev-only one (beta), a build-only one (gamma), one
# that is both normal and dev (delta), and one that exists only for the second target
# (maconly), so the walk and the union across targets are both exercised.
#
# Run: bash scripts/test/third-party-licenses.test.sh

set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRIPT="$ROOT/scripts/third-party-licenses.py"

pass=0
fail=0

ok() { echo "  ok   — $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL — $1"; fail=$((fail + 1)); }

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

# The script resolves the project from its own location, so it runs from a copy.
mkdir -p "$TMP/proj/scripts" "$TMP/proj/src-tauri" "$TMP/bin" "$TMP/reg" "$TMP/node/zeta"
cp "$SCRIPT" "$TMP/proj/scripts/"

crate() { # crate <name> [<file> <text>]...
  local dir="$TMP/reg/$1"
  mkdir -p "$dir"
  shift
  while [ $# -gt 0 ]; do
    printf '%s\n' "$2" >"$dir/$1"
    shift 2
  done
}
crate wow-don
crate alpha LICENSE-MIT "MIT License for alpha" LICENSE-APACHE "Apache License
Version 2.0, January 2004"
crate beta LICENSE "beta is dev-only"
crate gamma LICENSE "gamma is build-only"
crate delta COPYING "delta copying text"
crate epsilon
crate maconly NOTICE "maconly notice"
printf 'zeta license text\n' >"$TMP/node/zeta/LICENSE"

python3 - "$TMP" <<'PY'
import json, sys
tmp = sys.argv[1]
def pkg(name, license="MIT"):
    return {"id": f"{name}-id", "name": name, "version": "1.0.0", "license": license,
            "manifest_path": f"{tmp}/reg/{name}/Cargo.toml"}
def dep(name, *kinds):
    return {"pkg": f"{name}-id", "dep_kinds": [{"kind": k} for k in kinds]}
def meta(extra):
    names = ["wow-don", "alpha", "beta", "gamma", "delta", "epsilon"] + extra
    root_deps = [dep("alpha", None), dep("beta", "dev"), dep("gamma", "build"),
                 dep("delta", None, "dev")] + [dep(n, None) for n in extra]
    nodes = [{"id": "wow-don-id", "deps": root_deps},
             {"id": "alpha-id", "deps": [dep("epsilon", None)]}]
    nodes += [{"id": f"{n}-id", "deps": []} for n in names if n not in ("wow-don", "alpha")]
    return {"packages": [pkg(n, "Apache-2.0 OR MIT" if n == "alpha" else "MIT") for n in names],
            "resolve": {"root": "wow-don-id", "nodes": nodes}}
json.dump(meta([]), open(f"{tmp}/meta-linux.json", "w"))
json.dump(meta(["maconly"]), open(f"{tmp}/meta-mac.json", "w"))
json.dump({"MIT": [{"name": "zeta", "versions": ["1.0.0"], "license": "MIT",
                    "paths": [f"{tmp}/node/zeta"]}]}, open(f"{tmp}/pnpm.json", "w"))
PY

cat >"$TMP/bin/cargo" <<EOF
#!/usr/bin/env bash
printf '%s\n' "\$*" >>"$TMP/cargo-args"
[ -n "\${FAKE_CARGO_FAIL:-}" ] && { echo "cargo exploded" >&2; exit 101; }
case "\$*" in
  *"--filter-platform mac-target"*) cat "$TMP/meta-mac.json" ;;
  *) cat "$TMP/meta-linux.json" ;;
esac
EOF
cat >"$TMP/bin/pnpm" <<EOF
#!/usr/bin/env bash
printf '%s\n' "\$*" >>"$TMP/pnpm-args"
cat "$TMP/pnpm.json"
EOF
chmod +x "$TMP/bin/cargo" "$TMP/bin/pnpm"

run() { # run <output> <target>...
  local out="$1"
  shift
  local args=()
  for t in "$@"; do args+=(--target "$t"); done
  PATH="$TMP/bin:$PATH" python3 "$TMP/proj/scripts/third-party-licenses.py" "${args[@]}" --output "$out"
}

echo "== one target"
OUT="$TMP/out/one.txt"
if run "$OUT" linux-target 2>"$TMP/err"; then ok "exits 0"; else bad "exits non-zero: $(cat "$TMP/err")"; fi
for entry in "--- alpha 1.0.0 (Apache-2.0 OR MIT) ---" "--- delta 1.0.0 (MIT) ---" "--- zeta 1.0.0 (MIT) ---"; do
  grep -qF -- "$entry" "$OUT" && ok "lists '$entry'" || bad "missing '$entry'"
done
grep -qF "MIT License for alpha" "$OUT" && ok "carries the license text" || bad "license text missing"
grep -qF "delta copying text" "$OUT" && ok "reads COPYING files" || bad "COPYING not read"
for absent in "beta" "gamma" "--- wow-don" "maconly"; do
  grep -qF -- "$absent" "$OUT" && bad "'$absent' must not be listed" || ok "'$absent' is not listed"
done
sed -n '/^Packages without a license file/,/^Standard license texts/p' "$OUT" | grep -qF "epsilon 1.0.0 (MIT)" \
  && ok "a crate without license files is listed with its expression" || bad "epsilon not listed as missing"
sed -n '/^Standard license texts/,$p' "$OUT" | grep -qF "Version 2.0, January 2004" \
  && ok "the standard Apache-2.0 text is appended" || bad "standard Apache text missing"
a=$(grep -n -- "--- alpha" "$OUT" | cut -d: -f1)
d=$(grep -n -- "--- delta" "$OUT" | cut -d: -f1)
z=$(grep -n -- "--- zeta" "$OUT" | cut -d: -f1)
[ "$a" -lt "$d" ] && [ "$d" -lt "$z" ] && ok "sorted by name, Rust before npm" || bad "order is alpha=$a delta=$d zeta=$z"
grep -qF "$TMP" "$OUT" && bad "the output names a build path" || ok "no build path in the output"
grep -qF -- "--locked" "$TMP/cargo-args" && grep -qF -- "--filter-platform linux-target" "$TMP/cargo-args" \
  && ok "cargo metadata is locked and filtered by target" || bad "cargo args: $(cat "$TMP/cargo-args")"
grep -qF -- "licenses list" "$TMP/pnpm-args" && grep -qF -- "--prod" "$TMP/pnpm-args" \
  && ok "pnpm licenses list is run with --prod" || bad "pnpm args: $(cat "$TMP/pnpm-args")"

echo "== determinism"
run "$TMP/out/again.txt" linux-target 2>/dev/null
cmp -s "$OUT" "$TMP/out/again.txt" && ok "two runs write the same bytes" || bad "output differs between runs"

echo "== two targets (universal macOS)"
run "$TMP/out/two.txt" linux-target mac-target 2>/dev/null
grep -qF -- "--- maconly 1.0.0 (MIT) ---" "$TMP/out/two.txt" && ok "a crate of the second target is included" || bad "maconly missing"
[ "$(grep -c -- "--- alpha 1.0.0" "$TMP/out/two.txt")" = 1 ] && ok "a crate of both targets is listed once" || bad "alpha duplicated"

echo "== cargo failure"
if FAKE_CARGO_FAIL=1 run "$TMP/out/fail.txt" linux-target 2>/dev/null; then
  bad "a failing cargo metadata must fail the script"
else
  ok "a failing cargo metadata fails the script"
fi
[ -e "$TMP/out/fail.txt" ] && bad "no file is written after a failure" || ok "no file is written after a failure"

echo
echo "third-party-licenses: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
