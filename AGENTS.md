# WoW-don — Tauri 2 (Rust + React)

Addon manager for World of Warcraft on **Linux, Windows and macOS**: on Linux the game runs
inside a Wine prefix, on Windows and on macOS it is the native client installed normally.
Native desktop app: **Rust** backend
with **Tauri 2**, **React + TypeScript** frontend (Vite, pnpm). Context, decisions,
architecture and roadmap are in `docs/`.

## Repository structure

```
wow-don/
├── AGENTS.md             # This file — global rules (authoritative for the pipeline)
├── CLAUDE.md             # Points Claude Code to this file
├── README.md             # Project overview
├── src/                  # React + TypeScript frontend
├── src-tauri/            # Rust backend and Tauri configuration
├── docs/                 # context, decisions, architecture, roadmap
├── scripts/              # gh-automation.sh, CI classifier and gate, automation tests
├── .github/              # CI, DeepSeek review, PR labeler, iteration lifecycle
├── .claude/skills/       # Canonical Claude skills (/dev-issue, /process-pr, /develop-iteration, /scrum-master)
├── .agents/skills/       # Generated Codex adapters ($dev-issue, $process-pr, $develop-iteration, $scrum-master)
└── .opencode/skills/     # Generated OpenCode adapters (/dev-issue, /process-pr, /develop-iteration, /scrum-master)
```

## Areas

| Area       | Folders | Technology | Responsibility |
| ---------- | -------- | ---------- | -------------- |
| `backend`  | `src-tauri/` | Rust (edition 2024), Tauri 2 | Tauri commands, configuration, WoW installations, reading `.toc` files, fingerprints, source clients, download and extraction, local state |
| `frontend` | `src/`, `public/`, `index.html`, `vite.config.ts`, `tsconfig*.json` | React 19, TypeScript, Vite | Lists, search, filters, sorting, `invoke()` calls |
| `shared`   | root | — | Changes that touch both areas or the common configuration (`package.json`, `pnpm-lock.yaml`) |

### Network and disk live in the backend; the app holds no third-party key — always

The frontend shows data and calls Tauri commands; it does not talk to the sources. Everything
that touches the network or the file system is in the backend. The CurseForge key is not in the
app at all (D44): the backend reaches CurseForge through the wow-don-proxy Worker, which keeps
the key in Cloudflare secrets. A `fetch` to the sources from the frontend, or key plumbing in
the app (a config field, an environment variable, an `x-api-key` header built locally), is a
bug even when it "works".

### Source rules

- The CurseForge key never reaches the app: it stays in the proxy's Cloudflare secrets, and no
  key appears in logs, in returned errors or in tests.
- No scraping of the CurseForge site and no reuse of other apps' keys.
- Addons with `allowModDistribution = false` are not downloaded automatically.
- Zips are extracted into a temporary folder, and before they are moved it is checked that no
  path leaves `Interface/AddOns`.

### Changes that touch both areas

1. Backend first: serialized types (`serde`) and the Tauri command.
2. Then the frontend, against the command's real behaviour.
3. Run the gates of both areas before opening the PR.

### Platforms: Linux, Windows and macOS

The app must work on **Linux, Windows and macOS** (D23, extended by D29) and is distributed as
a **`.deb`** on Linux and as an **NSIS installer** on Windows, never as a portable version
(D24); on macOS the game client is native, without Wine, and there is no package yet. Every
change must hold on all three:

- **Paths only with `Path`/`PathBuf`** and `join`: never hand-written separators, never
  comparing paths as strings. Config and data folders are asked of Tauri
  (`app.path()`), never `~` or `%APPDATA%` written in the code.
- **Upper and lower case**: Windows (and the game under Wine) does not distinguish them in
  file names, Linux does, macOS usually does not (APFS does not distinguish, except on volumes
  formatted to do so). Comparisons on the game's file names (`.toc`, `_<flavor>_` folders) are
  made case-insensitively when the game does the same. A test that creates two names differing
  only in case runs only on Linux.
- **`canonicalize` on Windows** returns `\\?\C:\...` paths: they must be shown and saved
  without that prefix.
- **No shell commands or single-platform tools** (`sh`, `xdg-open`,
  `cmd`): to open files and links there is the `opener` plugin.
- **Tests run on all three**: no absolute Unix paths in tests that touch the
  disk; they are built under a temporary folder.
- Platform-specific functions sit behind `#[cfg(target_os = "...")]`, with defined
  behaviour on the other two as well. `not(target_os = "windows")` covers Linux **and**
  macOS: the comment must not assume "not Windows = Linux under Wine", and
  `cfg(unix)` includes macOS. Functions tied to Wine and Lutris are Linux only.

### Interface: minimal and modern

The UI must be **minimal and modern** (decision D20 in `docs/decisions.md`). In practice:

- **Few surfaces.** A screen does one thing; no panels, bars and accessory windows that the
  task does not need. Secondary actions sit behind a menu or appear on hover, not all in
  view.
- **Space and hierarchy, not decoration.** Generous and regular spacing, clean typography
  (system font), at most two weights and few sizes. No gradients, heavy shadows, double
  borders, decorative icons or animations that do not communicate a state.
- **Colour sparingly.** A neutral palette with a single accent colour; colour signals
  state (error, warning, selection), it does not embellish. Light and dark are both mandatory:
  new colours are defined as CSS variables in `:root`, with the dark value as `--dark-*`
  assigned in the two dark-theme blocks of `src/App.css`, instead of being written in the
  individual components.
- **Tokens, not values (D42).** Sizes, weights, control heights, radii, spacing and colours
  are the variables at the top of `src/App.css`, listed in the comment that opens them along
  with the control classes (`.button` and variants, `.switch`, `.segmented`, `.pill`). Every
  `font-size` is a `--text-*` token or `inherit`, and `src/styleTokens.test.ts` verifies it.
- **Data first.** Search, filters and sorting are the app's strong point (D4):
  the search box is always visible, filters are compact, tables are dense but
  readable.
- **States always handled**: loading, empty list (with a sentence saying what to do) and
  error, without blocking browser dialogs.
- **No component or style library** without an explicit decision: plain CSS as long as it
  is enough.

A PR that touches `src/` describes in one line how it respects these rules.

## Privacy boundary: write as if the repository were public

The repository is private today and may become public. When that happens, everything that has
already been published becomes readable forever: files, commit metadata (author and
committer), branch names, commit and PR messages, review comments, CI logs,
artifacts. That is why the rules of a public repository already apply now.

- **Never a real email address.** Commits use the `noreply` address provided by GitHub
  (copy it from Settings → Emails, do not type the number by hand). Code, fixtures and messages
  may contain only GitHub `noreply` identities and addresses on reserved example domains.
- **Never paths from your own machine.** No usernames, home directories, mount points or
  checkout locations. Documentation uses placeholders such as `<repo-root>`, `<worktree>` and
  `<workspace>`; tests use clearly synthetic paths.
- **Never personal data** in issues, PRs or code: real names, phone numbers,
  locations, account identifiers. The project's public identity is the `@FabioSM46` handle.
- **Never secrets or operational identifiers.** API keys, tokens, credentials, internal
  hostnames and non-public endpoints live in GitHub Secrets or in ignored local files (`.env`),
  never in git, logs, issues or PRs.
- **Never a reference to an agent session.** A `Claude-Session:` trailer or a
  `claude.ai/code/session_…` link in a message is an endpoint tied to an account.
- **Never attribute a commit to an account that is not yours.** An address
  `<id>+<login>@users.noreply.github.com` credits whoever owns `<id>`, not `<login>`: the wrong
  number next to the right name credits a stranger. The three privacy scripts keep a
  login → id table for the identities the repository publishes.

**Three scripts split the work, and share the same three categories** (non-public email,
machine path, private network address), copied into each one and kept identical by
`scripts/test/commit-privacy.test.sh`:

- `bash scripts/check-publication-privacy.sh` reads the content of the files in the index (what
  a push publishes, not the working folder); with `<base> <head>` it also reads the tree of every
  commit in the range, because data added in one commit and removed in the next leaves the tip
  clean but stays in the published history; with `--all-history` it reads every reachable commit;
- `bash scripts/check-commit-privacy.sh <base> <head>` reads the author, committer and message of
  every commit the branch adds;
- `bash scripts/check-body-privacy.sh` reads an issue or PR body from stdin. Use it before
  publishing: `bash scripts/check-body-privacy.sh < body.md`.

Run the first two before every push, both with the branch range
(`"origin/$BASE_BRANCH" HEAD`): CI runs them in the `detect` job on the PR range, and a violation
blocks the PR even with everything else green. Diagnostics name only the location and category,
never the value found; when a check fails, remove the data before continuing instead of
reprinting it in another message.

**An issue or PR body can only be checked afterwards.** `.github/workflows/body-privacy.yml`
runs on `issues` and `pull_request_target` (`opened`, `edited`): if it finds something it adds
the `needs-privacy-review` label and a comment with category and line; it does not close, edit or
hide anything. **It cannot prevent a leak, it can only shorten the time nobody knows about it**:
a green run means "nothing is published now", never "nothing was ever published". It is the
surface a machine writes most often: an agent writing a PR body transcribes what it has just
done, and it did it in a folder with a name.

**Before making the repository public**: run `check-publication-privacy.sh --all-history`
and `check-commit-privacy.sh` over the whole history. The scripts recognise only the three
categories (emails, paths, private network addresses) and `.env` files: they are not a secret
scanner. API keys and tokens of other shapes are invisible to them, so also run a dedicated
scanner (for example gitleaks) over the whole history. Then reread by hand the titles and bodies
of issues and PRs already open, check the secrets and verify that the rulesets are still active
after the visibility change.

## Common conventions

- **No secrets in code**: keys, tokens and credentials live in GitHub Secrets or in ignored
  local files, never in git, logs, issues or PRs.
- **English everywhere**: everything in the repository is written in English — code, comments,
  documentation, skills, templates — and so are commit messages, branch names, PR and issue
  titles and bodies, and review replies (D43).
- **Commits**: never commit without an explicit request; conventional commit style (`feat:`,
  `fix:`, `refactor:`, `docs:`, `ci:`).
- **Read first, then write**: look for existing patterns before implementing.
- **No new dependencies** (crates or npm packages) without an explicit instruction.
- **Never read `.env` files**: AI agents must NOT read, open or inspect any
  `.env` file. The app no longer uses one (D44: the CurseForge key lives only in the
  wow-don-proxy Worker's secrets), but a leftover local `.env` can still hold a real key.
- **No tool signatures**: no agent `Co-Authored-By` trailers, no
  "Generated with ..." in PRs, no `claude.ai/code/session_…` links in messages.
- **NEVER push or merge to `main`**: `main` is the release branch and stays human only. The AI
  must never run `git push origin main` nor merge a PR into `main`. PRs targeting `main` are
  allowed; the human does the merge.
- **Merging into bases other than `main` is up to the AI**, at its own judgement. `READY TO
  MERGE` is a signal to read, not a precondition. The merge is done with
  **`bash scripts/gh-automation.sh pr-merge <pr>`**, which refuses a `main` base by name and
  fails closed on an unreadable base. See "What actually stops a merge" below.
- **Isolation with git worktree**: every change and every git operation happens in an isolated
  `git worktree`, never in the main folder. Use
  `<parent-folder>/wow-don-issue-<number>` when the work is tied to an issue; if there is no
  issue do not invent a number, use `<parent-folder>/wow-don-issue-<descriptive-slug>`. The
  `/process-pr` worktrees are `wow-don-pr-<number>`. Check that you are in the worktree before
  editing (`git rev-parse --show-toplevel`). After opening the PR remove it with
  `git worktree remove` and `git worktree prune`. Reuse an existing worktree for the same branch.

## Branch strategy

- `main` — release branch
- `develop` — integration branch and default base (the repository's default branch); stacked
  sub-branches may start from their feature branch
- `feature/<issue>-<slug>` — features and improvements
- `fix/<issue>-<slug>` — fixes
- `refactor/<issue>-<slug>` — restructuring without behaviour changes

For authorised work without an issue, omit the number instead of inventing it:
`<type>/<descriptive-slug>` (for example `refactor/extract-config`).

## AI-driven development pipeline

The repository has a pipeline that takes issues from creation to merge-ready PRs, on three
levels:

| Level        | Technology                              | Responsibility |
| ------------ | --------------------------------------- | -------------- |
| **Skill**    | Skills for Claude, Codex and OpenCode   | Implementation, PR handling, iteration delivery, scrum ceremonies |
| **Workflow** | GitHub Actions (`.github/workflows/`)   | CI, PR review, labelling, iteration transitions |
| **Helper**   | `scripts/gh-automation.sh`              | Common wrapper for the REST and GraphQL APIs |

The whole infrastructure is ported from voxelheim-v2; comments in the workflow and script code
cite issue and PR numbers from that repository, where the full history lives.

### Skill sync across runtimes

`.claude/skills/` is the canonical source. Codex uses the adapters generated in `.agents/skills/`,
OpenCode those in `.opencode/skills/`. When a canonical skill changes:

1. Edit only `.claude/skills/<skill>/SKILL.md`.
2. Run `bash scripts/sync-agent-skills.sh` from the root.
3. Run `bash scripts/test/agent-skills-sync.test.sh` and commit the three versions together.

Never edit the generated adapters by hand. CI fails if an adapter is stale.

### Flow

```
Issue opened from the template
        │
        ▼
/dev-issue <number> in Claude/OpenCode, or $dev-issue <number> in Codex
        │
        ▼
skill: reads the issue → validates the area → reads AGENTS.md and docs/
        │
        ▼
skill: worktree + branch from the base (develop by default) → implements → gates (fmt, clippy, build, test)
        │
        ▼
skill: opens the PR against that base → EXITS (stateless)
        │
        ▼
deepseek-pr-review.yml: inline review comments on the diff ─────────┐
        │                                                           │
        ▼                                                           ▼ (in parallel)
pr-labeler.yml (at the end of CI/review + a pass every 6h): reads ci-gate and threads
        │
    ┌───┴──────────────┐
    │ Frozen rule?      │──yes─→ label READY TO MERGE
    │ CI red?           │──yes─→ label needs-work
    │ otherwise         │──────→ needs-review
    └───────────────────┘
        │
        ▼
/process-pr (Claude/OpenCode) or $process-pr (Codex): conflicts, CI, feedback
        │
        ▼
The AI reads the result and the ordering constraints → merge into bases other than main
```

`/develop-iteration` (Claude/OpenCode) or `$develop-iteration` (Codex) runs all of this for
the issues already assigned to a milestone: one agent per issue with `/dev-issue`, parallel
waves only for independent work, `/process-pr` on the PRs that need fixing, and merging of the
ready leaves in dependency order. On `develop` there is no post-merge verification (CI runs only
on `pull_request`): that is why the skill merges one PR at a time into `develop` and realigns
every other PR to the new tip before merging it.

### Skills

| Skill          | Claude / OpenCode         | Codex                     | Behaviour |
| -------------- | ------------------------- | ------------------------- | ------------- |
| `dev-issue`    | `/dev-issue <number>`     | `$dev-issue <number>`     | Stateless: issue → PR → exits |
| `process-pr`   | `/process-pr <pr-number>` | `$process-pr <pr-number>` | Active fixing: conflicts, CI, review feedback |
| `develop-iteration` | `/develop-iteration [iteration]` | `$develop-iteration [iteration]` | Parallel agents per issue → fixes → merge of PRs into bases other than main |
| `scrum-master` | `/scrum-master <ceremony>` | `$scrum-master <ceremony>` | backlog-refine, iteration-plan, feature-spec |

Skills can be started by an agent as well as by a person: do not use
`disable-model-invocation`. Keep `allowed-tools` tight: they are the reason to prefer a
skill over a hand-built agent.

Iteration ceremonies keep the human in the loop: choosing which issues enter the
next iteration is the user's decision. `/develop-iteration` delivers the chosen scope and
stops: it does not close the milestone and does not run the ceremonies.

#### What actually stops a merge

**Into `develop`: the rulesets and the AI's judgement.** The branch ruleset requires a PR,
resolved review threads and (once the second ruleset is activated) a green `ci-gate`. The
frozen rule computes `READY TO MERGE`; it is a signal, not a block.

**Into another feature branch: the fresh check by whoever merges.** The rulesets cover
only `develop` and `main`. Before the merge run `is-ready-to-merge` right before `pr-merge`,
and pass `--head <observed-sha>` and `--base-head <observed-base-sha>` so that a moved head or
base invalidates the read.

**Into `main`: the instruction, plus a check on the designated path.** A deny can block
`git push origin main` because the command names the ref. **`gh pr merge <n>` names no
branch**: the base is a property of the PR, so no `Bash(...)` prefix tells a merge into
`develop` from one into `main`. In its place there is `pr-merge`, which reads `baseRefName` and
refuses `main`, failing closed on an empty or unreadable base without attempting the merge
(`scripts/test/pr-merge-guard.test.sh`).

**It stops an accident, not a decision.** `gh pr merge` and
`gh api -X PUT repos/OWNER/REPO/pulls/N/merge` bypass the helper, and nothing stops an agent that
wants to use them. The allowlist is not a sandbox: the skills have `Bash(gh *)` and
`Bash(bash *)`, and both are necessary. What holds `main` is the instruction.

The `deny` entries in `.claude/settings.json` for `git push origin main` (and the variants
`--force`, `-f`, `+main`) remain as a convenience: underneath there is a full server-side block.

**Residual risk.** Nothing reads a PR before it reaches `develop` unless the AI does.
`unresolved_threads == 0` does not mean a person has looked: the AI can resolve a
thread it has replied to, even when rejecting the suggestion, and that reply is the whole audit
trail. Ordering between PRs is invisible to the frozen rule: an order written in a PR body binds
whoever merges. **Read the bodies.**

### Frozen acceptance rule

**The `READY TO MERGE` label is added only when: the stable `ci-gate` check is present and
green on the head commit, no check has failed or is in progress, the PR is mergeable, no review
requests changes, unresolved review threads are zero, no DeepSeek review has unread findings in
its body (they are closed with the `DEEPSEEK_REVIEW_READ` label) and the DeepSeek review is
definitively finished (clean verdict, rounds exhausted or `NO_DEEPSEEK_REVIEW` exemption).**

The rule has a single implementation, `cmd_pr_status_json` in `scripts/gh-automation.sh`, used
by `pr-labeler.yml` and by `pr-status`. Every condition fails closed: an unreadable count is
`-1`, never `0`.

"Green" is not "not red". A PR **with no checks at all** satisfies "zero failed, zero in
progress", and it is a reachable state: a PR in conflict has no merge ref and therefore runs no
`pull_request` workflow. That is why the helper requires `ci-gate` **present and green** and
`mergeable == MERGEABLE`.

The labeler runs on `workflow_run` when CI or the DeepSeek review finishes, plus a pass every
six hours for transitions without an event (resolution of the last thread, mergeability
recomputed after a merge on the base). `workflow_dispatch` or `/process-pr` force a pass. Do not
add a `pull_request` trigger to the labeler.

**A failed label write makes the labeler fail.** `pr-label` exits non-zero if the
write does not land, and `pr-check-label` answers **0 present / 1 absent / 2 not
determinable**. A red labeler run means a write failed: read the log.

#### The thread count sees only half the review

DeepSeek delivers findings in two forms. Inline comments create **threads**, which the rule
counts. General comments sit in the review **body** and create no threads: a review made only of
those counts `0 unresolved` while having real findings. That is why `pr-status-json` also counts
`deepseek_unread_findings`. The clean verdict is recognised by the
`<!-- deepseek:no-findings -->` marker at the start of the body, without `<!-- deepseek:full-review -->`.

**The acknowledgement is the `DEEPSEEK_REVIEW_READ` label, and it is dated**: only reviews
submitted before the label was applied count as read. A second forced review blocks
again; removing the label cancels the acknowledgement. A human can apply it after reading the
findings. The AI only after reading **every** finding, fixing the valid ones or rejecting the
others with evidence, and posting the replies on the PR; its write must be more recent than the
reviews it acknowledges. `NO_DEEPSEEK_REVIEW` remains human only.

### Completion-driven iteration lifecycle

There is no calendar and milestones have no due date. `iteration-lifecycle.yml` runs when
an issue with a milestone or with the `ceremony` label is closed, identifies the active milestone
and evaluates it:

1. As long as the active iteration has open issues, it does nothing.
2. When the last issue is closed, it creates a backlog refinement ceremony for that
   milestone.
3. Closing that ceremony creates an iteration planning ceremony.
4. `/scrum-master iteration-plan` picks a batch of work, creates and populates the next
   `Iteration N` milestone, then closes the completed one.

**There can be several open iterations**: the active one is the open milestone with the
**lowest** sequence (from `Iteration N` / `Sprint N`, otherwise from the milestone number),
compared as a number. Two open milestones with the same lowest sequence make the run fail.

**Cold start**: with no milestone at all, `iteration-advance` does nothing. The first
`/scrum-master iteration-plan` creates `Iteration 1` directly.

### Automated review (DeepSeek)

`deepseek-pr-review.yml` reviews the code with the DeepSeek API (`deepseek-flash`,
`reasoning_effort=high`):

| Mode | Trigger | Behaviour |
|------|---------|---------------|
| **A — Full review** | PR opened or new commit | Reads the whole diff and posts inline comments with suggestions. A finding that names a file must be anchored to that file; the body is reserved for observations that do not belong to a single file. |
| **B — Reply** | Reply to a DeepSeek comment | Reads the diff and the thread, replies in the same thread. Never touches the code. |

**No approval**: GitHub does not let Actions approve PRs. The clean verdict is
a COMMENT review that starts with `<!-- deepseek:no-findings -->`.

**Round limit**: automatic full reviews are at most **1** per PR (`MAX_ROUNDS`).
After that, pushes trigger no further reviews and a notice is posted once. Replies (Mode B)
continue without limit. To force another pass:
`bash scripts/gh-automation.sh pr-deepseek-force-review <pr> [ref]` (`ref` defaults to `develop`).
With `--measure-only` it re-reviews a diff without publishing anything.

**Which copy of the configuration applies**: `pull_request` runs use the merge ref, so they
execute the workflow and scripts of the **base branch**.

**Diff limit: 90,000 characters** (`DEEPSEEK_MAX_DIFF_CHARS`, measured in voxelheim-v2 with
seventeen replays at `high`). Beyond that, the diff is truncated, every unread file is named
in the log **and in the review as a finding**, and the PR stays blocked until someone
acknowledges it. A truncated review cannot come back clean. Splitting the PR is the reliable
solution; better still, size the issue beforehand (see `/scrum-master` and `/dev-issue` Step 5).
The limit and the effort are a single measurement: changing either makes
`deepseek-budget.test.sh` fail until the limit is re-measured.

Generated code (`gen/` folders, `*_generated.*`) and lockfiles (`Cargo.lock`, `pnpm-lock.yaml`,
`go.sum`, by exact name) are excluded from the diff. Manifests (`Cargo.toml`, `package.json`)
stay: a new dependency is exactly what a reviewer must see.

**Budget and timing**: `DEEPSEEK_MAX_OUTPUT_TOKENS=384000`; the request budget is
`DEEPSEEK_REQUEST_TIMEOUT_SECONDS` × (`DEEPSEEK_MAX_RETRIES` + 1) = 2700s × 2 = 90 minutes, and the
job has `timeout-minutes: 100`. **The budget must stay below the job limit**:
`scripts/test/deepseek-budget.test.sh` verifies it. If the job limit wins, the step ends up
`cancelled` with no output and `pr-status-json` counts `CANCELLED` as failed.

**An unreadable diff makes the run fail** (fetch error, file without a patch, network errors),
and an unreadable round count is not zero.

**Security**: DeepSeek creates no commits, does not push and does not edit files. It ignores
its own comments. It never claims to have taken actions its script cannot take (such as
resolving a thread).

### Prerequisites

0. **Local tools**: `gh` (2.18.0 or later) and `jq`, see the table in `README.md`.
   `scripts/gh-automation.sh` requires both.
1. **GitHub token**: a fine-grained PAT with `contents: read/write`, `pull_requests: read/write`,
   `issues: read/write` on this repository, saved as the `GH_PIPELINE_TOKEN` secret. The
   labeler and the iteration lifecycle need it (writes made with `GITHUB_TOKEN` do not
   trigger other workflows).

   **This token does not read the CI state, and no permission fixes that**: the Checks API is
   GitHub App only. `pr-labeler.yml` passes `GH_CI_TOKEN: ${{ secrets.GITHUB_TOKEN }}` with
   `checks: read`, `statuses: read` and `actions: read`, and `gh_ci()` routes that single
   read there. **Every new caller that reads checks must use `gh_ci`, not `gh`.**
2. **DeepSeek API key**: `DEEPSEEK_API_KEY` secret (`Settings → Secrets and variables →
   Actions`).
3. **Merge protection**: two rulesets on `develop` and `main`, see
   `.github/branch-protection.md`. The second one (`ci-gate` required) is activated only after
   `ci-gate` has run on a real PR.
4. **Labels**: `READY TO MERGE`, `needs-review`, `needs-work`, `DEEPSEEK_REVIEW_READ`,
   `NO_DEEPSEEK_REVIEW`, `ceremony`, `feature`, `bug`, `needs-refinement`, `needs-triage`,
   `ready-for-dev`, `needs-privacy-review`.

## Definition of Done

**Every PR opened by the pipeline MUST verify ALL of this before it is created:**

- [ ] **Formatting** — `cargo fmt --all --check` (in `src-tauri/`)
- [ ] **Lint** — `cargo clippy --all-targets --locked -- -D warnings`
- [ ] **Build** — `cargo build --locked` and `pnpm build` (`tsc && vite build`)
- [ ] **Tests** — `cargo test --locked` and `pnpm test` (`node --test`, tests of the frontend's pure functions)
- [ ] **Automation** — `scripts/test/*.test.sh` and `python3 .github/scripts/test_deepseek_review.py`
- [ ] **No debug prints** in production paths
- [ ] **No secrets** — no keys, tokens, passwords or credentials in code or tests
- [ ] **No CurseForge key in the app** — not in the frontend, the backend, the logs or the tests (D44)
- [ ] **Clean publication privacy** — no real email, personal data, machine path, hostname or
      private endpoint; `scripts/check-publication-privacy.sh` passes on the tree and
      `scripts/check-commit-privacy.sh` on the commits the branch adds

If a gate fails, the implementation is not finished.

### What CI enforces

`.github/workflows/ci.yml` runs:

| Job          | Gate |
| ------------ | ---- |
| `detect`     | Privacy checks on the PR's tree and commits, then classifies the changed files with `scripts/changed-areas.sh` |
| `backend`    | `cargo fmt --check`, `cargo clippy -D warnings`, `cargo build --locked`, `cargo test --locked` (with Tauri's system libraries) |
| `backend-windows` | `cargo clippy -D warnings`, `cargo build --locked`, `cargo test --locked` on `windows-latest` (D23); same selector as `backend` |
| `backend-macos` | The same gates as `backend-windows` on `macos-latest` (D29), with its own selector (`macos`): it runs only on PRs targeting `main` and on PRs that modify `ci.yml` |
| `frontend`   | `pnpm install --frozen-lockfile`, `pnpm build`, `pnpm test` |
| `automation` | The whole `scripts/test/*.test.sh` suite plus `test_deepseek_review.py` |
| `ci-gate`    | The only stable check: verifies that every selected job succeeded and every skip was authorised |

On a PR targeting a base other than `main`, `detect` selects the jobs from the diff: `src-tauri/**`
activates `backend` and `backend-windows`, `src/**` and the frontend toolchain files activate
`frontend`, `package.json`/`pnpm-lock.yaml` activate both, `ci.yml` activates everything,
documentation activates nothing. `automation` always runs. An unrecognised path or an unreadable
diff activates everything (failing open). On a PR targeting `main` everything runs.

`backend-macos` is narrower by the user's choice: in a private repository macOS minutes
cost ten times Linux ones. On a base other than `main` it runs only if the diff touches
`ci.yml`, so it can be seen running every time its definition changes; elsewhere the skip is
authorised and `ci-gate` accepts it. An unrecognised path does not activate it (`backend` and
`backend-windows` already take care of that), an unreadable diff does. A backend change that
breaks only macOS is therefore discovered at the release PR to `main`.

Two traps:

1. **Never `paths:` filters at workflow or job level.** A filtered workflow does not produce a
   result `ci-gate` can verify. The right tool is a job-level `if:`.
2. **`cancel-in-progress` produces CANCELLED jobs, which `pr-status-json` counts as failed.** In
   `ci.yml` cancellation applies only to `pull_request` events, where the cancelled run always
   belongs to a commit that is no longer the head.

### Release

`.github/workflows/release.yml` starts on a `v*` tag, accepted **only on a commit already in
`main`** (hence one that went through a PR with a green `ci-gate`): the `verify` job checks it
before building. It then builds the `.deb` on `ubuntu-latest`, the NSIS installer on
`windows-latest` and the universal `.dmg` (`universal-apple-darwin`) on `macos-latest` (D24)
and creates a **draft release** with the three packages; if the draft already exists it replaces
its packages, while an already published release is never touched.
Publishing stays human. Only the job that creates the draft has `contents: write`. No package
is signed: Windows shows the SmartScreen warning and macOS the Gatekeeper one (the `.dmg` is not
even notarised) until a signing approach is decided.

Actions are pinned by full SHA with the release label, and checkout does not
persist credentials (`scripts/test/actions-hardening.test.sh`). The Rust toolchain is pinned
in `src-tauri/rust-toolchain.toml`, aligned with `dtolnay/rust-toolchain` in CI.
