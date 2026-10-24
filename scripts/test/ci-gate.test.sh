#!/usr/bin/env bash
# Deterministic result-matrix tests for scripts/ci-gate.sh.
#
# Pinned hardest: on `main` every selector must be true and every job must
# succeed — a false selector is a silently narrowed release matrix.

set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
GATE="${ROOT}/scripts/ci-gate.sh"
pass=0
fail=0

run_case() {
  local name="$1" expected="$2"
  shift 2
  local output actual
  output=$(env -i PATH="$PATH" "$@" bash "$GATE" 2>&1)
  actual=$?
  if [ "$actual" -eq "$expected" ]; then
    echo "  ok   — ${name}"
    pass=$((pass + 1))
  else
    echo "  FAIL — ${name}: expected exit ${expected}, got ${actual}"
    echo "$output" | sed 's/^/         /'
    fail=$((fail + 1))
  fi
}

non_main_common=(
  BASE_REF=develop DETECT_RESULT=success
  BACKEND_SELECTED=true BACKEND_RESULT=success BACKEND_WINDOWS_RESULT=success
  MACOS_SELECTED=false BACKEND_MACOS_RESULT=skipped
  FRONTEND_SELECTED=false FRONTEND_RESULT=skipped
  HELPERS_SELECTED=false AUTOMATION_RESULT=skipped
)

main_common=(
  BASE_REF=main DETECT_RESULT=success
  BACKEND_SELECTED=true BACKEND_RESULT=success BACKEND_WINDOWS_RESULT=success
  MACOS_SELECTED=true BACKEND_MACOS_RESULT=success
  FRONTEND_SELECTED=true FRONTEND_RESULT=success
  HELPERS_SELECTED=true AUTOMATION_RESULT=success
)

echo "ci-gate — non-main policy"
run_case "develop accepts selected work while unrelated jobs skip" 0 "${non_main_common[@]}"
run_case "a feature base uses the same selected-work policy" 0 \
  "${non_main_common[@]/BASE_REF=develop/BASE_REF=feature/parent-branch}"
run_case "docs-only PR accepts an entirely skipped workload" 0 \
  BASE_REF=develop DETECT_RESULT=success \
  BACKEND_SELECTED=false BACKEND_RESULT=skipped BACKEND_WINDOWS_RESULT=skipped \
  MACOS_SELECTED=false BACKEND_MACOS_RESULT=skipped \
  FRONTEND_SELECTED=false FRONTEND_RESULT=skipped \
  HELPERS_SELECTED=false AUTOMATION_RESULT=skipped
run_case "selected job may not skip" 1 "${non_main_common[@]/BACKEND_RESULT=success/BACKEND_RESULT=skipped}"
run_case "unselected job may not disappear" 1 "${non_main_common[@]/FRONTEND_RESULT=skipped/FRONTEND_RESULT=missing}"
run_case "the Windows backend job may not skip when backend is selected" 1 \
  "${non_main_common[@]/BACKEND_WINDOWS_RESULT=success/BACKEND_WINDOWS_RESULT=skipped}"
run_case "a failed Windows backend job fails the gate" 1 \
  "${non_main_common[@]/BACKEND_WINDOWS_RESULT=success/BACKEND_WINDOWS_RESULT=failure}"
run_case "a missing Windows backend result fails closed" 1 \
  "${non_main_common[@]/BACKEND_WINDOWS_RESULT=success/BACKEND_WINDOWS_RESULT=}"
run_case "cancelled selected job fails closed" 1 "${non_main_common[@]/BACKEND_RESULT=success/BACKEND_RESULT=cancelled}"
run_case "failed detect cannot be hidden" 1 "${non_main_common[@]/DETECT_RESULT=success/DETECT_RESULT=failure}"
run_case "missing selector fails closed" 1 "${non_main_common[@]/FRONTEND_SELECTED=false/FRONTEND_SELECTED=}"
run_case "malformed selector is not read as false" 1 "${non_main_common[@]/FRONTEND_SELECTED=false/FRONTEND_SELECTED=maybe}"
run_case "an empty base still fails closed" 1 "${non_main_common[@]/BASE_REF=develop/BASE_REF=}"

echo
echo "ci-gate — the macOS backend job has its own selector (D29)"
run_case "an unselected macOS job is an authorized skip" 0 "${non_main_common[@]}"
macos_on=("${non_main_common[@]/MACOS_SELECTED=false/MACOS_SELECTED=true}")
run_case "a selected macOS job may not skip" 1 "${macos_on[@]}"
run_case "a selected macOS job that succeeds passes" 0 \
  "${macos_on[@]/BACKEND_MACOS_RESULT=skipped/BACKEND_MACOS_RESULT=success}"
run_case "a failed macOS job fails the gate" 1 \
  "${macos_on[@]/BACKEND_MACOS_RESULT=skipped/BACKEND_MACOS_RESULT=failure}"
run_case "an unselected macOS job may not run anyway" 1 \
  "${non_main_common[@]/BACKEND_MACOS_RESULT=skipped/BACKEND_MACOS_RESULT=success}"
run_case "a missing macOS result fails closed" 1 \
  "${non_main_common[@]/BACKEND_MACOS_RESULT=skipped/BACKEND_MACOS_RESULT=}"
run_case "a missing macOS selector fails closed" 1 \
  "${non_main_common[@]/MACOS_SELECTED=false/MACOS_SELECTED=}"
run_case "a malformed macOS selector is not read as false" 1 \
  "${non_main_common[@]/MACOS_SELECTED=false/MACOS_SELECTED=maybe}"

echo
echo "ci-gate — the helper-scope selector the classifier cannot compute for itself"
helpers_only=("${non_main_common[@]/HELPERS_SELECTED=false/HELPERS_SELECTED=true}")
run_case "helper-scope diffs cannot skip the job hosting the helper tests" 1 \
  "${helpers_only[@]}"
run_case "helper scope alone is enough for automation to have run" 0 \
  "${helpers_only[@]/AUTOMATION_RESULT=skipped/AUTOMATION_RESULT=success}"
run_case "missing helper-scope selector fails closed" 1 \
  "${non_main_common[@]/HELPERS_SELECTED=false/HELPERS_SELECTED=}"
run_case "a malformed helper-scope selector is not read as false" 1 \
  "${non_main_common[@]/HELPERS_SELECTED=false/HELPERS_SELECTED=maybe}"

echo
echo "ci-gate — main release policy validates everything"
run_case "complete release matrix succeeds" 0 "${main_common[@]}"
run_case "release job may not skip" 1 \
  "${main_common[@]/BACKEND_RESULT=success/BACKEND_RESULT=skipped}"
narrowed=("${main_common[@]/FRONTEND_SELECTED=true/FRONTEND_SELECTED=false}")
narrowed=("${narrowed[@]/FRONTEND_RESULT=success/FRONTEND_RESULT=skipped}")
run_case "release selector may not narrow the matrix" 1 "${narrowed[@]}"
run_case "release helper scope may not narrow the matrix" 1 \
  "${main_common[@]/HELPERS_SELECTED=true/HELPERS_SELECTED=false}"
run_case "release Windows backend may not skip" 1 \
  "${main_common[@]/BACKEND_WINDOWS_RESULT=success/BACKEND_WINDOWS_RESULT=skipped}"
run_case "release macOS backend may not skip" 1 \
  "${main_common[@]/BACKEND_MACOS_RESULT=success/BACKEND_MACOS_RESULT=skipped}"
run_case "release macOS selector may not narrow the matrix" 1 \
  "${main_common[@]/MACOS_SELECTED=true/MACOS_SELECTED=false}"
run_case "a failed release macOS backend fails the gate" 1 \
  "${main_common[@]/BACKEND_MACOS_RESULT=success/BACKEND_MACOS_RESULT=failure}"
run_case "release automation may not skip" 1 \
  "${main_common[@]/AUTOMATION_RESULT=success/AUTOMATION_RESULT=skipped}"
run_case "pending release job fails closed" 1 \
  "${main_common[@]/FRONTEND_RESULT=success/FRONTEND_RESULT=pending}"

echo
echo "ci-gate — ci.yml wires every workload job into the gate"
# The matrix above tests the script; this reads the workflow. A job missing from
# `ci_gate.needs`, or without its result in the gate's environment, is never audited.
if structure=$(python3 - "${ROOT}/.github/workflows/ci.yml" 2>&1 <<'CHECK'
import re
import sys

text = open(sys.argv[1]).read()
jobs_block = text.split("\njobs:\n", 1)[1]
jobs = re.findall(r"^  ([A-Za-z0-9_-]+):\s*$", jobs_block, re.MULTILINE)
assert "ci_gate" in jobs, "ci_gate job not found"
workload = [job for job in jobs if job != "ci_gate"]
gate = jobs_block.split("\n  ci_gate:\n", 1)[1]
needs = re.search(r"^    needs: \[([^\]]*)\]", gate, re.MULTILINE)
assert needs, "ci_gate has no needs list"
listed = [item.strip() for item in needs.group(1).split(",")]
assert sorted(listed) == sorted(workload), f"ci_gate.needs {listed} != jobs {workload}"
for job in workload:
    assert "${{ needs.%s.result }}" % job in gate, f"ci_gate env does not read {job}.result"

macos = jobs_block.split("\n  backend-macos:\n", 1)[1]
macos = macos[: macos.find("\n\n")]
assert "runs-on: macos-latest" in macos, "backend-macos must run on macos-latest"
assert "needs.detect.outputs.macos != 'false'" in macos, "backend-macos must use the macos selector"
assert "macos: ${{ steps.classify.outputs.macos }}" in text, "detect must expose the macos selector"
assert text.count("macos=true") >= 2, "main and the fail-open branch must both set macos=true"
assert "MACOS_SELECTED: ${{ needs.detect.outputs.macos }}" in gate, "ci_gate must read the macos selector"
print(f"{len(workload)} workload jobs wired, backend-macos on its own selector")
CHECK
); then
  echo "  ok   — ${structure}"
  pass=$((pass + 1))
else
  echo "  FAIL — ci.yml structure"
  echo "$structure" | sed 's/^/         /'
  fail=$((fail + 1))
fi

echo
echo "${pass} passed, ${fail} failed"
[ "$fail" -eq 0 ]
