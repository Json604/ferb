# Fixtures: the Python daemon as the Rust daemon must reproduce it (ENG-3)

Made by `make.py` from the Python build at the commit it was run on:

    cd <repo> && uv run python rust/fixtures/daemon/make.py            # every scenario
    cd <repo> && uv run python rust/fixtures/daemon/make.py sc_timer   # some scenarios (the files then hold only those)

| File | What |
|---|---|
| `rpc_transcripts.json` | 12 scenarios; every line both ways for every method of `auth.METHODS` (coverage below) |
| `chat_scenarios.json` | 28 end-to-end chat scenarios: every frame, every model call (118 prompts), every gateway call, the store at the end |
| `bodies.json` | `{sha256: text}`: every text of 2,048 characters or more, stored once (see "Reading the files") |

Total 3,622,988 bytes. Two runs give identical bytes (checked when the files were made; see "Determinism").

## How a scenario is made

Each scenario starts a **real `ferb.daemon.main.Daemon`** (real `Server`, `Gateway`, stores, `Supervisor`) on its own
event-loop thread, in a fresh home `<root>/<NN>/home` with its own `HOME` `<root>/<NN>/user`, built exactly as
`tests/conftest.py` `RunningDaemon` and the `daemon` fixture build it (`watchers`, `distill`, `retention`,
`resources`, `notifications`, `snapshots`, `screen_at_send`, `preload_secrets`, `embeddings` off; `persona_scan`
None; `data_access` False; `developer_tools` []; `watch_owner_input` False; `_runtime_available` codex and claude;
`supervisor.availability = _default_availability`; the egress proxy's test resolver). `make.py` (`Host._make`,
`new_host`) is that composition; it imports `RunningDaemon`, `Script`, `FakeRuntime`, `STALL`, `role_of` from
`tests/conftest.py` and `egress`, `no_real_trash` from `tests/support`.

The client is a raw Unix-socket connection (`make.py` `Conn`) writing `json.dumps(request)` lines exactly as
`ferb/cli/client.py` does, and recording every line it writes and reads. `<root>` is `$TMPDIR/ferbdx` (a fixed folder,
emptied first, removed after): FERB hashes real paths into a few records, so a random folder would change them.
Every occurrence of that folder is written `/FX` (so a scenario's home is `/FX/<NN>/home`).

### Python entry points exercised

- The wire: `ferb/daemon/server.py:222` `_connection` (a line, JSON parse, `Request` validation), `:259` `_hello`
  (roles; refusal ends the connection), `:290` `_dispatch` (role table, `APP_ONLY`, unary, `CANCEL_WHEN_LEFT`,
  streams), `:408` `_stream` (`<method>.event` notifications, then `{"ended": true}`), `:451` `_line`
  (`json.dumps(msg)` defaults + `\n`). Roles: `ferb/daemon/auth.py:28` `METHODS`, `:61` `APP_ONLY`, `:155`
  `check_native_host`.
- The method table: `ferb/daemon/areas/rpc.py:130` `_register_rpc`; the handlers in `ferb/daemon/areas/*.py` (each
  scenario's `python` field names the files).
- Chat: `ferb/daemon/areas/chat.py:322` `_chat_send` (ack first, then the supervisor's events merged with the bus's
  `tool.called`, `approval.requested`, `action.*` for this turn's tasks), `:191-289` approvals, `:201`
  `_chat_correct`; `ferb/orchestrator/supervisor.py:2388` `send`, `:2866` `_control` (stop, pause, continue, retry;
  `:2979` `_then_start` for ORCH-638), `:2755` `_exit`, `:4786` `_step_state` (the `step` frames), `:2324`
  `resume_plans`, `:1629` `run_scheduled`, `:6745` `correct_from_question`; `ferb/orchestrator/worker.py:436`
  (`make_runtime` for a worker); `ferb/orchestrator/steering.py:66` `deliver` (a live correction, `continue_`);
  `ferb/orchestrator/tree.py:154` `interrupt_running`; `ferb/daemon/main.py:418-430, 469` (what a restart recovers).
- Tools: `ferb/daemon/gateway.py:735` `Gateway.call` (every tool call; recorded under `gateway_calls`).
- Time: `ferb/clock/timers.py:212` `tick`, `:244` `run`; `ferb/daemon/areas/schedules.py`; `ferb/clock/wait.py`.

## Determinism: what is faked and how

| Source of variation | How it is fixed | Where |
|---|---|---|
| Wall time | `FakeClock` at 2026-01-01T09:00:00Z; moves only at a `clock` step | `ferb/clock/clock.py:23` |
| Record ids | `SequentialIds("id")` (`id-000001`…); after a restart `SequentialIds("r1")` (as `tests/support/schedules.py` `own_daemon`; FERB's real ids are random) | `ferb/core/ids.py:13` |
| Run tokens | `ferb.daemon.auth.secrets` → `run-token-0001`…, reset per scenario | `auth.py:108` |
| Approval ids, timer tokens, shell ids, new chat ids | each module's `secrets` → a counter (`ap-a00000000001`, `chat-c00000000001`…), reset per scenario | `daemon/questions.py:126`, `clock/timers.py:188`, `act/shell/manager.py:135`, `orchestrator/continuity.py:51` |
| The untrusted-data wrapper's id | fixed `0123456789ab` (as `rust/fixtures/catalog`) | `sanitize/untrusted.py:117` |
| A file's identity facts in fs results | `files.identity` keeps size and path, invents `modified_at` 2026-01-01T09:00:00+00:00, `owner_uid` 501, `owner_gid` 20, `device` 1, `inode` = first 6 hex of SHA-256 of the `/FX` path, `mtime_ns` 1767258000000000000 | `act/fs/files.py:45`, `act/fs/look.py:17` |
| The Mac | invented: macOS `26.1`; running apps Finder (4101, front) and TextEdit (4102); desktop samples always Finder in front; permission reads `accessibility`, `screen_recording`, `input_monitoring` granted, `full_disk_access` denied | `STUBS` in `make.py` |
| The timer loop's wait | `Timers.run` waits real asyncio seconds (`next_due_in()`); after each `clock` step the fixture runs one `timers.tick()` (`timers_tick` step), as `tests/support/schedules.py` `fire` does | `clock/timers.py:244` |
| Thread scheduling | each step waits until the daemon is quiet (no new id, no database change, no new loop task for 0.25 s) before the next | `make.py` `settle` |

Every stand-in is listed with its reason in `make.py` `install_stubs` and `Host._make` (Keychain, the secret helper,
`security`, TCC reads and prompts, the pasteboard, Automation, self-tests, `codex`/`claude` programs, browser launch,
`sandbox-exec`'s probe, Vision OCR, the login session, the report relay). Nothing in a scenario reaches a model, the
network, the Keychain, a browser or the owner's screen; nothing reads or writes `~/.ferb`.

### `volatile`

What no stand-in fixes is replaced by the string `"<volatile>"`, **under these keys at any depth of any JSON** in the
fixture (frames, gateway replies, table cells holding JSON, JSON quoted inside a string, and `"key": value` written
in prompt text): `pid` (the daemon's process id: `ping`, `doctor.check`, the journal's `daemon.started`),
`max_sample_ms` (a timing in the desktop observer's facts), `executable`, `code_hash` (doctor). A scenario's own
`volatile` list adds keys (none do today). Table columns SQLite fills from its own clock are `"<volatile>"` too:
`perception_floor.at` (migration 0035's `strftime('now')`). A Rust test applies the same masks to its own output
before comparing (`make.py` `scrub` and `replace_text` are the reference). With these, the two runs that made the
files are byte-identical.

## Reading the files

- **Lines are bytes.** Each `send`/`recv` is the exact line without its `\n`. Lines the daemon writes are
  `json.dumps(msg)` with Python's defaults (`", "`, `": "`, `\uXXXX` escapes). Key order is contract: read with
  `serde_json` `preserve_order`.
- **Bodies.** Any string of 2,048 characters or more (a long line, a prompt, a table cell) is stored as
  `{"sha256": <hex>}`, the SHA-256 of its UTF-8 bytes, with the text in `bodies.json`. Inside table cells, a model's
  instructions copied into a span's input are written `"@@body:<sha256>:utf8@@"` (or `:ascii@@`): that token,
  quotes included, stands for `json.dumps(bodies[sha], ensure_ascii=False)` (or with `ensure_ascii=True`). Two forms
  of the same instructions exist: as sent (`model_calls[].system`) and as the trace stores them, redacted (the
  redactor masks a word after "passwd:" in the worker instructions, so the stored form differs by a few bytes).
- **Paths** are written `/FX/...`; the real bytes the daemon hashed had `$TMPDIR/ferbdx/...`.

### A scenario

```
{"name", "what", "python": [entry points], "volatile": [], "steps": [...],
 "model_calls": [...], "gateway_calls": [...], "tables": {...}}
```

`steps`, in the order the recorder did them:

| `do` | Meaning |
|---|---|
| `connect` | open connection `conn` (each connection of a scenario has its own label; `app#2` is the second `app`) |
| `call` | `lines`: what was written and read on `conn` for one request (hello included); a unary call's reply is the line with its `id` |
| `stream` | a streaming request written on `conn` (its frames come at `stream_end`) |
| `stream_end` | every line the stream wrote, ending with `{"result": {"ended": true}}` or an `error` |
| `stream_cut` | the client closed a stream that never ends by itself (`notifications.subscribe`, `logs.subscribe`); `lines` is what arrived first |
| `stream_end_at_shutdown` | a stream the daemon's shutdown ended (restart, `update.prepare`) |
| `close` | the client closed `conn` |
| `read_until_closed` | the daemon closed `conn` (a refused hello) |
| `wait` | the recorder waited for a condition (`until`) before the next step; each is observable to a Rust test (the scripted model received a call; `chat.status` lists an approval; a task row's status) |
| `release_model` | a held model turn (`Gate`) may now answer |
| `clock` | the FakeClock moved `advance_s` seconds (to `now`) |
| `timers_tick` | one pass of the timer loop; `fired` timers handed over |
| `restart` | the daemon's clean `shutdown()`, then a new `Daemon` on the same home; ids continue under `ids_prefix` |
| `note` | something the recorder did outside the socket (a file it wrote, a switch it flipped) |

`model_calls[]`, one per `RuntimeAdapter.start` FERB made, in order: `key` (`supervisor`, `worker`, `worker.verify`,
`step.<id>`, `step.<id>.verify`, `worker.fix`, `worker.recheck`; tests/conftest.py's keys), `role_played`,
`runtime_role` and `mode` (what FERB passed to `make_runtime`), `task_id`, `schema` (the answer schema's name in
`rust/fixtures/catalog/answer_schemas.json`, or its SHA-256), `effort`, `tools` (the tool names it was given, registry
order), `system` and `parts` (the `ModelRequest`, byte for byte), `cacheable`, `images`, `events` (the scripted
answer: `usage`, then `final {text}`; or "(no answer: FERB cancelled or left the call)"), `continued_with` (text FERB
sent into the running call: a live correction), `tool_bridge_lines` (the lines the scripted model's tool calls wrote
on its `mcp_shim` connection). Every scripted `final` was checked against the schema that call was given.

`gateway_calls[]`: every `Gateway.call` in completion order: `task_id`, `worker_id`, `tool`, `arguments`, `reply`.

`tables`: every non-empty table at the end (except `schema_version`, `schema_migrations` and FTS shadow tables),
`{columns, rows}` in rowid order; bytes as `{"hex"}`.

## How a Rust test replays a scenario

1. Build the Rust daemon in a temporary home with the same stand-ins: FakeClock at 2026-01-01T09:00:00Z, sequential
   ids `id-000001` (restart: `r1-000001`), the counters above, the invented Mac facts, no runtimes signed in, no API
   key, no Full Disk Access, every APP_ONLY caller accepted (except where a `note` says it is refused).
2. Give it a scripted model: for each model call it makes, answer with the recorded `events` of the call with the
   same `key`, in order (a `Gate`d call answers only after `release_model`; a STALL call never answers and must be
   cancelled by FERB). **Inputs:** the requests' lines, the model's answers, the clock steps, the files a `note`
   names. **Expected outputs:** every `recv` line, every model call's `system`, `parts`, `tools`, `schema`, every
   gateway call and reply, every table row (after the `volatile` masks).
3. Walk `steps`: `connect`/`call`/`stream`/`close` drive the socket; compare each `recv` line byte for byte. Lines
   on one connection come in a fixed order; lines on different connections may interleave differently (only each
   connection's own order is a contract). `wait` steps are synchronisation, not output.

## Coverage

From `rpc_transcripts.json` `coverage` (computed over both files' lines):

| Role | Methods | Called | Answered with a result or events | Also called with a refusal |
|---|---|---|---|---|
| cli | 106 | 106 | 91 | 61 |
| mcp_shim | 3 | 3 | 3 | 0 |
| native_host | 0 | 0 | 0 | 0 |

Every cli method was called at least twice, once with parameters meant to be valid and once with parameters meant to be invalid. Many handlers ignore unknown or wrongly typed parameters, so their "invalid" call still answers (listed under `with_result` only). Called but never answered with a result (15): `attention.ack`, `browsers.installed`, `browsers.pair`, `browsers.unpair`, `conversations.change`, `identity.pair`, `memory.import.start`, `ps.attach`, `ps.send`, `runtimes.switch_task`, `secrets.delete`, `seen.image`, `setup.browser_open`, `setup.browser_pair`, `ui.click_through`. The state they need is in the next section.
The native host role has no methods (`auth.METHODS[NATIVE_HOST]` is empty): its hello is recorded, refused and accepted (`protocol`).

RPC scenarios: `protocol`, `settings_setup_about`, `browsers`, `runtimes`, `memory`, `chat_history`, `tasks_and_records`, `questions`, `schedules_reminders_timers`, `streams_and_processes`, `mcp_shim`, `update_prepare`.

Chat scenarios: `plain_reply`, `clarify_then_answer`, `one_step_task`, `one_step_with_tools`, `checker_skips_change_records`, `multi_step_plan`, `confirm_yes`, `confirm_no`, `confirm_stop_pending`, `pause_steer_resume`, `steer_running_task`, `stop_by_task_stop`, `stop_by_chat`, `stop_plus_new_task`, `global_stop`, `second_message_merged`, `schedule_created_and_fired`, `timer_set_and_fired`, `wait_for_time`, `tool_failure_then_retry`, `plan_repair`, `checker_rejects`, `checker_findings_fix`, `worker_question_answered`, `check_in_after_five`, `restart_mid_task`, `restart_with_open_question`, `attachment_and_bad_params`.

## What could not be recorded faithfully with fakes

- **Real Mac capabilities**, answered by stand-ins, so their replies show the stand-in, not a real Mac: `doctor.check`
  (permissions, self-tests, clipboard state, containment: `sandbox-exec` is never run, it counts as available),
  `setup.permissions` / `setup.permission_request` / `setup.finish` (no TCC), `clipboard.request`,
  `automation.request`, `ui.click_through` (the caller isn't FERB.app's process: refused), `reports.diagnostic`
  (doctor's facts).
- **Recorded only as refusals or empty answers**, because the state they act on needs the Mac, a browser or a
  provider: `browsers.pair`, `browsers.installed`, `browsers.unpair`, `setup.browser_open`, `setup.browser_pair`,
  `identity.pair` and the rest of the browser area (no Full Disk Access, no discovered profile, no extension
  connected); `ps.send`, `ps.attach`, `ps.kill` (no FERB shell process: starting one runs `sandbox-exec`);
  `secrets.delete`, `seen.image` (no saved secret, no stored screenshot); `conversations.change`, `attention.ack`
  (no delegated conversation, no notice: perception is paused); `runtimes.switch_task` (no task stopped on a usage
  limit); `runtimes.check` / `check_calls` / `model` (no runtime signed in: the real builders answer `SignedOut`
  before any program runs); `runtimes.sign_in` (the `claude` program is a stand-in that isn't there);
  `memory.import.start` (only refusals: a real import makes model calls this fixture doesn't script);
  `memory.import.estimate` with Claude Code transcripts is real (one invented session).
- **The native host after its hello**: the connection becomes the extension's message channel
  (`rust/fixtures/native-host` covers it); here only the hello is recorded.
- **`tasks.preview`, `questions.picture`**: no worker surface or payment picture exists, so `state: none` / `{}`.
- **Real model runtimes** (Codex app-server, Claude stream-json, the APIs): the scripted model stands for
  `RuntimeAdapter`; what each adapter sends on the wire is part 4's fixtures, not these.
- **Timing**: the planner's 180 s silence guard, real waits, the `CLIENT_WAIT_S` slow-call record.

## Surprises a porter must know

Each is seen in these fixtures (scenario named).

- **Frame order on a chat stream.** `ack` first, always, before routing (`chat.py:353`); then the supervisor's
  events and the bus's events for this turn's tasks, merged through one queue (`chat.py:365-418`). Every frame gets
  `request_id` and `at` stamped when it is queued (`stamped`, `chat.py:362`), so `at` is the clock then, not when
  the event was made. A bus event is wrapped `{"type": "event", "topic", "data"}`; an `action.*` outside any task
  goes to every open chat with `request_id: null`. The stream ends with `{"result": {"ended": true}}`.
- **`step` frames carry `seq`**, the UTC time to the microsecond, strictly increasing even with a frozen clock
  (`09:00:00.000000`, `.000001`…: ORCH-332). The app orders by it.
- **Live lines** are `{"type": "progress", "text", "task_id", "step_task_id", "started_at"}` and are not events
  of record (tests/conftest.py `turn_events` drops them); they still are bytes on the wire (`wait_for_time`).
- **A second message before the first reply** (`second_message_merged`): its stream is `ack`, `{"type": "merged",
  "into": <first request id>}`, end. The first stream carries the one answer; the planner was called twice for the
  same request id (the first call cancelled).
- **Errors differ by kind.** A unary `FerbError` is `-32004` with the bare text; a streaming call's error is
  `-32004` with `"<ExceptionType>: <text>"` (`server.py:432`, `chat.send` input_paths, `logs.subscribe`); any other
  exception is `-32603` with only the type name (`"KeyError"`). An unknown method is `-32001 "<m> not allowed for
  cli"` (the role check comes first; `METHOD_NOT_FOUND` only for a listed but unregistered method). A second
  `auth.hello` on a connection is refused the same way. A refused hello closes the connection.
- **A restart ends open `chat.send` streams with a normal `{"ended": true}`** and no outcome
  (`restart_mid_task`); the result reaches the app through `chat.status` `results` (with `request_id` and `at`)
  until `results.ack`.
- **Pause/steer replies are code's words first.** Pause: `PAUSED_SAID` + "\n\n" + the planner's text. A correction
  of a paused task gets " I'll apply it at my next step." (not delivered into a live run); one delivered into the
  running model call (`continue_`) gets no suffix (`steer_running_task`, the delivered text is in
  `model_calls[].continued_with`).
- **Stop told in chat** (ORCH-579): the reply is the task's own end ("Stopped, as you asked."). Stop plus a new task
  (ORCH-638): the new task's `plan.text` is the stop line, a blank line, then the plan line.
- **A check that never read the change records** of a step that wrote a file can't verify it (L39): the checker is
  asked again, then the task ends `done_unverified` with "I couldn't confirm part of this."
  (`checker_skips_change_records`). Other scripted checkers of file-writing steps read them first
  (`read_change_records`).
- **The trace keeps a redacted copy of the instructions**: `supervisor.md:24` contains "...pwd or passwd: a typed...",
  and the redactor masks it as a labelled secret, so the `model.turn` span's `system_prompt` is not the
  bytes sent (`@@body` references point at both forms).
- **FERB's own check-ins take only their options**: typed words are `-32004 "Pick Yes or No."`; a button is
  `chat.approve` with `choice` (1-based) (`check_in_after_five`). After a restart an open check-in is asked again
  under a new approval id (`restart_with_open_question`).
- **The timer loop sleeps in real time**: moving the clock alone fires nothing until the loop's next pass.
- **`update.prepare`** answers `ready: false` with the running tasks unless `even_if_busy`; once ready, `chat.send`
  is refused ("daemon is shutting down…").
- **Idempotent or forgiving calls**: `timers.repeat` on a fired timer twice returns the same repeat; `task.stop` of
  a finished task returns `stopped: []`; `schedules.change` on an unknown id answers with text, not an error;
  `trace.get` of an unknown id returns an empty trace; `runtimes.check` of an unknown runtime is `ok: false`, while
  `runtimes.models` of one is an error. Most handlers ignore unknown parameters.

## Found in the Python build (reported, not fixed here)

- **A file result's `mtime_ns` is redacted as a card number about one time in ten.** It is a 19-digit integer; when
  it passes the Luhn check the redactor replaces it with the string `"[redacted]"` (a type change too) in tool
  results, prompts and traces. Checked directly: `Redactor()({"mtime_ns": 1794538250392642525})` gives
  `'[redacted]'` (`ferb/act/fs/files.py:51`). This made two runs differ until the fixture fixed the file facts.
- **Missing or wrong parameters crash as internal errors** instead of a `FerbError`: `chat.approve` / `chat.answer`
  with an unknown id (`KeyError`) or on a question already set aside (`ValueError`), `memory.add` / `show` / `edit`
  / `retract` / `forget`, `schedules.change`, `reminders.cancel`, `attention.ack`, `conversations.change` without
  their keys (`KeyError`), `results.ack`, `timeline.get`, `conversations.list` with a bad value (`ValueError`),
  `automation.request` with a non-list (`TypeError`). Each also writes a `daemon.internal_error` journal row.
- **The global stop is told as the task's words**: a task ended by `stop.engage` is `failed` with summary
  `"(from the task) FERB was stopped"`, though the line is FERB's own (`global_stop`).
- The trace's copy of the instructions is redacted (above), so a trace isn't what the model got.
