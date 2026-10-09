# Working on FERB (for Codex and any other agent)

`CLAUDE.md` is the working agreement for every agent in this repository, not only Claude. Read it first and follow
it; everything below adds to it. Where this file and `CLAUDE.md` differ, ask the owner.

## The owner's working preferences (learned while building 0.27)

- **The owner audits the code and the plans.** Before saying how code behaves, read the code path and cite
  `file:line`. Before saying what happened, read the trace (`ferb trace TASK --full`/`--json`) or the database. Say
  "I inferred" or "I didn't verify" for anything unchecked, and correct an earlier wrong claim as soon as you find it.
- **"Already built" needs evidence.** Explain an existing mechanism step by step (trigger → code path → storage →
  where it's used, with `file:line`) plus live evidence that it runs (counts, dates, errors from traces).
- **Before saying "impossible", read the old trail end to end** when the owner says something used to work: which
  step produced the result, and how (background vs real input).
- **Limits are the owner's decisions.** Size caps, counts, per-task budgets, retention periods, rate limits, tab
  caps: never introduce one in a ruling or because a reviewer suggested it. Bring the risk and a recommended number
  to the owner first. (In F4 a 100 MB download cap nobody chose blocked a real task.)
- **Sequencing is yours to decide.** When a question is only when or in which milestone a decided feature lands,
  decide, record the ruling, and tell the owner. What a feature does, who/which, and real product decisions go to the
  owner (CLAUDE.md rule 19: never ask the owner a method).
- **Quality, goals and security, never build time or effort**, decide designs.
- **Tests come from real documented scenarios on owner-controlled surfaces** (D17), not toy apps; unit tests still
  guard code; live results are reported apart from offline ones.
- **Production code is written fresh in `ferb/`**; `spikes/` findings and tests may be ported, never imported.
- **Live tests (`-m real`) use the Codex runtime only**; never run `-m real_claude` (the owner runs it).
- **"Codex astra" / "code astra" means the Codex CLI running the `gpt-6-astra` model**, never an editor plugin.
- **The owner's north star:** an agent that controls the Mac the way the owner can, in the background, and keeps
  learning by doing.

## How milestones are built

1. **Plan.** A plan in `docs/superpowers/plans/<date>-<name>.md`, written against the real code (Goal,
   Architecture, Spec, Global Constraints, Review Focus, then tasks with files, interfaces, a failing test first,
   implementation, commit citing decision IDs, and a Self-Review). The owner reviews it, unless they already said
   to start once it is made.
2. **One task at a time**, each by a fresh implementer (a new context, e.g. a separate `codex exec` run) given the
   task's text, the interfaces and rulings from earlier tasks, and a report file to write. The implementer writes the
   failing test first, implements, runs the task's focused tests, then `uv run pytest -q`, `uv run mypy ferb/core`
   and `git diff --check`, and commits.
3. **A fresh reviewer per task** (spec compliance, then code quality), given the task text, the implementer's report
   and the diff, read-only. Findings go back to the implementer as a fix round (up to three with the same
   implementer, then a fresh one on a stronger model), each followed by a scoped re-review, until no Critical or
   Important finding remains.
4. **Rulings.** The controller (the agent running the plan) decides conflicts and ambiguities instead of stalling,
   and records each in the ledger as `Ruling: <what> — <why> — <what it costs if wrong>`. It stops only for an
   owner decision, an irreversible or security-sensitive action, or a plan too broken to follow.
5. **Push** `astra/0.27-upgrade` after each task passes review (never merge, never force-push).
6. **After all tasks:** one whole-branch review on the most capable model, one fix wave, then the owner installs
   (`ferb setup --update`, run by the owner) and runs the live checks listed in `docs/OWNER_ACCEPTANCE.md`. The
   owner's verdict and sign-off are recorded there.

## Where the state lives

- **The plan of record:** `docs/0.27_PLAN.md` (decisions, with the owner's dated clarifications) and
  `docs/0.27_DECISIONS.toml` (each decision's milestone and tests).
- **Each plan's ledger:** `.superpowers/sdd/<plan file stem>/progress.md`. It is git-ignored and lives only on this
  Mac. It holds the pre-flight scan, every dispatch, review verdict, ruling, parked item and completed task with its
  commits. Trust it and `git log` over memory: tasks with a `Task N: complete` line are done; resume at the first
  task without one. Task texts, reports and review diffs sit beside it.
- **Helper scripts** (plain bash, usable by any agent), in
  `~/.claude/plugins/cache/claude-plugins-official/superpowers/6.4.1/skills/subagent-driven-development/scripts/`:
  `sdd-workspace PLAN` prints the plan's ledger folder; `task-brief PLAN N` writes task N's text to a file;
  `review-package PLAN BASE HEAD` writes the commit list, stat and diff for a reviewer.
- **Live results and pending checks:** `docs/OWNER_ACCEPTANCE.md`.

## Where things stood on 2026-10-09

F0–F4 are built and signed off by the owner. F5 is split into F5a (containment and confirmations: the shell sandbox,
never-read/never-write lists, the network proxy, the escape hatch, the D23 code-floor confirmations and the S4 sink
rule, the worker permission model, plus the items F3 and F4 carried into F5) and F5b (secret classes and redaction,
screenshot redaction, the start-of-task screenshot, Jev's destructive classifier in shadow, then Full Disk Access by
default). The F5a plan is `docs/superpowers/plans/2026-10-10-f5a-containment.md` (committed 9891157). The owner answered its
eight open questions "all recommended" (2026-10-09; recorded in its ledger). Tasks 1-2 are complete (the owner also confirmed
2026-10-10 that agents FERB starts run inside its sandbox with their own sandbox off); Task 3 (the egress proxy) is
in progress. Its ledger is
`.superpowers/sdd/2026-10-10-f5a-containment/progress.md`.
