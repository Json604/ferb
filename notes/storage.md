# Storage and data growth: decisions and fix list

Area: `ferb/store/*` (db.py, migrations runner, migrations/*.sql, journal, spans), storage parts of `ferb/trace`,
`ferb/orchestrator/{tree,plan_store,episodes,schedules,continuity,conversations}.py`, timers, `ferb/sense/trail.py`,
retention in `ferb/daemon/main.py` (~2427-2613) and `ferb/store/retention.py`.
Basis: a read of every line (2026-10-30) and read-only queries of the live DB (`mode=ro`; counts, sizes, kinds only).

## Owner decisions (2026-10-30)
1. **Raw traces (spans) kept 30 days** after the task's episode exists (today's rule).
2. **The hand-made copy `~/.ferb/ferb.db.before-restore-20261025-224330` deleted** (done 2026-10-30, 3.09 GB freed; no
   process had it open).
3. **Operational records kept 90 days** after their task or timer ends: action ledger, confirmations, plan steps,
   timer deliveries, operational journal kinds. **Task results stay forever** with tasks (the app's task history joins
   them, main.py:4972-4973).
4. **One-time cleanup of dead tables: yes**, once the new code is installed: a setup step with the daemon stopped
   shows what it will delete, waits for the owner's OK, deletes in batches, journals the counts, then compacts
   (VACUUM with incremental auto-vacuum). Never touches memory, episodes, chat, tasks, schedules or in-window traces.
5. **Upgrading from any older FERB is a first-class path** (owner: crucial for friends on older versions). The new
   version must upgrade an install from any earlier build safely and automatically:
   - migrations from every older schema (including installs that applied the deleted 0040), then the cleanup step
     when the old tables hold data;
   - config from older layouts (persona proposal moved out of config.toml, removed keys ignored, new defaults);
   - the old `~/Library/LaunchAgents/com.ferb.daemon.plist` removed when the app registers its service (SMAppService);
   - native-host manifests rewritten; an extension older than the new minimum asked to update (protocol bump for tab
     groups);
   - Codex re-pinned; `permissions_version` and new permissions re-checked;
   - old secrets, memories, schedules and episodes carried over.
   Tested on copies of real older databases and configs (the owner's own and an older build's) before release.

## What exists today (verified)

- `~/.ferb/ferb.db` 3.09 GB: 2.32 GB used, **0.65 GB free pages** (`freelist_count` 159k), `auto_vacuum=0`, WAL
  17.7 MB, schema 48. `ferb setup` compacted 8.46 → 2.30 GB on 10-24; it refilled to 3.1 GB and stays flat because new
  data fills free pages. **A second 3.09 GB copy, `~/.ferb/ferb.db.before-restore-20261025-224330`, exists; no FERB
  code creates or reads it** (made by hand on 10-25).

| Table | MB | Rows | Per day | Retention | After the decisions |
|---|---|---|---|---|---|
| spans | 1,114 | 87k | ~6.3k rows, ~55 MB | 30 d, once the episode exists | short-lived; 191 MB is `source_refs` lists in context spans; task.evidence outputs 204 MB (10-11…14) |
| trail (+fts, indexes) | 455 | 803k | ~59k | 30 d, summarized days only; first purge ran 10-30 | accessibility (92%) → 7 d |
| learning_sources | 457 | 1.6M | spiky (1.51M on 10-25) | none | mostly removed (97% are `trail:` refs on episode cards) |
| perception_receipts + batches | 138 | 886k | ~66k | **none** | stop (Jev off) |
| journal | 91.5 | 4.9k | ~0.7 MB | **never** | 79 MB is `chat.reply.source_refs`, read only by the echo scrub (removed) |
| decisions | 36 | 127k | ~8k | 30 d | stop (Jev off) |
| tasks, episodes, ledger, memory, schedules, timers, plans, confirmations… | < 3 each | small | small | mostly none | keep |
| owner_questions, notification_rules/told, empty learning receipts/digests | 0 | 0 | — | — | dead (owner_questions has no SQL anywhere). Conversation and standing-permission tables stay (orchestration decisions 2, 6) |

- The file never shrinks: `VACUUM` only in `Database.compact` (db.py:39-50), called once by setup's old card
  clean-out, gated so it won't run again; no incremental vacuum; no `journal_size_limit`.
- `secure_delete=ON` zeroes every deleted page (purges cost about twice the writes).
- Trail purge deletes a whole day in one autocommit (up to 112k rows, each firing FTS and `trail_changes` triggers);
  no cascade (68 perception receipts already point at purged rows).
- One loop connection, 5 s default busy wait, no explicit `busy_timeout`; worker-thread connections can hold write
  locks long enough to make loop writes fail with "database is locked" (suspected).
- `Continuity.history` and `Journal.entries` read and parse the whole journal (1-2 MB reply rows included).

### Migrations
- Runner applies files with a number above the current maximum, each in a transaction (db.py:52-75). **A new file
  numbered at or below 48 is silently skipped on every existing install**; two files with the same number both apply;
  no checksum, so editing an applied file goes unnoticed; no downgrade; no "DB newer than code" check.
- Gaps: 0027, 0029, 0031 never existed; 0040 was added 10-19 and deleted 10-20 (installs that ran it keep an unused
  column).

### Confirmed bugs
1. Continued trees get degraded episodes: `root = roots[0] if len(roots) == 1 else {}` (spans.py:340-341); all 12
   continued-tree episodes have `verified` NULL and a wrong `ended_at`.
2. Non-ASCII evidence search fails: spans/journal written with `\uXXXX` escapes (spans.py:59,80; journal.py:62),
   searched with `instr(lower(...))` (spans.py:431); 4,965 of 13,189 tool.call outputs contain escapes.
3. Episodes drop context.exposure origins (compose.py:21 vs spans.py:182): after spans age out, outside-content
   labels from exposures are lost.
4. A crash between marking a schedule timer handled and booking the next one strands the schedule (main.py:2871,
   3574-3577); nothing reconciles schedules without a pending timer at start-up.
5. Scheduled runs ignore the global stop (main.py:3567; supervisor.py:835): planner turns, spans, failure counts and
   retries continue while stopped (the gateway still refuses tools).
6. `owner_questions` unused; `TimeSetTimer` writes kind `reminder` (clock/tools.py:38); `TaskTree.set_status` uses
   `datetime.now` (tree.py:92, 129).

### Projection
- Today's code: ~8-30 GB after 1 year, ~13-58 GB after 2.
- After the decisions already made, with today's windows: ~2.1-2.4 GB after 1 year, ~2.3-2.8 GB after 2.
- With the proposed policy below: ~1.0-1.2 GB after 1 year, ~1.2-1.5 GB after 2 (only once space is reclaimed).

## Target design
1. **Retention per table** (decisions 1, 3; accessibility 7 d per memory decision 3): forever — episodes, memory, verdicts, schedules, known destinations,
   tasks, episode/how-note nodes, the journal's owner/chat kinds and day summaries; operational journal kinds 90 days;
   spans 30 days after the episode exists; trail accessibility 7 d, others 30 d, deleted in chunks; ledger,
   confirmations, plans, timers, loop state 90 days after the task or timer ends; task results forever; evidence and recovery
   7 d; resource samples 30 d.
2. **Stop storing ref lists** (`source_refs`) in context spans and chat replies; only `entry:` refs matter now.
3. **Reclaim space**: one-time `auto_vacuum=INCREMENTAL` + `VACUUM` (daemon stopped, free disk ≥ DB size), then a slice
   of `incremental_vacuum` and `wal_checkpoint(TRUNCATE)` after each hourly pass; `journal_size_limit`.
4. **One-time cleanup** after the writers are gone: an owner-confirmed setup step with the daemon stopped empties the
   dead tables (perception_*, decisions, learning jobs/receipts/digests (embedding tables stay), rollup nodes and their sources,
   unneeded learning_sources, task_memory_refs), journals the counts, then VACUUM. Dropping tables in a migration
   above 48. Rewriting journal rows needs a migration that drops and recreates the append-only trigger.
5. **Migrations**: checksummed, strictly increasing versions; refuse to start on a DB newer than the code; refuse a
   new file numbered at or below the current maximum.
6. Explicit `busy_timeout`; JSON written with `ensure_ascii=False` (or search that matches escapes); indexed time
   columns for purges (`decisions.at` while it exists).

## Depends on
- Removals first (memory learning, Jev off, notification rules, task_memory_refs writers), then the cleanup.
- Decision 5 (upgrade path) spans notes/daemon.md (SMAppService plist removal, persona out of config,
  permissions_version, venv swap), notes/hands.md (extension minimum), notes/runtimes.md (Codex re-pinned only if a
  role uses Codex), notes/memory.md (initial Tier 1 and embeddings).

## Fix list
- [ ] Retention policy (decisions 1, 3) in one place, chunked deletes, cascades to dependent rows.
- [ ] Stop writing `source_refs` lists; one-time migration to strip them from the journal.
- [ ] Incremental vacuum + `journal_size_limit`; WAL truncate after the hourly pass.
- [ ] One-time cleanup step (decision 4).
- [ ] Upgrade path from any older build (decision 5), tested on copies of real older databases and configs.
- [x] Migration runner: checksums, newer-DB refusal, numbering check, duplicate numbers (step 1; tests/test_migrations.py;
      accepted the live DB's 44 versions with gaps and backfilled their checksums).
- [ ] Continued-tree episodes; non-ASCII search; exposure origins in episodes.
- [x] Schedule reconciliation and the global stop for scheduled runs: built in notes/orchestration.md.
- [ ] Drop `owner_questions`, `notification_rules`/`notification_told`, the dead learning tables (jobs, receipts,
      digests), and duplicate tombstone tables → one. Tables and triggers of switched-off code (perception_*, decisions,
      `trail_changes`, `trail.relevance`) keep their schema; only their data is emptied (memory decision 4).
- [ ] `TimeSetTimer` kind `timer` (the `TaskTree` clock is owned by notes/orchestration.md). (Step 8a: the `TaskTree` clock done.)
- [ ] Keep `secure_delete=ON` (deleted data zeroed, privacy); offset its cost with chunked purges.
- [ ] Explicit `busy_timeout`; whole-journal reads replaced by bounded queries.
- [x] The 3.09 GB `ferb.db.before-restore-20261025-224330` deleted (decision 2).
