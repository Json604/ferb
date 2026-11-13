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
  question names its request) and `_question_record` (`question.asked` / `question.answered`), the global stop's
  `stop.engage` / `stop.lift` (ferb-gateway's `StopControl` order: block, revoke the run tokens, the hooks), and
  group B's notifier (each banner becomes the `notifications.subscribe.event` line a connected FERB.app gets).
- **The daemon's group A is real** where a scenario reaches it (`tests/replay/daemon_side.rs`): ferb-daemon's
  `areas_a` on its own loop thread, served by a real ferb-rpc server on a socket in the run's folder (its
  `schedules.*`, `reminders.*`, `timers.*` lines compared byte for byte), its timer loop's pass at each `timers_tick`
  step, a schedule's run and its bookkeeping, with the supervisor in its seams (`flow::daemon_seams`). The timer claim
  token counter starts where the recorder's real-time loop was (read from the recording's `timers` table).
- **A restart** stops the supervisor as the daemon's shutdown does (`Supervisor::shut_down`: suspend, `cancel_all`,
  every turn drained), writes the desktop observer's shutdown summary and `daemon.stopped`, then builds a new gateway
  and supervisor on the same store with ids under the step's prefix (`recover_after_restart`, the start's recorded
  rows, `mark_unknown_after_restart`, `start_resume`).
- Inputs read from `scenarios.json`: each planner call's environment and live facts (`supervise`), each role's
  instruction inputs (`system`), each context's activity search (its `/FX` written as the run's folder) and desktop
  observations (`context`); the desktop observer's spans are copied from the daemon fixture's `spans` table.

All 28 replay fully (2,271 comparisons). One deliberate difference is applied to the recording before it is compared:
GATE-581, `global_stop`'s "(from the task) FERB was stopped" is "FERB was stopped" (`FIXES` in tests/replay.rs).

`tests/rpc_server.rs` replays 20 recorded chat and task request lines of `rpc_transcripts.json` through
`chat_rpc::register` on a real ferb-rpc server (one deliberate difference: `conversations.list {id}` of an unknown
conversation is refused in plain words, where Python answered an internal error).

## more_scenarios.json: the paths no earlier recording reached (`make_more.py`)

    cd <repo> && uv run python rust/fixtures/supervisor/make_more.py [mm_<name> …]   # all (about 4 minutes), or some

Same shape as `control_scenarios.json` (`scenarios`, `records`, `inputs`, `catalogs`); texts of 2,048 characters or
more that no other bodies file holds are in `more_bodies.json`. Two full runs give identical bytes. Each scenario runs
on the real Python daemon through the daemon recorder's harness (its stand-ins, its guard against launching a browser,
`/FX`), plus:

- **a tripwire** on `subprocess.Popen`, `asyncio.create_subprocess_exec`/`_shell`, `os.system`, `os.posix_spawn[p]`:
  only FERB's own episode-card reader (`python -I -m ferb.memory.card_reader` on the temp home) may start; the run
  prints every launch and fails on anything else;
- **a card row's seq** (parts.py `_next_seq`, the wall clock) reads the scenario's FakeClock; the Rust replay injects
  its clock the same way;
- **`port` records**: memory's `remember` and `forget_request` with their arguments, their answer and the ids the
  memory part took (`ids`: the counter before and after); the replay answers the same, takes those ids, writes the
  journal rows the memory part wrote in between and leaves the `memory_*` tables to that part;
- **`seed` steps**: journal rows written before the first message (a long earlier chat), inserted by the replay as
  the start's rows are;
- **`context.memory`**: the blocks the memory port gave each worker context (`_worker_memory`).

| Scenario | Path (supervisor.py) |
|---|---|
| summary_fold | `_keep_recent`, `_summarize`, `_fold`, `_foldable`, `_summary`, `_covered`: two summary calls, then a turn reading the summary |
| several_asks | the card (ORCH-319): a direct part and a step's part, `_post_part`, `_part_event`, `_final_card`, `card_state` |
| plan_parts_checklist | a three-step plan with two parts: the middle step fails, the last never runs (`_never_ran`, `_close_rows`, `_couldnt_do`) |
| remember_then_forget | the memory side entries: `_quote_source`, `_remember_about`, the memory port, a forget's question |
| answer_in_chat | `_answer`: a step's question answered in chat by id; `_task_state`'s cards, the questions block's task label |
| checklist_single | `_checklist` of a single ask with a three-step plan |
| ask_step | `_plan_question`, `_ask_card` |
| wait_step | plan_executor `_wait` (a time already come) |
| route_question | a worker's owner.ask routed through `route_question` (`worker.question`), then asked |
| checker_runtime_down | the checker's usage limit: `_RuntimeDown`, `_service_down` (role checker), the owner's pick |
| last_call_empty | ORCH-558: a planner call without an answer gets one last call with no tools |
| unbacked_part | `_unbacked_parts`: a direct "" part no entry quotes goes back for repair |
| correct_pending_check | chat.correct on a pending confirmation (GATE-510, ORCH-538): the check set aside, the task steered |
| earlier_result | ORCH-619: `_result_problems`, `_earlier`, `_worker_room`, `_result_blocks` |
| loop_steer_finish | L31: `_loop_step` on LoopRunner, a round that carries on, the loop watching, the owner's correction read at the loop's next look (`wake_loops` from steering), `_loop_facts` and `preview_status` in the planner's task state, a round that reports the end, `_loop_end_reached`, `_judged` |
| loop_pause_stop | a round that needs the owner: the loop pauses, its notice (`_loop_notice`: an attention notice), `_pause_question` with Stop, `loop_statuses` in chat.status, Stop ends it |
| mention_paused | ORCH-312: `mention_paused`, `_mention_pause` the next day (a `clock` step), Stop |
| owner_closed_tab | HAND-201: `notify_plan_event('browser.owner_stopped')`, `owner_stopped_part`, Stop |
| reason_findings | ORCH-562: findings on a not-done report's reason, settled by code after the fix turn |
| runtime_switch_tap | ORCH-315: the worker's usage limit, `runtime_offer`, `answer_runtime_switch` (the app's one tap) |
| event_wait | a plan's wait step on an event: `notify_plan_event`, the plan's wake, the next step |
| conversation_handover | F9: `_conversation_steps`, `_conversation_step` on ConversationRunner: the scope's approval (its card), one turn reading the thread through the gateway (`conversation.read`), a hand-over with evidence, `conversations.list {id}`'s digest |
| conversation_change | a waiting conversation paused by a chat turn's `conversation_changes` (one applied by `change_conversation`, one whose quote the owner never wrote refused after the planner's one repair, one naming no conversation told back), then closed by `conversations.change` |
| stated_scope | D24/D30: `_plan_statement` of a big job with standing sends (one scope granted under `plan.stated`, one not typed), a correction widening the scope (`restate_scope`), granted once said (`grant_stated`) |
| front_window | L12: `_screen_at_send` and `front_window_block` in the planner's turn, the image refused once (ORCH-524: run again without it), two messages read together (`_by_window`, two images), the start-of-task screenshot |
| preview_attention | ORCH-500: `Supervisor.preview` of a two-step plan; L10: `Supervisor.attention` twice on one invented moment (one answer citing its source, one dropped) |
| browse_as_corrected | APP-777: `_browse_as` (the owner's picked profile, the worker's identity, `task.browses_as_owner`); a correction giving the running worker a new goal and condition, read by its check |
| probe_watch | L31: a look-only round (`_look_only`: its check skipped) arms a probe on fs.read (`arm`, the runner's `_arm`: a baseline read), the watch's reads (`_watch`), the owner's change and the probe interval (a `clock` step), a trip, the next round told what it observed (`_probe_exposure`), the end checked |
| login_sites | L36 §5: a login name given in chat s2 while correcting a task of chat s1: `_turn_sites` (none typed in s2) and `_sites_named_elsewhere` (the site typed in s1) reach the memory port |
| displaced_question | L3, rule 14: a forget waits and the planner asked a password question; "yes" could answer either, so FERB asks which (`_answer`, WHICH_ONE); the next one-word message is hidden (`_hide_code_reply`: `asked_only_about` true for FERB's which-question about a memory question, `displaced_question` the planner's) |
| inventory_walk | ORCH-580/625: `_problems` walks a plan's inputs (a folder of invented receipts, an absent file): the absent input and the measured inventory told back; the repaired answer is a reply |
| conversation_resume | F9: a conversation's first turn under way when the owner stops its task; `conversations.change resume` of a conversation whose tree no longer runs (`change_conversation`: its step interrupted, the plan suspended, saved plans resumed); its next turn hands over |
| capture_timeout | DMN-520, I4: a capture at send that never ends: the planner waits FRONT_WINDOW_WAIT_S (10 s of real time), journals `screen.capture_timeout` and plans without the window (CAPTURE_LATE) |

A run of some scenarios numbers each one's folder (`/FX/NN`) as a full run does (its place in `SCENARIOS`), so a
partial run writes the same bytes for them; the merged file keeps the other scenarios as they were and puts the ones
run at its end (a full run writes them in `SCENARIOS` order).

Steps these add: `write_file` (an invented file the owner writes in the workspace, written by the replay at the
same point), the waits `the probe has read N times` and `the task is done`, `mention_paused`, `plan_event` (the supervisor's call on the daemon's loop), `runtime_switch` (with
the offer and the answer recorded), `preview` and `attention` (the supervisor's calls on the daemon's loop, with what
they answered), and waits `the loop watches`, `the plan waits` and `the conversation waits`. `uuid.uuid4` is a counter (a
loop notice's id). `port` records also hold memory's `memory_candidates` (the attention check) and the browsers' area
`browsing` (a task given to FERB), and the gateway's loop probe reads (`probe`: the grant's task, worker, scopes and
identity, the tool, its arguments, the span; the gateway's answer: `patch_probe`), answered by the in-process replay
(the program's replay runs its real gateway's probe on the real file). A scenario's `captures` are the front windows the daemon captured at send
(`capture_at_send`, invented: two windows in turn, a few invented bytes as the screenshot).

`tests/replay_more.rs` replays all 33 fully with tests/replay.rs's harness (moved to `tests/replay/harness.rs`). One
deliberate difference is applied to `remember_then_forget`'s recording before it is compared (`FIXES`): Python's reply
tags the forget question's "FERB deletes:" line, FERB's own words, as "(from the task)"; the port does not. The same
difference is applied to `displaced_question`, whose later planner turns show that reply in their history, so their
context is 16 bytes shorter and its budget line counts 16 tokens fewer.

## wait_cut.json (`make_wait_cut.py`)

    cd <repo> && uv run python rust/fixtures/supervisor/make_wait_cut.py

supervisor.py:5657 `_wait_cut` (ORCH-633), the real method on the real journal with invented `wait.ended` rows: the
fact a step's next run gets when FERB's restart ended its tree's wait.for (the latest row of the tree, its time in the
Mac's zone, its line made printable; none for another tree's or no row), in UTC and Asia/Kolkata. Replayed by
`ferb-supervisor/tests/control_scenarios.rs` `a_wait_a_restart_cut_is_told_as_python_tells_it`, through the supervisor
on a scenario's store and as text for each zone.
