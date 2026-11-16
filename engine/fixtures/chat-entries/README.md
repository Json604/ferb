# Fixtures: the chat as entries (stage 2, ENG-8; stage 3's phone transcript, S6)

**Not Python-recorded.** The Python build has no chat entries: these are the Rust engine's own projection
(`ferb-supervisor/src/flow/entries.rs` `ChatProjector`) of what FERB.app's reducer keeps today, and what FERB.app's fold
of those entries shows. They are not parity fixtures (ENG-3) and no Python script makes them.

| File | What | Written by |
|---|---|---|
| `scenarios.json` | 84 scenarios: each one's `timeline` (built from the recordings, or for an `invented:` source written in `chat_entries.rs`, ENG-18) and the `entries` it ends with | `engine/crates/ferb-supervisor/tests/chat_entries.rs` |
| `engine_chat.json`, `engine_more.json` | the 28 and 33 scenarios ferbd replays through the real program: each one's `timeline` in the order the ENGINE fed its chat log (its real interleaving across connections and the bus) and the `entries` it stored | `engine/crates/ferbd/tests/chat_replay.rs` |
| `transcripts.json` | for every scenario of the three files, each conversation as FERB.app's fold (`EntryFold`) shows it, in a platform-neutral shape a phone renders (below, "The phone's transcript"; ENG-22) | `apps/mac/FERBNotch/Tests/NotchCoreTests/EntryFoldTranscriptTests.swift` |

    cd engine && UPDATE_GOLDEN=1 cargo test -p ferb-supervisor --test chat_entries    # scenarios.json again
    cd engine && cargo test -p ferb-supervisor --test chat_entries                    # compare, byte for byte
    cd apps/mac/FERBNotch && UPDATE_GOLDEN=1 swift test --filter EntryFoldTranscriptTests    # transcripts.json again
    cd apps/mac/FERBNotch && swift test --filter EntryFoldTranscriptTests                    # compare, byte for byte

(`source scripts/swiftpm-anonymous.sh` first for Swift.) Two runs write the same bytes (ids `ce-000001`… per scenario,
the recordings' frozen clock, one change counter per conversation; sorted keys). Regenerate `transcripts.json` after
`scenarios.json` or `engine_*.json` change.

## Where the timelines come from

The 71 recorded scenarios of the Python daemon: `engine/fixtures/daemon/chat_scenarios.json` (28),
`engine/fixtures/supervisor/control_scenarios.json` (10) and `more_scenarios.json` (33). Each scenario becomes one
timeline of the conversation(s) it touched. Then `stop_beside_new_work_two_turns` (APP-919, a live case) and 12
timelines written for the phone (S6): `gaps_single_task`, `gaps_checklist`, `gaps_card_row` (ORCH-651),
`sealed_ids_in_message` (GATE-604), `legacy_then_new` (ENG-12), `deleted_turn_between`, `deleted_last_turn`,
`reply_after_a_deleted_turn` (STORE-424), `fresh_conversation` (STORE-427), `turn_from_another_client` (APP-916),
`questions_answered_and_expired` (ENG-13), `question_left_open` (APP-300, APP-581).

| `do` | From the recording | Fed to the engine (`Input`) and to FERB.app (`EntryFoldTests`) |
|---|---|---|
| `send` | a `chat.send` the daemon acknowledged (a refused one is left out) | the app's `begin`; the engine only adds `input_paths` to the ack |
| `frame` | each frame of that stream: its `ack` right after the send, the rest where the stream ended | `Frame` / `receiveChatEvent`; a stream whose ack names a `client_id` (another device) reaches the app as its entries through the watch (`applyEntryChange`, APP-916) |
| `bus` `task.result` | after every `outcome` frame (chat.rs `task_result` publishes it) | `Bus` / `receiveBusEvent` |
| `end` | the stream's end (`ended` or the daemon's shutdown) | `StreamEnded` / the stream task's end and `finishSubmission` |
| `result`, `notice`, `bus approval.requested` | the first `chat.status` reply holding a task result, notice or question no stream carried | `Bus` / `receiveOutcome(announce: false)`, `receiveNotices`, `receiveBusEvent` |
| `bus approval.resolved` | after the `chat.approve`/`chat.answer`/`chat.correct` call, with the facts of the scenario's `question.answered` journal row; a question set aside by a stop or steer at the end | `Bus` / `receiveBusEvent` |
| `notification` | a `notifications.subscribe` banner (a reminder's is posted in the chat) | `Notification` / `receiveReminder` |

A recording keeps each connection's lines in order, not their order across connections; putting a stream's frames
where it ended is a choice, the same one for the engine and the app. `preview_attention` sends no message (an empty
timeline); `login_sites` and `fresh_conversation` have two conversations (`s1`, `s2`).

### The steps the store writes itself (written timelines only)

Some entries are not made by the projector: the store makes them. `chat_entries.rs` keeps a copy of what the store holds
(`Conv`) and does what the engine does, citing the code it mirrors; after each, the conversation's projector is dropped
and rebuilt from that copy as `ChatLog::projector` rebuilds it (open entries, their messages, the last 60, every ref; a
stream still open keeps its session, `streams_open`).

| `do` | The engine | FERB.app (`EntryFoldTests.appModel`) |
|---|---|---|
| `legacy` `{id, kind, at, day, ref?, body}` | a `chat_history` row carried over (migration 0063): entry `h:<id>`, kind `legacy`, body `{"kind", "body": <the app's bytes>}`, positions just below 2^32 (the stream's own start at 2^32) | its history read back at launch (`absorb`) |
| `delete` `{request_id}` | STORE-424's `data.delete`: every entry of the request or its tasks tombstoned (`deleted`, body `{}`, `rev + 1`) | `removeTurns` (Settings → Stored data) |
| `fresh` | STORE-427's `mark_fresh`: the new conversation's first entry, a notice `ref conversation:<id>` | `startFreshConversation`'s notice |
| `restart` `{resumed}` | the start-up pass (`after_restart`, ENG-13): cut work interrupted, open questions expired | a relaunch: a new model reads the saved history back |

The `legacy` bodies are the app's own saved form (`StoredEntry.encoded()`: sorted keys, dates as seconds since 2001,
UUIDs); `EntryFoldTranscriptTests.testTheLegacyRowsAreTheAppsOwnSavedForm` decodes and re-encodes each to the same JSON.

## The engine's own interleaving (`engine_*.json`)

Written from a run of the real `ferbd daemon` program (the test build's stand-ins `chat_trace`: ferb-supervisor
`ChatLog::set_trace` writes every input the projectors are fed, in order, with the changes it made):

    cd engine && UPDATE_GOLDEN=1 cargo test -p ferbd --test chat_replay -- the_recorded_chat the_more_scenarios   # write
    cd engine && cargo test -p ferbd --test chat_replay                                                            # compare

It is written only from a full run in which every scenario replays fully, and compared otherwise: a run that
interleaved differently fails as `<name>: replays partly — engine order: …`. The timeline uses `scenarios.json`'s words
with these differences: every bus event is `bus` (the engine reads all of them from the bus, as FERB.app's
`logs.subscribe` does; there is no `result` or `notice` read from `chat.status`), a bus copy the engine held for its
stream's end is placed where it was fed (that end), a `task.result` the stream fed itself is not repeated, and
`{"do": "restart"}` marks the program's restart (its start-up pass). Entry ids are renamed `ce-000001`… by place (the log's
own ids are random); paths are under `/FX`.

## How it is checked

- `chat_entries.rs`: the golden bytes; one message per ack; nothing live stored; `seq` grows, `pos` fixed, each revision
  `rev + 1` and different from the one before; merged messages name their request; late answers say what they show; and
  the rules on invented frames (APP-580 placement, ORCH-579, ORCH-318, an error, DMN-583, a card's transitions only, a
  deleted turn no longer ending the chat, a reply on a stream open across a delete).
- `apps/mac/FERBNotch/Tests/NotchCoreTests/EntryFoldTests.swift` reads this folder from the repository and runs each
  timeline through FERB.app's own reducer, folds the entries with `EntryFold`, and compares the two transcripts on what
  the owner sees (ids, `createdAt`, a notice's `noticedAt` and `tag`, a turn's `sentElsewhere` left out): all 85
  conversations equal, and the open questions the app holds equal the entries' open questions (not where a timeline
  delivers a question again after its answer: the recorded order puts a stream's frames at its end). It does the same
  with `engine_*.json`: the app's reducer driven in the engine's real order against the engine's stored entries folded.
- `EntryFoldTranscriptTests.swift`: `transcripts.json` byte for byte against the fold, written the same twice, and what
  the twelve phone scenarios show.
- The order rules (`engine/crates/ferb-supervisor/tests/support_entries/mod.rs`, ENG-15): checked on the engine's trace
  after every scenario ferbd replays, and on the pure projector over each timeline of `scenarios.json` and 40 invented
  interleavings of its lanes each (a timeline with a step the store writes keeps its one order).

## The phone's transcript (`transcripts.json`, for S7)

What FERB.app shows for a conversation, computed from the stored entries alone by FERB.app's fold (`EntryFold.archive`,
proven equal to the app's live reducer above). The phone's fold (ferb-mobile, S7) reads the same entries and must
produce the same items. Keys are sorted; `null` is written, never left out.

```
{"about", "scenarios": [{"file", "scenario", "conversations": [{"conversation", "items": [...], "questions": [...]}]}]}
```

`items` are the chat top to bottom, as drawn (`ConversationArchive.transcript()`). Every item has `item`, `entry` (the
id of the entry that made it: a message, a recovered task's or a late answer's answer entry, a notice or post, or the
`legacy` entry holding the app's saved form) and `legacy` (true for a row carried from `chat_history`).

**`turn`**: one message and FERB's answer to it, or FERB's own turn for a task no message here started (APP-585).

| Field | What | From |
|---|---|---|
| `from` | `owner`; `ferb` for a recovered turn (no message: an answer entry with no `answers`) | message entry / answer entry |
| `text`, `files`, `hidden`, `sealed`, `hidden_notice` | the owner's words as recorded and redacted, attachments' paths, secret kinds hidden, IDs kept sealed (GATE-604, `{kind, name}`), and the Mac's line under the message (built from `hidden` and `sealed`) | message `body.text`, `attachments`, `hidden`, `sealed` |
| `goal` | a recovered turn's description (instead of `text`) | answer `body.name`/`goal` |
| `request_id`, `task_id`, `client_id` | the request, the task bound to the turn, the client that sent it (APP-916; null for the Mac's own in these fixtures) | message entry fields, plan `task_id` |
| `task_name` | the task's short name | plan `body.name`/`goal` |
| `first_reply` | FERB's first bubble: `direct` answers (empty when a card shows them in its rows), `receipts`, `receipt_cards`, `plan_line` and `plan_lines` (`plan` only when there is no plan line), `says` (whether the bubble has words at all); null when no plan came | plan body |
| `card` | a multi-ask card (ORCH-319): `task_id`, `request_id`, `rows` (each `part`, `label`, `state`, `text`, `note`, `why`, `verified`, `direct`, `step`, `question`… and `shows` `{kind, words}`: the row as history draws it, `RowDisplay` with no live step state), `done`, `total`, `line`, `lines`, `headline`, `plan` | plan `body.card` |
| `checklist` | a single ask's steps (APP-894): `steps`, `ended` (each step's end), `finished`, `lines` (`{step, kind, words}` as history draws them; `gaps` = "Parts not done"), `collapsed` ("2 of 3 steps done" once finished) | plan `body.checklist` |
| `status`, `status_inferred`, `notice` | `working`, `needs_user`, `done`, `done_unverified`, `done_with_gaps` (ORCH-651: "Delivered, with parts not done"), `failed`, `cancelled`, `interrupted`, `merged`…; whether the Mac inferred it (an interrupted message); the words under an interrupted message | the shown answer's `body.status`, else the message's `state` and `body.notice` |
| `merged_into` | the request a merged message was read with (ORCH-318) | message `body.merged_into` |
| `answer` | `text` (Markdown; a gapped answer ends with code's "Not done: <condition> — <reason>." lines, there is no separate field), `display_text`, `verified` (the check mark, APP-580), `artifacts`, `note`, `lines`, `cards`, `runtime_switch`; null while there is none | the turn's shown answer: its newest `answer`/`error` entry that `follows` nothing |
| `question` | the question FERB asked (a `needs_user` answer) kept above a later final answer | shown answer `body.question` |
| `answered_below` | "Answered below" (APP-580): a `reply` item further down shows this turn's answer | an answer entry with `placement: below` |
| `parts` | told parts of turns saved before cards (legacy only) | legacy body |

**`reply`**: an answer shown at the bottom (APP-580) or a later answer (APP-914). `answers` (the turn's `entry`),
`after` (the turn it is drawn after), `part` (`answer` or `question`: what of the turn it shows), `quotes_request`
(drawn with a one-line quote of the turn's words), `said` (a later answer's own `text`, `status`, `verified`, … ; null:
the reply shows the answered turn's own `answer`, or its `question` for part `question`). From an answer entry with
`placement: below`, or one whose `body.follows` names the answer it comes after.

**`notice`** / **`reminder`**: FERB's own line in the chat (a notice: `text`, `quotes` `{text, origin}` shown as quotes,
`cards`; a reminder that came due: its `cards` hold the reminder card). `ref` is FERB's id for it (posted once), `after`
the turn it follows (null before any turn: a fresh conversation's marker comes first, STORE-427). From `notice` and
`post` entries.

**`questions`**: every question entry not deleted, in `pos` order: `entry`, `approval_id`, `request_id`, `task_id`,
`state` (`open`, `answered`, `declined`, `expired`…), `open`, `body` (the engine's question: `reason`, `options`,
`labels`, `part`, `secret`, `answer`…) and `shown`: where the Mac draws it, `card_row` (inside the row of its turn's
card that asks it, `row` `{turn, part}`, APP-300), `end_of_chat` (an open question with no row, at the end of today's
chat, APP-581), or null (no longer open: the Mac shows nothing of it; its answer shows in the task's own words).

### What drives what

- An entry's newest revision (`rev`) counts; a `deleted` entry (tombstone, STORE-424) is left out and leaves no trace:
  a late answer another turn showed after it is drawn after the kept turn before it.
- Items follow `pos` for turns; a `reply` or `notice` goes after the turn that was newest when it was made, in the
  order made. A `legacy` row is read as the app saved it (its own `createdAt`, `afterTurnID`).
- A turn's answer is its newest `answer`/`error` that `follows` nothing; an entry with `follows` is a later answer.
- `placement` is decided once, by the engine (APP-580); the phone never decides it again.

### Live state is never an entry (ENG-8)

The entries hold what the Mac saves. What only shows while it happens comes from `chat.entries.watch` as
`{"type": "live", "request_id", "frame"}` and is never stored: the typing dots after an ack, the streamed plan line
(`plan_delta`), the one live line under a working turn (`progress`, with its elapsed time), each card row's and
checklist step's live state (`step`, `subtask`: "Working", "Waiting for …", "Checking the result"; `transcripts.json`
shows rows and steps as history does, without them), `verification`, `memory`, a timer's countdown. A phone draws them
from the watch while a turn is open and drops them at the turn's answer or the stream's end; after a restart of the
watch it has none until the next frame.

### What a phone handles that the Mac doesn't

- **Paging from the middle.** FERB.app holds its own history; a phone fetches `chat.entries.page {before_pos}` on
  scroll, newest first. A page can start inside a turn (its answer without its message, a `reply` whose turn is on an
  older page, a `legacy` reply naming a legacy turn not yet fetched): keep what was fetched by id and fold the union,
  so an item resolves when its turn arrives; never draw a reply's quote from nothing. `pos` below 2^32 is carried
  history (legacy), the stream's own entries start at 2^32.
- **Changes since a cursor.** `chat.entries.changes {epoch, after_seq}` and the watch's catch-up give each changed entry
  at its newest revision: replace by id (`rev`), including a tombstone that empties a fetched entry.
- **`reset_required`**: the cursor's epoch is not the store's any more (a new store): drop the cursor and what was
  fetched for that conversation and read again from the newest page. **`cursor_ahead`**: the cursor is past the head:
  same. **`behind`** on the watch: read `changes` from the cursor, then watch again.
- **`body_cut {chars, full_chars}`**: a reply under `max_bytes` cut an entry's words; fetch the whole entry with
  `chat.entries.get` before folding it (a cut body never replaces a whole one).
- **Questions** are entries here (the Mac holds open ones apart from the bus): draw the open ones by `shown`, answer
  through the question's own call, and let the entry's new revision close it.
- **A fresh conversation** (STORE-427) is a new conversation id whose first entry is the marker notice; the Mac shows
  it in its one chat, a phone shows it at the top of the new conversation.
- What only the Mac holds: a sealed ID's "Let the AI see" (`shown`, GATE-602) is the Mac's own saved state, not in the
  entries; which device's turn it is (`sentElsewhere`) is the Mac's writer bookkeeping (APP-917).
