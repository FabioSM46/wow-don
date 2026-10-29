# CLAUDE.md

Read and follow the instructions in the @AGENTS.md file at the project root.

The repository is private but **must be treated as public**: it may become public, and
everything already published would stay readable forever. Never a real email address, personal
data, username or machine path, private hostname, secret or credential in a tracked file,
commit, branch, PR, review, CI log or artifact. Use GitHub's `noreply` address, reserved
example values and placeholders such as `<repo-root>`. Before every push run
`bash scripts/check-publication-privacy.sh <base> <head>` on the content (the index and every
commit of the branch) and `bash scripts/check-commit-privacy.sh <base> <head>` on authors and
messages; before publishing an issue or PR body, `bash scripts/check-body-privacy.sh`.

The CurseForge key lives only in `.env` (ignored by git) and in the Rust backend. Agents never
read `.env`; to know which variables are needed they use `.env.example`. No tool signatures in
commits and PRs: no agent `Co-Authored-By` trailer, no "Generated with", no
`claude.ai/code/session_…` link and no `Claude-Session:` trailer.

Everything is written in English: code, comments, docs, skills, templates, commit messages,
branch names, PR and issue titles and bodies, and review replies.

Quick reference — the pipeline skills:

- `/dev-issue <number>` — implements a GitHub issue end to end (worktree → gates → PR)
- `/process-pr [number]` — forced loop on an open PR (DeepSeek feedback + CI fixes)
- `/develop-iteration [iteration]` — delivers a milestone's issues (parallel agents → PR → merge)
- `/scrum-master feature-spec | backlog-refine | iteration-plan` — scrum ceremonies

Rules that override everything else: work only in git worktrees, start from `develop`, never
push or merge to `main`, never read `.env` files, network/disk/API key only in the backend (the
frontend does not see the key and does not talk to the sources), no new dependency without an
explicit request. A failed privacy check blocks the push even when build and tests are all green.
