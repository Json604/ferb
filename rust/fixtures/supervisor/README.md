# Fixtures: what the supervisor composes (ENG-3)

Made by `make.py` from the Python build:

    cd <repo> && uv run python rust/fixtures/supervisor/make.py           # both files (about 5 minutes)
    cd <repo> && uv run python rust/fixtures/supervisor/make.py --units   # units.json only

| File | What |
|---|---|
| `units.json` | supervisor.py's module-level functions and pure methods on invented inputs: 328 cases in 42 groups |
| `scenarios.json` | the 28 chat scenarios of `rust/fixtures/daemon/make.py`, run again with supervisor.py's composition points recorded (1,019 records) |
| `bodies.json` | texts of 2,048 characters or more not already in `rust/fixtures/daemon/bodies.json` (none today) |

## units.json

`{"token_method", "units": {group: [{"args", "ok"}]}}`. Token counts are the byte ceiling
(`utf8_byte_ceiling_tokenizer_unavailable`): the daemon fixture's stand-in, so the same rule holds in both. Times are
UTC (`TZ=UTC`). Groups: the planner's history rules (`short_report`, `heading_only`, `first_content_line`,
`without_history_labels`, `result_line`, `result_reference`, `entry_payload`, `stored_values`), owner-facing lines
(`sleeping_line`, `quoted`, `with_reasons`, `not_set`, `not_saved`, `reminder_line`, `already_set`, `day`,
`plain_choice`, `takes_words`, `sites_line`, `work_in`, `sentence`, `clipped`, `bare`, `since_line`,
`stopped_plan_summary`, `without_whole`), conditions (`watch_condition`, `checked_conditions`, `judged_kind`), blocks
(`front_window_block`, `summary_blocks`, `within_tokens`, `sent_from`, `said`, `within`, `stored_refs`,
`exposure_output`), the chat line's arrival order (`chat_line`: arrive / reply / release scripts, the roles and the
holder after each step) `since_block` (`_since_block` and `_since_event` on a stand-in `self` answering their
reads from invented journal rows covering every SINCE kind), `questions_block` (open approvals of every kind, with
`_card_row`), `latest_blocks` (a task's answer, a multi-part card, a row of it, a scheduled post, an answer over the
token limit) and `previous_run_block`.

## scenarios.json

`{"scenarios": {name: {"records": [...]}}, "catalogs": {sha: catalog}}`. Records, in the order the Python code made
them (one scenario runs at a time; `kind` names the point):

| kind | Recorded at | Holds |
|---|---|---|
| `system` | `Supervisor._system` | name, live, roles, the six inputs (catalogs by `{"catalog": sha}`), the text |
| `newly_always_on` | `_with_newly_always_on` | Tier 1's pending text, block counts |
| `assemble` | `worker.assemble` (every model call) | the blocks, budget, reserve, cap, the parts of the `/FX` blocks assembled again, cacheable, shown |
| `supervise` | `_supervise` (each planner call) | the turn's request, session, environment, now, signals, live facts, repair, last call, entries, payload, open question |
| `timeline` | `_timeline` | the journal rows it read, task ids, covered handles, recorded outcomes, its entries and ages |
| `conversation_blocks` | `_conversation_blocks` | the summary row, the blocks, ages, the latest result |
| `task_state` | `_task_state` | each running tree as the method reads it, the waiting candidates, the block |
| `questions_block`, `latest_blocks`, `since_block` | those builders | their reads and their block(s) |
| `context` | `_context` (worker, checker, fix, question router) | every read, the `_source_block` / `_worker_memory` / `_result_blocks` / `_schedule_messages` / `_schedule_block` calls inside it, the blocks |
| `verifier`, `code_facts`, `recorded_calls`, `changed_files`, `check` | the checker's builders | their reads and blocks |
| `run_worker` | `WorkerRuns._run_worker_once` | role, prompt, roles, time inputs, browser context, corrections |

Reading: texts of 2,048 characters or more are `{"sha256"}` (in `rust/fixtures/daemon/bodies.json`); temporary paths are
`/FX` as in the daemon fixture. **The budget line's token estimate counts the real temporary path's bytes**, so the
parts sent differ from parts assembled from `/FX` texts by that many tokens: `assemble.parts` is the `/FX` blocks
assembled again (make.py checks that the parts actually assembled are, call by call, the parts in
`rust/fixtures/daemon/chat_scenarios.json`).

Determinism: the daemon fixture's (FakeClock, sequential ids, counters, invented Mac); two runs give the same bytes.

## The flows' replay (`ferb-supervisor/tests/replay.rs`)

The forward path (intake, the planner's decision and its routing, the plan run, the worker and checker round, the
task's end, `chat.send`'s stream) is replayed against `rust/fixtures/daemon/chat_scenarios.json`: every frame each
connection received (bytes and order), every model call (runtime role, mode, tools, answer schema, instructions, parts)
and every store table except those another part owns (`learning_*`, written by the memory part's episode capture
after an outcome; `perception_floor`, `recall_state`, `trail_changes`, written by the daemon's start).

What other parts gave the Python supervisor is read from `scenarios.json` here, in the order it was read: each planner
call's environment and live facts (`supervise`), each role's instruction inputs (`system`), each context's activity
search and desktop observations (`context`). The desktop observer's own spans (`disturbance.baseline`,
`disturbance.summary`) are copied from the daemon fixture's `spans` table when the supervisor calls the desktop seam,
under the id the run gives them. The gateway's listing is `rust/fixtures/catalog/tools.json` restricted to the daemon's
registered tools. No new recording was needed: every read the forward path makes is one of these.
