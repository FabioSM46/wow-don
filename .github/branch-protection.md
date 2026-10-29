# Merge protection

The merge contract for `main` and `develop` is enforced by two GitHub **rulesets**. This file
says what they enforce, why there are two and how to recreate them. Configuration ported from
voxelheim-v2.

In the UI they live under **Settings → Rules → Rulesets**.

## What is enforced

| Ruleset | Rule | Who can bypass it |
| ------- | ---- | ----------------- |
| `PR required — unresolved review threads block merge` | `pull_request`: `required_review_thread_resolution: true`, `required_approving_review_count: 0` | **nobody** |
| `ci-gate must pass (admin can bypass)` | `required_status_checks`: `ci-gate` | repository admin (`bypass_mode: always`) |

Both apply to `refs/heads/develop` and `refs/heads/main`.

In practice:

- **An unresolved review thread blocks the merge for everyone**, the owner included. You read
  the finding, fix it or reject it with a reason, and resolve the thread.
- **A red, pending or missing `ci-gate` blocks the merge, but the admin sees the bypass
  option** and can force it with an explicit confirmation. It is there for when CI cannot be
  green for reasons outside the diff.

## Why two rulesets

**The bypass list belongs to the ruleset, not to the rule.** With a single ruleset the two rules
would both be bypassable or neither would. The API reports it per ruleset as
`current_user_can_bypass`: `never` for the first, `always` for the second.

## Consequences to know

- **The `pull_request` rule also blocks direct pushes to `develop` and `main`**, for everyone
  and without bypass. Everything goes through a PR.
- **`required_approving_review_count` is 0 on purpose.** GitHub does not let the author
  approve their own PR; on a repository with a single developer a mandatory approval would
  make every PR unmergeable except through the bypass.
- **`strict_required_status_checks_policy` is false**: branches do not have to be up to date
  with the base. With several PRs open in parallel, every merge would require a new push.
- **Only `ci-gate` is required.** `labeler` and `review` stay out: DeepSeek skips runs when the
  round budget is exhausted, and requiring `review` would block PRs. `ci-gate` is the only
  stable check and contains the per-branch rule:
  - towards bases other than `main`: the jobs selected by the diff and `automation` must
    succeed; jobs not selected may come out `SKIPPED`. `backend-windows` follows the same
    selector as `backend`; `backend-macos` has its own selector and runs only on PRs that
    modify `ci.yml`, otherwise it comes out `SKIPPED`;
  - towards `main`: every job must run and succeed.

## Why `READY TO MERGE` is not redundant

The ruleset is a floor. GitHub's native rule counts review **threads**, but DeepSeek can also
leave findings in the review **body**, which creates no thread. A review made only of those
counts zero open threads and GitHub would allow the merge.

`pr-status-json` counts them separately as `deepseek_unread_findings`, and treats them as read
via the dated `DEEPSEEK_REVIEW_READ` label. A human may apply it after reading the findings; the
AI only after fixing or rejecting every finding with evidence and posting the reasons on the
PR. `NO_DEEPSEEK_REVIEW` remains a human-only exemption. So:

- the **ruleset** guarantees that nobody merges by mistake on top of an open thread or a red
  `ci-gate`;
- the **`READY TO MERGE` label** additionally guarantees that the review was read, the round is
  over and the PR is genuinely mergeable.

## Recreating the configuration

```bash
gh api repos/OWNER/REPO/rulesets -X POST --input - <<'JSON'
{
  "name": "PR required — unresolved review threads block merge",
  "target": "branch",
  "enforcement": "active",
  "bypass_actors": [],
  "conditions": { "ref_name": { "include": ["refs/heads/develop", "refs/heads/main"], "exclude": [] } },
  "rules": [
    { "type": "pull_request",
      "parameters": {
        "required_approving_review_count": 0,
        "dismiss_stale_reviews_on_push": false,
        "require_code_owner_review": false,
        "require_last_push_approval": false,
        "required_review_thread_resolution": true
      }
    }
  ]
}
JSON

gh api repos/OWNER/REPO/rulesets -X POST --input - <<'JSON'
{
  "name": "ci-gate must pass (admin can bypass)",
  "target": "branch",
  "enforcement": "active",
  "bypass_actors": [ { "actor_id": 5, "actor_type": "RepositoryRole", "bypass_mode": "always" } ],
  "conditions": { "ref_name": { "include": ["refs/heads/develop", "refs/heads/main"], "exclude": [] } },
  "rules": [
    { "type": "required_status_checks",
      "parameters": {
        "strict_required_status_checks_policy": false,
        "required_status_checks": [ { "context": "ci-gate" } ]
      }
    }
  ]
}
JSON
```

`actor_id: 5` with `actor_type: RepositoryRole` is the **admin** role.

## Activation order

Do not require `ci-gate` before it has run on a real PR: a required check that has never run
leaves every PR in "expected, waiting". The safe order is:

1. Create the first ruleset (PR required).
2. Open a PR towards `develop` and check that it reports a green `ci-gate`.
3. Only then create the second ruleset.

## Verifying

```bash
gh api repos/OWNER/REPO/rulesets --jq '.[] | "\(.name) → \(.enforcement)"'
gh pr view <number> --json mergeable,mergeStateStatus
```

`mergeable: MERGEABLE` together with `mergeStateStatus: BLOCKED` points to a rule holding the
merge back: the PR is clean, it is the policy saying no.
