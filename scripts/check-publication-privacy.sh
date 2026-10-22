#!/usr/bin/env bash
# Reject tracked content that would expose private identity or workstation data.
# Diagnostics name only a location and category; the rejected value is never printed.
#
# This is where the three categories were first defined, and every other privacy check
# reads them: check-commit-privacy.sh for commit identities and messages,
# check-body-privacy.sh for issue and pull-request bodies. Each holds its own copy and
# scripts/test/commit-privacy.test.sh pins the set to each other, so widening a pattern
# here without widening it there fails until they agree. Changing anything below is
# therefore a change to every surface at once — which is the intent.
#
# What is scanned is what a push publishes, never the working tree:
#
#   (no arguments)            the index — the tree the next commit or push carries;
#   <base> <head>             the index, and then every commit in <base>..<head>, one
#                             tree at a time. A value added in one commit and removed in a
#                             later one leaves a clean tip and is still published with the
#                             history; only a per-commit scan can see it;
#   --all-history             every commit reachable from any ref. Run it once before the
#                             repository's visibility changes.
#
# A finding inside a commit is reported as `<short-sha> <path>:<line>`.

set -euo pipefail

REPO_ROOT=$(git rev-parse --show-toplevel)
cd "$REPO_ROOT"

commits=()
case "$#" in
  0) ;;
  1)
    if [ "$1" != "--all-history" ]; then
      echo "usage: check-publication-privacy.sh [<base> <head> | --all-history]" >&2
      exit 2
    fi
    mapfile -t commits < <(git rev-list --all)
    ;;
  2)
    for rev in "$1" "$2"; do
      if ! git rev-parse --verify --quiet "${rev}^{commit}" >/dev/null; then
        echo "publication privacy: cannot resolve commit range endpoint" >&2
        exit 2
      fi
    done
    mapfile -t commits < <(git rev-list "$1..$2")
    ;;
  *)
    echo "usage: check-publication-privacy.sh [<base> <head> | --all-history]" >&2
    exit 2
    ;;
esac

violations=0

is_approved_email() {
  local email=${1,,}
  case "$email" in
    # noreply@anthropic.com is the co-author trailer an agent's harness appends. It names
    # no person, it is a vendor no-reply that is already public, and much of develop's
    # history already carries it — so refusing it would redden a whole class of legitimate
    # commits while protecting nothing, and a check that fires on a non-leak is one people
    # learn to work around. No commit count is written here on purpose: it moves with every
    # merge, and a number nobody re-measures is how a claim about the world goes stale.
    # Approved by name and not as `noreply@*`: a no-reply under a private host would publish
    # an internal hostname, which is the thing these scans exist to catch. No example of one
    # is written here either, because the file scan reads this file too — the same problem
    # the assembled-from-pieces path prefixes below solve the same way.
    # HiDPI asset names (`128x128@2x.png`, as Tauri's icon set uses) have the shape of an
    # address whose domain is `2x.png`. They name a pixel density, not a mailbox.
    noreply@github.com|noreply@anthropic.com|*@users.noreply.github.com|*@example.invalid|*@example.com|*@example.org|*@example.net|*@[1-9]x.png|*@[1-9]x.jpg|*@[1-9]x.webp|*@[1-9]x.svg)
      return 0
      ;;
    *)
      return 1
      ;;
  esac
}

# A GitHub noreply address is resolved by the NUMBER in front of the plus, and by nothing
# else. `<id>+<login>@users.noreply.github.com` credits whoever owns `<id>`; the login
# beside it is decoration GitHub never reads. So an address naming the right person with
# the wrong number is well-formed, passes every shape test above — `*@users.noreply…`
# approves it — and silently credits a stranger's account for work they never did.
#
# It happened here three times, in co-author trailers a tool appended, and it was found
# the only way it could be: by a human noticing two accounts nobody recognised in the
# repository's contributor graph, one commit each, weeks later. Nothing was red. This is
# the same family as the generated session trailer — a machine-written value recurs
# *because* it is generated rather than because anybody was careless — and it earns a
# rule for the same reason.
#
# The table is the only claim about the world that can be checked without the network,
# and it is deliberately narrow: for a login it names, the id must be that login's; for a
# login it does not name, it says nothing. That is the exact shape of the observed defect
# — the right name beside the wrong number — and it is what keeps the rule from rejecting
# the synthetic identities the privacy tests are built from. It does not attempt to
# enumerate who may appear; that is the approved-identity rule in AGENTS.md, enforced by
# a reader rather than by a pattern.
github_noreply_ids=(
  "fabiosm46:124870035"
  "github-actions[bot]:41898282"
)

# The id group is optional because the legacy form `<login>@users.noreply.github.com`
# carries no number and still resolves correctly — most of this repository's history uses
# it. A plus with nothing before it is a different thing: it resolves to nobody, and it is
# rejected whatever login follows, because an address that credits no account is a defect
# on any name.
github_noreply_pattern='^(([0-9]*)\+)?([^@+]+)@users\.noreply\.github\.com$'

# Extracted separately from the address pattern above, and this is not a refinement — it is
# the difference between the table having two entries and having one. `email_pattern` has no
# bracket in its local-part class, so an address whose login carries one is not merely
# matched imprecisely, it is never extracted at all: the bot login this repository publishes
# alongside the handle was invisible to every message-driven scan, and only the author and
# committer fields, which are read whole and never grepped, could see it. Found in review on
# #797, on the change that introduced the table.
#
# **It starts at the id, and that is the whole of what keeps it honest.** The first version
# started at "anything that is not whitespace or an angle bracket", which reads as a harmless
# widening and is not: `grep -Eo` takes the leftmost-longest run, so an address in parentheses
# or after a comma is extracted *with* the punctuation, the anchored rule below cannot parse
# what it is handed, and the address passes. Not a truncated diagnostic — a silent miss, in
# the same direction as the defect the table exists to catch, found in review on #800.
#
# Requiring the plus costs nothing the rule uses. An address with no id is the legacy form,
# about which the rule says nothing anyway, so declining to extract it removes only work; the
# id-less plus is still reached, because `[0-9]*` matches empty. What it removes is every
# character the match could have swallowed on its left.
github_noreply_extract_pattern='[0-9]*\+[^@[:space:]<>]+@users\.noreply\.github\.com'

is_misattributed_noreply() {
  local email=${1,,} plus prefix login entry
  [[ "$email" =~ $github_noreply_pattern ]] || return 1
  plus=${BASH_REMATCH[1]}
  prefix=${BASH_REMATCH[2]}
  login=${BASH_REMATCH[3]}
  if [ -n "$plus" ] && [ -z "$prefix" ]; then
    return 0
  fi
  for entry in "${github_noreply_ids[@]}"; do
    [ "${entry%:*}" = "$login" ] || continue
    [ -z "$prefix" ] && return 1
    [ "$prefix" = "${entry##*:}" ] && return 1
    return 0
  done
  return 1
}

email_pattern='[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}'

slash=/
backslash='\'
private_path_prefixes=(
  "${slash}home${slash}"
  "${slash}Users${slash}"
  "${slash}workspace${slash}"
  "${slash}workspaces${slash}"
  "C:${backslash}Users${backslash}"
)


private_network_pattern='(^|[^0-9])(10\.[0-9]{1,3}\.[0-9]{1,3}\.[0-9]{1,3}|192\.168\.[0-9]{1,3}\.[0-9]{1,3}|172\.(1[6-9]|2[0-9]|3[01])\.[0-9]{1,3}\.[0-9]{1,3})([^0-9]|$)'

# report <where> <path> <line> <category> — where is empty for the index, a short sha for
# a commit. Only the location and the category are printed, never the value.
report() {
  local where="$1" path="$2" line="$3" category="$4"
  echo "publication privacy: ${where:+${where} }${path}:${line} ${category}" >&2
  violations=$((violations + 1))
}

# grep_scannable <git-grep-args>... — exit 1 from git grep means "no match"; anything
# above it is a failed scan, and a failed scan must never read as a clean tree. Called
# through command substitution, so `exit` here ends the substitution and `set -e` ends
# the script with the same status.
grep_scannable() {
  local out rc=0
  out=$(git grep "${grep_opts[@]}" "$@" "${grep_rev[@]}" -- "${scannable_files[@]}") || rc=$?
  if [ "$rc" -gt 1 ]; then
    echo "publication privacy: git grep failed (exit ${rc}); refusing to report a clean scan" >&2
    exit 2
  fi
  printf '%s\n' "$out"
}

# scan_target <target> — target is `--cached` (the index) or a commit sha.
scan_target() {
  local target="$1" where="" prefix_out matches path basename match remainder line content email noreply prefix
  local -a files=() scannable_files=()
  if [ "$target" = "--cached" ]; then
    mapfile -d '' files < <(git ls-files -z)
    if [ "${#files[@]}" -eq 0 ]; then
      echo "publication privacy: no tracked files found" >&2
      exit 2
    fi
  else
    where=$(git rev-parse --short "$target")
    mapfile -d '' files < <(git ls-tree -r -z --name-only "$target")
    [ "${#files[@]}" -gt 0 ] || return 0
  fi
  # `git grep <sha>` prefixes every match with `<sha>:`; the index prints none.
  local -a grep_opts=() grep_rev=()
  prefix_out=""
  if [ "$target" = "--cached" ]; then
    grep_opts=(--cached)
  else
    grep_rev=("$target")
    prefix_out="${target}:"
  fi

  for path in "${files[@]}"; do
    basename=${path##*/}
    case "$basename" in
      .env)
        report "$where" "$path" "-" "is a tracked secret-bearing environment file"
        ;;
      .env.*)
        if [ "$basename" = ".env.example" ]; then
          scannable_files+=("$path")
        else
          report "$where" "$path" "-" "is a tracked secret-bearing environment file"
        fi
        ;;
      *)
        scannable_files+=("$path")
        ;;
    esac
  done
  [ "${#scannable_files[@]}" -gt 0 ] || return 0

  matches=$(grep_scannable -I -n -E "$email_pattern")
  while IFS= read -r match; do
    [ -n "$match" ] || continue
    match=${match#"$prefix_out"}
    path=${match%%:*}
    remainder=${match#*:}
    line=${remainder%%:*}
    content=${remainder#*:}
    while IFS= read -r email; do
      if ! is_approved_email "$email"; then
        report "$where" "$path" "$line" "contains a non-public email address"
      fi
    done < <(grep -Eo "$email_pattern" <<< "$content" || true)
  done <<< "$matches"

  # Its own pass, for the reason given beside the pattern: a login carrying a bracket is not
  # extracted by the address pattern above, so a second test inside that loop would have been
  # reached only by the addresses that were never the problem.
  matches=$(grep_scannable -I -n -E "$github_noreply_extract_pattern")
  while IFS= read -r match; do
    [ -n "$match" ] || continue
    match=${match#"$prefix_out"}
    path=${match%%:*}
    remainder=${match#*:}
    line=${remainder%%:*}
    content=${remainder#*:}
    while IFS= read -r noreply; do
      if is_misattributed_noreply "$noreply"; then
        report "$where" "$path" "$line" "contains a misattributed GitHub account id"
      fi
    done < <(grep -Eo "$github_noreply_extract_pattern" <<< "$content" || true)
  done <<< "$matches"

  for prefix in "${private_path_prefixes[@]}"; do
    matches=$(grep_scannable -I -n -F "$prefix")
    while IFS= read -r match; do
      [ -n "$match" ] || continue
      match=${match#"$prefix_out"}
      path=${match%%:*}
      remainder=${match#*:}
      line=${remainder%%:*}
      report "$where" "$path" "$line" "contains a workstation-specific path"
    done <<< "$matches"
  done

  matches=$(grep_scannable -I -n -E "$private_network_pattern")
  while IFS= read -r match; do
    [ -n "$match" ] || continue
    match=${match#"$prefix_out"}
    path=${match%%:*}
    remainder=${match#*:}
    line=${remainder%%:*}
    report "$where" "$path" "$line" "contains a private-network address"
  done <<< "$matches"
}

scan_target --cached
for commit in "${commits[@]}"; do
  scan_target "$commit"
done

if [ "$violations" -ne 0 ]; then
  echo "publication privacy: rejected ${violations} private-data occurrence(s)" >&2
  exit 1
fi

if [ "${#commits[@]}" -gt 0 ]; then
  echo "publication privacy — the index and ${#commits[@]} commit(s) contain only approved public identities and synthetic paths"
else
  echo "publication privacy — tracked content contains only approved public identities and synthetic paths"
fi
