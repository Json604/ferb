# ferb-mobile

The phone's core, shared by the Android and iPhone apps: the chat fold, the live state, the encrypted store, sync
over `chat.entries.*`, and the outbox. Stage 3 slice S7 (kb T-1207); `engine/JOURNAL.md` entry 107 says what was
done and why. No UI and no FFI: slice S8 (`ferb-mobile-ffi`, UniFFI) wraps it; S9 builds it into `apps/android`.

The data lives on the Mac (ENG-10): the phone reads the current conversation like from a server and keeps what it
fetched until "Forget this Mac" (PHN-408). FERB.app's own fold is the golden for what the phone shows.

## Modules

| Module | What |
|---|---|
| `fold` | Stored chat entries → the transcript FERB.app shows: a port of `EntryFold.archive` and the archive's transcript (`apps/mac/FERBNotch/Sources/NotchCore`). Items are `turn`, `reply`, `notice`, `reminder`, plus each question and where the Mac draws it, in the shape of `engine/fixtures/chat-entries/transcripts.json` (README there). |
| `entries` | What was fetched of one conversation: each entry's newest copy (`rev`, then `seq`), whatever order pages, changes and frames came in; a cut body (`body_cut`) is never kept. |
| `live` | The watch's `live` frames (ENG-8: never an entry): typing dots, the streamed plan and reply, the live line, step task lines, step states, the activity line; per turn, gone at its answer and on a restart of the watch. |
| `store` | One SQLCipher database: entries by conversation, cursors, older-page state, entries to read whole, the outbox. Its own schema versions (`user_version`, `MIGRATIONS`). |
| `transport` | How the core reaches the Mac: the `Transport` trait (JSON as text), the link's codes and what each means. |
| `door` | `DoorTransport`: the real link through ferb-link's JSON door (`LinkClient::command`/`poll`), without depending on ferb-link. |
| `api` | `Mobile`, the object the apps use. |
| `notify` | What a phone notification says, in FERB.app's words: a task's end as the Mac's banner (its short name, the answer as the chat draws it), a question as the Mac's banner (kb PHN-795, PHN-796). |
| `reply` | Reply to a message (APP-1146, FERB.app's `ReplyQuote.swift`): `ReplyQuote {entry_id, kind, by_owner, excerpt}`. The fold gives each bubble drawn from an engine entry its quote (`TurnItem` `message_quote`, `first_reply_quote`, `answer_quote`; `ReplyItem`/`NoticeItem` `quote`) and the owner's message what it quotes (`TurnItem.reply_to`, from his entry); none for FERB.app's carried rows or in the temporary chat; `fold::quoted_item` finds the item showing an entry. None of it is in the golden's bytes. |
| `device` | The phone's own record for FERB and the Mac's request for one precise fix (T-1214); recent files and the name search (T-1211). |

## The API (for S8)

Plain Rust, UniFFI-friendly: owned data, no generics or lifetimes in public types, callbacks as traits behind `Arc`,
one error type with stable codes, objects with an explicit close, time and ids injected.

```rust
let m = Mobile::open(path, key /* 32 bytes, zeroized */, Config::new(client_id), Env { clock, ids, spawner, listener })?;
m.connect(Arc::new(transport));          // the link, as a Transport (DoorTransport over LinkClient)
m.link_event(json);                      // forward the link's `state`, `bulk`, `relay_state` events
m.sync()?;                               // also started by itself when the link comes online
m.conversations()?; m.current_conversation()?;
m.transcript(conversation)?              // TranscriptView { items, questions, live, outbox, older }
m.send(text)?;                           // -> OutboxItem: in the outbox at once, sent now when online
m.cancel(outbox_id)?;                    // -> Cancelled { may_have_arrived }
m.answer(approval_id, Answer::…, signer)?;   // never queued
m.stop(&task_ids)?;
m.load_older(conversation)?;             // -> OlderState
m.mark_read(conversation, pos)?;
m.link_state();                          // LinkState { path: Offline|Direct|OwnRelay|PublicRelay, bulk_allowed, relay_note, watching }
m.forget_this_mac()?;                    // deletes the store's files; every call then answers `closed`
m.close();
// T-1221 (journal 116; `remote`, `status`)
m.lock()?; m.unlocked();                 // the app's lock: link.lock and the watch closed until the unlock (PHN-510)
m.stop_everything()?;                    // stop.engage, works locked
m.status()?; m.last_status()?;           // chat.status as the gate shapes it; also pushed by the bridge (`status` event)
m.mark_read_latest(conversation)?;       // the read mark at the newest entry held
m.send_with(text, &attachments)?;        // a message with this phone's uploads, by id, through the outbox
m.send_reply(text, &attachments, Some(&quote))?;  // APP-1146: quoting a bubble (an item's quote), as `reply_to`
m.files(path, signer)?; m.tree(path, signer)?;                     // a key place signs its path (PHN-121)
m.fetch(id, path, save_to, offset, signer, sink)?; m.upload(id, local, name, resume, group, rel, sink)?;
m.put_group(name)?; m.cancel_transfer(id)?;                        // a cancel is `Cancelled` to the waiting call
m.preview(task_id, have)?;               // one live-preview answer, the frame in memory only (PHN-454)
m.settings()?; m.set_setting(key, value_json, signer)?;            // settings.set signed (PHN-404)
m.tasks(limit, before)?; m.mac_info()?; m.cached_mac_info()?; m.register_keys(device, strong, signer)?;
// T-1214, T-1211 (journal 117; `device`)
m.report_device(report)?;               // phone.device_report when it changed; again at each connection's sync
m.location_wanted()?; m.answer_location(request_id, fix, why_not)?;  // the Mac's one request for a precise fix
m.recent_files(limit)?; m.search_files(query, path)?; m.fetch_task_file(id, task_id, index, save_to, sink)?;
```

Files, folders and the preview are refused before anything is asked when the link's path carries no bulk
(`NeedsBetterPath`, PHN-444). The `Transport` trait has `get`, `put` and `cancel` with defaults that refuse
(`unsupported`), so a transport that carries calls only still works; `door` routes their `progress`, `accepted` and
outcome. The store is at schema v4 (an outbox row's `attachments`, v3; the message it quotes, `reply_to`, v4).

The app's seams (`Env`): `Clock::now_iso`, `Ids::new_id` (a message's submission id: a random UUID), `Spawner::spawn`
(`ThreadSpawner` spawns a thread), `Listener::changed(Change)` (`Transcript`, `Live`, `Conversations`, `Outbox`,
`Link`, `Older`, `Closed`: read again what changed). `Signer::sign(payload) -> Option<sig>` signs one exact text with
the phone's lock-bound key after its fingerprint prompt; the core never holds a key.

Errors: `MobileError { code: ErrorCode, message, link_code }`; `ErrorCode::as_str` gives the stable code: `closed`,
`bad_key`, `wrong_key`, `store_newer`, `store`, `offline`, `needs_better_path`, `needs_unlock`, `refused`,
`needs_signer`, `not_signed`, `not_found`, `in_flight`, `bad_reply`. `link_code` is the link's own (ferb-link
`errors.rs`).

For S8: `TranscriptView` holds the fold's types; `QuestionItem.body` and a turn's cards are `serde_json` values or
structs with optional fields, which the FFI layer passes as JSON text or maps to records. `Item` serializes to the
golden's JSON exactly (`serde_json::to_value`), which is one way to hand it over.

## Threading model

`Mobile` is `Send + Sync`; every method is blocking and bounded: store work, plus at most the transport's own call
deadline per network call (the link answers `mac_unreachable` after its 15 s; `DoorTransport` adds a 60 s backstop).
State sits behind one mutex (`api::StateLock`) that is never held across a call to the Mac, a callback to the app, or
the end of a transport or a watch: the thread that holds it does store and memory work only. The rule is checked, not
only said (kb PHN-681): every call to the Mac and every change told asserts that this thread holds no state guard
(debug and test builds, so every test of the core holds every path to it), and taking the lock twice on one thread is
refused in every build. It matters twice over: every reader of the core queues behind that lock, and the link's one
pump thread (`door`) needs it to hand over a watch frame or a link event, so a call awaited under it can never be
answered. That happened: `sync` read the cursor inside an `if let` whose guard lived through the body's
`chat.entries.mark` call, the watch's first frame stopped the pump, and each sync ended only at the door's 60 s
backstop with the state locked throughout (tests/no_wait_under_lock.rs). The last status has its own small lock, held
only to copy it. Two more locks serialize work: one sync at a time, one outbox flush at a time (so messages go in order
and once). The watch's frames arrive on the transport's thread and are applied there under the state lock. Work the
core starts by itself (a sync when the link comes online, a resync after `behind`, reading a cut entry, an older page
offered again when the path improves) goes to the `Spawner`. Call the API from a background thread, never the UI
thread.

## Behaviour

- **Sync** (ENG-14): a one-entry page names the current conversation and its epoch; a conversation new to the phone
  starts from its newest page (`page_limit`, 50); with a cursor, `changes` after it page by page; `reset_required` or
  `cursor_ahead` drop what was held and read the newest page again; entries whose words a reply cut are recorded
  (`pending_gets`) before the cursor moves and read whole by `chat.entries.get` in pieces; then the watch from the
  cursor; then the outbox. `behind` resyncs once; a watch that fails waits for the link or the app's sync (no loop).
  Every read passes `max_bytes` (the link's control frame less 1 KiB).
- **Older pages**: `load_older` asks for the page before the lowest `pos` held. When the link says its path does not
  carry bulk (the public relay, PHN-444, PHN-461) it is refused before anything is asked: `OlderState::NeedsBetterPath`
  with the words "Older messages are available when connected directly or through your relay.", fetched by itself
  once when the link's `bulk {allowed: true}` arrives.
- **Outbox** (PHN-401): every message is written to the outbox first (visible, cancellable), with a submission id;
  the flush sends waiting messages in order with `client_id` and `submission_id` (ENG-14), so a retry after an unknown
  outcome gets the Mac's same acceptance and no second message. A failure the link can't carry stops the flush and
  waits for the link to return (no timer, no retry count); `locked`/`unlock_required` waits for the phone's unlock; a
  refusal fails the message with the Mac's words, never sent again. A row left `sending` by a killed app waits again
  at open. A row is removed at its ack (its entry shows it in the chat). The outbox holds the message's words until
  then, encrypted in the store.
- **Answers** are never queued (PHN-448; CONTRACT "The phone never queues an answer"). With a `Signer`, the core takes
  the question's own challenge (`phone.challenge`) or a fresh `link.challenge`, signs the exact
  `{approval_id, challenge, method, params}` (a hidden input: `{approval_id, challenge, method: chat.answer, sealed:
  base64url SHA-256}`), and sends `proof {payload, sig}`. A hidden value is zeroized when the `Answer` drops; it never
  reaches the store, a listener or the signed text.
- **Live state**: per turn while it is working or asking; cleared at the turn's answer, by a merged or interrupted
  message, on a deletion, and wholly when the link goes offline or the watch restarts.

## The store

SQLCipher through rusqlite 0.42 (the workspace's version: one `libsqlite3-sys` per build). On Apple targets
`bundled-sqlcipher` uses CommonCrypto (no OpenSSL); elsewhere `bundled-sqlcipher-vendored-openssl` builds OpenSSL
3.6 from source, so neither phone needs a system OpenSSL. The key is the app's random 32 bytes (wrapped by a
non-exportable Keystore or Keychain key, S9), given to SQLCipher as a raw key (no key derivation); this crate
zeroizes its copies and never writes the key. `secure_delete` is on, so a removed outbox row's words are overwritten.

Size cost (2026-11-16, a one-function cdylib, release, `opt-level = "z"`, LTO, stripped): Android arm64 1.36 MB with
plain SQLite → 5.33 MB with SQLCipher and vendored OpenSSL (+3.98 MB); iOS arm64 1.18 MB → 1.24 MB with SQLCipher on
CommonCrypto (+67 KB).

If this crate joins the workspace, cargo unifies `libsqlite3-sys`'s features across every crate built together: a
`cargo test --workspace` would then build ferb-store's SQLite as SQLCipher (with no key it reads and writes plain
SQLite files) and vendored OpenSSL for the Mac. Keep it excluded, or give the phone crates their own workspace.

## Build

```sh
. ~/.cargo/env; export CARGO_TARGET_DIR=engine/target/agents/mobile
cargo test --manifest-path engine/crates/ferb-mobile/Cargo.toml
cargo check --manifest-path engine/crates/ferb-mobile/Cargo.toml --locked --target aarch64-apple-ios
# Android: the NDK's clang, ar and ranlib (OpenSSL's build calls ranlib by the target's name otherwise)
TC=$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/darwin-x86_64/bin \
CC_aarch64_linux_android=$TC/aarch64-linux-android30-clang AR_aarch64_linux_android=$TC/llvm-ar \
RANLIB_aarch64_linux_android=$TC/llvm-ranlib \
cargo check --manifest-path engine/crates/ferb-mobile/Cargo.toml --locked --target aarch64-linux-android
```

An iOS app linking the library sets `IPHONEOS_DEPLOYMENT_TARGET` (the C code is built for the SDK's own version
otherwise, and the link fails on `___chkstk_darwin`).

## Tests

- `golden.rs`: every scenario of `scenarios.json`, `engine_chat.json` and `engine_more.json` folded and compared with
  `transcripts.json` as JSON values: 146 of 146 conversations equal, exactly (the golden writes a path as the entry
  stored it since S8, journal 112).
- `incremental.rs`: every scenario reached entry by entry (shuffled, in pages newest first, with duplicates, older
  revisions late, tombstones last; a seeded generator) equals the whole list's fold; a late tombstone; a cut entry.
- `live.rs`: every recorded timeline's live frames through the reducer; EntryWatchTests' phone turn; reset.
- `reply.rs`: each bubble's quote and the owner's kept one, a reply sent as `reply_to`, a queued reply across a
  reopen, the v3 store upgraded, none in the temporary chat (APP-1146).
- `sync.rs`, `outbox.rs`: the core against a fake Mac built from the fixtures (`tests/common/mac.rs`).
- `store.rs`: encryption, the wrong key, the v1 upgrade, a newer schema refused, forget.
- `door.rs`: the JSON door adapter.
