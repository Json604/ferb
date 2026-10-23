# Database clean-out, perception loop cost, browser procedures without a hold — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** stop the installed FERB from heating the Mac and filling the disk, and let browser procedures learned by
L29 work without the owner's confirm, before the owner's next `ferb setup --update`.

**Architecture:**
1. A one-time clean-out run by setup while the daemon is stopped. It rewrites each run card's source edges to
   what the current card rule gives, then compacts the database.
2. The perception batcher keeps its decisions exactly, but stops re-reading, re-encoding and re-counting every
   unsent event each second.
3. One provenance rule in the learning card reader: a step's site, and a step with no recorded provenance, no
   longer hold a procedure.

**Tech Stack:** Python 3.12, SQLite (WAL), tiktoken o200k, pytest.

**Spec:** this plan is the spec for this work. It records the owner's decisions and the measured evidence (no
separate spec file):
- **Owner, 2026-10-22:** "i dont want my mac hot al the time". FERB was stopped at the owner's request.
- **Owner, 2026-10-23:** "clean out yes, so we dont face performance issues with ferb again". On browser holds: "i
  dont want that annoying rec again, cant we just leave browser thigns as it is … make it just bare minimum so that
  our loop also does not break". This amends L29 spec §"a step built on outside content quarantines the procedure",
  as Task 2 defines.
- **Measured, 2026-10-22, on the owner's installed daemon (3a4985a, the 2026-10-15 02:21 install):**
  - The process ran 7 d 5 h and used 17 h 43 m of CPU. Its footprint was 9.8 GB (peak 12.6 GB).
  - In a 15 s sample the main thread was about 93% busy, in tiktoken, JSON encoding and SQLite scans.
  - `top` showed a steady 22–30%, with bursts to 100%.
  - The DB is 8.1 GB:
    - `learning_sources`: 2.5 GB, its primary-key autoindex 2.8 GB, and the `learning_source_ref` index 1.2 GB.
    - That is 22.6M rows on 438 episode-card versions of 85 tasks, about 57k `trail:` refs each.
    - The cause: the installed card builder copied every `context.exposure` source ref. This was fixed in e0e344b,
      but the old versions keep their rows, and an untraced re-read copies the old card's sources.
    - All 85 tasks still have spans.
  - The trail gets about 150 events a minute, 93% of them accessibility events (AXValueChanged and focus changes
    from VS Code and Chrome), even overnight. There were 32,718 Jev batches in 7 days.
  - `EventBatcher.run` (`ferb/sense/batches.py`, unchanged at HEAD) wakes at least every second. On each wake it
    runs `pending()`, a LEFT JOIN of the whole trail since the floor against receipts, then
    `count_tokens(json.dumps(all pending))`.

## Global Constraints

- One branch `astra_resume`, cherry-pick integration, never force. Never read `vibed/`. Subagents never read
  `~/.ferb`.
- Never install, restart or start FERB, FERB.app or a browser. The owner runs `ferb setup --update`.
- No new numeric limit that changes behaviour (EVENT_BUDGET = 10000 and OLDEST_SECONDS stay exactly as they are). A
  precondition such as "enough free disk to compact" is not a limit.
- No keyword or label list judges intent or content (R2). Secrets never reach a model, trace, log or the DB (rule
  14).
- Runtime Markdown is executable input. Failing test first. Focused tests, then the full suite at integration, plus
  `mypy ferb/core` and `git diff --check`.
- Deleting data happens only for rows the current rule provably would not write. It never adds edges, and it never
  touches non-episode nodes.
- The commit messages for this plan cite: L28 (the card's lineage) and C1 for Task 1; P1/D10 for Task 3; L29, MP2
  and L2 for Task 2.

## Review Focus

1. A card whose trace aged out must keep every source the current builder would keep. The clean-out leaves it alone
   rather than guess.
2. After the clean-out, forgetting a trail row that a card's action really used must still forget the card.
3. A clean-out interrupted halfway (power loss, Ctrl-C) leaves a consistent store and finishes on the next run.
4. The batcher's flush decisions must match the old code exactly, at the EVENT_BUDGET boundary and under bursts of
   notifications and interrupts.
5. A recipient, account, contact, path or amount found only in outside content must still hold a procedure after
   Task 2.

---

### Task 1: Clean out the card source rows and compact the database (lane P1)

**Files:**
- Modify: `ferb/memory/learning.py`. Factor the source collection out of `prepare_episode` (`:523-620`) into
  `card_source_refs(task_id) -> set[str] | None`, where None means untraced. Add `prune_card_sources()`.
- Modify: `ferb/cli/setup.py` and/or `ferb/device/install.py`. Add a setup step, run while the daemon is stopped.
- Modify: `ferb/store/db.py`, only if a compaction helper belongs there.
- Maybe add `ferb/store/migrations/0043_*.sql`, for a maintenance marker if one is needed.
- Test: `tests/test_card_source_cleanout.py`.

**Interfaces:**
- Produces:
  - `Learning.card_source_refs(task_id: str) -> set[str] | None`, used by `prepare_episode`. No behaviour change.
  - `Learning.prune_card_sources() -> dict`, which returns counts: nodes, rows removed, untraced nodes left alone.
  - A setup step that prints what it did, e.g. "Cleaned FERB's database: 22,6xx,xxx unused rows removed; 8.1 GB →
    x.x GB".

- [ ] **Step 1: Failing tests.**
  - An episode node (any version, any state) whose edges hold refs from non-request `context.exposure` spans loses
    exactly those rows. Its `episode:`, request-exposure, procedure-source and action-derived refs stay. Build
    the spans the way the gateway and supervisor record them; check against `card_source_refs`.
  - A card built by the current builder loses nothing.
  - An untraced card is left alone and counted.
  - Non-episode nodes are untouched.
  - The run is idempotent: a second run removes 0.
  - After pruning, `forget` of an action-derived trail row still forgets the card and its descendants.
  - After pruning, `forget` of an exposure-only trail row no longer touches the card. That is the current design:
    see the `prepare_episode` docstring.
  - An interrupted run, with an exception thrown mid-way, leaves every node either fully pruned or untouched, and
    the next run completes.
  - Compaction runs only when free disk ≥ the DB plus its WAL; otherwise it is skipped, with a message.
  - The setup step runs before the daemon is loaded or started, and never while it is running. Verify the real
    order in `install.py` and `setup.py`.
- [ ] **Step 2: Implement.**
  - Delete per node, in its own transaction, using a keep-set temp table. Never delete outside episode nodes.
  - Compact with `VACUUM` after a WAL checkpoint, on setup's own connection with the daemon stopped.
  - Report sizes before and after.
  - `ferb doctor` should show the DB size and whether a clean-out is pending, if that's cheap.
- [ ] **Step 3: Cost probe (not a committed test).** Build a synthetic store at the measured shape: 438 versions ×
  58k refs, or a tenth of that, then extrapolate. Time the prune and the VACUUM. Report the numbers.
- [ ] **Step 4: Docs.**
  - CHANGELOG: under the current unreleased section, describe the bug and the clean-out, with the measured numbers.
  - README: setup or troubleshooting, saying the first update cleans the database and may take minutes.
  - ARCHITECTURE: the store and learning sections.
  - OWNER_ACCEPTANCE: add live check 79. After `ferb setup --update`, record the setup line, `ls -la ~/.ferb/ferb.db`,
    and Activity Monitor CPU after 10 idle minutes.
- [ ] **Step 5: Focused tests, mypy, diff-check, commit.**

### Task 2: Browser procedures without a hold, bare minimum (lane P1, after Task 1)

**Files:**
- Modify: `ferb/memory/learning.py`: `outside_targets` / `outside_action` (`:291-322`), and `CARD_MAKER`.
- Test: `tests/test_learning.py` (next to the I1a tests near `:634`).
- Docs: CHANGELOG (the L29 entry and residuals), README, docs/FEATURES.md, ARCHITECTURE ("Outside actions" and
  "Procedure identity across revisions"), OWNER_ACCEPTANCE check 78, `docs/0.27_DECISIONS.toml` [L29] note, and a
  dated amendment note at the end of the L29 spec.

**Rule (owner, 2026-10-23):** only the card reader changes. There is no gateway change.
- A step's `site` key never makes it outside. Under L2, browsing any site needs no question. The site's own page
  names its address, so every typing or clicking step on a site FERB had read counted as outside.
- An action with no `arg_sources` at all (the reversible kinds: opening a site, app or tab, focusing, Trash, timers,
  recovery points, stopping a command) counts as FERB's own choice: not outside.
- Everything else is unchanged:
  - a recipient, account, contact, path or amount found only in outside content still makes the step outside, and
    holds the procedure until the owner confirms;
  - the owner's words and answers count as the owner's, and "unknown" is not outside;
  - the I1 carry-over rule is unchanged.
- Bump `CARD_MAKER` (to `code:evidence-card/4`) so existing cards are read again once. Labels carry forward with no
  model call.

- [ ] **Step 1: Failing tests.** Build each `arg_sources` with `provenance.arg_sources` itself.
  - Navigate, type and click on a site whose page FERB read, with the owner never naming it: a candidate, then
    `learned` after three verified runs with two dishes. No confirm.
  - A WhatsApp-style send to a recipient found only in an email body: quarantined.
  - The same recipient typed by the owner: candidate.
  - An account found only in outside content: quarantined.
  - A missing `arg_sources`: not outside.
  - The `CARD_MAKER` re-read.
  - Remove or update the tests that pinned the old rule (missing `arg_sources` is outside; the site is outside) and
    say which.
- [ ] **Step 2: Implement** (a few lines in `outside_targets` / `outside_action`).
- [ ] **Step 3: Docs.**
  - Remove the "browser procedures start quarantined" residual and the confirm step in check 78. Say what still
    holds a procedure.
  - Record the amendment, quoting the owner, in the decision record and the spec note.
  - The owner's earlier option of recording provenance for reversible kinds is now moot. Say so.
- [ ] **Step 4: Focused tests (learning, consolidation, intent labels, e2e, planner context, f6), mypy, diff-check,
  commit.**

### Task 3: The perception batcher's cost (lane P2, parallel with Tasks 1–2)

**Files:**
- Modify: `ferb/sense/batches.py`: `EventBatcher.run`, `pending`, `flush`, `batch_events`, and the notification
  trimming in `_request` (`:184`).
- Maybe add a migration for an index on `trail(seq)`, if one is missing. Check `ferb/store/migrations`.
- Test: `tests/test_batcher_cost.py`, plus a reference copy of the current module at
  `tests/support/batches_reference.py` for the equivalence tests.

**Requirements:**
- Every decision stays identical to the current code:
  - whether to flush, and the reason (interrupt, tokens, age);
  - when it flushes;
  - which events go in which batch, and the excerpts;
  - the notification context trimming, `omitted_oldest_messages` included;
  - receipt handling, the floor, and behaviour when Jev is unavailable or comes back.
- EVENT_BUDGET and OLDEST_SECONDS keep their values and meaning, and token counts at decision points stay exact.
- The cost of each wake is proportional to the new events, not to the trail's or the pending set's size:
  - read new trail rows by `seq` after the last one seen, and keep the pending set in memory, consistent with
    receipts and the floor (reload it when a flush or a restart changes them);
  - keep the per-event JSON and token counts cached;
  - get the exact list count without re-encoding and re-counting every event on every wake: an exact incremental
    method, or exact counting only when a proven bound can't decide;
  - don't wake on a fixed 1 s timer when nothing changed. Wake on new events, or at the computed age deadline, and
    stay responsive to stop;
  - `batch_events` and the notification trimming loop must not be quadratic.

- [ ] **Step 1: Failing tests.**
  - Equivalence against `batches_reference.py` on seeded random event streams: sizes around the budget, every
    source, notification bursts, interrupts, excerpts, Jev unavailable then available, and restart with pending
    events. Compare flush reasons, batches and requests.
  - A cost test that asserts work done (rows read, events encoded, tokens counted) per wake with N pending, not
    wall time.
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Cost probe (not committed).** Use a synthetic trail of 400k rows with 1,200 pending. Measure the
  per-wake time and the CPU over 60 s of simulated idle, old against new.
- [ ] **Step 4: Docs.** ARCHITECTURE perception section, and the CHANGELOG with the measured numbers.
- [ ] **Step 5: Focused tests (batches, perception, notifications, sense), mypy, diff-check, commit.**

## Not in this plan (owner decisions, raised separately)

- Fewer accessibility events: merging repeated value-changed events for one field, or not recording some apps'
  events. This changes what FERB perceives.
- How often Jev is called.
- The activity summaries' source edges (54,857 rows on 47 nodes): small, and as designed.
