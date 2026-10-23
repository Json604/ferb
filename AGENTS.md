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
- **The app is the product** (owner, 2026-10-14): the notch app / FERB.app is the main interface; the CLI is a
  development and power-user tool (CLAUDE.md rule 18).

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
5. **Push** `main` after each task passes review and is integrated (CLAUDE.md rule 1: never force-push, and no
   branch per step; `astra_resume` became `main` on 2026-10-24).
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

## Where things stood on 2026-10-14 (launch week)

**Done and signed off:** F0–F4 (owner sign-off in `docs/OWNER_ACCEPTANCE.md`). F5–F10 were built and installed
(2026-10-11) with full offline validation; their owner live acceptance is still pending.

**Launch-week fixes (L1–L28), integrated on `main` (then `astra_resume`); installed 2026-10-24, live checks pending.** Spec
`docs/superpowers/specs/2026-10-13-launch-week-fixes-design.md` (owner-approved 2026-10-13, with L25–L28 added at
approval and on 2026-10-14); plan `docs/superpowers/plans/2026-10-13-launch-week-fixes.md`; ledger
`.superpowers/sdd/2026-10-13-launch-week-fixes/progress.md` (read it first: every dispatch, ruling, residual and
parked item, plus each task's report and the integration reports). Tasks 1–18 and 20–22 and the UI follow-ups
U7b, U8 and U9 are reviewed and integrated; Task 19 is these docs. What landed, in short:
- questions: FERB asks only for money, account security, permanent deletion and sends to someone new outside a
  standing scope (plus S4, D31, P19); shell and browser reach any site, recorded and listed in the answer; answers
  are routed by question id in the owner's own words; questions never expire; big-job plans are stated, not blocking;
- chat: the daemon's instant ack, the plan line, one live line, late answers at the bottom, notices in chat, inline
  questions, and every event carrying `request_id` and `at`;
- seeing and hearing: the front window with every message, browser windows and recordings redacted instead of
  refused, `audio.listen` during tasks with on-device transcription;
- context: the planner uses each model's real window with the situation first (Codex model `gpt-5.6-sol`);
- broken features: learning, typed text, choice answers, Codex account errors, waits after sleep, the inventory walk;
- distribution: one ad-hoc signed FERB.app in a DMG (the daemon runs as the agent app FERB), extension auto-install
  requests and the Chrome Web Store kit, the launch campaign path through standing permissions and schedules;
- the license is Apache-2.0.

**Learning by intent (L29, 2026-10-21), not yet installed or live-tested.** Spec
`docs/superpowers/specs/2026-10-21-learning-by-intent-design.md` (owner-approved: "yes proceed"); plan
`docs/superpowers/plans/2026-10-21-learning-by-intent.md`; ledger
`.superpowers/sdd/2026-10-21-learning-by-intent/progress.md`. Each card gets a model-written intent label; runs are
consolidated by intent after three verified runs; the planner picks a procedure by intent and fills its slots; the
worker starts from the filled steps, lessons and the latest runs' tool calls. Live check 78 in
`docs/OWNER_ACCEPTANCE.md`; residuals in CHANGELOG.

**Pending, in order:**
1. The whole-branch final review on the most capable model, then one fix wave (the Task 7+18 and Task 21
   integration follow-ups are in, up to `6ac9fb5`).
2. The owner installs: `ferb setup --update` (run by the owner) and a rebuilt notch app, or FERB.app from a DMG.
   Done 2026-10-24 (the clean-out took the database from 8.5 GB to 2.3 GB); later fixes need another update.
3. The live week: the owner runs the checks in `docs/OWNER_ACCEPTANCE.md` ("0.27 launch week") and records
   `ferb verdict TASK pass|fail`; the campaign's first posts run supervised under test mode.
4. The Chrome Web Store listing (id `plbfkifcmgkgnlfgikdeaaglgncdhkma`, the built-in `STORE_EXTENSION_ID`) has carried
   0.15.6 / protocol 17, which this build needs, since 2026-10-22.
5. Git cleanup: done 2026-10-24 (`astra_resume` fast-forwarded into `main`, the other branches deleted, the Safari
   spike kept as the tag `archive/spike-5-safari`; L14). Next: the phase-2 security design (spec section 6).

**Process notes:** commits between the first Task 22 pick (`9db0a5a`) and `906cb61` are not green on their own (the
fixture fix is `1af6731`): never install from them, and skip that range in `git bisect`. Parked for the final
review: two load-sensitive tests (`test_downloads` concurrent-transfer timing, `test_acceptance_one_worker::test_3`)
that passed when rerun alone, a `ShadowDecider._drain` teardown warning in `test_mac_shell`, and the five
`test_orchestrated_control` tests that fail only without the tiktoken o200k_base cache (from F5a). Real tests use the
Codex runtime only. The F5a handoff state of 2026-10-10 is in Git history and its own ledger.
