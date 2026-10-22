#!/usr/bin/env bash
# Evaluate the tiered CI matrix after every workload job has reached a terminal
# result. GitHub branch protection and READY TO MERGE require this one stable
# check; branch-specific policy stays here instead of being duplicated in settings.
#
#   * non-main bases: selectors come from the diff classifier. A selected job must
#     succeed, an unselected one must be skipped.
#   * main: a release promotion validates everything. Every selector must be true
#     and every job must succeed — a false selector narrows release validation.
#
# The automation job answers to detect's classifier-independent `helpers` selector.

set -uo pipefail

fail=0

reject() {
  echo "::error::$*" >&2
  fail=1
}

expect_exact() {
  local label="$1" actual="$2" expected="$3"
  if [ "$actual" != "$expected" ]; then
    reject "${label}: expected ${expected}, got ${actual:-<empty>}"
  else
    echo "[PASS] ${label}: ${actual}"
  fi
}

expect_selected() {
  local label="$1" selected="$2" result="$3"
  case "$selected" in
    true) expect_exact "$label" "$result" success ;;
    false) expect_exact "$label" "$result" skipped ;;
    *) reject "${label}: selector must be true or false, got ${selected:-<empty>}" ;;
  esac
}

base_ref="${BASE_REF:-}"
detect_result="${DETECT_RESULT:-}"
backend_result="${BACKEND_RESULT:-}"
frontend_result="${FRONTEND_RESULT:-}"
automation_result="${AUTOMATION_RESULT:-}"

expect_exact "detect" "$detect_result" success

case "$base_ref" in
  main)
    # Checking both the selectors and the results catches a future workflow
    # edit that silently narrows production validation while all executed jobs
    # remain green.
    expect_exact "backend selector" "${BACKEND_SELECTED:-}" true
    expect_exact "backend" "$backend_result" success
    expect_exact "frontend selector" "${FRONTEND_SELECTED:-}" true
    expect_exact "frontend" "$frontend_result" success
    expect_exact "helper-scope selector" "${HELPERS_SELECTED:-}" true
    expect_exact "automation" "$automation_result" success
    ;;
  "") reject "BASE_REF must be a non-empty branch name" ;;
  *)
    expect_selected "backend" "${BACKEND_SELECTED:-}" "$backend_result"
    expect_selected "frontend" "${FRONTEND_SELECTED:-}" "$frontend_result"
    expect_selected "automation" "${HELPERS_SELECTED:-}" "$automation_result"
    ;;
esac

if [ "$fail" -ne 0 ]; then
  echo "CI gate rejected the ${base_ref:-unknown} matrix" >&2
  exit 1
fi

echo "CI gate accepted the ${base_ref} matrix"
