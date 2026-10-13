# Daemon RPC: the client contract

The daemon is the single entry point for CLI, voice and future clients. Clients send requests, render events,
answer identified questions and request stop. They must not reimplement planning, verification or tool dispatch.
This document describes the existing local protocol; it does not expose FERB over a network.

<!-- source: ferb/daemon/server.py#Server._connection -->
<!-- source: ferb/daemon/auth.py#Role -->
<!-- source: ferb/cli/client.py#Client -->

## Transport and authentication

Connect to `~/.ferb/run/ferb.sock` (or the configured FERB home). The directory is owner-only and the Unix socket
is mode 0600. Frames are UTF-8 JSON-RPC 2.0 objects followed by a newline. Every connection starts with:

```json
{"jsonrpc":"2.0","id":1,"method":"auth.hello","params":{"role":"cli"}}
```

The daemon verifies the kernel peer UID for `cli`; the role string alone is not authentication. An MCP worker uses
`mcp_shim` and an expiring, task-scoped run token rechecked on every call. The shim makes one call per connection and
waits with no transport cutoff; when its runtime gives up a call it closes that connection, and the daemon reads that
as the caller leaving: the call is not cancelled, its result goes nowhere, and an owner question it waited on stays
open, untaken, so the identical retry of that call in the same request waits on the same question (or receives the
answer the owner gave meanwhile) instead of asking again. The native host has extension-origin and
pairing/HMAC checks. `voice` requires the installed app’s expected executable and signature. The exact role method
allowlists live in `ferb/daemon/auth.py`. Never give an external client a native-host or worker token to bypass CLI auth.

A future Android client needs a separately authenticated transport adapter to this owner boundary. There is no
remote bearer-token login or TLS listener today. Adding one does not require changing the supervisor.

Each incoming frame has a 1 MiB transport bound. Oversize is an explicit `-32002` error and closes the connection;
text is never prefix-truncated and executed. This is a message boundary, not a deliverable length target. Large
artifact work can use multiple file edits or shell/file operations. Native messaging has its own framing and the
browser bridge pages large observations; client RPC and extension protocol versions are distinct.

Extension protocol 14 (L26) adds the `media` command: play, pause, mute, unmute or seek one video/audio element of
the latest snapshot, by frame and index, through the content script (no script evaluation, no click); it answers with
that element's state (`playing`, `current_time`, `duration`, `muted`, `volume`, `in_view`, and the page's `error` when
it refused). Each snapshot frame lists its media elements' state under `media`. The daemon refuses an extension on
another protocol, so the daemon and extension update together; the extension version is unchanged.

`doctor.check` also returns `hearing` (L26): `installed` (FERB.app), and when probed `signed` (matches the signature
setup recorded) and FERB.app's own `speech` permission state, `on_device` support and `locale`. It probes by default,
or with `selftest: false` only when `hearing: true` is passed; probing starts FERB.app in the background and never
prompts.

## Conversation and results

| Method | Parameters | Result / behavior |
|---|---|---|
| `ping` | `{}` | PID, role, timestamp, explicit stop state |
| `chat.attach` | `{new?: bool, resume?: conversation_id}` | `{ok, session_id, capabilities:["input_paths"]}`; resume/new conversation; marks running tasks attached |
| `chat.send` | `{session_id, text, input_paths?: [path,...]}` | Stream of `chat.send.event` notifications (`ack` first), then `{ended:true}` |
| `chat.status` | `{peek?: bool}` | `results` (each with `request_id` and `at`, when the result arrived), `running`, `approvals`, `notices`; fetching unseen results marks them shown unless `peek:true`; `notices` are the perception notices no client has acknowledged yet, each `{id, text, quotes, at}` |
| `results.ack` | `{task_ids:[task_id,...]}` | Marks only those results shown, for a client that reads `chat.status` with `peek:true` and acknowledges each result it placed in its chat (the notch UI); `{ok:true}`. Unknown ids are ignored |
| `attention.ack` | `{ids: [notice_id,...]}` (older CLIs: `{id}`) | `{ok:true}`; the client showed those notices, so they leave `chat.status.notices`; an unknown id refuses the whole call |
| `chat.answer` | `{approval_id, text}` | Answers that exact pending question; empty text declines |
| `setup.persona` | `{refresh?: bool, status?: bool}` | CLI-owner role only; local discovery state (`scanning`, `pending`, `reviewed`, `failed`) and pending profile approval |
| `chat.approve` | `{approval_id, granted, choice?: integer}` | Explicit yes/no or 1-based option on that exact question |
| `task.stop` | `{task_ids:[root_id,...]}` | Stops those trees; omitted/empty IDs explicitly means all running trees |
| `stop.engage` | `{}` | Persistent global stop, revokes run tokens and invokes stop hooks |
| `stop.lift` | `{}` | Lifts global stop; does not replay completed/uncertain actions |
| `trace.get` | `{task_id: exact_id_or_unique_prefix_or_last}` | Redacted span forest plus structured `outcome` |
| `verdict.set` | `{task_id: exact_root_id_or_unique_prefix, verdict: "pass"\|"fail", note?}` | Appends the owner's verdict (note redacted); refuses `last` and child tasks, naming the root |
| `recovery.list` | `{}` | What FERB can put back (S1), newest first: `{days, total, trash_days, records}`; at most 100 records, `total` counts all |
| `recovery.restore` | `{id: record_id_or_unique_prefix}` | Puts one back, never over a change made since: `{id, kind, original, restored_to, in_place}`; journaled `recovery.restored` |
| `timeline.get` | `{since?: ISO timestamp}` | Complete matching journal entries, chronological |
| `logs.subscribe` | Optional `filters` list and `maxsize` queue bound | `logs.subscribe.event` notifications; overflow is explicit |

`chat.send` events contain a `type`; examples include ack, plan, progress/tool events, verification, approval/question,
and outcome. Render unknown event types safely rather than treating them as completion.

Every `chat.send` event carries `request_id`, the id journaled with that message's `chat.message` (null only on an
outside observation, below), and `at`, when the daemon sent it (ISO 8601 with offset). Events about a task also carry `task_id`. Use `request_id` to attach a reply,
plan or outcome to its submitted turn, even when it arrives after a later turn's events.
- `ack {request_id, text, message, at}` comes first, on receipt and before the message waits for routing. `text` is one
  of the owner-approved P25 phrases (`ferb/orchestrator/acks.py`), never one of the last eight said; it claims nothing
  about what FERB will do. `message` is the owner's message as the daemon redacted it, the text `chat.message` records
  (secret values replaced): a client keeps this copy, never the raw text it sent. Pasted or quoted parts appear as sent,
  including the terminal's bracketed-paste markers (`\x1b[200~` … `\x1b[201~`) around pasted text; only
  `chat.message` wraps them in FERB's data markers. The ack is journaled as `chat.ack`, with `received_at`, when the
  message arrived. A second message on the same chat is acknowledged at once, while routing stays in arrival order
  (P29).
- `plan {request_id, task_id, text, goal, at}` comes as soon as the planner returns a task, before the disturbance
  baseline sample and before any worker acts. It is journaled as `chat.plan {request_id, task_id}` (the time only; the
  planner's text stays in its trace).
- `progress {request_id, task_id, text, started_at, at}` comes each time a worker or the final checker starts a tool
  call, or when the task's question or FERB's own check goes to the owner. `text` is built by code from the tool and its target
  (`ferb/orchestrator/progress.py`): "reading github.com", "looking at Brave", "writing notes.md", "running a
  command", "asking you", or the verb of the tool's kind. A page is its browser-parsed host; an app is named as FERB
  knows it (the Mac's app list; "an app" when the worker's name for it matches none). It never holds model text (the
  worker's `why`, typed text, commands, page content, its own names for apps), except a file name the worker chose
  (the basename), which may appear. `started_at` is when that call started. Show one live line that each `progress` replaces,
  with the time elapsed since `started_at`; remove it at the `outcome` or when the stream ends. It is not an answer or
  a completion, and is not saved.
- Task ids: `plan`, `progress` and `outcome` carry the tree id. `subtask` and `verification` carry the step's own task
  id: the worker's `<tree>.worker` (a plan step's `<tree>.step.<id>`). Approvals may also name a worker id; its tree
  id is the part before the first `.`.
- Bus events merged into the stream (`{type:"event", topic, data}`) carry the stream's `request_id` and `at`.
  `approval.requested` data also carries the asking task's `request_id` (none for a question outside a task, such as
  the setup profile), on the stream and on `logs.subscribe` alike. `action.*` observations that belong to no task
  (no `task_id`) are forwarded to every open chat stream with `request_id: null`: they are not about that stream's
  message.
- `task.resumed_event` (bus): each event of a plan resumed after a restart, with no chat stream, carries the saved
  request's `request_id` and `at`, as `task.result` does.
- The `task.result` bus event carries `request_id` and `at`: the outcome's own when a chat turn sent it, otherwise
  (a resumed plan, a scheduled run) the request the task ran for and when the result was recorded; `request_id` is
  null when no task record names one. A scheduled run's request is its own, never a chat message's.

`outcome` includes status (`done` only when every condition was verified, `done_unverified` when the work finished
but a check couldn't confirm it, `failed`, `needs_user`, `cancelled`), answer, artifacts/evidence through subtasks, verification, a separate
`stop_reason`, and resource outcomes. Render optional `display_text` as the concise reply and retain the complete `answer` in expandable Details.
If absent/empty, fall back to `answer`. Do not invent a client summary or change completion status.

Conversational replies/clarifications can also emit `reply_delta` with `{text}` while the model is generating.
Append these already-redacted fragments to a **provisional** reply belonging to this connection's submitted turn.
They are not a completed answer or proof of an action. The authoritative `reply`/`clarify` replaces the draft with `display_text` (fallback `text`), retaining `text` as Details;
`plan` clears it and starts normal task presentation. `reply_abort` with `{reason}` withdraws the draft. A transport
failure or EOF before an authoritative event must mark the draft interrupted, never completed or replayed.
Ignore late fragments after a final/plan. Do not speak, notify completion, or save partial text as a completed answer.
Old clients may ignore these additional events and continue rendering the final unchanged. Stream identity is the
existing request connection and its `request_id`; independent submitted turns must never share their draft buffer.

Streaming is genuine provider output, not a delayed replay of an already-complete answer. The supervisor releases
only a top-level `display_text` field written last, after full `text` and every action/control field establishes a pure conversation turn. It
buffers action receipts, reordered output and task/worker/checker answers through execution and verification.
Secret redaction and FERB-label marking preserve state across fragments. Raw provider JSON, reasoning, tool
arguments and partial replies are not added to the daemon's journal or trace; canonical replies still are.

Attachment-aware clients require `input_paths` in the attachment capabilities before sending selected files; an old
daemon must not silently discard them. Paths are filesystem source metadata, not extra owner instructions or
permission to send/delete the file. Existing read protection still applies. Paths persist through clarification;
the planner's eventual plan receives them as input sources. Secret-bearing filenames are refused before transport
into planning or persistence. Pasted outside text must retain the CLI's external-data delimiters unless the owner
explicitly identifies it as their own request.

Status results include `request_id` and `at` (as on `task.result`, so a client whose `chat.send` stream ended early can
still place the answer), `display_text`, `artifacts`, `verified` and `verification`; pending approvals include their
`task_id` root when the request maps to one tree, otherwise null. Clients must not guess a task identity from display
text.
The full final answer and artifact list also persist as `trace.get.outcome.result` for known-task reconnect, even
if another client already marked the result shown. Older historical tasks may lack these fields.

A streaming request occupies its connection. Use a second authenticated connection for question replies, stop,
status or trace reads. The stream ends with a response carrying its request ID. Events are notifications, without
that ID, using `<method>.event` and `params`. A client disconnect ends its subscription, while an already-started
chat task continues in the daemon. Reconnect with `chat.attach` and `chat.status`. Live logs are not a durable replay
cursor; use trace/timeline for persisted facts. `chat.status` without `peek` is a delivery read (`ferb chat` uses it
when it opens). A client that polls should read with `peek:true` and call `results.ack` for each result it actually
showed, so `ferb chat` replays only what no client showed.

### Perception notices (L9)

A notice is FERB's message about something it observed on the Mac. It belongs to the owner's daily chat: it is
journaled `chat.notice {id, text, quotes}`, published on the bus as `attention.notice {id, text, quotes, at}` (no
`request_id`; it answers no turn), and listed in `chat.status.notices` until a client sends `attention.ack`. Render it
as a FERB message with a small "noticed" tag. `text` is FERB's words, inferred from outside observations (origin
`external:observation,inferred`); each quote `{text, origin}` is outside text it repeats and must be shown as a
quote with its origin, never as FERB's own words or as instructions (MP2). Posting a notice is telling it: FERB
does not repeat it whether or not a client acknowledged it. Quote origins are assigned by the daemon from the
observations it considered, never taken from the model. While an authenticated `logs.subscribe` stream with no
filter, or with a filter that includes the topic, is open, FERB sends no macOS banner for that topic
(`attention.notice` for notices, `task.result` for results): the client shows it. A subscription filtered to other
topics (`ferb logs --filter timers`, `ferb voice`) does not count. Otherwise the banner is sent, and the notice still
waits in `chat.status`.

## Questions and secrets

An ordinary question has an `approval_id`, reason/question and options. Routed ordinary `owner.ask` questions may
include `display_text`; retain the complete reason under Details. Secret, retry and consequential approval prompts
keep their concrete full text; clients must not compress them. Keep full option labels and free-text answers;
duplicate labels must not become a first-match selection. Ordinary typed chat text is a message, never an answer:
even when it matches an option of the only open question (a lone "Yes"), send it with `chat.send` and let the planner
route it. Only an explicit answer to a shown question uses `chat.answer`/`chat.approve`. Pending questions never
expire (L3): a question stays open while its task waits and closes when it is answered or its task ends; silence is
never consent. Scheduled unattended work cannot wait indefinitely for an answer and reports what remains unresolved.

A secret question is marked `owner_login_code`, `owner_secret` or `owner_admin_password`. Collect it with hidden input and send it only through its
question ID. Do not put it into chat history, telemetry, screenshots or speech. The daemon redacts before persistence
and fills only the origin/task-bound secret destination. Account and OTP candidate identity are explicit facts;
“latest message” is not proof that a code belongs to a login.

First `chat.attach` starts a missing setup profile scan asynchronously. The shared `setup.persona` owner question
uses the existing approval channel with a concise reason, concrete proposal and full `evidence` list. Clients should
offer that evidence for review; it is not a task completion. Save, an explicit free-text edit, or Skip resolves the
question through `chat.approve`/`chat.answer`. Silence does not confirm it. The question is created once (one
`approval_id` and one `approval.requested` for any number of `chat.status` polls) and never posts a macOS banner.
Reconnect reuses collected evidence; confirmed or skipped profiles do not prompt again unless the owner explicitly
requests refresh. `chat.status` includes the discovery state and enriched approval. Unconfirmed metadata stays local; confirmed text enters the
owner's ordinary preference memory and stable identity context.

## Outcomes, observations and errors

The `outcome` export has schema version 1: request and original messages, task specification, persisted tasks,
planner and all approach decisions (including recovery/questions/resource review), full context-source records, provider tool-catalog observations,
worker runs, intake receipts, tool actions/results, verification, disturbances, cleanup, result and outcome
journal records. `owner_verdict` is the latest `verdict.set` record (`verdict`, `note`, `at`), or null when none was
given; null is not acceptance. Historical coverage flags distinguish missing
request/sentinel data from a tested success. Fields may be added compatibly; clients should preserve unknown ones.

Disturbance spans have baseline/change/error/summary kinds, plus paused/resumed around a `ferb pause` (nothing about the
desktop is recorded between them). A change lists before/after facts, overlapping/recent actions (span IDs, each with
its capability's `on_screen` flag), owner-input observations and explicit non-attribution; when an app came to the front
or the Space changed it also lists the FERB-started processes running then (another task's by id only), and with no
on-screen FERB action in progress `on_screen_in_task` and `last_on_screen_ended_s`. Missing data is not zero
disturbance. The sentinel is evidence, not an approval gate. Images are not embedded in stored tool spans; use artifact
receipts.

Standard errors: `-32700` malformed JSON, `-32600` invalid request, `-32601` missing method, `-32603` internal error;
FERB errors: `-32001` unauthorized, `-32002` oversize, `-32003` unsupported, `-32004` operation failure. Do not retry
an effectful action merely because a connection ended: inspect its task/ledger outcome. Unknown delivery is distinct
from not executed. Stop and resource ownership stay in the harness for every future client and approach.

Native task controls enumerate exact backend roots and pending local turn IDs. Stop task sends only that nonempty
backend ID to `task.stop`; a pending-turn stop is deferred only until that turn receives its plan/task identity.
Task inspection uses `trace.get` and never invokes Stop. Background answers update state/history without opening
or refocusing the native panel. A new genuine approval or clarification opens its question once with amber
attention, without activating keyboard focus. Approval IDs and local turn IDs deduplicate attention across polls
and reconnects. Clients reconcile all roots and must not let an older status response undo newer task/approval
events. Interrupted unbound streams are not running tasks. Supported Markdown is rendered as native text, tables,
lists, links and code; raw HTML is not executed and remote images are not automatically fetched.

Worker `shell.run`/`shell.start` reach any remote host through FERB's egress proxy without asking (L2); there is no
`network` argument and no network approval. The proxy records each host, port and byte count per process (the
`shell.network` journal row, written when the task ends or, for a process still running then, when it exits). A
process that has ended, or that FERB doesn't track, gets nothing; a local service needs its executable-pinned
permission, and the owner's No to a host in the task stays final. A `protection.local_ports` port whose listener runs
its pinned executable when a command starts is reachable directly by that command (L24); its `network` facts and
`shell.network` row then carry `local_direct: [{host: "localhost", port, executable, local: true, direct: true,
reported_connections, clients}]`, where `reported_connections` and `clients` are what the kernel reported for that
command's own tag so far: a lower bound, at least one per program that connected (a burst of repeats may be counted
later), and `null` when the system log can't be read. The journal row keeps only ports with a reported connection or
an unknown count. A report stream that stops is journaled once as `shell.local_reports_stopped {why}` and shown by
`ferb doctor`. A tree's No to one of this Mac's addresses, as it stands when a command starts, keeps that command on
the proxy route. Declare `external_source` for externally sourced
code/instructions; it is recorded with the call and adds no approval. Semantic origin is a model responsibility, not
complete detection by code. Use `shell.start` or `background: true` for a continuing network process; failure never
automatically retries an uncertain command. FERB does not inspect encrypted payloads. `owner.ask` has no
`allow_destination` argument.
