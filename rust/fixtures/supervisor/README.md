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

The 28 chat scenarios of `rust/fixtures/daemon/chat_scenarios.json` are replayed on a temporary store: every frame each
connection received (bytes and order), every model call (runtime role, mode, tools, answer schema, instructions,
parts), every tool call's reply through the bridge, the chat RPCs' replies, and every store table except those another
part owns (`learning_*`; `perception_floor`, `recall_state`, `trail_changes`, written by the daemon's start).

- **The gateway is real** (ferb-gateway's `Gateway` on the scenario's store, the orchestrator's ledger, ferb-fs's change
  record behind its hooks). The tools are scripted: each answers with the result the Python daemon recorded for it
  (`gateway_calls`), after a file tool really ran (the file is there for the change record and the checker). The
  scripted model makes its recorded `tool_bridge_lines` calls with its run token, as the bridge's `tools.call` does.
- **The run's folder** is a real one under `$TMPDIR` as long as the recorder's `$TMPDIR/ferbdx` (budget lines count
  path bytes), never the recorder's own (a recorder may be running in it). Hashes FERB takes over real paths (P19's
  payload hash, a protected path's confirmation key) are therefore compared as `<sha256>`.
- **Python's suspension points** are kept where the order of frames depends on them: the scripted model's step runs in
  a thread (a yield before and after each bridge call), file tools run in a thread (a yield after the call). A chat
  stream takes the events that became ready together as Python's loop hands them over (grouped by queue, the queue
  that got the first event first: `chat.rs` `as_python_wakes`).
- **The daemon's own plumbing** a chat scenario reaches is emulated in the harness: compose.py's `_publish` (a task's
  question names its request) and `_question_record` (`question.asked` / `question.answered`). The daemon's other RPC
  areas (`reminders.*`, `timers.*`, `schedules.*`, `stop.engage`, `notifications.subscribe`) and the timer loop's
  delivery are passed over and named in the report.
- Inputs read from `scenarios.json`: each planner call's environment and live facts (`supervise`), each role's
  instruction inputs (`system`), each context's activity search (its `/FX` written as the run's folder) and desktop
  observations (`context`); the desktop observer's spans are copied from the daemon fixture's `spans` table.

23 of the 28 replay fully. The other five: `timer_set_and_fired` and `schedule_created_and_fired` reach the daemon's
schedule and timer RPCs and its timer loop (and a schedule made in chat isn't ported), `global_stop` needs the daemon's
`stop.engage`, `restart_mid_task` and `restart_with_open_question` need a restart.
