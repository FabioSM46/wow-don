#!/usr/bin/env bash

set -euo pipefail

REPO_ROOT=$(git rev-parse --show-toplevel)
CHECK="$REPO_ROOT/scripts/check-publication-privacy.sh"
TEST_ROOT=$(mktemp -d)
trap 'rm -rf -- "$TEST_ROOT"' EXIT

mkdir -p "$TEST_ROOT/scripts"
cp "$CHECK" "$TEST_ROOT/scripts/check-publication-privacy.sh"
git -C "$TEST_ROOT" init -q -b develop

printf '%s\n' \
  'commit=1000+privacy@users.noreply.github.com' \
  'fixture=private-address@example.invalid' \
  'path=<workspace>/wow-don' \
  'icon=icons/128x128@2x.png' > "$TEST_ROOT/safe.txt"
git -C "$TEST_ROOT" add scripts/check-publication-privacy.sh safe.txt
(cd "$TEST_ROOT" && bash scripts/check-publication-privacy.sh >/dev/null)

unsafe_email="person""@""private.test"
printf 'contact=%s\n' "$unsafe_email" > "$TEST_ROOT/contact.txt"
git -C "$TEST_ROOT" add contact.txt
set +e
email_output=$(cd "$TEST_ROOT" && bash scripts/check-publication-privacy.sh 2>&1)
email_status=$?
set -e
if [ "$email_status" -ne 1 ]; then
  echo "expected a private email fixture to exit 1, got $email_status" >&2
  exit 1
fi
grep -Fq 'contact.txt:1 contains a non-public email address' <<< "$email_output"
if grep -Fq "$unsafe_email" <<< "$email_output"; then
  echo "privacy failure: the rejected email reached the diagnostic" >&2
  exit 1
fi

# A HiDPI asset name is approved by its density suffix only, not by its extension.
git -C "$TEST_ROOT" rm -f -q contact.txt
printf 'asset=%s\n' "icon""@""private.png" > "$TEST_ROOT/asset.txt"
git -C "$TEST_ROOT" add asset.txt
set +e
(cd "$TEST_ROOT" && bash scripts/check-publication-privacy.sh >/dev/null 2>&1)
asset_status=$?
set -e
if [ "$asset_status" -ne 1 ]; then
  echo "expected a non-density @*.png name to exit 1, got $asset_status" >&2
  exit 1
fi

git -C "$TEST_ROOT" rm -f -q asset.txt
internal_path="/""home/private/worktree"
printf 'checkout=%s\n' "$internal_path" > "$TEST_ROOT/path.txt"
git -C "$TEST_ROOT" add path.txt
set +e
path_output=$(cd "$TEST_ROOT" && bash scripts/check-publication-privacy.sh 2>&1)
path_status=$?
set -e
if [ "$path_status" -ne 1 ]; then
  echo "expected an internal path fixture to exit 1, got $path_status" >&2
  exit 1
fi
grep -Fq 'path.txt:1 contains a workstation-specific path' <<< "$path_output"
if grep -Fq "$internal_path" <<< "$path_output"; then
  echo "privacy failure: the rejected path reached the diagnostic" >&2
  exit 1
fi

git -C "$TEST_ROOT" rm -f -q path.txt
# The number in front of the plus is the whole identity: this address names the project's
# approved handle and credits somebody else. Nothing above can see it — it is a GitHub
# noreply address, which the approved-email list allows by shape — and that is the point.
wrong_id="9999""999999999"
printf 'Co-authored-by: FabioSM46 <%s+FabioSM46@''users.noreply.github.com>\n' "$wrong_id" \
  > "$TEST_ROOT/trailer.txt"
git -C "$TEST_ROOT" add trailer.txt
set +e
id_output=$(cd "$TEST_ROOT" && bash scripts/check-publication-privacy.sh 2>&1)
id_status=$?
set -e
if [ "$id_status" -ne 1 ]; then
  echo "expected a misattributed account id to exit 1, got $id_status" >&2
  exit 1
fi
grep -Fq 'trailer.txt:1 contains a misattributed GitHub account id' <<< "$id_output"
if grep -Fq "$wrong_id" <<< "$id_output"; then
  echo "privacy failure: the rejected account id reached the diagnostic" >&2
  exit 1
fi

# The bot login carries a bracket, which `email_pattern` cannot extract — so this file is
# invisible to the address scan and only the rule's own pass can refuse it.
git -C "$TEST_ROOT" rm -f -q trailer.txt
bracket_login="github-""actions[bot]"
printf 'Co-authored-by: %s <%s+%s@''users.noreply.github.com>\n' \
  "$bracket_login" "$wrong_id" "$bracket_login" > "$TEST_ROOT/bracket.txt"
git -C "$TEST_ROOT" add bracket.txt
set +e
bracket_output=$(cd "$TEST_ROOT" && bash scripts/check-publication-privacy.sh 2>&1)
bracket_status=$?
set -e
if [ "$bracket_status" -ne 1 ]; then
  echo "expected a bracketed login with another account's id to exit 1, got $bracket_status" >&2
  exit 1
fi
grep -Fq 'bracket.txt:1 contains a misattributed GitHub account id' <<< "$bracket_output"
if grep -Fq "$wrong_id" <<< "$bracket_output"; then
  echo "privacy failure: the rejected account id reached the diagnostic" >&2
  exit 1
fi
git -C "$TEST_ROOT" rm -f -q bracket.txt
git -C "$TEST_ROOT" checkout -q -- . 2>/dev/null || true
printf 'Co-authored-by: FabioSM46 <%s+FabioSM46@''users.noreply.github.com>\n' "$wrong_id" \
  > "$TEST_ROOT/trailer.txt"
git -C "$TEST_ROOT" add trailer.txt

# The same handle with its own id passes, which is what keeps the rule from being a ban on
# the address rather than a check of the number in it.
git -C "$TEST_ROOT" rm -f -q trailer.txt
printf 'Co-authored-by: FabioSM46 <124870035+FabioSM46@users.noreply.github.com>\n' \
  > "$TEST_ROOT/correct.txt"
git -C "$TEST_ROOT" add correct.txt
(cd "$TEST_ROOT" && bash scripts/check-publication-privacy.sh >/dev/null)


# ── What a push publishes, not what the working tree shows ───────────────────────────
commit_fixture() {
  git -C "$TEST_ROOT" -c user.name=fixture -c user.email=fixture@example.invalid \
    commit -q --no-verify -m "$1"
}
expect_rejection() {
  local label="$1" expected_line="$2"
  shift 2
  set +e
  local out status
  out=$(cd "$TEST_ROOT" && bash scripts/check-publication-privacy.sh "$@" 2>&1)
  status=$?
  set -e
  if [ "$status" -ne 1 ]; then
    echo "expected ${label} to exit 1, got $status" >&2
    exit 1
  fi
  grep -Fq "$expected_line" <<< "$out" || { echo "missing diagnostic for ${label}" >&2; exit 1; }
  if grep -Fq "$unsafe_email" <<< "$out"; then
    echo "privacy failure: the rejected email reached the diagnostic (${label})" >&2
    exit 1
  fi
}

commit_fixture "clean base"
base_sha=$(git -C "$TEST_ROOT" rev-parse HEAD)

# Committed, then deleted from the working tree without staging the deletion: the push
# still carries the blob, so the scan must still see it.
printf 'contact=%s\n' "$unsafe_email" > "$TEST_ROOT/staged.txt"
git -C "$TEST_ROOT" add staged.txt
commit_fixture "add contact"
rm "$TEST_ROOT/staged.txt"
expect_rejection "a committed value missing from the working tree" \
  'staged.txt:1 contains a non-public email address'

# Added in one commit and removed in the next: the tip is clean, the history is not.
git -C "$TEST_ROOT" rm -q --cached staged.txt
commit_fixture "remove contact"
(cd "$TEST_ROOT" && bash scripts/check-publication-privacy.sh >/dev/null)
added_sha=$(git -C "$TEST_ROOT" rev-parse --short HEAD~1)
expect_rejection "a value added and removed inside the range" \
  "${added_sha} staged.txt:1 contains a non-public email address" "$base_sha" HEAD
expect_rejection "the same value across the whole history" \
  "${added_sha} staged.txt:1 contains a non-public email address" --all-history

set +e
(cd "$TEST_ROOT" && bash scripts/check-publication-privacy.sh "$base_sha" >/dev/null 2>&1)
usage_status=$?
(cd "$TEST_ROOT" && bash scripts/check-publication-privacy.sh "$base_sha" no-such-ref >/dev/null 2>&1)
range_status=$?
set -e
[ "$usage_status" -eq 2 ] || { echo "expected a lone range endpoint to exit 2, got $usage_status" >&2; exit 1; }
[ "$range_status" -eq 2 ] || { echo "expected an unresolvable endpoint to exit 2, got $range_status" >&2; exit 1; }

# A scan that could not run is not a clean scan. Stub `git grep` to fail the way a bad
# argument does and require a non-zero exit instead of a green line.
STUB_DIR="$TEST_ROOT/stub-bin"
mkdir -p "$STUB_DIR"
REAL_GIT=$(command -v git)
cat > "$STUB_DIR/git" <<STUB
#!/usr/bin/env bash
[ "\$1" = grep ] && { echo "fatal: stubbed grep failure" >&2; exit 128; }
exec "$REAL_GIT" "\$@"
STUB
chmod +x "$STUB_DIR/git"
set +e
grep_out=$(cd "$TEST_ROOT" && PATH="$STUB_DIR:$PATH" bash scripts/check-publication-privacy.sh 2>&1)
grep_status=$?
set -e
if [ "$grep_status" -ne 2 ]; then
  echo "expected a failed git grep to exit 2, got $grep_status" >&2
  exit 1
fi
if grep -Fq 'contain only approved public identities' <<< "$grep_out"; then
  echo "a failed git grep still printed the clean verdict" >&2
  exit 1
fi

echo "publication privacy — safe placeholders pass, private values fail without leaking, and a noreply address carrying another account's id is refused; the index and every commit in a range are scanned, and a failed scan fails closed"
