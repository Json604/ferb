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
- [x] Retention policy (decisions 1, 3) in one place, chunked deletes, cascades to dependent rows.
      (Step 12a: ferb/store/retention.py holds every rule (spans 30 d after the episode, operational records 90 d after
      their task or timer ends: ledger, confirmations, plans + steps, loop state, delivered/cancelled timers +
      deliveries, `OPERATIONAL_KINDS` journal rows; decisions 30 d; trail accessibility 7 d, others 30 d once summarized;
      evidence/recovery 7 d and resource samples 30 d stay with their stores); `[retention] operational_days` (90, also a
      setting). The pass reads what goes off the loop (`retention.plan`, read-only connection in a thread) and deletes on
      the loop's connection `BATCH` (200) rows a transaction with dependents first (`DEPENDENTS`: plan steps, timer
      deliveries, perception receipts of purged trail rows), yielding between batches; trail purges moved here from the
      distiller. Tests: tests/test_retention.py.)
- [x] Stop writing `source_refs` lists; one-time migration to strip them from the journal.
      (Step 12a: chat replies/questions carry none; context spans keep a block's own refs (entry:/journal:/memory:/node
      ids: a card's request entry, the notice purge, the repeat measure read them) but no `trail:` lists
      (`supervisor._stored_refs`); migration 0056 strips them from `chat.reply`/`chat.clarify` rows, dropping and
      remaking `journal_append_only` in the same transaction (`chat.summary` keeps its entry handles). Tests:
      tests/test_upgrade.py, tests/test_planner_context.py.)
- [x] Incremental vacuum + `journal_size_limit`; WAL truncate after the hourly pass.
      (Step 12a: new stores start with `auto_vacuum=INCREMENTAL`; older ones get it from the clean-up's compaction;
      after each hourly pass up to 8192 free pages go back in 512-page slices, then `wal_checkpoint(TRUNCATE)`;
      `journal_size_limit` 64 MiB on every connection.)
- [x] One-time cleanup step (decision 4).
      (Step 12a: ferb/store/cleanup.py; `ferb setup --cleanup` prints the plan as JSON (tables, rows, what each held,
      `compact`, size, free disk, `ok` digest; read-only, safe while the daemon runs); `ferb setup --cleanup --ok DIGEST`
      stops the daemon (`Installer.while_stopped`), reads the plan again and refuses (exit 2, nothing removed) unless it
      matches, deletes in batches, journals `store.cleaned` (counts) and compacts with incremental auto-vacuum when free
      disk ≥ the file (`store.compacted`), then starts the daemon again whatever happens. It empties switched-off code's
      data (decisions, perception receipts/batches/labels, notification receipts, trail.relevance), removed code's data
      (learning_vectors/evals, task_memory_refs, rollup nodes + their sources and change rows) and card source rows the
      current card rule no longer gives (`Learning.card_source_plan`), and drops the tables no code reads
      (`DROPPED`, journaled as `dropped`). Never memory, episodes, chat, tasks, schedules or
      in-window traces (`cleanup.PROTECTED`, asserted). The automatic C1 card clean-out during `--update` is folded into
      it (it now waits for the OK too). The app screen is step 12b. Tests: tests/test_setup_flow.py,
      tests/test_card_source_cleanout.py.)
- [x] Upgrade path from any older build (decision 5), tested on copies of real older databases and configs.
      (Step 12a: tests/test_upgrade.py builds stores as older builds left them (schemas 18, 30, 40 with the deleted 0040,
      44, 48 with and without 0040, 52, 55; the pre-checksum runner) and an older config.toml (persona under [setup],
      FERB-written limits, a retired key, keys no longer read): all reach 0056 with their data and a new install's tables;
      the newer-DB refusal holds; the first start upgrades config once, adds `operational_days`, moves the persona and
      seeds Tier 1 once. Measured on a copy of the owner's store (Measurements, step 12a). The plist, native-host,
      extension-minimum, Codex and permissions parts of decision 5 belong to notes/daemon.md, hands.md, runtimes.md.)
      (Step 12b: the plist part is built: FERB.app's setup retires ~/Library/LaunchAgents/com.ferb.daemon.plist before
      the app registers its own service; Codex re-pinned only when a role uses it; `permissions_version` re-checked
      against every permission; notes/daemon.md.)
      (Step 9c: migration 0055 (`memory_import_runs`, `memory_import_done` part hashes, `memory_import_items` with a
      trigger that drops a deleted fact's rows): ids, counts, hashes and dates only, never conversation text.)
      (Step 9b: migration 0054 (`recall_vectors`, `recall_state`, `chat_fts`, `memory_set_aside`, triggers that drop a
      deleted fact's or node's vector); the conversation index and the vectors are filled in the background at the first
      start (nothing at migration time); open fact/identity copies closed once (journaled `memory.fact_nodes_closed`).
      The `memory` optional extra (mlx-embeddings, sqlite-vec) is gone; `learning_vectors`/`learning_embedding_evals`
      are no longer written (their rows, if any, go with the step-12 clean-up, as do the 1.5M old card source rows).)
      (Step 9a: migration 0053 (`memory_context`, `memory_profile`, re-runnable tables beside `memory_items`); the
      one-time memory clean-up and Tier 1 seed at first start (ferb/memory/upgrade.py, journaled); old waiting Keep
      questions dropped; `persona.json` gains `seen`; tests/test_memory_upgrade.py on a schema-52 fixture.)
- [x] Migration runner: checksums, newer-DB refusal, numbering check, duplicate numbers (step 1; tests/test_migrations.py;
      accepted the live DB's 44 versions with gaps and backfilled their checksums).
- [x] Continued-tree episodes; non-ASCII search; exposure origins in episodes.
      (Step 12a: `SpanStore.outcome` takes the request from a tree's first run and the result from its last; `compose`
      ends at the last run; `context.exposure` origins join the episode's (ORIGIN_KINDS, as `task_origins`); spans and
      journal written with `ensure_ascii=False`; the evidence search matches both forms, and so do the forget scrub and
      page bounds. Existing episodes are rebuilt only if their spans still exist (not rewritten). Tests:
      tests/test_storage_fixes.py.)
- [x] Schedule reconciliation and the global stop for scheduled runs: built in notes/orchestration.md.
- [x] Drop `owner_questions`, `notification_rules`/`notification_told`, the dead learning tables (jobs, receipts,
      digests), and duplicate tombstone tables → one. Tables and triggers of switched-off code (perception_*, decisions,
      `trail_changes`, `trail.relevance`) keep their schema; only their data is emptied (memory decision 4).
      (Step 12a: `owner_questions`, `notification_rules`, `learning_receipts`, `learning_jobs` are read and written by no
      code any more (the forget path's job delete removed) and are dropped only by the owner's OK'd clean-up
      (`cleanup.DROPPED`: listed with their rows in the preview, dropped and journaled by apply); an install where the
      clean-up hasn't run keeps them, unread. Migration 0056 merges `learning_forgotten` and
      `conversation_forgotten_sources` into `forgotten_refs` (rows kept). Kept: `notification_told` (switched-off
      perception still names it), `learning_digests` (the app's identity digest reads it), `memory_forgotten` (content
      fingerprints, not ids). The clean-up empties the switched-off tables.)
- [x] `TimeSetTimer` kind `timer` (the `TaskTree` clock is owned by notes/orchestration.md). (Step 8a: the `TaskTree` clock done.)
      (Step 12a: `time.set_timer` books kind `timer`; the live-facts list and the chat cancel treat it like a reminder.)
- [x] Keep `secure_delete=ON` (deleted data zeroed, privacy); offset its cost with chunked purges.
      (Step 12a: unchanged; every purge is batched.)
- [x] Explicit `busy_timeout`; whole-journal reads replaced by bounded queries.
      (Step 12a: `busy_timeout` 5000 ms on every `Database` connection and the pass's reader; `Journal.entries` filters
      by time in SQL and takes a limit; `timeline.get` returns the newest 1,000 rows and `more`. `Continuity.history`
      was already paged in step 9b.)
- [x] The 3.09 GB `ferb.db.before-restore-20261025-224330` deleted (decision 2).

## Rulings (step 12a, technical)
- Retention lives in ferb/store/retention.py; the daemon's area only runs it hourly. Reads that scan (the intake
  planner's spans by request id: one scan per pass for every tree) run in a worker thread on a read-only connection;
  deletes run on the loop's own connection, `BATCH` = 200 rows a transaction, dependents first, the loop free between
  batches (a worker-thread writer would make loop writes wait on SQLite's lock).
- A record whose task isn't in the task list counts 90 days from its own last write; a journal row a memory fact cites
  (`memory_sources`) is never aged out; a journal row whose task still runs (or ended within 90 days) stays.
- "Operational journal kinds" (decision 3) = `retention.OPERATIONAL_KINDS`: the daemon's bookkeeping, failure and
  progress rows (daemon start/stop, slow calls, retention and trail purge records, plan steps/repairs, loop rounds,
  dispatch/steering/digest records, watcher, voice, screen and runtime failures, memory job failures, card ref reports).
  Not in it: chat and owner kinds, day summaries, schedule, timer-set/delivered/cancelled and memory records, and the
  audit-like kinds (`security.alignment`, `shell.network`, `escape.used`, `clipboard.access`): those stay (owner question
  below).
- Trail purges moved from the distiller to the retention pass (one place); both are chunked and take the Jev perception
  receipts of deleted rows with them.
- `busy_timeout` 5000 ms (what sqlite3 waited before, now explicit everywhere); `journal_size_limit` 64 MiB; after each
  pass up to 8192 free pages are handed back in 512-page slices, then `wal_checkpoint(TRUNCATE)`. New stores start with
  `auto_vacuum=INCREMENTAL`; an older store gets it only from the clean-up's VACUUM.
- Context spans keep a block's own refs and drop `trail:` lists: the notes said "only `entry:` refs", but the repeat
  measure (step 9b) finds learned node ids and a card cites the owner's request entry, so those stay. `chat.summary`
  keeps its entry handles (small; the summary's lineage).
- Migration 0056 deletes no owner data (decision 4; coordinator, 2026-10-31): it strips the reply ref lists, merges the
  tombstones (rows kept) and adds an index. Tables no code reads are dropped only by the clean-up after the OK, listed
  with their rows even when empty (so a run leaves none behind); `pending` (doctor) counts only those holding rows, so a
  new install isn't asked about empty ones.
- The clean-up's OK is the preview's `ok` digest (its steps and counts); apply reads the plan again with the daemon
  stopped and refuses on any difference. It journals counts even when stopped (`stopped: true`); a re-run finishes.
- The card rule for old cards is today's (`card_source_plan`): a traced card keeps what its trace gives, an untraced one
  what it cites, then one activity row/learned node per distinct origin set (step 9b). Once a trace and its trail rows
  aged out, those rows' origins come from the cards that cite them, so an untraced card keeps fewer rows than it would
  have while traced. Labels are stored on the node and never change. Rollups the same run removes are read as gone, so a
  second plan finds nothing.
- The automatic C1 card clean-out that `ferb setup --update` ran is folded into the clean-up: it waits for the OK too.
- `timeline.get` returns the newest 1,000 rows unless asked for more (developer listing).
- Existing episodes are not rewritten for the continued-tree/exposure-origin fixes (the permanent record stays as
  composed); a missing episode is still built from spans while they exist.
- Not covered: installs made in the same-day windows when 0018, 0025 and 0042 were edited (10-08, 10-09, 10-21) lack the
  columns those edits added; SQL migrations can't add a column only when it is missing.

## Measurements (step 12a, a copy of the owner's store, read-only backup, 2026-10-31)
Store before: 3.09 GB file, 2.63 GB used, schema 48, `auto_vacuum` 0; 86.9 MB of reply ref lists in the journal.
- Migrations 0049–0056: 0.5 s; used space 2.63 → 2.55 GB (the journal's ref lists).
- First start of this build (test-daemon flags: no watchers, no Keychain, no embedding model): 3.4–4.2 s; journaled
  `memory.cleanup`, `memory.profile_seeded` (46 Tier 1 items), `memory.fact_nodes_closed`, `store.upgraded`,
  `config.upgraded` once each.
- First hourly retention pass: 69 s (95 s with the test suite running beside it); longest loop gap 189 ms (360 ms);
  removed 368,078 accessibility rows older than 7 days (and their 368k perception receipts), 236 spans of trees older than
  30 days (5 missing episodes built first), 516 decisions, 14 evidence rows, 2 recovery records; nothing operational is older
  than 90 days yet. A second pass: 0.1–0.6 s. Used space 2.55 → 2.33 GB (file unchanged: not compacted yet).
- Clean-up preview: 18.7 s (153 s before memoizing source origins); 2,223,776 rows (the measured run, before the
  dropped tables joined the clean-up; on the same store they add `learning_jobs` 294 and `learning_receipts` 6 rows,
  `owner_questions` and `notification_rules` empty): card source rows 1,480,537 (127 of
  303 card versions), perception receipts 497,499, decisions 131,567, perception batches 92,873, task memory refs
  19,498, rollup nodes 240 (+1,320 source rows, 242 change rows).
- Clean-up apply (daemon-stopped time): 45 s: plan 19 s, cards 7.8 s, perception receipts 9.8 s, decisions 1.6 s, the
  rest < 3 s, VACUUM 7.5 s. File 3.16 GB (with WAL) → 1.66 GB, `auto_vacuum` 2, no free pages. A preview after: 0 rows,
  nothing to compact. Tasks (1,197), task results (349) and memory items (61) unchanged by the clean-up.

## Live checks for step 13
- `ferb setup --update`, then the first start: migrations 0049–0056 applied, `store.upgraded`/memory records once, no
  `retention.failed`; the first hourly pass's `retention.purged` and `trail.purged` (accessibility backlog) rows.
- `ferb setup --cleanup` on the live store (owner reads the list), then `--ok <digest>` with the owner's yes: the daemon
  stops and comes back, `store.cleaned` and `store.compacted` journaled, the file near 1.7 GB, `ferb doctor` ✓ database.
- A day later: the file stays flat (free pages handed back hourly; `PRAGMA freelist_count` small), WAL under 64 MiB.
- A non-ASCII search in `task.evidence` finds Hindi text; a continued task's episode shows its last run's result.
- `time.set_timer` from a task shows as a timer in the notch and can be cancelled from chat.
- The embedding model's first fill on the live store (`memory.embeddings` count rises once the model is fetched).

## Open owner questions (step 12a; until answered, today's behaviour stays: kept)
- Audit-like journal kinds (`security.alignment` 886 rows, `shell.network` 481, `escape.used` 26, `clipboard.access`
  28 on the live store): kept forever today; or 90 days with the other operational records?
- No decided period: `perception_receipts`/`perception_batches` (writers off; the clean-up empties them; they would grow
  again if Jev comes back), `plan_events` (the loop wake-up queue, 0 rows live), `attention_notices` and
  `learning_changes` (small). Kept today: nothing ages them out.
