#!/usr/bin/env bash
# =============================================================================
# Regression tests for scripts/changed-areas.sh — the classifier behind ci.yml's
# `detect` job.
#
# These flags decide which workload jobs ci-gate permits to report SKIPPED. A
# wrong `false` is therefore a silently untested PR — so every rule is pinned
# here, and the fail-open behaviours (unrecognised paths, empty input) are
# pinned hardest.
#
# Each assertion compares the classifier's ENTIRE stdout, not one flag: the
# detect job appends it verbatim to $GITHUB_OUTPUT.
#
# Run: bash scripts/test/changed-areas.test.sh
# =============================================================================

set -uo pipefail

SCRIPT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/changed-areas.sh"

pass=0
fail=0

# expect <name> <backend> <frontend> <macos> <<'EOF'
#   one changed path per line
# EOF
expect() {
  local name="$1" out expected
  expected="backend=$2
frontend=$3
macos=$4"
  out=$(bash "$SCRIPT" 2>/dev/null)
  if [ "$out" = "$expected" ]; then
    echo "  ok   — ${name}"
    pass=$((pass + 1))
  else
    echo "  FAIL — ${name}"
    echo "         expected: $(echo "$expected" | tr '\n' ' ')"
    echo "         got:      $(echo "$out" | tr '\n' ' ')"
    fail=$((fail + 1))
  fi
}

echo "changed-areas — one workspace runs its own job"

expect "a backend file runs the backend job only" \
  true false false <<'EOF'
src-tauri/src/commands.rs
EOF

expect "tauri.conf.json is backend" \
  true false false <<'EOF'
src-tauri/tauri.conf.json
EOF

expect "a frontend file runs the frontend job only" \
  false true false <<'EOF'
src/App.tsx
EOF

expect "frontend toolchain files run the frontend job" \
  false true false <<'EOF'
index.html
vite.config.ts
tsconfig.json
tsconfig.node.json
public/tauri.svg
EOF

echo
echo "changed-areas — package manifests touch both sides"

expect "package.json runs backend and frontend" \
  true true false <<'EOF'
package.json
EOF

expect "pnpm-lock.yaml runs backend and frontend" \
  true true false <<'EOF'
pnpm-lock.yaml
EOF

echo
echo "changed-areas — markdown is inert only OUTSIDE the workspaces"

expect "root AGENTS.md runs nothing" \
  false false false <<'EOF'
AGENTS.md
EOF

expect "docs / agent skills / README bundle runs nothing" \
  false false false <<'EOF'
docs/roadmap.md
.claude/skills/dev-issue/SKILL.md
.agents/skills/dev-issue/SKILL.md
.opencode/skills/dev-issue/SKILL.md
README.md
EOF

expect "markdown inside a workspace still runs that workspace" \
  true false false <<'EOF'
src-tauri/NOTES.md
EOF

echo
echo "changed-areas — automation helpers are the automation job's problem"

expect "gh-automation.sh and its tests trigger no workspace job" \
  false false false <<'EOF'
scripts/gh-automation.sh
scripts/test/pr-status-frozen-rule.test.sh
EOF

expect "deepseek_review.py triggers no workspace job" \
  false false false <<'EOF'
.github/scripts/deepseek_review.py
EOF

echo
echo "changed-areas — global paths run everything"

expect "ci.yml runs everything, macOS included (the jobs' own meaning changed)" \
  true true true <<'EOF'
.github/workflows/ci.yml
EOF

expect "a workflow that is not ci.yml runs nothing" \
  false false false <<'EOF'
.github/workflows/pr-labeler.yml
EOF

expect ".gitignore and .env.example run nothing" \
  false false false <<'EOF'
.gitignore
.env.example
EOF

echo
echo "changed-areas — fail open"

expect "an unrecognised path runs everything but macOS" \
  true true false <<'EOF'
tools/new-top-level-thing.sh
EOF

expect "empty input runs everything, macOS included" \
  true true true <<'EOF'
EOF

expect "one unrecognised path among inert ones still runs everything" \
  true true false <<'EOF'
AGENTS.md
tools/new-top-level-thing.sh
EOF

echo
echo "changed-areas — the macOS job runs only when ci.yml changes (D29)"

expect "a backend change alone does not run macOS" \
  true false false <<'EOF'
src-tauri/src/install.rs
src-tauri/Cargo.toml
EOF

expect "ci.yml among other changes still runs macOS" \
  true true true <<'EOF'
AGENTS.md
src-tauri/src/install.rs
.github/workflows/ci.yml
EOF

expect "another workflow does not run macOS" \
  false false false <<'EOF'
.github/workflows/release.yml
EOF

expect "ci.yml as the old side of a rename still runs macOS" \
  true true true <<'EOF'
.github/workflows/ci-renamed.yml
.github/workflows/ci.yml
EOF

echo
echo "changed-areas — mixes and renames"

expect "a cross-workspace rename runs both workspaces" \
  true true false <<'EOF'
src/moved.ts
src-tauri/src/moved.rs
EOF

echo
echo "${pass} passed, ${fail} failed"
[ "$fail" -eq 0 ]
