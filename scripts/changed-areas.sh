#!/usr/bin/env bash
# =============================================================================
# changed-areas.sh — classify a changed-file list into the CI jobs it can affect
#
# Reads newline-separated repository paths on stdin (for renames, feed BOTH the
# new and the previous path). Prints exactly two `key=value` lines on stdout, in
# the shape `$GITHUB_OUTPUT` consumes:
#
#   backend=…    run the `backend` job (Rust/Tauri gates in src-tauri/)
#   frontend=…   run the `frontend` job (React/TypeScript gates)
#
# The `helpers` selector (which gates the `automation` job) is deliberately NOT
# computed here: the automation job runs this script's own tests, so gating it on
# this script's output would let a classifier bug exempt itself. ci.yml always
# sets `helpers=true`.
#
# ci.yml consumes these through `if: … != 'false'`. This script FAILS OPEN:
#
#   * a path no rule recognises sets every flag true, so a new top-level
#     file is tested until someone classifies it here, and
#   * empty input sets every flag true — an empty list means the caller could
#     not produce one, and an unreadable diff must run everything, not nothing.
#
# scripts/test/changed-areas.test.sh pins every rule below.
# =============================================================================

set -euo pipefail

ws_backend=false   # src-tauri/**
ws_frontend=false  # src/**, public/**, and the frontend toolchain files
global=false       # affects every workspace, or unrecognised (fail open)

saw_any=false

while IFS= read -r path; do
  [ -n "$path" ] || continue
  saw_any=true
  # First match wins, top to bottom. Workspace prefixes come BEFORE the inert arm
  # on purpose: markdown inside a workspace is that workspace's problem.
  case "$path" in
    src-tauri/*) ws_backend=true ;;
    src/* | public/* | index.html | vite.config.ts | tsconfig.json | tsconfig.node.json) ws_frontend=true ;;
    # package.json also carries the Tauri CLI, and `tauri.conf.json` runs
    # `pnpm build` — a dependency change can affect both sides.
    package.json | pnpm-lock.yaml) ws_frontend=true; ws_backend=true ;;
    # Changes every job's meaning: the workflow itself.
    .github/workflows/ci.yml) global=true ;;
    # Automation helpers: tested by the `automation` job, which always runs.
    scripts/* | .github/scripts/*) : ;;
    # Inert for workspace builds; automation still verifies these files.
    *.md | docs/* | .github/* | .claude/* | .agents/* | .opencode/* | .vscode/* | \
      .gitignore | .gitattributes | .env.example | LICENSE*) : ;;
    # Fail open. Anything unrecognised runs everything; the fix for the cost is
    # to add a rule here WITH a test, never to guess from the workflow.
    *) global=true ;;
  esac
done

if [ "$saw_any" = false ]; then
  echo "changed-areas: empty input — failing open, all jobs run" >&2
  ws_backend=true ws_frontend=true global=true
fi

flag() { # flag <name> <cond>... — true if ANY listed bucket is true
  local name="$1" value=false bucket
  shift
  for bucket in "$@"; do
    [ "$bucket" = true ] && value=true
  done
  printf '%s=%s\n' "$name" "$value"
}

echo "changed-areas: ws_backend=${ws_backend} ws_frontend=${ws_frontend} global=${global}" >&2

flag backend "$ws_backend" "$global"
flag frontend "$ws_frontend" "$global"
