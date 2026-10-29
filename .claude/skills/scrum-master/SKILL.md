---
name: scrum-master
description: Scrum ceremony facilitator. Use for backlog refinement, iteration planning, and feature specification.
argument-hint: <ceremony>
allowed-tools: Bash(gh *) Bash(git *) Bash(bash *) Bash(cat *) Bash(xargs *) Bash(date *) Bash(echo *) Bash(set *) Bash(jq *)
---

# scrum-master — scrum ceremonies

Trigger: `/scrum-master <ceremony>`, where the ceremony is one of:
- `backlog-refine` — backlog refinement
- `iteration-plan` — planning of a completion-driven iteration
- `feature-spec` — feature → technical specification → issues

## Purpose

Manages the repository's development cycle. The user brings the ideas; this skill listens,
writes the technical specifications, creates the issues and organises backlog and iterations.

Everything this skill publishes (issue titles and bodies, comments, plans, ceremony summaries)
is written in English.

## Ceremonies

### 1. backlog-refine

Trigger: `/scrum-master backlog-refine` or from a ceremony issue.

**Workflow:**

1. Read all open issues:
   ```bash
   gh issue list --state open --json number,title,labels,createdAt,updatedAt --limit 100
   ```

2. Exclude ceremony issues (label `ceremony`).

3. Classify each issue against the same required-field contract that `/dev-issue` applies
   (Area, Type, Priority, User story, Acceptance criteria, Technical context, Out of scope),
   reading **only the issue body**: it is all that `/dev-issue` sees. Older issues carry the
   same fields under their Italian headings (`### Tipo`, `### Priorità`,
   `### Criteri di accettazione`, `### Contesto tecnico`, `### Fuori scope`, …): accept them.
   - **Ready**: all seven fields present and substantive in the body
   - **Needs specification**: a required field is missing or is only a placeholder
   - **Needs triage**: new or unclassified
   - **Stale**: no activity for 30+ days → propose closing or redefining

4. For each **Needs specification** issue:
   - read the body;
   - write the missing acceptance criteria starting from the user story;
   - write the missing technical context starting from the area and the existing code;
   - write **Out of scope** if missing, with concrete files/modules/behaviours, never
     "unrelated changes": it is `/dev-issue`'s only defence against scope creep;
   - **write every field in the issue body. A comment is not enough.** `/dev-issue` reads the
     issue with `gh issue view <number> --json title,body,labels` and derives the fields only
     from the body, it never reads the comments.

     `gh issue edit --body-file` replaces the **whole** body, so the edit must be surgical by
     discipline: replace only the `### <Field>` section and reproduce every other byte. Read,
     rewrite, replace, then reread the body and verify that the only difference is that section
     (GitHub adds a single trailing newline). When you add a section to an older issue, use the
     English heading; leave the existing Italian headings as they are.

     ```bash
     gh issue view <number> --json body --jq '.body' > /tmp/issue-<number>.md
     # Rewrite the file with the section replaced — a heredoc
     # `cat > /tmp/issue-<number>.md <<'BODY' … BODY`, because this skill's allowed-tools
     # include `cat` and no file-editing tool — then:
     gh issue edit <number> --body-file /tmp/issue-<number>.md
     gh issue view <number> --json body --jq '.body'   # reread and compare
     ```

   - Also add a comment when a finding must reach a person through notifications (an ordering
     risk, a dependency between issues). The comment is the trail, the body edit is the
     delivery. Never only the comment.

5. Reorder priorities based on:
   - dependency chains (backend module before the Tauri command, command before the UI);
   - value (management of installed addons before the catalogue, the catalogue before polish);
   - effort (alternate quick wins and large pieces of work).

6. Update the labels when an issue is ready: `needs-refinement` → `ready-for-dev` only when all
   seven fields are present and substantive **in the body**. `iteration-plan` picks work from
   `ready-for-dev`, so a label earned with a comment plans work on which `/dev-issue` would stop
   at the first step.

7. Post a summary comment on the ceremony issue (or in a new issue if started by hand) with:
   - total of open issues;
   - breakdown by category;
   - suggested priority order for the next iteration;
   - issues proposed for closing.

8. Close the ceremony issue.

### 2. iteration-plan

Trigger: `/scrum-master iteration-plan` from the ceremony issue created by
`iteration-lifecycle.yml` for the milestone.

**Cold start (new repository)**: if NO iteration milestone exists, open or closed, there is no
milestone to verify and no ceremony to wait for. Skip steps 1–4: directly create the undated
milestone `Iteration 1`, populate it from `ready-for-dev` (steps 5–8) and publish the plan.
After the first iteration everything goes through the completion-driven state machine.

**Workflow:**

1. Identify the active iteration: the open milestone with the **lowest** sequence, read from an
   `Iteration N` / `Sprint N` title and failing that from the milestone number, the same rule as
   `iteration-advance`. Fail closed if no milestone is open (outside the cold start) or if two
   open milestones have the same **lowest** sequence. A collision higher up does not count until
   it becomes the lowest.
   ```bash
   REPO=$(gh repo view --json nameWithOwner --jq '.nameWithOwner')
   gh api "repos/$REPO/milestones?state=open&per_page=100"
   ```

   **Several open milestones are an expected state.** Planning ahead is the point: a coherent
   block of work can be assigned to a future iteration while the current one is still in
   progress. Creating and populating an iteration is always possible; **closing** a completed
   milestone is what steps 2 and 3 control.

2. **To close the completed milestone it must have zero open issues.** Iterations are
   completion-driven: never move unfinished work automatically and never close a milestone that
   still has open issues. This applies only to step 9.

3. **Closing it also requires its refinement.** Read the
   `<!-- iteration-lifecycle:iteration-plan:milestone-N -->` marker of the planning ceremony and
   require exactly one closed backlog-refinement issue with the matching marker
   `<!-- iteration-lifecycle:backlog-refine:milestone-N -->`. Refuse to close if the refinement
   is missing, open or duplicated.

4. Identify the target sequence: **the one in the ceremony's title, one above the active
   iteration.** With planning ahead it may already exist:

   - **it exists and contains work**: the batch was chosen when it was planned. Steps 5–7 have
     nothing to select and step 8 creates nothing; step 9 remains.
   - **it does not exist, or exists empty**: this is the normal case, select the work and create
     it in step 8.

   **Never create a second milestone on a sequence that already has one.** To plan further
   ahead, take the sequence above the **highest** existing one, open or closed. Do not change
   the milestones' state yet.

5. Read the issues in priority order from the completed refinement or from the priority labels.

6. Select the issues for the iteration:
   - a realistic batch for a single developer;
   - balanced between backend and frontend when the backlog allows it;
   - at least one bug or piece of technical debt if there are any;
   - respect every blocking dependency;
   - at least one issue selected; if none is ready, explain the blocker and leave the ceremony
     and the completed milestone open. This applies only if step 4 identified a milestone to
     create or to add work to.

7. For each selected issue:
   - estimate: S (hours), M (1–2 days), L (3–5 days);
   - check the blocking dependencies;
   - suggest an implementation order.

8. Create the undated milestone for the step 4 sequence (or `Iteration 1` at cold start),
   assign it every selected issue and verify the assignments before closing the completed
   milestone. This order leaves the previous milestone recoverable if creation or assignment
   fails. If the target milestone already exists, assign the work there.
   ```bash
   for issue in <issue-numbers>; do
     gh issue edit "$issue" --milestone "Iteration <number>"
   done
   ```

9. Close the completed milestone only after the new iteration contains the chosen work, and
   only if steps 2 and 3 are satisfied. If the active iteration still has open issues, leave it
   open. (Cold start: nothing to close.)

10. Publish the plan as a comment with:
    - iteration goal (one sentence);
    - list of issues with estimate and area;
    - total estimated effort;
    - dependencies and risks.

11. Close the ceremony issue. Closing it relaunches the lifecycle helper, which should see the
    new active iteration and change nothing.

### 3. feature-spec

Trigger: `/scrum-master feature-spec <feature-description>`

**Workflow:**

1. Compare the description with the project documents: `docs/context.md` (why WoW-don exists
   and what it must do better than WowUp-CF), `docs/roadmap.md` (milestones),
   `docs/architecture.md` (planned modules) and `docs/decisions.md` (decisions made and open).
   Ask questions if:
   - the area is not stated and not obvious;
   - the scope is ambiguous;
   - the dependencies are unclear;
   - the feature contradicts a recorded decision (say which one, and ask).

2. Write a technical specification (in conversation):
   - **Summary**: one paragraph about the feature
   - **Areas involved**: backend, frontend or both
   - **Backend (Rust/Tauri)**: modules in `src-tauri/src/`, exposed Tauri commands, types
     serialized to the frontend, disk I/O, error handling
   - **Frontend (React/TypeScript)**: components, state, search/filters/sorting, `invoke()`
     calls, following the rules of the "Interface" section of `AGENTS.md` (minimal and modern
     UI); every frontend issue repeats the constraint in the Technical context
   - **External sources** (if needed): CurseForge endpoints used, limits, fields to verify
     against the official documentation
   - **Data and persistence** (if needed): local state, format, migrations
   - **Constraints**: API key only in the backend and never in the logs; no scraping and no
     keys from other apps; no automatic download with `allowModDistribution = false`; safe zip
     extraction (no path outside `AddOns`); paths inside Wine prefixes with spaces; no new
     dependency without discussing it
   - **Risks**: what can go wrong, what needs attention

3. Get the specification approved. Do NOT create issues until the user confirms.

4. After approval create the issues in the feature template's format.

   **The issue body is a contract with `/dev-issue`.** That skill reads these headings and
   treats Area, Type, Priority, User story, Acceptance criteria, Technical context and Out of
   scope as required, like `.github/ISSUE_TEMPLATE/feature_request.yml`. An issue without one of
   these blocks the pipeline at the first step. Write **all seven**, always.

   The dropdown values are fixed strings; use them verbatim:

   | Field | Allowed values |
   |-------|----------------|
   | Area | `backend (Rust / Tauri)` · `frontend (React / TypeScript)` · `shared (Cross-cutting)` |
   | Type | `feature (New feature)` · `enhancement (Improvement to an existing feature)` · `refactor (Restructuring with no behaviour change)` |
   | Priority | `high (Blocks other work)` · `medium (Normal priority)` · `low (Useful but not urgent)` |

   For each unit of work:

   ```bash
   gh issue create \
     --title "[Feature]: <component description>" \
     --body "$(cat <<'EOF'
   ### Area

   <one of the three allowed values, verbatim>

   ### Type

   <one of the three allowed values, verbatim>

   ### Priority

   <one of the three allowed values, verbatim>

   ### User story

   As a <role>, I want <goal>, so that <reason>

   ### Acceptance criteria

   - [ ] <criterion 1>
   - [ ] <criterion 2>

   ### Technical context

   <relevant files, patterns and constraints from the specification>

   ### Out of scope

   - <what this issue must NOT touch>

   ### Code pointers

   - `src-tauri/src/commands.rs:12` — <why it matters>

   ### Dependencies

   <blocking issues, or "None">

   ### Test strategy

   <what to test and at which level>
   EOF
   )" \
     --label "feature" \
     --label "needs-refinement"
   ```

   **Out of scope is the field with the most leverage.** It is the only boundary `/dev-issue`
   has against scope creep, and it is the one most often left vague. Name concrete files,
   modules or behaviours.

   Bugs use `.github/ISSUE_TEMPLATE/bug_report.yml`, which requires Area, What happened?,
   What should have happened?, Steps to reproduce, Technical context and Out of scope, without
   Type/Priority/User story.

5. Group the issues:
   - **foundations first**: backend types and modules, data formats, source clients;
   - **then the core**: Tauri commands and main UI flows;
   - **finally polish**: edge cases, error handling, visual feedback.

6. Report to the user:
   - number of issues created;
   - suggested implementation order;
   - total estimated effort;
   - link to the specification.

## Sizing: an issue is written against the review limit

`DEEPSEEK_MAX_DIFF_CHARS` is **90,000** characters. A PR beyond the limit is truncated, every
unread file becomes a finding and the PR stays blocked until someone acknowledges it.
`/dev-issue` Step 5 splits an issue that is too large into parts, but it is a remedy: when
`/dev-issue` reads the issue, its shape is already fixed. This is the moment when a boundary is
chosen without code already written.

### Estimate every issue you write or refine

Count, instead of guessing:

- the **areas** (backend, frontend);
- the **kinds of surface** the issue names: a mechanism, a data model, a Tauri command, a UI
  screen or control, persistent state, a client for an external source. One kind is one PR; two
  kinds are normally two PRs;
- the **acceptance criteria**: beyond six or seven, the groups they fall into are the
  boundaries;
- the multipliers: tests are worth about two thirds of a change with real coverage.

Two symptoms that there is more than one part, readable while you write the body:

- **the summary needs an "and" between two nouns of a different kind** (a parser *and* a
  screen, a client *and* the UI that uses it);
- **more than six or seven acceptance criteria.**

### What to do when an issue is too large

**Better to split the issue.** Two issues, each one PR, each with its own criteria and its own
`Dependencies` line, are better than one issue and a note: the milestone counts the real work
and `/dev-issue` works without having to decide on its own.

**When the work really is a single issue**, keep it whole and write the boundary in the body
under a `Suggested split` heading, with each part, what it contains and the merge order.

**Never split on a character count.** The boundary is one the code already draws.

### Write the axes of the estimate, not the verdict

Put the estimate and the reasoning in the body (or in the refinement comment): areas counted,
kinds of surface counted and named, multipliers, and a band stated as a **minimum, not as a
count**. Do not write "One PR" as a verdict: an agent that reads an already written number
inherits it instead of redoing the estimate, and the decision is made by whoever is furthest
from the code.

## Guardrails

- Never create issues without the user confirming the specification.
- Never modify closed issues or milestones.
- Respect the areas: no backend issue for a frontend-only feature.
- A feature that brings data from the backend to the frontend ALWAYS has a backend part (Tauri
  command and serialized types): make it explicit instead of hiding it in a UI issue.
- **Size every issue against the 90,000-character limit before assigning it to a
  milestone.**
- When in doubt about the scope, ask.
- **Every issue body and every comment goes through `bash scripts/check-body-privacy.sh` before
  being published** (for example `bash scripts/check-body-privacy.sh < /tmp/issue-<number>.md`):
  no real email addresses, paths of your own machine or private network addresses. In code
  pointers use paths relative to the repository.
- Ceremony issues (label `ceremony`) are excluded from all reports.

## References

- Context, roadmap, architecture, decisions: `docs/`
- Issue templates: `.github/ISSUE_TEMPLATE/`
- Pipeline: `AGENTS.md`
- The review limit and its costs: `AGENTS.md`, "Automated review (DeepSeek)"; `/dev-issue` Step 5
- Helper: `scripts/gh-automation.sh`
