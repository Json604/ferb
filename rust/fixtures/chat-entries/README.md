# Fixtures: the chat as entries (stage 2, ENG-8)

**Not Python-recorded.** The Python build has no chat entries: these are the Rust engine's own projection
(`ferb-supervisor/src/flow/entries.rs` `ChatProjector`) of what FERB.app's reducer keeps today. They are not parity
fixtures (ENG-3) and no Python script makes them.

| File | What |
|---|---|
| `scenarios.json` | 71 scenarios: each one's `timeline` (built from the recordings) and the `entries` it ends with |

Written by `rust/crates/ferb-supervisor/tests/chat_entries.rs`:

    cd rust && UPDATE_GOLDEN=1 cargo test -p ferb-supervisor --test chat_entries    # write it again
    cd rust && cargo test -p ferb-supervisor --test chat_entries                    # compare, byte for byte

Two runs write the same bytes (ids `ce-000001`… per scenario, the recordings' frozen clock, one change counter per
conversation).

## Where the timelines come from

The 71 recorded scenarios of the Python daemon: `rust/fixtures/daemon/chat_scenarios.json` (28),
`rust/fixtures/supervisor/control_scenarios.json` (10) and `more_scenarios.json` (33). Each scenario becomes one
timeline of the conversation(s) it touched:

| `do` | From the recording | Fed to the engine (`Input`) and to FERB.app (`EntryFoldTests`) |
|---|---|---|
| `send` | a `chat.send` the daemon acknowledged (a refused one is left out) | the app's `begin`; the engine only adds `input_paths` to the ack |
| `frame` | each frame of that stream: its `ack` right after the send, the rest where the stream ended | `Frame` / `receiveChatEvent` |
| `bus` `task.result` | after every `outcome` frame (chat.rs `task_result` publishes it) | `Bus` / `receiveBusEvent` |
| `end` | the stream's end (`ended` or the daemon's shutdown) | `StreamEnded` / the stream task's end and `finishSubmission` |
| `result`, `notice`, `bus approval.requested` | the first `chat.status` reply holding a task result, notice or question no stream carried | `Bus` / `receiveOutcome(announce: false)`, `receiveNotices`, `receiveBusEvent` |
| `bus approval.resolved` | after the `chat.approve`/`chat.answer`/`chat.correct` call, with the facts of the scenario's `question.answered` journal row; a question set aside by a stop or steer at the end | `Bus` / `receiveBusEvent` |
| `notification` | a `notifications.subscribe` banner (a reminder's is posted in the chat) | `Notification` / `receiveReminder` |

A recording keeps each connection's lines in order, not their order across connections; putting a stream's frames
where it ended is a choice, the same one for the engine and the app. `preview_attention` sends no message (an empty
timeline); `login_sites` has two conversations (`s1`, `s2`).

## How it is checked

- `chat_entries.rs`: the golden bytes; one message per ack; nothing live stored; `seq` grows, `pos` fixed, each revision
  `rev + 1` and different from the one before; merged messages name their request; late answers say what they show; and
  the rules on invented frames (APP-580 placement, ORCH-579, ORCH-318, an error, DMN-583, a card's transitions only).
- `ui/mac/FERBNotch/Tests/NotchCoreTests/EntryFoldTests.swift` reads this file from the repository and runs each
  timeline through FERB.app's own reducer, folds the entries with `EntryFold`, and compares the two transcripts on what
  the owner sees (ids, `createdAt` and a notice's `noticedAt` left out): all 71 conversations equal.
