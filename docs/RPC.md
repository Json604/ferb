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
`mcp_shim` and an expiring, task-scoped run token rechecked on every call. The native host has extension-origin and
pairing/HMAC checks. `voice` requires the installed app’s expected executable and signature. The exact role method
allowlists live in `ferb/daemon/auth.py`. Never give an external client a native-host or worker token to bypass CLI auth.

A future Android client needs a separately authenticated transport adapter to this owner boundary. There is no
remote bearer-token login or TLS listener today. Adding one does not require changing the supervisor.

Each incoming frame has a 1 MiB transport bound. Oversize is an explicit `-32002` error and closes the connection;
text is never prefix-truncated and executed. This is a message boundary, not a deliverable length target. Large
artifact work can use multiple file edits or shell/file operations. Native messaging has its own framing and the
browser bridge pages large observations; client RPC and extension protocol versions are distinct.

## Conversation and results

| Method | Parameters | Result / behavior |
|---|---|---|
| `ping` | `{}` | PID, role, timestamp, explicit stop state |
| `chat.attach` | `{new?: bool, resume?: conversation_id}` | `{ok, session_id, capabilities:["input_paths"]}`; resume/new conversation; marks running tasks attached |
| `chat.send` | `{session_id, text, input_paths?: [path,...]}` | Stream of `chat.send.event` notifications, then `{ended:true}` |
| `chat.status` | `{peek?: bool}` | `results`, `running`, `approvals`; fetching unseen results marks them shown unless `peek:true` |
| `chat.answer` | `{approval_id, text}` | Answers that exact pending question; empty text declines |
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

`chat.send` events contain a `type`; examples include plan, progress/tool events, verification, approval/question,
and outcome. Render unknown event types safely rather than treating them as completion. `outcome` includes status
(`done` only when every condition was verified, `done_unverified` when the work finished but a check couldn't confirm
it, `failed`, `needs_user`, `cancelled`), answer, artifacts/evidence through subtasks, verification, a separate
`stop_reason`, and resource outcomes. Display the supplied `answer` verbatim: rebuilding a cheerful summary can hide
failed conditions.

Conversational replies/clarifications can also emit `reply_delta` with `{text}` while the model is generating.
Append these already-redacted fragments to a **provisional** reply belonging to this connection's submitted turn.
They are not a completed answer or proof of an action. The authoritative `reply`/`clarify` replaces the draft;
`plan` clears it and starts normal task presentation. `reply_abort` with `{reason}` withdraws the draft. A transport
failure or EOF before an authoritative event must mark the draft interrupted, never completed or replayed.
Ignore late fragments after a final/plan. Do not speak, notify completion, or save partial text as a completed answer.
Old clients may ignore these additional events and continue rendering the final unchanged. Stream identity is the
existing request connection; independent submitted turns must never share their draft buffer.

Streaming is genuine provider output, not a delayed replay of an already-complete answer. The supervisor releases
only a top-level `text` field written after every action/control field establishes a pure conversation turn. It
buffers action receipts, reordered output and task/worker/checker answers through execution and verification.
Secret redaction and FERB-label marking preserve state across fragments. Raw provider JSON, reasoning, tool
arguments and partial replies are not added to the daemon's journal or trace; canonical replies still are.

Attachment-aware clients require `input_paths` in the attachment capabilities before sending selected files; an old
daemon must not silently discard them. Paths are filesystem source metadata, not extra owner instructions or
permission to send/delete the file. Existing read protection still applies. Paths persist through clarification;
the planner's eventual plan receives them as input sources. Secret-bearing filenames are refused before transport
into planning or persistence. Pasted outside text must retain the CLI's external-data delimiters unless the owner
explicitly identifies it as their own request.

Status results include `artifacts`, `verified` and `verification`; pending approvals include their `task_id` root
when the request maps to one tree, otherwise null. Clients must not guess a task identity from display text.
The full final answer and artifact list also persist as `trace.get.outcome.result` for known-task reconnect, even
if another client already marked the result shown. Older historical tasks may lack these fields.

A streaming request occupies its connection. Use a second authenticated connection for question replies, stop,
status or trace reads. The stream ends with a response carrying its request ID. Events are notifications, without
that ID, using `<method>.event` and `params`. A client disconnect ends its subscription, while an already-started
chat task continues in the daemon. Reconnect with `chat.attach` and `chat.status`. Live logs are not a durable replay
cursor; use trace/timeline for persisted facts. `chat.status` is currently a delivery read, not an acknowledged queue:
a future client wanting durable multi-device delivery needs explicit acknowledgments, not orchestration changes.

## Questions and secrets

An ordinary question has an `approval_id`, reason/question and options. Keep full labels and free-text answers;
duplicate labels must not become a first-match selection. Pending questions expire after 600 seconds; timeout is
not consent. Scheduled unattended work cannot wait indefinitely for an answer and reports what remains unresolved.

A secret question is marked `owner_login_code`, `owner_secret` or `owner_admin_password`. Collect it with hidden input and send it only through its
question ID. Do not put it into chat history, telemetry, screenshots or speech. The daemon redacts before persistence
and fills only the origin/task-bound secret destination. Account and OTP candidate identity are explicit facts;
“latest message” is not proof that a code belongs to a login.

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
