---
name: process-pr
description: Use ONLY when the user explicitly requests /process-pr. Manages PR feedback loops. Use to resolve CI failures and review feedback on open PRs.
compatibility: opencode
metadata:
  opencode/autoinvoke: "false"
---


# process-pr — fixing a PR

Trigger: `/process-pr <pr-number>` or `/process-pr` (uses the current branch's PR).

## Purpose

A fixing loop for conflicts, red CI and review feedback. The user or an orchestrator can start
it.

The passive path (`pr-labeler.yml`) covers the normal case. This skill is for when you want a
result now.

**All the work happens in a `git worktree`**, never in the main folder. If a worktree already
exists for the PR's branch (from a `/dev-issue` or `/process-pr` that was not cleaned up), it is
reused.

## Real timings — read before touching the polling

The DeepSeek review job has a budget of `DEEPSEEK_REQUEST_TIMEOUT_SECONDS` (2700) ×
(`DEEPSEEK_MAX_RETRIES` (1) + 1) = 90 minutes, under a `timeout-minutes: 100` with 10 minutes
reserved for setup and publishing. Real timings depend on the size of the diff; the diff limit
is 90,000 characters, and a PR close to the limit can use a good part of the budget.
**A wait shorter than the budget does not "check": it expires and reports a stale state.**
`DEEPSEEK_WAIT_SECONDS` below is 5700 (95 minutes), covers the budget and ends before the job
cap; do not lower it without changing the workflow first.

The round budget is `MAX_ROUNDS: "1"` (in `.github/workflows/deepseek-pr-review.yml`). Only one
full review (Mode A) is automatic; thread replies (Mode B) do not consume budget and continue
without limit. So the normal run of this skill is **a single pass**.

`pull_request` runs use the merge ref, so the workflow that runs is the **base branch**'s copy.
To diagnose the review's behaviour, read the version of `deepseek-pr-review.yml` on the PR's
real base.

## Workflow

### Step 0: common setup

```bash
set -euo pipefail
REPO=$(gh repo view --json nameWithOwner --jq '.nameWithOwner')
REPO_ROOT=$(git rev-parse --show-toplevel)
DEEPSEEK_WAIT_SECONDS=5700
POLL_INTERVAL=30
```

### Step 1: PR number

If the user gave it, use it. Otherwise look up the current branch's PR:

```bash
gh pr list --head "$(git branch --show-current)" --json number --jq '.[0].number'
```

If there is no PR, exit with an error.

### Step 2: create or reuse the worktree and detect the areas

```bash
PR_META=$(gh pr view <pr-number> --json headRefName,baseRefName,state)
BRANCH=$(echo "$PR_META" | jq -er '.headRefName | select(length > 0)')
BASE_BRANCH=$(echo "$PR_META" | jq -er '.baseRefName | select(length > 0)')
[ "$(echo "$PR_META" | jq -r '.state')" = "OPEN" ] || exit 1
git fetch origin "$BRANCH" "$BASE_BRANCH"
REMOTE_HEAD=$(git rev-parse "origin/$BRANCH")

# Reuse a worktree already open for this branch.
# substr() and not $2: worktree paths can contain spaces.
WORKTREE_DIR=$(git worktree list --porcelain | awk -v b="refs/heads/$BRANCH" '
  /^worktree /{wt=substr($0,10)}
  /^branch /{if (substr($0,8)==b) {print wt; exit}}')

if [ -n "$WORKTREE_DIR" ]; then
  echo "Reusing existing worktree: $WORKTREE_DIR"
  cd "$WORKTREE_DIR"
  [ -z "$(git status --porcelain)" ] || {
    echo "Existing worktree is dirty; preserving it and stopping"
    exit 1
  }
  LOCAL_HEAD=$(git rev-parse HEAD)
  if [ "$LOCAL_HEAD" = "$REMOTE_HEAD" ]; then
    :
  elif git merge-base --is-ancestor "$LOCAL_HEAD" "$REMOTE_HEAD"; then
    git merge --ff-only "origin/$BRANCH"
  else
    echo "Existing worktree has local commits or diverged history; preserving it and stopping"
    exit 1
  fi
else
  WORKTREE_DIR="$(dirname "$REPO_ROOT")/wow-don-pr-<pr-number>"
  if git show-ref --verify --quiet "refs/heads/$BRANCH"; then
    [ "$(git rev-parse "refs/heads/$BRANCH")" = "$REMOTE_HEAD" ] || {
      echo "Local branch differs from origin; preserving it and stopping"
      exit 1
    }
    git worktree add "$WORKTREE_DIR" "$BRANCH"
  else
    git worktree add -b "$BRANCH" "$WORKTREE_DIR" "origin/$BRANCH"
  fi
  cd "$WORKTREE_DIR"
fi

pnpm install --frozen-lockfile
(cd src-tauri && cargo fetch --locked)

[ "$(git rev-parse --show-toplevel)" = "$WORKTREE_DIR" ] || { echo "NOT in worktree — abort"; exit 1; }
```

**Detect every area touched, not just the first.** The classification is the same as
`scripts/changed-areas.sh`, which is also CI's:

```bash
AREAS=$(gh pr view <pr-number> --json files --jq '.files[].path' | bash scripts/changed-areas.sh)
echo "$AREAS"   # backend=true|false, frontend=true|false
```

**Every PR runs the automation suite**, documentation-only ones included. The automation tests
read files from the whole repo, so no path selector exempts them. If a PR modifies a skill,
regenerate the adapters too.

**Gates — they mirror `.github/workflows/ci.yml`. Run one per detected area:**

Run each chain as it is and take its exit status as the verdict. Piping it to `head` or `tail`
returns the pager's success instead of the gate's; `/dev-issue` Step 6 explains why.

| Area | Command (from `$WORKTREE_DIR`) |
| ---- | ------------------------------ |
| `backend` | `cd src-tauri && cargo fmt --all --check && cargo clippy --all-targets --locked -- -D warnings && cargo build --locked && cargo test --locked` |
| `frontend` | `pnpm install --frozen-lockfile && pnpm build && pnpm test` |
| Every PR | the whole `scripts/test/*.test.sh` suite (with the glob, never rewriting the list) plus `python3 .github/scripts/test_deepseek_review.py`, with a failure flag so that a red test is the block's exit status, as in `/dev-issue` Step 6 |

Formatting is the most often skipped gate and the one that turns CI red most often.

#### Step 2b: catch up with the current base

Do it on every run. If the current tip of the base is not an ancestor of the PR's head, merge
the base into the head without rebase or force-push, resolve the conflicts, run all the gates of
the areas involved and push. This also invalidates stale CI on a technically mergeable child PR
after its feature base has moved ahead.

```bash
BASE_HEAD=$(git rev-parse "origin/$BASE_BRANCH")
if ! git merge-base --is-ancestor "$BASE_HEAD" HEAD; then
  [ "$(git rev-parse "origin/$BRANCH")" = "$REMOTE_HEAD" ] || exit 1
  git merge --no-ff --no-commit "origin/$BASE_BRANCH"
  # Resolve the conflicts; if the intent is ambiguous, abort and stop without publishing.
  # Run all the gates involved, then:
  git add -A
  git commit -m "fix: reconcile ${BASE_BRANCH} on PR #<pr-number>"
  bash scripts/check-publication-privacy.sh "origin/$BASE_BRANCH" HEAD
  bash scripts/check-commit-privacy.sh "origin/$BASE_BRANCH" HEAD
  git push origin HEAD
  REMOTE_HEAD=$(git rev-parse HEAD)
fi
```

After this push new CI is needed. The base's commits have already been reviewed in their own
PRs, so do not wait for an automatic review that cannot start with the budget exhausted.

### Step 3: PR state

```bash
bash scripts/gh-automation.sh pr-status <pr-number>
```

It evaluates the whole READY TO MERGE frozen rule and prints a `[FAIL]` line for every
condition not met. There are **seven** conditions, all necessary:

1. Unresolved (and not outdated) review threads = 0
2. No review requesting changes
3. No failed CI check
4. No CI check in progress
5. No *missing* required check: the stable `ci-gate` check must have run successfully
6. `mergeable == MERGEABLE`
7. DeepSeek has finished (clean verdict, rounds exhausted or `NO_DEEPSEEK_REVIEW` exemption),
   with no unread findings in a review body (they are closed with the `DEEPSEEK_REVIEW_READ`
   label)

Conditions 5 and 6 exist because **a conflicting PR runs no checks**: with nothing red, a naive
reading would call it green. A conflict is a fix to make, not a wait: Step 2b merges the current
base into the PR's head so that CI can start again.

The helper fails closed: an unreadable value counts as a failure, never as a success.

### Step 4: handle the DeepSeek review round (in the worktree)

With `MAX_ROUNDS: "1"` it is **a single pass**. Repeat 4a–4e only if `max_rounds` is raised in
the workflow or after an explicit `pr-deepseek-force-review`. Safety limit: **5 iterations**.

#### 4a — Read the DeepSeek state

```bash
ROUNDS=$(bash scripts/gh-automation.sh pr-deepseek-rounds <pr-number>) || true
if ! echo "$ROUNDS" | jq -e '.' >/dev/null 2>&1; then
  echo "ERROR: pr-deepseek-rounds returned invalid JSON. Raw output: ${ROUNDS:0:200}"
  exit 1
fi
# The helper reports configuration/API errors as {"error":...} with a non-zero exit,
# so a zero count always means "no review", never "read failed".
if echo "$ROUNDS" | jq -e '.error' >/dev/null 2>&1; then
  echo "ERROR: pr-deepseek-rounds failed: $(echo "$ROUNDS" | jq -r '.error')"
  exit 1
fi
REVIEW_COMPLETE=$(echo "$ROUNDS" | jq -r '.review_complete')
ROUNDS_EXHAUSTED=$(echo "$ROUNDS" | jq -r '.review_rounds_exhausted')
BOT_REVIEW_COUNT=$(echo "$ROUNDS" | jq -r '.bot_review_count')
PREV_REVIEW_ID=$(echo "$ROUNDS" | jq -r '.latest_review_id')
ROUND_CAP=$(echo "$ROUNDS" | jq -r '.max_rounds')
echo "DeepSeek state: rounds=$BOT_REVIEW_COUNT/$ROUND_CAP complete=$REVIEW_COMPLETE exhausted=$ROUNDS_EXHAUSTED"
```

**Guard**: if `review_complete=true` or `review_rounds_exhausted=true` there is no round to wait
for. Skip 4b and go to 4c: a round that has already arrived still has threads to handle.

#### 4b — Wait for the review (only if none has arrived)

Only when `PREV_REVIEW_ID` is `0`.

```bash
START_TIME=$(date +%s)
while true; do
  sleep "$POLL_INTERVAL"

  # The deadline check goes BEFORE any guard that can `continue`:
  # otherwise a helper that always fails would loop forever.
  ELAPSED=$(($(date +%s) - START_TIME))
  if [ "$ELAPSED" -ge "$DEEPSEEK_WAIT_SECONDS" ]; then
    echo "Timeout (${DEEPSEEK_WAIT_SECONDS}s) waiting for DeepSeek — the job may be approaching its own 100-minute cap."
    echo "Check: gh run list --workflow=deepseek-pr-review.yml --limit 3"
    break
  fi

  CURRENT=$(bash scripts/gh-automation.sh pr-deepseek-rounds <pr-number>) || true
  if ! echo "$CURRENT" | jq -e '.' >/dev/null 2>&1; then
    echo "WARNING: pr-deepseek-rounds returned invalid JSON, retrying... (${CURRENT:0:200})"
    continue
  fi
  if echo "$CURRENT" | jq -e '.error' >/dev/null 2>&1; then
    echo "WARNING: pr-deepseek-rounds failed ($(echo "$CURRENT" | jq -r '.error')), retrying..."
    continue
  fi
  NEW_ID=$(echo "$CURRENT" | jq -r '.latest_review_id')

  # A new review ID first: even a review with review_complete=true carries feedback.
  if [ "$NEW_ID" != "0" ] && [ "$NEW_ID" != "$PREV_REVIEW_ID" ]; then
    echo "DeepSeek review arrived (id=$NEW_ID)"
    PREV_REVIEW_ID="$NEW_ID"
    BOT_REVIEW_COUNT=$(echo "$CURRENT" | jq -r '.bot_review_count')
    break
  fi

  if [ "$(echo "$CURRENT" | jq -r '.review_complete')" = "true" ]; then
    echo "DeepSeek approved with no issues found"
    break
  fi

  echo "Waiting for DeepSeek review... (elapsed: ${ELAPSED}s / ${DEEPSEEK_WAIT_SECONDS}s)"
done
```

A timeout here is a real signal. A job cancelled by the 100-minute cap produces **no output**,
and `pr-status-json` counts `CANCELLED` as failed: the PR stays on `needs-work` with nothing in
the log explaining why. If that happened, say so explicitly.

#### 4c — Handle the round's feedback

1. **Read the unresolved, non-outdated threads**:

   ```bash
   bash scripts/gh-automation.sh pr-comments <pr-number>

   gh api graphql --raw-field 'query=query($owner:String!,$repo:String!,$pr:Int!){repository(owner:$owner,name:$repo){pullRequest(number:$pr){reviewThreads(first:50){nodes{id isResolved isOutdated comments(first:2){nodes{databaseId body}}}}}}}' \
     -f owner="${REPO%%/*}" -f repo="${REPO##*/}" -F pr=<pr-number> \
     --jq '.data.repository.pullRequest.reviewThreads.nodes[] | select(.isResolved == false and .isOutdated == false) | {id, comments: [.comments.nodes[] | {databaseId, body}]}'
   ```

2. **Fix every thread** (files under `$WORKTREE_DIR/`):
   - bug or valid suggestion → implement it
   - question → answer it
   - detail under 5 minutes → just fix it
   - out of scope → reply explaining why; do not silently ignore it

   **Publishing order:** prepare the replies in points 3–5, but do not send replies, do not
   resolve threads, do not publish the body verification and do not write `DEEPSEEK_REVIEW_READ`
   until Step 4e has pushed every fix and verified the remote head. A fix that exists only in
   the worktree has not arrived.

3. **Prepare a reply for every thread**, including those you did not act on. Note the comment
   ID, thread ID and text for Step 4f; no writes to GitHub for now.

4. **Prepare the resolution** only of the threads whose fix will have arrived or whose point is
   really handled. Note the decision for Step 4f.

   A thread whose suggestion you **rejected** can be resolved, but only after a reply that
   carries the evidence: that reply is the whole audit trail. Never resolve a thread you have
   not replied to.

5. **Findings in the review body** create no threads and cannot be resolved. Read the full
   bodies of the recent DeepSeek reviews, not just the threads, and read every finding before
   deciding whether to acknowledge them:

   ```bash
   ACK_STATE=$(bash scripts/gh-automation.sh pr-deepseek-rounds <pr-number>)
   ACK_REVIEW_ID=$(echo "$ACK_STATE" | jq -er '.latest_review_id | select(. != 0)')
   if ! BODY_REVIEWS=$(gh api graphql \
     --raw-field 'query=query($owner:String!,$repo:String!,$pr:Int!){repository(owner:$owner,name:$repo){pullRequest(number:$pr){reviews(last:100,states:[APPROVED,CHANGES_REQUESTED,COMMENTED]){nodes{databaseId submittedAt state author{login} body}}}}}' \
     -f owner="${REPO%%/*}" -f repo="${REPO##*/}" -F pr=<pr-number>); then
     echo "Could not fetch DeepSeek review bodies; acknowledgement is blocked"
     exit 1
   fi
   echo "$BODY_REVIEWS" | jq --arg bot "${DEEPSEEK_BOT_USER:-github-actions[bot]}" '
     def canon: sub("\\[bot\\]$"; "");
     .data.repository.pullRequest.reviews.nodes[]
     | select((.author.login // "" | canon) == ($bot | canon))
     | select((.body // "") != "")
     | {databaseId, submittedAt, state, body}'
   ```

   Fix every valid point in the code. A rejected point requires concrete evidence from the
   diff, from a test or from measured behaviour: a disagreement without evidence is not a reply.
   If a finding is unclear or has no evidence either way, leave it unacknowledged and report the
   blocker.

   Note a reply for every body finding for Step 4f. Never use `NO_DEEPSEEK_REVIEW`: that
   exemption stays human-only.

#### 4d — Fix CI

```bash
bash scripts/gh-automation.sh pr-status <pr-number>
```

If CI fails, read the logs (`gh run view <run-id> --log-failed`), fix, then run the Step 2 gate
for **every** area involved before the push.

If the problem is infrastructure (runner, package mirror, expired token), report it and stop.
Do not hide it with code changes.

#### 4e — Commit and push

If the feedback requires fixing the PR's title or body, **do not use `gh pr edit`**: on
`gh` 2.45.0 it fails before applying the flags. Use
`bash scripts/gh-automation.sh pr-edit <pr> --title "<title>" --body-file <path>`.

Commit messages, replies, comments and PR text are written in English.

```bash
cd "$WORKTREE_DIR"
git add -A
if git diff --cached --quiet; then
  echo "No source changes to commit"
else
  git commit -m "fix: address feedback on PR #<pr-number>"
  bash scripts/check-publication-privacy.sh "origin/$BASE_BRANCH" HEAD
  bash scripts/check-commit-privacy.sh "origin/$BASE_BRANCH" HEAD
  git fetch origin "$BRANCH"
  [ "$(git rev-parse "origin/$BRANCH")" = "$REMOTE_HEAD" ] || {
    echo "PR head moved; preserve local commits and reconstruct state"
    exit 1
  }
  git push origin HEAD
  REMOTE_HEAD=$(git rev-parse HEAD)
fi

# Confirm the PR is still open at REMOTE_HEAD. Only then execute the prepared
# replies/resolutions and body acknowledgement sequence from steps 3–5.
CURRENT_PR=$(gh pr view <pr-number> --json state,headRefOid)
[ "$(echo "$CURRENT_PR" | jq -r '.state')" = "OPEN" ] || exit 1
[ "$(echo "$CURRENT_PR" | jq -r '.headRefOid')" = "$REMOTE_HEAD" ] || exit 1
```

**NEVER run `git add` / `git commit` / `git push` from the main folder.** Always from
`$WORKTREE_DIR`.

With the round budget exhausted, this push does **not** start another review. If another pass
is really needed:

```bash
bash scripts/gh-automation.sh pr-deepseek-force-review <pr-number> [ref]
```

`ref` defaults to `develop` and must be a branch that contains the bypass, because the launched
run executes the workflow definition of that ref.

#### 4f — Publish the replies after the push

Only after Step 4e has verified the remote head, publish every prepared reply and then resolve
its thread:

```bash
gh api "repos/$REPO/pulls/<pr-number>/comments/<comment-databaseId>/replies" \
  --field body="<reply naming the published fix or the evidence for the rejection>" || exit 1
gh api graphql \
  -f query='mutation($id:ID!){resolveReviewThread(input:{threadId:$id}){thread{id isResolved}}}' \
  -f id="<THREAD_ID>" || exit 1
```

For the body findings publish one reply per finding (fixed, with file/test, or rejected, with
the evidence), after passing the exact text to `scripts/check-body-privacy.sh`. It is the
public audit trail. Then reread `latest_review_id`: it must
still equal `ACK_REVIEW_ID`, otherwise repeat this step from reading the bodies.
Renew `DEEPSEEK_REVIEW_READ` only after the trail exists, treating the label read as
three-state (present/absent/unreadable), and verify both the new label and the postconditions:

```bash
ACK_COMMENT='<one point per DeepSeek body finding, with the review ID and evidence>'
printf '%s\n' "$ACK_COMMENT" | bash scripts/check-body-privacy.sh || exit 1
gh pr comment <pr-number> --body "$ACK_COMMENT" || exit 1
LATEST_BEFORE_ACK=$(bash scripts/gh-automation.sh pr-deepseek-rounds <pr-number> \
  | jq -er '.latest_review_id')
[ "$LATEST_BEFORE_ACK" = "$ACK_REVIEW_ID" ] || exit 1
if bash scripts/gh-automation.sh pr-check-label <pr-number> DEEPSEEK_REVIEW_READ; then
  bash scripts/gh-automation.sh pr-label <pr-number> remove DEEPSEEK_REVIEW_READ || exit $?
  if bash scripts/gh-automation.sh pr-check-label <pr-number> DEEPSEEK_REVIEW_READ; then
    echo "Stale acknowledgement label is still present"
    exit 1
  else
    LABEL_RC=$?
    [ "$LABEL_RC" -eq 1 ] || exit "$LABEL_RC"
  fi
else
  LABEL_RC=$?
  [ "$LABEL_RC" -eq 1 ] || exit "$LABEL_RC"
fi
bash scripts/gh-automation.sh pr-label <pr-number> add DEEPSEEK_REVIEW_READ || exit $?
if ! bash scripts/gh-automation.sh pr-check-label <pr-number> DEEPSEEK_REVIEW_READ; then
  echo "Could not verify the fresh acknowledgement label"
  exit 1
fi
LATEST_AFTER_ACK=$(bash scripts/gh-automation.sh pr-deepseek-rounds <pr-number> \
  | jq -er '.latest_review_id')
UNREAD_AFTER_ACK=$(bash scripts/gh-automation.sh pr-status-json <pr-number> \
  | jq -er '.deepseek_unread_findings')
if [ "$LATEST_AFTER_ACK" != "$ACK_REVIEW_ID" ] || [ "$UNREAD_AFTER_ACK" != "0" ]; then
  bash scripts/gh-automation.sh pr-label <pr-number> remove DEEPSEEK_REVIEW_READ
  exit 1
fi
```

A more recent review stays unread because its timestamp follows the label. If a reply is not
understood or has no evidence, leave the label absent. Never apply `NO_DEEPSEEK_REVIEW`.

### Step 5: final check

```bash
bash scripts/gh-automation.sh pr-status <pr-number>
bash scripts/gh-automation.sh pr-check-label <pr-number> "READY TO MERGE"
```

`pr-labeler.yml` applies the label when CI finishes, in the six-hourly pass and on manual
dispatch. If `pr-status` says the rule is satisfied but the label is missing, the labeler has
not passed yet.

### Step 6: remove the worktree

```bash
MAIN_REPO=$(dirname "$(git rev-parse --path-format=absolute --git-common-dir)")
cd "$MAIN_REPO"

git worktree remove "$WORKTREE_DIR"
git worktree prune
```

If `git worktree remove` fails, warn the user and give the command for manual cleanup.

### Step 7: report

```
PR #<number>: <title>
├── Mergeable:   ✅ no conflicts
├── CI:          ✅ ci-gate ran and is green
├── DeepSeek:    ✅ clean verdict / rounds exhausted
├── Threads:     ✅ 0 unresolved
├── Body:        ✅ no unread findings (or: N unacknowledged, with the reason)
├── Label:       READY TO MERGE
└── State:       Ready for an autonomous merge into a base other than main
```

Report every line from what `pr-status` actually printed.

**Do not merge from this skill.** It is a separation of responsibilities: the merge decision
for bases other than `main` belongs to whoever reads the finished result and the PR's ordering
constraints.

## Guardrails

- **Worktree only**: changes, git and gates inside the worktree, never in the main folder.
- **Verify the worktree**: before every change, `git rev-parse --show-toplevel` must be
  `$WORKTREE_DIR`.
- One pass by default (`MAX_ROUNDS: "1"`); limit of 5 iterations if the budget is raised.
- Run the gates of **every** area touched.
- If CI fails because of infrastructure, report it and stop.
- **Do not poll `mergeable` and do not block on it.** An **already merged** PR reports it as
  `null` forever, so waiting for it to change is not slow, it is impossible. `pr-status` retries
  it internally and the labeler rereads it on every pass. Report what you measured and exit.
- **Before waiting for anything on a PR, check that it is still open.**
- If DeepSeek does not arrive within `DEEPSEEK_WAIT_SECONDS` (5700), report it as a likely job
  timeout and leave the loop: do not treat it as "no findings".
- **GraphQL limits**: a wait is at most 190 requests (5700s ÷ 30s) of about 3–5 points each,
  under 950 points. GitHub grants 5000 per hour. Avoid forced loops in parallel on several PRs.
- NEVER push directly to `main` and never merge a PR into `main`: those are human-only. Merging
  into bases other than `main` is authorised through
  **`bash scripts/gh-automation.sh pr-merge <pr> --head <observed-sha> --base-head
  <observed-base-sha>`**: it refuses a `main` base, fails closed on an unreadable base and
  refuses a head or base that moved after the read. Read the PR body before the merge: a stated
  ordering relative to another PR binds whoever merges, and the frozen rule does not see it.
- Never force-push or rebase without explicit instruction from the user. Conflicts are resolved
  with a merge of the base into the PR's head, preserving history.
- Always run the gates locally before publishing fixes, and the two privacy checks
  (`check-publication-privacy.sh`, `check-commit-privacy.sh`) before every push.
- Apply `DEEPSEEK_REVIEW_READ` only with the read/reply/public audit/fresh write sequence of
  steps 4c and 4f. Never `NO_DEEPSEEK_REVIEW`.

## References

- Helper: `scripts/gh-automation.sh` (`pr-status`, `pr-deepseek-rounds`, `pr-deepseek-force-review`)
- DeepSeek reviewer: `.github/workflows/deepseek-pr-review.yml` / `.github/scripts/deepseek_review.py`
- Passive monitor: `.github/workflows/pr-labeler.yml`
- Gates (source of truth): `.github/workflows/ci.yml`
- Branch protection: `.github/branch-protection.md`
