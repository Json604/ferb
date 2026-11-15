# Rust rewrite plan

The forward plan: what is left, in what order, what each part depends on and the traps already found. The state of
each part is in `STATUS.md`; what was done and why is in `JOURNAL.md`; the method is in `AGENTS.md` ("The Rust
rewrite"). Change this file when the plan changes, in the same commit, and say why in the journal.

## Picking this up

1. Read `AGENTS.md`, then `STATUS.md` (top: the crate layout), then the last five entries of `JOURNAL.md`.
2. `. ~/.cargo/env && cd rust && ./gate.sh fmt && ./gate.sh lint && ./gate.sh test && ./gate.sh portable`. If the
   toolchain is missing: `rustup` installs the version pinned in `rust-toolchain.toml`.
3. `git status` and `git log origin/rust..rust`: is anything uncommitted or unpushed (see "Open with the owner")?
4. Take the first part below whose "needs" are verified in `STATUS.md`. Parts with no shared files can run in
   parallel, one agent per crate.
5. For each part: the six steps under "How a part is ported" in `AGENTS.md`. End with the journal entry, the status
   row and the commit.

## Goal and end state

The Rust engine working with the Mac, iPhone and Android apps (PROJ-508). In order (ENG-1):

| Stage | End state | Cut over when |
|---|---|---|
| 1 | FERB.app runs a Rust daemon with today's behavior; no Python in the bundle | Fixture tests pass, the fake-model end-to-end runs pass on the Rust daemon, the owner's live checks pass on an installed build |
| 2 | The engine owns the chat history; every client pages it and gets changes | Mac app shows the same history through the new calls; old archives imported once |
| 3 | The daemon has a phone role; the Android app drives real tasks | The phone lab's tests pass against the real daemon; owner's live check on a phone |
| 4 | The iPhone app does what the Android app does | Owner's live check on an iPhone |
| 5 | Task replies and questions reach a closed phone app | Owner's live check; needs the owner's Apple push key and a host |

## Layout (target)

Portable crates compile for iOS and Android (`./gate.sh portable`); only `ferb-platform-macos`, `ferb-store`, the
binaries and the Unix server module are Mac-side.

| Crate | Holds | Portable |
|---|---|---|
| `ferb-py` | Python-exact helpers: `json.dumps`, times, value text | yes |
| `ferb-core` | Types, ids, errors, config, paths, the event bus, runtime and capability traits | yes |
| `ferb-sanitize` | Redaction, origins, the untrusted wrapper | yes |
| `ferb-rpc` | Wire frames, the role/method table, run tokens; the Unix-socket server (unix only) | wire: yes |
| `ferb-identity` | Secret type, owner secrets, origins, seals; the helper behind a trait | yes |
| `ferb-store` | SQLite store, migrations, typed tables | Mac-side |
| `ferb-policy` (to add) | Confirmations, protection, payments, destinations, provenance | yes |
| `ferb-gateway` (to add) | The one path every tool call takes; questions, ledger, stop, resources | yes |
| `ferb-runtimes` (to add) | Codex, Claude, API adapters; the tool bridge | Mac-side (spawns programs) |
| `ferb-engine` (to add) | Orchestration, time, memory, learning | yes, behind store traits |
| `ferb-hands` (to add) | Files, shell, browser bridge: the logic | mostly |
| `ferb-platform-macos` | Everything that calls Apple frameworks or macOS tools | Mac only |
| `ferbd` | The daemon: composes the above, registers the RPC handlers | Mac only |
| `ferb-native-host` | The browser's relay | Mac only |
| `ferb-link`, `ferb-mobile-ffi` (stage 3) | From the phone lab | yes |

Add a crate only when a part needs it. The engine never names a bundle id, an AX call or a path under
`/Applications`; it asks a trait.

## Stage 1: the remaining parts, in order

Lines are Python lines at `63369cb3`. "Fixtures" names what exists; a part without one writes its own `make.py`.

### 1. Policy (4,666 lines) → `ferb-policy`
- **Python:** `ferb/policy/` (`confirm.py` 1,872, `protection.py` 524, `tls.py`, `payments.py`, `provenance.py`,
  `destinations.py`).
- **Needs:** core, store, sanitize.
- **Traps:** `policy` imports `orchestrator` and `act` in circles; take what it needs through traits. Known versus
  approved destinations are different things (`destinations.py:80` and `:86`). Protection checks the opened file's
  identity, not the path string (`protection.py:170`); the descriptor work goes to the platform crate.
  Confirmations: answers are durable, pending questions are in memory (`ferb/daemon/questions.py:121`); keep that in
  stage 1. GATE-585/586: one terminal Yes per app per task tree. The 30-minute window covers routine questions only
  (`confirm.py:87`).

### 2. Gateway and daemon state (about 6,000 lines) → `ferb-gateway`
- **Python:** `ferb/daemon/gateway.py` (2,887), `questions.py`, `resources.py`, `call_hooks.py`, `hidden_input.py`,
  `pay_picture.py`, `pay_press.py`, `refused_fills.py`, `ferb/orchestrator/ledger.py`, `ferb/daemon/stop.py`,
  `ferb/registry.py`.
- **Needs:** policy.
- **Fixtures:** `fixtures/catalog/tools.json` (97 tools, 23 listings by grant), `validation_errors.json` (627
  cases through the real `Gateway.call`).
- **Traps:** the stage order inside `Gateway.call` is the contract (`gateway.py:1069` through `:2012`): intent is
  recorded before the effect and unknown outcomes after. The tool list varies by identity, scopes, `observe_only`
  and grant, never the schema. Schemas and validation errors are pydantic's text: serve the frozen schema bytes and
  reproduce the error text against the fixture. In-memory authority that dies with the process: run tokens,
  pending approvals, `_undelivered`, `_amount_asked`, `_terminal_arms` (`gateway.py:640,1134,1275`). Stop order is
  block, revoke, hooks (`stop.py:22-34`). Startup marks interrupted ledger rows unknown (`ledger.py:156`,
  `daemon/main.py:470`). Found in Python: three validators leave a `ValueError` in the reply so it cannot be
  serialized (`browser.pdf`, `ferb_browser.pdf`, `audio.listen`).

### 3. Prompt context and structured answers (about 1,100 lines) → `ferb-engine`
- **Python:** `ferb/core/context.py`, `prompts.py`, `structured.py`, `ferb/orchestrator/schemas.py`,
  `ferb/runtimes/schema_shape.py`.
- **Needs:** core, py.
- **Fixtures:** `fixtures/catalog/prompt_static.json`, `answer_schemas.json` (86), `repair_notes.json` (117).
- **Traps:** token counting is o200k with 8,192-character slices and a byte fallback (`core/context.py:24-30,
  77-91`); memory counts 4 characters per token (`memory/profile.py:39`); do not unify them. `jsonschema`'s
  `best_match` text goes back to the model, cut at 200 characters (code points). Default `json.dumps` escapes
  non-ASCII and 0x7f and writes floats as `1e+16`.

### 4. Model runtimes and the tool bridge (4,876 + 300 lines) → `ferb-runtimes`
- **Python:** `ferb/runtimes/` (`codex_stream.py` 859, `codex_exec.py`, `claude_sdk.py`, `api.py`,
  `anthropic_api.py`, `catalog.py`), `ferb/cli/mcp.py`.
- **Needs:** gateway, part 3.
- **Traps:** Codex is `app-server --stdio --strict-config` only (`codex_stream.py:289`), with the refresh-write
  race check (`:231-245`). Claude (ENG-2): start the pinned bundled `claude` program in stream-json mode with the
  flags `claude_sdk.py` sets (`:181-224`), send the SDK's `initialize` control request, steer with a second user
  frame acknowledged by uuid (`:461-472`); the bundled program needs a new pinned source once the Python SDK is
  gone. The tool bridge returns unknown-outcome text on a broken connection (`cli/mcp.py:94-97`). Scripted fake
  providers only; a real provider run is the owner's.

### 5. Files, recovery, shell, proxy (part of 21,723 lines) → `ferb-hands` + platform
- **Python:** `ferb/act/fs/`, `ferb/act/shell/`, `ferb/act/recording.py`, downloads.
- **Needs:** gateway.
- **Traps:** write modes: in place for hardlinks, APFS clone, sibling temp and rename, ACL not copied in the
  fallback (`act/fs/files.py:203-281`). `mac.applescript` is `/usr/bin/osascript` under Seatbelt, not in-process
  (`act/mac/applescript.py:57-60`). Port the sandbox profile text, not the Python around it (`sandbox.py`).
  PID-reuse defense (`pty_proc.py:242-266`). The proxy authorizes before DNS (`proxy.py:277`) and verifies the
  connected local peer (`local.py:146-170`). Private or hand-laid-out calls: `sandbox_check`, libproc socket
  structs, `renameatx_np`, `clonefileat`, `fchflags`. ffmpeg is found on the PATH, not bundled
  (`recording.py:43-51`).

### 6. Browser hands (part of 21,723 lines) → `ferb-hands`
- **Python:** `ferb/act/browser/` (`ext_bridge.py`, `ext_protocol.py`, `pairing.py`, `tools.py`),
  `ferb/daemon/areas/browsers.py`.
- **Needs:** gateway, identity (sealed fills).
- **Traps:** the extension stays JavaScript and unchanged: manifest 0.19.10, protocol 21, minimum 0.19.0. `html`
  and `owner_view` work but are missing from the Python type (`ext_protocol.py:180-188`). HMAC inputs:
  `install_id|seq|type|body_json` and `nonce|seq|type|body` (`:312,337`). Only the extension-to-daemon `last_seq`
  is persisted (`pairing.py:99-104`); the other counter is per connection. The browser is identified as the relay's
  parent pid (`ext_bridge.py:249-260`). At least 27 verbs in two families, `browser.*` and `ferb_browser.*`.

### 7. Mac hands (part of 21,723 lines) → `ferb-platform-macos`
- **Python:** `ferb/act/mac/`, `ferb/act/listen.py`, `ferb/device/permissions.py`, `autorelease.py`,
  `ferb/sanitize/images.py`.
- **Needs:** gateway.
- **Traps:** `objc2` crates for AX, CGEvent, ScreenCaptureKit, Vision, PDFKit, NSWorkspace; raw FFI for
  `SecCode*`, `AEDeterminePermissionToAutomateTarget`, IOHID, SkyLight (optional: missing symbols mean the route is
  unavailable, `window_input.py:29-42`), libproc. AXObserver, event taps and FSEvents need their own run-loop
  thread. Completion blocks must outlive a waiter that gave up (`listen.py:85-129`). Every worker thread drains its
  own autorelease pool. HAND-518: light AX walks alongside the shot. Speech runs in FERB.app, nothing to port.
  Permissions attach to the nested agent app: the Rust binary must be that bundle's executable (part 12). No
  per-app background tricks; foreground input asks first.

### 8. Orchestration (15,817 lines) → `ferb-engine`
- **Python:** `ferb/orchestrator/` (`supervisor.py` is 7,681 lines in one class), `ferb/decide/`.
- **Needs:** parts 2, 3, 4; hands only to run real tools.
- **Fixtures:** `prompt_static.json` covers `_system`; the list of all 238 `json.dumps` sites is in it. Write
  golden prompt fixtures per role before porting anything else here.
- **Traps:** the highest drift risk. Split by responsibility, not by line order: prompt composition, intake and
  routing (merge, reply-only, clarify, control plus new work), plan store and repair, worker loop, checker, pause /
  steer / stop / suspend / resume, continuity. The planner sees the last 20 exchanges whole plus a summary, live
  facts and front-window pictures per message (ORCH-637). A `reply` event ends the turn in the app; nothing later
  in that turn may be needed (ORCH-638). The checker gets no learned memory (`supervisor.py:6852`). Regexes with
  lookaround and backreferences: `schedules.py:408-409`, `loops.py:152`, `answer_marks.py:118`,
  `continuity.py:299`. Port the fake-model end-to-end tests here (TEST-500): they are the stage's real proof.

### 9. Time (1,388 lines) → `ferb-engine`
- **Python:** `ferb/clock/`, `ferb/orchestrator/schedules.py`, `ferb/daemon/areas/schedules.py`.
- **Traps:** waiting is code, no model calls (ORCH-631..633, 639). Injected clocks only. Found in Python: after the
  next timer is booked (`areas/schedules.py:659-661`) and before the task exists (`:687`), a crash loses that run.

### 10. Memory, learning, import (6,096 lines) → `ferb-engine`
- **Python:** `ferb/memory/`.
- **Traps:** embeddings are standard-library code: own WordPiece, potion-base-8M read by mmap, int8 quantize,
  Hamming prefilter then cosine (`embeddings.py`); `round()` is half-to-even, rows accumulate as f64, slices are by
  code point. Five import parsers (`import_sources.py`), zstd for Codex rollouts. Import writes are not atomic
  (`import_job.py:186-190`); mirror, then fix both. Compare FTS5 ranking on the bundled SQLite (3.46) with
  Python's (3.53.4) here.

### 11. Perception and traces (3,817 + 759 lines)
- **Python:** `ferb/sense/` (watchers to the platform crate, the trail and receipts to the engine), `ferb/trace/`.
- **Traps:** secure fields are suppressed at the source (`sanitize/redact.py:244-248`,
  `sense/watchers/text_input.py:56`). Notices are paused; Jev stays off and makes no startup call
  (`daemon/areas/jev.py:24-46`). Do not switch on anything dormant.

### 12. The daemon, its handlers and the host (about 12,000 lines) → `ferbd`
- **Python:** `ferb/daemon/main.py`, `ferb/daemon/areas/*.py` (106 methods), `ferb/cli/` (the internal commands
  FERB.app calls: `setup --service [--update] [--app-service]`, `setup --cli`, `uninstall [--purge --yes]`, login
  and logout), `ferb/device/install.py`, `ui/mac/FERBAgent/main.c`.
- **Needs:** everything above.
- **Fixtures:** `fixtures/rpc` (frames); record one transcript per method from the Python daemon before porting it.
- **Traps:** the host is C embedding Python with four entry modes (`main.c:41-44`), a home device/inode check
  (`:50-55`) and a rebuilt environment (`:78-99`). Make the Rust daemon the executable of the same nested
  `com.ferb.daemon` app so permissions carry over; that can only be proven on an installed build. The daemon
  re-enters its host through `sys.executable` (`daemon/main.py:276,297`); the Rust daemon re-enters itself. The app
  reads `~/.ferb/service.json` and `~/.ferb/logs` and hardcodes the socket path (`DaemonRPC.swift:20`). The
  FERB.app-only check must pin the signers (T-9, DMN-588). `chat.py:161,375,423` (`_attached_trees`) and the
  `shown` flag (`chat.py:377`, `areas/notify.py:41-45`) are single-client state: keep them in stage 1, replace in
  stage 2. Events: `ack`, `plan`, `plan_delta`, `reply`, `step`, `outcome`, `merged`, each with `request_id` and
  `at` (APP-578). Old daemons exit with status 0 on a newer store and launchd does not restart them (STORE-3).

### 13. Packaging and cutover
- `scripts/build-app.sh` and `app-parts.sh` bundle Python today; `build-app.sh:67` and `release-appcast.sh:61-62`
  read constants out of Python sources with `sed`. Replace with the Rust binaries; keep signing, notarization and
  the Developer ID transition (REL-419..423, APP-902..905, DMN-587).
- The Rust release is a fresh install (STORE-421, owner): no in-place upgrade from the Python build, its data kept
  as an archive. FERB.app may change with the release (for example how it recognizes a checkout's service).
- Before cutover: a signed build's FERB.app check, the real secrets helper, a real browser through the Rust relay.
- Cutover is the owner's install: quit FERB.app, no running tasks (in-memory authority does not transfer), install,
  live checks (`kb/live.toml`). Python is deleted only after that, in its own commit, with the gate's Python
  checks retired by a recorded gate change.
- Prepared for the removal (journal 76, ENG-16): `ferb trace`, `stats` and `browsers extension` in the Rust program;
  the release scripts off pyproject and build-app.sh (sign-app.sh, PEP 723 locks, rust-only appcast), their tests in
  `scripts/tests`; the kb tool's `historical` paths (PROJ-509); the version's home (REL-425); fixture scripts kept
  as history (ENG-17, `rust/fixtures/README.md`).
- Done (journal 80): the Python code, its tests and the Python-only scripts were removed; the gate's Python checks
  retired; `kb/config.toml` reads removed paths at `9d3f84bc`. Left: T-1100..T-1110 (98 decisions whose only tests
  were Python), T-1112 (`ferb runtimes`, `memory dashboard`), T-1113 (leftovers outside the engine's behaviour).

## Stage 2: history owned by the engine

- Today: opaque rows written by FERB.app, last writer wins, paging inside one local day
  (`store/chat_history.py:82-129`), no client idempotency key (`areas/chat.py:348`), a global `shown` flag.
- Build: accepted submissions with a client submission id (repeat returns the same acceptance), entries with
  revisions, a conversation-wide cursor, changes after a cursor, per-client delivery state; the journal's
  `chat.message` and `chat.reply` rows are a candidate source to weigh first (`chat.py:392`).
- This is the first schema above 0062. Record the owner's words first: "shared convo. on scroll/ondeamn dchat
  fetching like from a server" (said to another agent; ask the owner to confirm before recording it as theirs). It
  supersedes APP-805 and must be squared with STORE-401 (history is kept).
- The Mac app moves to the new calls here; old archives are imported once and their bytes kept.

- Slices 1 and 2 are done (ENG-8, ENG-9, journal 69): the entry contract (`ferb-core::chat_entry`), the pure
  projector (`ferb-supervisor/src/flow/entries.rs`) and FERB.app's fold (`NotchCore/EntryFold.swift`), 71 of 71
  recorded conversations folding to the app's own transcript. Nothing is wired; no schema, no RPC.
- Slices 3 to 5 are built (ENG-12, ENG-13, ENG-14, journal 72): migration 0063 (`chat_entries`, `chat_streams`,
  `chat_submissions`, `chat_clients`, `chat_client_marks`, `chat_entries_from`; chat_history's rows carried over as
  legacy entries), ferb-store `chat_entries` (pages, changes, cursors, submissions, marks, tombstones, the archive
  import), the projector wired in `flow/chat_log.rs` (one queue in frame order; the ack in one transaction with its
  journal row; the start-up pass and the journal backstop), and the calls `chat.entries.page|changes|get|days|watch|mark`
  with `chat.send`'s `client_id`/`submission_id`. ferbd's `chat_replay` compares the stored entries with the golden
  after every scenario.
- Journal 73: the order check (ENG-15: seven rules on the wired log's trace and on invented interleavings; the
  engine's real interleaving kept in `fixtures/chat-entries/engine_*.json` and FERB.app's reducer driven in it equal to
  the stored entries on all 61 replayed scenarios) and FERB.app's one change (APP-916..918: it watches
  `chat.entries.watch` and shows turns another client sent; every entry of a turn names its client; `chat.attach` adds
  `entries`). Not live-checked: a second client against the real daemon is the owner's check.
- Remaining in stage 2: (1) the owner's live check of FERB.app showing another client's turn; (2) running the archive import once
  with the owner watching (`ferb_store::chat_entries::import_archive(db, archive, conversation)` with the daemon
  stopped, against the owner's archive folder; it refuses an archive whose `-wal` is not empty: checkpoint a copy first);
  no internal command calls it yet. Stage 3 then classifies the six calls for the phone role (reads Private, `mark`
  Plain) and decides what "seen everywhere for notifications" does to banners (today the shown flags stay global).
- Known gaps: FERB.app shows another client's turns only from the moment it first watches (no cursor: no import of
  older ones), and a result filed before its message's entry is joined to it only while the app runs; a watcher is not
  told live of a notice cleared by a forget (it reads it in `changes`); a revision made outside the log (`clear_notice`, a tombstone) leaves a loaded projector's copy of that entry stale (notices are never
  revised by it); the golden's timelines for 28 scenarios differ from the engine's real order (named in
  `chat_replay.rs` `ENTRY_DIFFERENCES`; the real order is now checked by ENG-15's rules and `engine_*.json`).
- Owner, 2026-11-15 (ENG-10, ENG-11): one conversation kept on the Mac; PHONES fetch it like from a server and store
  what they fetched; FERB.app's behaviour and UI for earlier messages stay as they are ("no only phoens ... current
  mac behavious adn ui for prev and od messages im ok with"); chat kept as now; the pre-Rust archive imported once,
  read-only; a result seen on one device is seen for notifications everywhere; stage 1 is cut over. So the Mac app is
  NOT moved onto `chat.entries.page`/`watch` for its history (the design's Mac steps 2 and 3 are dropped); what the
  Mac must gain is showing a turn that a phone started (owner: "ofc this, it should be shared"), with nothing
  visible changed: a contained change inside the app (e.g. the watch feeding its existing reducer for turns it did
  not send).
- App bugs found by the fold, not fixed: frames after a turn's first answer are dropped (`Conversation.swift:72-90`,
  `NotchSession.swift:226`); a one-ask outcome's `unconfirmed` is read as `note` (`ConversationArchive.swift:806`).

## Stage 3: the phone role and Android

- Owner, 2026-11-15: "i also made UI for hte phone apps that i want exactly same in both ios and android design wise,
  its at: ferb-glass-lab in desktop." The phone apps' look is `~/Desktop/ferb-glass-lab` (read-only: `DESIGN.md`,
  `tokens.json`, `Sources/FerbGlass` in SwiftUI, the `phone` scene; `./run.sh --scene phone`), the same on iPhone and
  Android. Record it as an owner decision in kb when stage 3 starts; the Android app reproduces the tokens and
  screens, it does not get its own design.
- Owner, 2026-11-15 (ENG-10): a phone stores what it fetched from the Mac locally.
- Owner, 2026-11-15 (T-1099): the phone apps have the same Claude / Codex / API-key sign-ins as the Mac, so they can
  use the models when the laptop is off ("laptop wont be on alwys"): "not by itself. just a bitton ilke in mac to
  login on phones too" — a sign-in button the owner presses, as in the Mac app's Settings; nothing signs in on its own. This makes the phone more than a remote for the
  Mac's engine: the portable crates already compile for iOS and Android (ENG-5), so the engine can run on the phone
  with its own runtime sign-in. To settle with the owner before building: what a phone does alone (chat and research
  with no Mac hands? its own history, merged into the Mac's when it is back?), where its secrets live (the phone's
  keystore, never synced: rule 7), and which runtimes can sign in on a phone at all (Codex and Claude sign-ins are
  desktop CLI flows today; an API key works anywhere). Live test on the owner's phones.

- Source: `~/Desktop/ferb-phone-link` at `57b3f34` (read only): `rust/src` (transport on iroh 1.4.1 with a small
  pinned patch, `role.rs` classifying all 106 methods with five never allowed: `about.guide`,
  `chat.history.import`, `chat.history.put`, `intro.preview`, `notifications.subscribe`), `android/` (Kotlin),
  `standin/` (a stand-in daemon: its answers are fake), `PORTING.md` (open questions Q1 to Q4), `kb/` (PHN-*,
  SEC-*, NET-* decisions, not yet in this repo's kb; T-1051 is the task to bring them in).
- Build: a `phone` role in the daemon with a mandatory test that every method is classified; pairing, revocation
  and the three proof keys verified by the daemon itself, not the link helper; file browse, fetch and upload through
  the same protection as model tools; the Android app moved into `ui/android` with its local chat replaced by the
  stage 2 calls.
- The phone library today has no close call, no `catch_unwind` and one global client per process
  (`rust/src/ffi.rs`); fix before the iPhone app depends on it.
- Android release needs: a non-debug build, the real app id (the lab uses `local.ferb.link`), the owner's Play
  account.

## Stage 4: the iPhone app

- Nothing exists. Swift app in `ui/iphone` on the same Rust client library, built for `aarch64-apple-ios`; Keychain
  and Secure Enclave keys converted to the wire form Android uses (P-256, SubjectPublicKeyInfo DER, SHA-256 ECDSA
  over the original bytes); transfers checkpoint before iOS suspends the app.
- Needs from the owner: bundle id, provisioning, a physical iPhone for the live check.

## Stage 5: closed-app notifications

- The owner's words to another agent: "Include background notifications in the foundation" and "Follow the phone's
  notification privacy settings"; confirm and record them (they reverse the phone lab's PHN-6, "No hosted push
  sender is built").
- Simplest form first: a push that says FERB needs you, carrying no words (the lab's own design, PORTING D6); the
  app fetches the text from the Mac when opened. Previews in the notification are a later step. Android needs FCM
  for the same case.
- Needs from the owner: an Apple push key, where the small sender runs, how an install enrols.

## Second last: an independent full-device security audit (T-1098)

After stage 5 and before the knowledge transfer. The owner, 2026-11-14: a fresh Fable agent, independent of the
agents that built the engine, treats this repo as its own and runs live security testing "at the extreme" with the
access the owner grants when it starts (the owner's Mac, their Motorola phone, and whatever else it needs). Its
starting material: the red-team review of the link rule and its eleven holes that exist today (journal 62), the
documented `today_…_is_not_asked` tests, the data-exit audit (shell egress recorded not blocked; uploads; AppleScript),
and rule 16 (real third parties only in a run the owner approves). Findings are fixed or recorded as decisions before
the rewrite is called done.

## Last: knowledge transfer to the owner (T-1097)

After stage 5. The owner, 2026-11-14: a system way to navigate the code, every process at the macro level with
interactive zoom down to its micro working, so the owner is sure what is built where, how it works, and its edge
cases. Generated from the code and kb so it cannot drift (processes: app, daemon, gateway, workers, browser bridge,
store, phone link; each opening into its code path with file:line, decisions, tests, known limits). Propose the form
to the owner before building it.

## Open with the owner

- **Pushing the branch.** GitHub's push protection refused `rust/fixtures/sanitize` (invented key-shaped test
  values; journal 8). On 2026-11-13 the owner switched off "Push protection for yourself" in their GitHub account
  and said it stays off until the rewrite is done. `rust` is pushed. **Remind the owner to switch it back on when
  the rewrite is finished.** While it is off nothing on GitHub's side stops a real secret in a push to any of the
  owner's public repos: the repo's own commit gate (`secrets`) is the only check, so never bypass it. Never
  re-encode fixtures to pass a scanner.
- The owner's words from the conversation with the other agent are not in the kb yet (stages 2 and 5 above).
- T-27's open question (a bridge release before the Developer ID release) is unchanged by the rewrite.

## Bugs found in Python

Fixed in Rust when the part is ported (AGENTS.md); the test names the deliberate difference. The redaction leak is
also worth fixing in the Python build on `main`, since that build is installed today; the rest end at cutover.

| Where | What | State (journal entry) |
|---|---|---|
| `ferb/sanitize/redact.py:395` | A key body under a non-text key between BEGIN and END items is left | fixed in Rust (16); Python patch ready |
| `ferb/native_host/main.py:53-57` | A parse error leaves the relay alive and deaf | fixed in Rust (6) |
| `ferb/native_host/main.py:61` | The rewrite inflates a 1 MiB non-ASCII frame past the daemon's limit | fixed in Rust (6) |
| `ferb/store/db.py:212` | `migrate` rolls back unconditionally and can hide the real error | fixed in Rust (3) |
| `ferb/daemon/areas/schedules.py:659-687` | A crash between booking the next run and creating the task loses a run | fixed in Rust (19) |
| `ferb/clock/timers.py` | A failure in `next_due_in` ends the timer loop; `Timers.add` is not atomic | fixed in Rust (19) |
| `ferb/act/fs/changes.py` `named()` | Output order changes between runs (set order) | fixed in Rust (18) |
| `ferb/memory/import_job.py:186-190` | Import writes are not atomic: a restart lost or doubled work in 64 of 116 points | fixed in Rust (32) |
| gateway validators (`browser.pdf`, `ferb_browser.pdf`, `audio.listen`) | A reply that cannot be serialized | fixed in Rust (34) |
| `ferb/store/retention.py` `_with_descendants` | Never ends on a span parent cycle | fixed in Rust (16) |
| `ferb/identity/helper.py` | The helper is verified, then run by path (a swap window) | fixed in Rust (16) |
| `ferb/memory/search.py:364-409` | Vectors stored for facts deleted mid-batch are never removed | fixed in Rust (22) |
| `ferb/memory/import_sources.py:452,485` | Gemini HTML parsers never closed: trailing text with `&` dropped | fixed in Rust (22) |
| `ferb/memory/import_sources.py:87-97` | `window_start` loops forever early in year 1 | fixed in Rust (22) |
| `ferb/act/shell/sockets.py` | A nettop port with a non-decimal digit raises out of `complete()` | fixed in Rust (20) |
| `ferb/act/browser/ext_bridge.py` | A closed connection fails the profile's commands on its newer connection too | fixed in Rust (21) |
| `ferb/act/fs/files.py:51` | A file's `mtime_ns` is redacted as a card number about one time in ten | fixed in Rust (34) |
| about 20 RPC handlers | Bad parameters answer an internal error instead of a refusal | fixed in Rust for 71 methods (40); the chat and task handlers remain |
| global stop | Tags FERB's own words "(from the task)" | fixed in Rust (43) |
| `ferb/act/shell/proxy.py` | A bare-LF head is forwarded with `Connection: close` after the body bytes | open: mirrored in `ferb-shell` (20) |
| forget question in chat | FERB's own heading tagged "(from the task)" | fixed in Rust (50) |
| `ferb/act/mac/tools.py:348` | A launched app looked for once, too early, goes unrecorded | fixed in Rust (47) |
