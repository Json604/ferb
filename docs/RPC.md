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
answer the owner gave meanwhile) instead of asking again. A retry is matched on the worker's own question and options,
before the orchestrator rewords them, and only to a question its own task asked; a P19 retry question is matched on
FERB's whole text, attempt count included, and the owner's Yes settles against the attempts that question named. An
action that ran to completion after its call's caller had gone is not run again by that task's identical retry (same
tool and arguments, as P19 binds them): the retry gets the earlier result with `completed_in_an_earlier_attempt: true`. `tools.call` takes an optional `call_id` (`"<threadId>/<callId>"` from the
runtime's `tools/call` `_meta`); Codex drops a call that outlives its own per-call timeout without telling the shim,
so the Codex adapter names that call (its failed `mcpToolCall` item carries the same id) and the daemon ends it as if
the shim had left, closing the shim's connection. An answer is never taken by a call that has left or been released.
Residual: between Codex's timeout and the adapter reading its failed item (milliseconds), an answer the owner gives can
still be taken by the dropped call and is then lost; the owner is asked again by the retry. The native host has extension-origin and
pairing/HMAC checks. `voice` requires the installed app’s expected executable and signature. The exact role method
allowlists live in `ferb/daemon/auth.py`. Never give an external client a native-host or worker token to bypass CLI auth.

A future Android client needs a separately authenticated transport adapter to this owner boundary. There is no
remote bearer-token login or TLS listener today. Adding one does not require changing the supervisor.

Each incoming frame has a 1 MiB transport bound. Oversize is an explicit `-32002` error and closes the connection;
text is never prefix-truncated and executed. This is a message boundary, not a deliverable length target. Large
artifact work can use multiple file edits or shell/file operations. Native messaging has its own framing and the
browser bridge pages large observations; client RPC and extension protocol versions are distinct.

Extension protocol 15 (fix wave 4, 0.15.4): `window_secrets` takes the captured window's `title` (to tell windows
at the same bounds apart by their active tab's title) and answers with that tab (`tab: {url, title}`, URL with secret
parameters masked). `screenshot` and `record_frame` capture a page the extension can't read instead of refusing:
the reply carries the picture, `unread` (why), `mask` (`{before, after: null}` when only the look after failed, else
null) and `window` (`{bounds, active, minimized}` of the tab's window); `withheld` with no picture remains only for a
secret field the extension found but can't cover.

Extension protocol 16 (fix wave 4 round 3, 0.15.5): `screenshot` and `record_frame` attach fully when the page can be
scripted, and with the debugger alone (no input-readiness probe) when it can't. `window` also carries the tab's
`title` and `emulated` (FERB lays the tab out at its own size). A page the debugger can't attach to answers
`{withheld, undebuggable: true, window}`, for the daemon's native window capture.

Extension protocol 17 (fix wave 4 round 5, 0.15.6): a capture of a page scripts can't reach stays armed (no full
attach and detach per capture), a tab already attached fully isn't attached again, and a scriptable page's input setup
or readiness failure refuses no capture: the reply carries `input_note`.

Extension protocol 14 (L26) adds the `media` command: play, pause, mute, unmute or seek one video/audio element of
the latest snapshot, by frame and index, through the content script (no script evaluation, no click); it answers with
that element's state (`playing`, `current_time`, `duration`, `muted`, `volume`, `in_view`, and the page's `error` when
it refused). Each snapshot frame lists its media elements' state under `media`. The daemon refuses an extension on
another protocol, so the daemon and extension update together; the extension version is unchanged.

`doctor.check` also returns `runtimes.limits` (L28): per role, the runtime and model with its documented
`context_window`, `max_output`, `context_budget` and `source`, or a `problem` when the window is unknown (the planner
then fails explicitly). `containment.local_reports` is the state of the kernel-report stream for pinned local ports
(L24), or null when no port is pinned. `doctor.check` also returns `hearing` (L26): `installed` (FERB.app), and when
probed `signed` (matches the signature
setup recorded) and FERB.app's own `speech` permission state, `on_device` support and `locale`. It probes by default,
or with `selftest: false` only when `hearing: true` is passed; probing starts FERB.app in the background and never
prompts.
`doctor.check` also returns `clipboard` (I3): how macOS treats the daemon reading the clipboard (`default`, `ask`,
`allow`, `deny`; `unrestricted` before macOS 15.4), read without the contents, or null. `clipboard.request` (CLI;
`ferb setup` only, with the owner's yes) reads the clipboard once so macOS shows its paste alert now, journals
`clipboard.access` and returns `{access}`; nothing read is kept.

## Conversation and results

| Method | Parameters | Result / behavior |
|---|---|---|
| `ping` | `{}` | PID, role, timestamp, explicit stop state |
| `chat.attach` | `{new?: bool, resume?: conversation_id}` | `{ok, session_id, capabilities:["input_paths"]}`; resume/new conversation; marks running tasks attached |
| `chat.send` | `{session_id, text, input_paths?: [path,...]}` | Stream of `chat.send.event` notifications (`ack` first), then `{ended:true}` |
| `chat.status` | `{peek?: bool}` | `results` (each with `request_id` and `at`, when the result arrived), `running`, `approvals`, `notices`, `loops`; fetching unseen results marks them shown unless `peek:true`; `notices` are the perception notices no client has acknowledged yet, each `{id, text, quotes, at}`; `loops` is each running loop step's latest `loop.status` payload (below) |
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
| `recovery.list` | `{}` | What FERB can put back (S1), newest first: `{days, total, trash_days, records}`; at most 100 records, `total` counts all; a record has `needs_owner: true` when what fs.trash left in the Trash isn't what it checked (restore refuses it) |
| `recovery.restore` | `{id: record_id_or_unique_prefix}` | Puts one back, never over a change made since: `{id, kind, original, restored_to, in_place}`; journaled `recovery.restored` |
| `timeline.get` | `{since?: ISO timestamp}` | Complete matching journal entries, chronological |
| `schedules.change` | `{id, action: "cancel"\|"pause"\|"resume"}` | `{text}` in plain words; cancel and pause also withdraw the standing scope recorded with that schedule from every task holding it (journaled `standing.revoked`) |
| `memory.learning` | `{op: "digest"}` (among other ops) | `{digest}`: the day's pending identity proposals, once a day, recorded as seen; the notch app shows them with Confirm (`op: "confirm"`) and Not right (`op: "retire"`) |
| `test.mode` | `{status: true}`, `{allow_send: [entry,...]}` or `{off: true}` | `{enabled, allow_send}`; an entry is an exact recipient, or `"SITE @ACCOUNT"` (a bare `"SITE"` for a post that declares no account) for a declared post on a page of that host from that account, never a payment; a URL-shaped entry is refused. Saved in config, so it survives a restart; journaled `test.mode` |
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
- A `progress` with `connection: "lost"` or `"back"` is FERB's own line about its AI service (fix wave 7): the first
  error the service retries itself in a run the owner waits on ("Your Mac seems to be offline: I'll carry on when
  it's back." with no network route, else "Lost the connection to the AI service I use (codex); it's retrying and the
  task carries on." and the service's own words; "…it's retrying, and I'll answer when it's back." during the
  planner's turn), and once that run gets a real event again or starts a tool call,
  "Reconnected to the AI service I use (codex).". Once per outage per run; `started_at` is when it was said. During
  the planner's turn, before any task exists, it has no `task_id`. Show it as the live line; `ferb chat` prints it.
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
- `loop.status` (bus, critical: never dropped for a slow subscriber; L31): a loop step's state each time it changes,
  `{task_id, step, goal, state, rounds, last_round_end, gap_seconds}`. `task_id` is the tree id, `step` the plan step's
  id, `goal` the step's goal; `state` is `acting` (a round runs), `watching`, `backing_off`, `paused` (waiting for the
  owner's reply), `finished` or `stopped`; `rounds` counts the rounds done; `last_round_end` is when the last round
  ended (epoch seconds, a whole or decimal number) or null before any; `gap_seconds` is the back-off gap between rounds
  (0 outside a back-off). Show a loop between rounds as standing work, not running work; drop it at `finished`,
  `stopped` or the task's `task.result`.
- The `task.result` bus event carries `request_id` and `at`: the outcome's own when a chat turn sent it, otherwise
  (a resumed plan, a scheduled run) the request the task ran for and when the result was recorded; `request_id` is
  null when no task record names one. A scheduled run's request is its own, never a chat message's.

`outcome` includes status (`done` only when every condition was verified, `done_unverified` when the work finished
but a check couldn't confirm it, `failed`, `needs_user`, `cancelled`), answer, artifacts/evidence through subtasks, verification, a separate
`stop_reason`, and resource outcomes. Render `answer` in full: it is the answer the owner reads, with no separate
short text (owner, 2026-10-24; outcomes recorded before then may still carry a `display_text`, which a client may show
with `answer` under Details). Do not invent a client summary or change completion status.
`verified` (bool) is true only for `done`, when every condition was checked true; show it as a check mark. `answer`
carries no "Verified." prefix (L11). For `failed`, `needs_user` and `done_unverified`, `answer` starts with FERB's
status line (such as "Done, but not confirmed:"), then a `needs_user` outcome's open question on its own
`Question: …` line, the checker's findings, why work stopped, what was confirmed and the files. A task that ended
without the owner's stop says why in plain words, never an exception's name.
When the task reached any site, `answer` ends with a code-built line `Sites contacted: host, host` (L2's list).
What FERB did with a turn's reminders, memory writes, reminder cancels and schedules is said in code-built lines
("⏰ Reminder set for …", "Remembered: …", "🗓 Scheduled: …", "⏰ Cancelled the reminder …", what already exists for
a reminder its message already had set, or one line naming what it refused; plain words, never an internal id),
never by the planner's text: a `reply`/`clarify` gets them appended to
`text`; on a task's turn refusals also arrive as `timer`/`memory` events (as the other lines always
do), and the outcome's `answer` carries every line. A `timer` event for a reminder just set carries
its id apart, in `timer_id` (the `text` stays plain words; `ferb chat -v` shows the id). A steering reply acknowledges
the change in the owner's own words (L11, owner 2026-10-24): its `display_text` is one short line, "Got it: <the
correction, on one line, clipped>", plus "I'll apply it at my next step." while no running step has taken it and any
restated standing scope; its `text` keeps the whole correction and the task's "Updated task: <goal>". Show
`display_text` first, with `text` under Details; a client that shows one text shows `text`. A big job's `plan` text ends with the stated plan ("Here's the plan; I'm starting now. Reply to
change anything:", its steps, estimates and standing scopes); work has already started, and a reply is a correction
(L3). A correction that changes a standing scope says so in its reply ("For this task I'll now send these without
asking each time: …"); a narrower scope applies at once, a wider one only once that reply is out. On a run nobody
started from chat (a scheduled run, or a plan resumed after a restart) nothing new or wider is granted: the reply lists
those scopes under "These weren't set up with you, so they aren't covered: …". A schedule's
confirmation lists the standing scopes recorded with it and those not recorded (their destination isn't in the
owner's typed words).

Conversational replies/clarifications can also emit `reply_delta` with `{text}` while the model is generating.
Append these already-redacted fragments to a **provisional** reply belonging to this connection's submitted turn.
They are not a completed answer or proof of an action. The authoritative `reply`/`clarify` replaces the draft with its `text`;
`plan` clears it and starts normal task presentation. `reply_abort` with `{reason}` withdraws the draft. A transport
failure or EOF before an authoritative event must mark the draft interrupted, never completed or replayed.
Ignore late fragments after a final/plan. Do not speak, notify completion, or save partial text as a completed answer.
Old clients may ignore these additional events and continue rendering the final unchanged. Stream identity is the
existing request connection and its `request_id`; independent submitted turns must never share their draft buffer.

Streaming is genuine provider output, not a delayed replay of an already-complete answer. The supervisor releases
only the top-level `text` field written last, after every action/control field establishes a pure conversation turn. It
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
still place the answer), `artifacts`, `verified` and `verification`; pending approvals include their
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
include `display_text`; retain the complete reason under Details. Honour `display_text` only on such a question
(capability `owner.ask`, kind `owner_question`): it is the orchestrator's rewording (D33). Secret, retry and
consequential approval prompts keep their concrete full text in `reason`; clients must not compress or reword it. The
questions FERB's own code builds also carry `summary`, one plain line that code writes from the facts it checked, never
from a task's words (P27, owner 2026-10-24): a `ferb.confirm` check ("FERB wants to send to bob@x.com, whom you didn't
name."), the escape hatch ("FERB wants to read <folder> once, with one command (normally off-limits: …)."),
`owner_secret` ("For <site or app> only: “<name>” · saved in your Mac's Keychain", or "For <site or app>: a password
or private answer — used once, not saved"), `owner_login_code` ("For <site>: a login code — used once") and
`owner_admin_password`. It is on `approval.requested` and on each `chat.status` approval that has one: show it first,
with the full `reason` under Details. A question without `summary` shows its `reason` (or an ordinary question's
`display_text`). Keep full option labels and free-text answers;
duplicate labels must not become a first-match selection. Ordinary typed chat text is a message, never an answer:
even when it matches an option of the only open question (a lone "Yes"), send it with `chat.send` and let the planner
route it. The planner routes an answer by the question's id to any open `owner.ask`, `plan.ask` or `ferb.confirm`
question (L3); the answer recorded is the owner's own words. FERB's own Yes/No questions carry the capability
`ferb.confirm` (the code floor and sink checks), `ferb.retry` (P19, one more attempt at an action whose outcome is
unknown) or `conversation.approve` (F9, a delegated conversation's scope). They take only a plain Yes or No; when the
reply could answer more than one open question (or the planner's own pending question), FERB asks which one in plain
words (a `clarify`). A
spoken transcript (`voice.transcript`) is a chat turn the same way (`{"handled": "request"}`), except while a secret
question waits (the old `voice.question_selection_required` topic is gone). `approval.resolved` may carry
`state: "superseded"`: the owner's correction changed what the
question was about (only questions the planner names as changed), or its task was stopped. Superseded is not a No;
clients remove the question. Only an explicit answer to a shown question uses `chat.answer`/`chat.approve`. Pending
questions never expire (L3): a question stays open while its task waits and closes when it is answered or its task
ends; silence is never consent. Scheduled unattended work cannot wait indefinitely for an answer and reports what
remains unresolved.

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
