# Plan: the front door (Concierge), built to be quick

Status: **agreed by the owner on 2026-11-26 ("yes go"), ORCH-726. Nothing here is built yet; the steps are
T-1338..T-1344 and T-1337.** Written by the coordinator after
reading the owner's spec (`FERB-v2-concierge-architecture-spec.md` at the repo's root, untracked, written with agy), the code at `b0d2edf6`, the
owner's store (read only) and real-model probes: 27 Codex calls, 14 Claude Sonnet requests (run by the owner) and
a few Gemini calls through agy.

Labels used below: **(owner)** his decision, with his words where recorded; **(agent)** a mechanism the coordinator
chose, changeable; **(measured)** seen in a probe or the store; **(inferred)**; **(unverified)**.

## How to pick this up

1. Read this file, then `kb/plans/context.md` (ORCH-719: its open steps T-1332..T-1337 stay, see "What happens to
   ORCH-719").
2. The owner's spec is the *what*; this file is the *order and the corrections*. Where they differ, the section
   "Corrections to the spec" says why, each with its evidence.
3. Raw results and the probe kit: `.local/front-door-probes/` (untracked). `out/*.cx.jsonl` are the Codex runs,
   `run_claude_probes.sh` is the Claude half. Earlier fact-checks of the spec: `.local/spec-v2-verify/` (three reports
   by an earlier session's agents: claims, checked only where this file cites them).
4. Build steps are in "Build order". Each names its files, its test and the number that says it worked. Do them in
   order; a step's number is measured on the owner's installed build before the next step starts.
5. Before any step: `tools/kb/run kb show <file>` for each file it touches. Prompt files change only with a `prompts`
   decision; fixtures change only as a named difference (ENG-17).

## Why FERB is slow today (measured)

The owner's store, 131 planner turns, 17-26 Nov, seconds for the model call (count):

| | Opus 5.5 | Sonnet 5.5 | gpt-5.6-sol | gpt-5.6-terra |
|---|---|---|---|---|
| Reply, no lookup | 6.4 (14) | 10.4 (23) | 6.3 (12) | 7.2 (10) |
| Reply with a lookup | 6.7 (1) | 8.8 (4) | 12.6 (6) | 13.8 (4) |
| Task, before a worker starts | 10.8 (21) | 9.5 (13) | 20.8 (6) | 13.9 (13) |

Probes on the bundled Codex 0.166.0, gpt-5.6-terra, effort low (what the planner runs with: `codex/mod.rs:253`),
the message "ok no worries", seconds from the program's start (first) or from the message (second, same program):

| What was sent | First word | Done | Thinking tokens |
|---|---|---|---|
| A chatbot-sized prompt, fresh program | 2.6-3.0 | 2.7-3.2 | 0 |
| The same, second message into the same program | 1.6-2.0 | 2.0-2.1 | 0 |
| Today's planner prompt + answer form (32k tokens), fresh program | 8.4 | 9.0 | 178 |
| The same, same program | 4.0 | 5.0 | 71 |
| Today's planner prompt, plain text (no form) | 5.9 / 10.0 | 7.2 / 10.5 | 132 / 220 |
| A lean front-door prompt with tools (5.6k tokens), fresh program | 2.4 | 2.5 | 0 |
| The lean prompt, task request: the dispatch call arrives | — | 3.9 (same program), 5.1 (fresh) | 0-12 |

The same on the bundled Claude Code 2.1.310, Sonnet 5.5, its default effort (medium, adaptive thinking), seconds
from the message:

| What was sent | First word | Done | Requests |
|---|---|---|---|
| A chatbot-sized prompt | 0.7-1.25 | 1.0-1.6 | 1 |
| The same, second message into the same program | 0.67 | 0.91 | 1 |
| Today's planner prompt + answer form (43.5k tokens without the lookup tools), nothing cached | 3.5 (answer starts) | 4.5 | 1 |
| The same, cached | 2.5 | 3.1 | 1 |
| Today's planner prompt, plain text | 2.9 | 3.2 | 1 |
| A lean front-door prompt with tools (8k tokens) | 1.3-1.6 | 1.35-1.7 | 1 |
| The lean prompt, task request, left to finish | call at 3.5, its text at 5.2 | 5.2 | 2 |
| The lean prompt, task request, ended when the call arrives | call at 3.25 | 3.25 | 1 |

Claude Code sends the request 0.02-0.07 s after the message and starts in 0.13-0.25 s: it has almost no overhead of
its own. On today's prompt it thinks for 1.0-1.4 s before answering; on the lean prompt's plain replies it does not
think at all. A task's call is written from 2.0-2.4 s, so its first field (the ack) can be shown from about 2.5 s.

What that shows:

1. **The big prompt makes the model think and re-read before it speaks.** Same message, same model: 0 thinking
   tokens and about 1 s to the first word with a lean prompt; 71-220 thinking tokens and 3-10 s with today's 300-line
   prompt. Taking the answer form away did not help: the prompt is the cost, not the form. (This corrects the
   coordinator's first reading of the store, "input size is not the driver": across models it looked that way; with
   everything else held equal it is the largest single cost.)
2. **A fresh program per message costs about 1 s on Codex, perhaps 2.** Its first request waits for an account
   check: the request goes out 1.2-1.7 s after the program starts, against 0.43-0.5 s after the message in a program
   already running, and the whole short reply is 2.7-3.2 s against 2.0-2.1 s. FERB's cached-thread resume adds
   0.46-0.98 s today (store: `start_steps_ms.thread_resume`); whether a kept program avoids it is unmeasured. The
   store shows more on real prompts (2.1-2.3 s before the request on average, 55 planner calls), so the gain there
   may be larger. Starting the program itself is 0.04-0.2 s.
3. **A tool call costs a second request that says nothing**: 1.1 s and 4 output tokens after `dispatch_task`
   returned. Ending the turn the moment the call arrives works on Codex: `turn/interrupt` ended it in 5 ms, one
   model call, its usage still reported (`x9_interrupt`).
4. **FERB waits for the program to exit before using the answer** (`ferb-worker/src/worker.rs:1480-1511`): about
   2.1 s on 3 of the 6 latest Claude turns (store, `steps_ms.close`).
5. **On Claude the answer is sometimes produced twice, and the cause is still open.** 2 of 2 no-lookup turns on the
   newest build, 7 of 72 before it (journal 251): two answer calls, two requests, about 4.3 s each. The probe did
   not reproduce it: 4 of 4 replays of the same turn (without the nine lookup tools and the screenshot) made one
   request. The `minItems` lead (an earlier session's report) is **not supported**: the relay shows the answer tool
   sent unenforced with `minItems` and without it alike, so taking it out changes nothing. What is left: something
   that happens only with the lookup tools present. Step 1's relay record (each request's tool names and error
   flags) will show it on the installed build.
6. **Claude's call timing has never been recorded**: its timer's clock is `|| 0.0` (`claude/mod.rs:282`, also
   `api/mod.rs:389`) and nothing sets a real one. Every Claude row says 0 ms.
7. **An agent program is not a chatbot.** agy on Gemini 3.8 Flash (low): 7.3-7.7 s to start, 3.3 s for the reply.
   A chatbot keeps one connection open, sends one small request and shows the first word as it arrives.
8. FERB's own preparation is 0.1-0.4 s, mostly the screenshot (354 ms on the measured turn).
9. The pinned question misleads on every runtime. On Codex the same input gave `answers_question` true once and
   false once (`x3_today`). On Claude, five replays of "ok no worries" all dragged the three-day-old Codex question
   into the reply: four dropped the request ("I'll drop the Codex takeover request") and one reminded him of it
   (`c1_*`, `c2_*`, `c3_big_plain`).

Cost per message today (Claude Sonnet, store): $0.028 with a warm cache, $0.156-0.178 when it missed, $0.10 and 19 s
for one summary fold.

## What the owner decided (2026-11-26, this conversation)

- **Build the system in his spec**: a lean front door that only talks and hands work off, an indexed library, small
  typed tools with plain-text replies, a live status line, and the five roles (front door, task architect, worker,
  checker, librarian). "i want the system i disucssed with agy in the spec file, even if you 'think' it wont matter"
- **And decode the calls and fix what makes them slow**, as one combined piece of work.
- **Lines imported from his coding-agent chats are never always-on.** "never alwasy on" (They leave the always-sent
  profile; lookups still find them. 65 such lines are always-on today: `memory_profile`, `added_by = 'owner'`,
  origin `inferred`.)
- **Controlled background model calls are fine.** "we can have controlled AI calls in the bg" (Each one gets a
  stated rate; see the Librarian.)
- **Prepare while he types**: "i like thiss. but this has to be thought well and planned too."
- **Real-model probes are allowed** on Sonnet and a similar Codex model. The coordinator caps them (40 a runtime).

- **A program kept ready is for every runtime**, not Codex alone. "lets make teh 'keeping a program running'
  universal, so that we dont miss any AI"
- **An idle program is kept for 5 minutes.** "5 minutes is the program uptime"
- **The pinned question goes.** Asked what the coordinator suggests (removing it), he said: "idk how it helps, ferb
  anyways has enought context"
- **A message from the phone carries no picture of the Mac's screen** (the coordinator's proposal; his answer: "why
  no?? why deos it matter??"). The window's name still goes; the picture is fetched when the message is about the
  Mac's screen.

## Corrections to the spec (each with its evidence)

1. **`reserved_tokens: 22,512` is not a schema tax.** It is instructions + tool schemas + answer form + 1024
   (`worker.rs:152-164`). The answer form is 17.2k characters; the nine lookup tools' descriptions are about 22k.
2. **A 3-5k-token fixed prompt is not reachable** with the owner's own rules kept: `interaction.md` alone is 5.2k
   characters. The probe's lean prompt (those rules + a short role + tools + the last exchanges + live state) was
   5.6k tokens on Codex and answered in 2.5 s. Target: **at most 8k fixed, at most 15k in all** on an ordinary turn
   (agent's estimate; the guard in step 1 holds the real number).
3. **`dispatch_task` must keep the fields code enforces.** `profiles` confines the browser tools (HAND-580,
   `ferb-context/src/schemas.rs:322`); `folders` scopes terminal changes; `standing_sends` is a send permission;
   `earlier_results` carries provenance; conditions carry code checks. Free-text `hints` would drop all of that.
4. **The three-gate card does not remove today's clutter**: all 65 lines pass gate 1 (his Approve). His new rule
   (imports never always-on) is what removes them, by where a line came from, which code can check (rule 4: code
   never judges content).
5. **"One request by removing the form" holds only for plain replies.** Any tool call needs the turn ended by FERB
   (finding 3). The same ending fixes today's path.
6. **"Under 1 s to the first word" is not reachable on these runtimes.** Measured floor on Codex: 0.45 s before the
   request + about 1.1 s to the first token. Targets are in "Targets".
7. **Skipping the checker on read-only tasks changes PRM-348** (owner: the worker-checker loop stays). It is last in
   the order and waits for his word.
8. **Limits in the spec are his numbers, not ours** (rule 18): the question's lifetime, top-8 people, the card's
   size. ORCH-719's numbers stand until he changes them (20 exchanges, 40 / 20k hard stop, 14 days x 6 lines, 40
   words a month, profile 1,500 tokens).

## The design

### The front door's turn

One model, one request for a reply, one for a hand-off, two when it looks something up.

- **Input, in cache order**: (1) the shared rules (`interaction.md`, unchanged) and a short front-door role
  (`concierge.md`, new); (2) the tools; (3) the confirmed card; (4) the library index; (5) one device summary;
  then the changing tail: the time and where the message came from, running tasks (id, one line, state, what they
  wait on), open cards, reminders and schedules, the last 20 exchanges, the new message.
- **A reply** is plain text, streamed as written through `RedactionStream` + `LineGuard` (D33, GATE-581 kept).
- **A hand-off** is one call of `dispatch_task`. Its first field is `ack`: the one line the owner reads. Code shows
  the ack only after the call passed every check, then ends the run while the call is still open (finding 3). A
  refused call goes back to the model as the tool's error, and the owner has seen nothing. Where the runtime streams
  a call's arguments (Claude Code, the API loop) the ack is shown provisionally as it is written, on the plan
  channel that `plan_abort` already withdraws (`turn.rs:997`); on Codex it appears when the call arrives.
  (agent; a promise can never be shown for work that did not start)
- **A lookup** shows a live status line from the tool-start event (the app already draws `progress` with
  `planning: true`, `turn.rs:1182`), never a chat bubble.

### The tools (agent; names can change, the fields code enforces cannot go)

| Tool | Does | Code checks |
|---|---|---|
| `memory_read`, `chat_history`, `task_records` | read memory and the library, older chat, a task's trace and evidence | read-only scope, origins join the turn |
| `dispatch_task` | `ack`, `goal`, `user_quote`, `complexity` (simple / complex), `identity`, `profiles`, `folders`, `earlier_results`, `conditions` (text + optional code check), `standing_sends`, `name` | the quote against the owner's own messages, folder breadth, profile resolution, result ids, provenance on the task |
| `control_task` | stop, pause, continue, retry, answer a card | task id exists, a card-only question is refused, the quote |
| `set_reminder` | reminders and timers: only what he said about when | the quote; code computes the time and writes the receipt |
| `schedule` | create / change / cancel / show schedules | the quote, the clock times, the receipt |
| `remember` | remember and forget, also for a durable fact said in passing | the quote; forget asks on its card |

- Code writes every "saved / set / stopped / scheduled" line, as now (PRM-312); the prompt says so once.
- The front door's token gets a write scope per tool (today the planner's token is read-only:
  `ferb-orchestrator/src/lookups.rs:21`).
- Several effects in one message are several calls. Each takes effect when accepted; a refused one is repaired or
  reported. Today's all-or-nothing answer goes away: a deliberate change, named in the journal.

### After the hand-off

- **simple**: the worker starts at once with the goal, the owner's quote, the enforced fields and the conditions.
  It decides how.
- **complex** (several dependent parts, a batch of files, a loop, a delegated conversation, parallel work): the
  **task architect** runs off the owner's path with today's planning rules (the plan half of `supervisor.md`,
  `roles/*.md`, the inventory walk) and the plan half of today's answer form. It reads the owner's own messages, not
  only the front door's wording. Its plan passes today's validation unchanged.
- **Checker**: unchanged (PRM-348). ORCH-719's middle path (T-1336) is its cost work.
- **Librarian** (owner, 2026-11-26: "no, then add a daily librarian call. one call isnt taht much anyways"): code
  does the upkeep, with one model call a day.
  - Code lists what FERB already records for the day: each task's short name (every task in the
    store has one, two to five words, PRM-328) with how it ended, reminders and schedules set, memory changes.
  - **One model call a day**, only on a day that had messages, after the day closes and never during a turn: it
    reads that day's exchanges and code's list and writes the day's topic lines (at most 6, ORCH-719), plain-chat
    topics included. Written once, never rewritten. On the smallest model of his chosen provider.
  - **One call a month** writes that month's 40-word line from its day lines; a year's line once a year.
  - A fact said in passing is saved by the front door's `remember` call in the same turn. Card suggestions are listed
    by code for his daily review, as now.
  - The running summary and its fold go (measured: $0.10 and 19 s a call on Sonnet, 2,710 output tokens each time,
    and it contradicted a live reminder in trace 5). Older chat stays stored and searchable. This replaces ORCH-304.
  - Rate: at most 1 call a day and 1 a month for any user, light or heavy; none on a silent day.
  It never reads his coding sessions unasked (MEM-466 stays).

### The card and the library

- **Card**: only lines he stated or approved, and never an imported line (owner). Rendered by code; one short label
  per line; a short handle in place of the 32-hex id. 1,500 tokens at most (ORCH-719).
- **Library index**: ORCH-719's dated index (14 days, 6 lines a day, months, years) plus a table of contents of what
  memory holds: each kind with its count and the most recently used names. A view over `memory_items`
  (`kind`, `subject`, `key`), no new store. It names 8 of each kind (owner, ORCH-727: "yeah 8 looks fine").
- **Conversation lines** lose what the model does not need: `[cN]` stays and lookups accept it in place of the
  32-hex id; the origin list becomes "has outside content"; notices that are FERB's own toasts leave the window
  (they stay in the journal and in search).

### Keeping the runtime ready (new; not in the spec)

- **One mechanism for every runtime (owner): a program is ready before the message arrives, and an idle one is
  kept 5 minutes (owner).** What "ready" means differs by runtime, behind the one interface: Codex keeps its
  app-server and takes each message as a new thread; Claude Code has the next program started and initialised ahead
  (a second message into the same program would carry the first one's history); the API loop keeps its connection
  open. Measured on Claude Code it is worth about 0.2 s (it starts in 0.13-0.25 s and sends the request 0.02-0.07 s
  after the message); on Codex 0.7-1.2 s or more. Measured gain on Codex: 0.7-1.2 s a message on a short prompt; up to 1 s more if the resume is
  avoided too (unmeasured).
- **Who closes a program** (agent): one keeper in the daemon per role, runtime and model owns the ready program and
  is the only thing that starts or ends it. It ends one after 5 idle minutes (owner), after its message on a
  runtime that uses a program once (Claude Code), when the model, sign-in or settings change, when the daemon
  stops, and when it crashes (a new one is started). Ending is today's code: Codex is killed and reaped, a refreshed
  sign-in published, its folder removed (`codex/stream.rs:278`); Claude Code gets stdin closed, then SIGTERM, then
  SIGKILL (`claude/transport.rs`). The start-up sweep of leftover run folders stays (ENG-21). At most one ready
  program per role beside those in use.
- **Caching does not depend on the program** (checked):
  - Claude: the cache is Anthropic's, keyed by the text itself, with FERB's one-hour mark. Measured: a new program
    read all 43,508 tokens the previous program had written (`c1_today_a`, `c1_today_b`).
  - Codex: FERB gives every run the same thread id for a role and model (`cache_thread_id`, `codex/mod.rs:394`) by
    resuming a two-line template thread it writes into the run's folder (`resume_cached`, `stream.rs:900`); the
    provider's cache follows that id. Measured without it: a new thread read 0 of 32,054 tokens on an identical
    repeat. So a kept program must still present that thread id for every message.
- **How a kept Codex program takes each message on the cached thread id** (measured 2026-11-26, `r_*` runs):
  before each message FERB sends `thread/unsubscribe` for that id, writes the two-line template again and sends
  `thread/resume`: 21 ms in all. The next message then carries nothing of the last one (input 5,248 / 5,263 / 5,271
  tokens over three messages; asked for a code word given one message earlier it answered UNKNOWN) and reads the
  cache (4,864 of 5,263 tokens). Whole lean replies in the kept program took 1.7 and 2.4 s. What does not work:
  resuming again without the unsubscribe (the earlier turns stay: 5,248 / 6,528 / 7,837 tokens, and it answered
  PINEAPPLE); `thread/fork` (drops the developer instructions and reads no cache). `thread/revert` wants a turn id
  and was not needed.
- **Open design points, settled in step 6 before code**:
  - The tool token. Codex starts the tool server again for every thread (measured: `mcp_started` once per thread),
    with the environment fixed when the program started. Proposed: the token file's path is fixed, FERB rewrites its
    content before each thread; one turn at a time per program, a second message at the same moment gets a second
    program. Needs a security read (gateway area).
  - The sign-in copy and its refresh (`save_refresh`, `stream.rs:236`) over a long-lived program.
  - The cached-thread resume (0.46-0.98 s today): whether a kept program can resume once and reuse it. Unmeasured.
  - The catalog check that reads Codex's trace (`RUST_LOG=trace`, 2-4 MB of log a turn): keep the check, read less.
- **Preparing while he types** rides on the same mechanism: the app tells FERB typing began (a new RPC; Mac and
  phone); FERB starts or wakes the program and gathers everything except the screenshot, which is taken at send.
  Nothing is sent to a model and nothing leaves the Mac before send.

## Targets (measured again on his installed build after each step)

| Turn | Today (store) | Target |
|---|---|---|
| "ok no worries": first word | not recorded; done at 6-12 s | Codex 2 s, done by 2.5 s; Claude 1.5 s, done by 2 s |
| A lookup answer | 7-14 s | 5 s, with a status line from the first second |
| A task: the owner sees the ack | 9-21 s (the plan line is written last) | Codex 4 s; Claude 2.5 s; the worker starts by 4 s / 3.5 s |
| Input on an ordinary turn | 21k-103k tokens | at most 15k |
| Requests on a reply / a hand-off | 1-3 | 1 |

## Build order

Each step lands with its tests through the hooks, its journal entry and its kb records. "Number" is what is measured
on the owner's installed build before the next step.

**0. The measurements: done 2026-11-26** (tables above; raw files in `.local/front-door-probes/out/`). Left open:
why Claude sometimes answers twice (step 1 instruments it).

**1. Stopwatches.** A real clock for the Claude and API timers (`claude/mod.rs:282`, `api/mod.rs:389`); time to the
first word shown to the owner recorded per turn; `ferb stats` shows both per role and runtime; FERB's Claude relay
keeps, per request, the tool names called and each tool result's error flag (names and flags only, as RUN-477 keeps
usage), so a doubled answer names its cause; a guard test on the front door's fixed size (extends ORCH-720). Test: a stand-in runtime with an injected clock. Number: every new turn
has a non-zero first-word time.

**2. Free seconds on today's path.** (a) Use the answer when it arrives; close the program in the background
(`worker.rs:1480-1511`). (b) Once step 1's record names it: the fix for Claude's doubled answer. Number:
`steps_ms.close` no longer in the turn; no-lookup turns with two answers: 0.

**3. The spec's Phase 1 fixes.** (a) FERB's toast notices leave the planner's window (`words.rs:67-73`). (b) The
pinned question goes (owner) (`turn.rs:2821`, `planner.rs:649-674`, `join_question` at `turn.rs:889`);
quotes may come from his earlier message in the window. (c) One line in the changing tail: how long ago his last
message and FERB's last reply were. (d) A message from the phone carries the Mac window's name, not its picture.
(e) Imported lines leave the always-on profile (`profile.rs:470-522`, the 65 rows moved, none deleted). Each is a
prompt-or-behaviour change with its decision.

**4. Every runtime streams plain text and reports a tool's start.** `claude/mod.rs:384` and `:833-873`,
`codex/stream.rs:1421-1479` (text streams when there is no answer form; a start event on `item/started`),
`api/mod.rs`; the worker's `on_tool` hook carries the tool's name and arguments (`worker.rs:113-124`); a plain-text
guard in `reply_stream.rs` keeping `RedactionStream` and `LineGuard`; FERB ends a run on a tool call it accepted as
final. Measured on Codex already: reply text arrives with phase `final_answer`. Tests: stand-in runtimes for all
five; D33 / GATE-581 tests unchanged.

**5. The front door.** `concierge.md` (shown to the owner before it lands: it is prompt text), the tools above with
their checks moved from `problems()` (`turn.rs:1027-1216`) and `side_quote_problems` (`:1245-1337`) to each tool, the
write scopes, the ack rule. Behind one switch beside `bounded_planner()`; the old path stays until the replay
passes, then is removed (one path, no two planners kept). Test: the recorded chat scenarios replayed through the new
door with stand-ins, compared effect by effect (task fields, reminders, memory entries, stops) with what the old
planner produced; differences listed by name. Number: the targets table.

*Step 5 in detail (agent, 2026-11-26, read from the code at 9496ebcf).*

- **Switch.** `Daemon::front_door()` beside `bounded_planner()` (`flow/mod.rs:165`), default false; ferbd turns it
  on once the replay below passes; a Python recording's replay keeps it off. `Inner::decide` (`turn.rs:1004`) calls
  the front door instead of `supervise` when it is on. Everything after `decide` (`route`, `side_entries`, `start`,
  `control`, `exit`) is today's code, unchanged.
- **The run.** Plain mode (no answer form, RUN-480), system = the shared rules + `concierge.md`, the same planner
  blocks (`planner_blocks`, `turn.rs:1832`), `ends_on = [chat.dispatch_task, chat.ask]` (ORCH-733), text through
  `PlainReply` (ORCH-734). A lookup's start shows today's `progress` line with `planning: true`.
- **The tools** (gateway tools composed in ferbd, `named_only` like `task.trace` (ORCH-663, `listing.rs:39`), so no
  worker is ever offered them; not observation tools, so the front door's grant is `observe_only: false` with
  `tools` = the lookups + these; scope `chat:act`). Each one's arguments are the matching part of today's answer
  form, same field names and types, so every check stays byte-for-byte today's:

  | Tool | Arguments (today's form) | Outcome it makes |
  |---|---|---|
  | `chat.dispatch_task` | `ack` first, `task` (today's task form, plan included until step 7), `answers_question` | kind `task`, text = ack |
  | `chat.control_task` | `control`, or `answer_question`, or `conversation_changes`; `answers_question` | today's control / card answer |
  | `chat.ask` | `question` | kind `clarify`: pending input carried, `question.asked` journaled, the app's question as today |
  | `chat.set_reminder` | `timers`, `reminder_cancels`, `answers_question` | today's side entries |
  | `chat.schedule` | `schedules`, `schedule_changes`, `show_schedules`, `answers_question` | today's side entries |
  | `chat.remember` | `remember`, `forget`, `answers_question` | today's side entries |

  `chat.ask` is new against the draft prompt (which said "ask in plain text"): a question must stay a question
  entry, because the next message joins its attachments and words through `join_question` (`turn.rs:901`) and
  `save_pending` (`turn.rs:2924`), and code may not decide from the text that it is a question (rule 4).
- **A call.** The gateway hands the call to `Inner::front_call(task_id, tool, arguments)`: the turn is found from the
  grant's task (`{request}.lookups`, `lookup_grant`, `turn.rs:2120`); `answers_question` true joins the earlier
  question first (`join_question`); the arguments are parsed as today's form parts (a parse error is the tool's
  error); `problems()` and `side_quote_problems()` run on that part alone; problems go back to the model as the
  tool's error, the owner seeing nothing; an accepted part is kept on the turn. A second `chat.dispatch_task` in one
  message is refused (one task per message, as today's form).
- **After the run.** The accepted parts are joined in call order into one `SupervisorOutcome`; its text is the
  streamed text (an ack or a question is the text when there is no other); the whole outcome passes `problems()`
  once more (the checks that look across parts, e.g. `unbacked_parts`); then `route` as today. Effects take place
  at the turn's end in today's one apply path, not at each call (agent: one apply path; the turn ends within
  seconds of the last call). No accepted call and no text: today's empty-answer failure. A run cut short keeps the
  parts already accepted.
- **Not moved in step 5:** the plan half of the task form (step 7 takes it to the architect), the lookups, the
  checker. The repair round (`decide`'s second `supervise`) is not used by the front door: a refusal is repaired
  inside the run.
- **Test.** Every recorded chat scenario (`ferbd/tests/chat_replay.rs`) whose stand-in planner answers with today's
  form is converted mechanically (each part of the answer → its tool call) and run through the front door; the
  effects (task row fields, reminders, schedules, memory rows, stops, cards answered, replies' kinds) are compared
  with the old path's. Every difference is listed by name in the test and the journal.
- **Shown to the owner before it lands:** `concierge.md` and each tool's description (both reach the model).

**6. The ready runtime and preparing while typing.** The design points above settled and recorded first; then the
kept program, then the typing RPC (daemon), then the app's side (the owner's, with Codex). Number: first word on a
second message within 2 s on Codex.

**7. The task architect.** `complexity: complex` goes to the async architect with today's plan rules and
validation; `simple` skips it. Test: every existing plan-validation test passes on the architect's output; a simple
task's worker starts with no plan step. Number: a simple task's worker starts within 1 s of the ack.

*Step 7 in detail (agent, 2026-11-27; owner: "YES TO ALLL" to starting step 7).*

- **The hand-off says which kind it is.** `chat.dispatch_task` gains `complexity` ("simple" or "complex"), and for a
  simple one the few fields code enforces, all flat: `quote` (the owner's exact words for the task), `name` (two to
  five words, PRM-328), `done_when` (one plain sentence: what the result must show), and when his words name them
  `profiles`, `folders`, `earlier_results` (result tags cN). Simple = one ask a worker can do in one go; complex =
  several dependent parts, a batch of files, a loop or wait, a delegated conversation, parallel work (the plan's own
  list). The Concierge decides (rule 4: a model judges intent); code checks every field as today.
- **Simple: no planner.** Code builds the TaskSpec from those fields: goal, name, identity owner, the default worker
  type, one part {quote}, one condition {text: done_when, layer outcome}, no plan, the profiles/folders/results named.
  `problems()` runs on it unchanged (the quote against his messages, folders, results, profiles), inside the call: a
  refusal goes back to the Concierge in the same run, as every call's does. Then `route` starts it: the worker starts
  right after the ack. No planner call; no planning line.
- **Complex: the architect.** Today's planner path (ORCH-743) with the planning line (ORCH-747), until the
  architect's own instructions land: the plan half of supervisor.md and of the answer form, reading the owner's own
  messages for quotes and the Concierge's goal as a summary (the stopgap HANDOFF sentence, owner-approved, says so now).
- **Model-facing text shown to the owner before it lands:** the concierge.md lines on simple vs complex and the four
  fields; chat.dispatch_task's description; later the architect's instructions.
- **Test:** every recorded single-ask hand-off replayed as simple starts the same task row fields that matter (goal,
  quote, profiles, folders) with no planner call; every multi-part one as complex runs as today; a simple hand-off
  with a quote he never wrote is refused in the run.
- **Number:** a simple task's worker starts within about 1 s of the ack (plan target).

**8. The library and the Librarian.** ORCH-719's T-1337 as designed, plus the table of contents and the Librarian
as decided (code's list, the daily call, the monthly line; the running summary and its fold removed). Test: the index's size on an invented five-year store; byte-stable within a day.

**9. Only on his word:** the checker skipped on read-only tasks (changes PRM-348).

## Older decisions each step changes

A step that changes one of these records the new decision (superseding the old one) in its own commit, with the
owner's words from "What the owner decided". Until that commit the old decision stands, because the code still does
what it says.

| Step | Older decision | What changes |
|---|---|---|
| 3b | ORCH-586 (agent): an open question is shown apart and joined when the planner says so | the question is no longer pinned or joined |
| 3d | none found for phone messages (ORCH-704 is about temporary chats) | a phone message carries no picture of the Mac's screen |
| 3e | MEM-483 (owner): his 65 approved imported lines stay always on | imported lines are never always-on |
| 5 | RUN-447, PRM-336 (owner): the planner answers through its form or an answer tool; ORCH-630 (agent): `text` last | plain text and typed tools |
| 8 | ORCH-304 (owner): the running summary | the Librarian's day lines |
| 9 | PRM-348 (owner): the worker-checker loop stays | only on his word |

Not changed: ORCH-507 (a question waits as long as its task waits), MEM-466, MEM-8 (facts said in passing are
captured: by the front door's `remember`), the ORCH-719 numbers.

## What happens to ORCH-719

T-1332 to T-1336 (snapshots, RUN-470 off, the worker's tool core, the fresh run, the checker's middle path) are worker
and checker work: 95% of all tokens. They stay as planned and can run beside this plan, one agent per crate. T-1337
becomes step 8 here. ORCH-719's "planner 27-32k per message" is replaced by this plan's 15k once step 5 lands.

## What must not change

Origin and provenance on everything read (`ferb_sanitize::origin`, `carried::derive`); the owner-quote check on every
lasting change; redaction and `LineGuard` on every word a model writes for him, the status line included; card-only
approvals; the sink rule; one path for every runtime; no model call he did not ask for outside the Librarian's
stated rate.

## Still to measure

- Why Claude sometimes answers twice (step 1).
- FERB's real tool server's start time (the probe used a stand-in: 20-40 ms).
- Whether Codex streams a tool call's arguments to its client (the probe saw none; the trace has them).
- The cached-thread resume inside a kept program.
- Gemini through FERB's own API route (agy is a different program; its numbers only show an agent program's cost).

## Waiting on the owner

1. **The checker skipped on read-only tasks** (step 9): only if he says so.

For his information: his weekly Codex allowance was at 73% during the probes (17 small calls used of it).
