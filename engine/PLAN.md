# Rust rewrite plan

The forward plan: what is left, in what order, what each part depends on and the traps already found. The state of
each part is in `STATUS.md`; what was done and why is in `JOURNAL.md`; the method is in `AGENTS.md` ("The Rust
rewrite"). Change this file when the plan changes, in the same commit, and say why in the journal.

## Picking this up

1. Read `AGENTS.md`, then `STATUS.md` (top: the crate layout), then the last five entries of `JOURNAL.md`.
2. `. ~/.cargo/env && cd engine && ./gate.sh fmt && ./gate.sh lint && ./gate.sh test && ./gate.sh portable`. If the
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
  as history (ENG-17, `engine/fixtures/README.md`).
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

Settled on 2026-11-16 (slice S0, journal 96). The kb area is `phone` (prefix PHN, `kb/decisions/phone.toml`; area
paths `apps/android` and `apps/ios`; the engine crates below sit under the `engine` area's paths). Read it with
`tools/kb/run kb area phone`.

**Sources (read only).** The link lab `~/Desktop/ferb-phone-link` at `57b3f34`: `rust/src` (iroh 1.4.1 with one pinned
patch; `role.rs` classifying the then 106 methods, five never allowed), `android/` (Kotlin), `standin/` (a stand-in
daemon: fake answers), `CONTRACT.md`, `PORTING.md` (open questions Q1 to Q4), `SECURITY.md`, `relay/`. The design lab
`~/Desktop/ferb-glass-lab` (`DESIGN.md`, `tokens.json`, `Sources/FerbGlass`, the `phone` scene, `./run.sh --scene
phone`). The lab's kb is imported: its PHN-n is PHN-n here, SEC-n is PHN-(100+n), NET-n PHN-(200+n), LIM-n
PHN-(300+n); its CONTRACT and PORTING docs cite the lab's ids. Every choice from the labs is reassessed before it
is ported, with its reason and benefit (owner, PHN-411).

**Decided by the owner** (verbatim words in the records):
- PHN-401..409: the first plan's nine recommendations ("yes to all"): Mac off → stored history and a visible,
  cancellable outbox; a phone engine after the iPhone app, API key first; the stored-data viewer, deleting turns and
  "let the AI see" (viewing unlocked, deleting and unmasking with the fingerprint); payments and terminal typing
  approved from the phone with the fingerprint, the payment page's picture shown; preview, saved-secrets list and
  settings from the phone; the lab's decisions carry over and may be countered (PHN-405); a release-signed APK
  sideloaded for live tests, Play later; public relays plus his own; fetched chat kept until "Forget this Mac"; the
  sky at the phone's location, Android's system font.
- PHN-410: what goes over the public (iroh/n0) relay is limited: free, no uptime guarantee, a rate limit, an
  ingress limit.
- PHN-418 (superseding PHN-414 and the lab's NET-1 = PHN-201): no own relay: direct when available, else the public
  relay with a message that an own relay performs better, pointing to FERB on the Mac for how; own relay set: always
  that relay, never the public one, direct preferred whenever it exists, and the link keeps looking for direct.
  The rule is the coordinator's reading of his words.
- PHN-415, PHN-417: questions appear in notifications; routine ones (GATE-207's class) are answered from the
  notification's buttons with no fingerprint; money, deletion, account security, terminal typing and screen use are
  shown but the Yes needs the fingerprint; lock-screen text follows the phone's setting.
- PHN-419..422 ("yes i agree" to the remaining recommendations): at most 5 phones; an unused phone warned at 30
  days, never removed by itself; a second phone removes a lost one with its fingerprint; a scanner without Google
  services; whole-app screenshot blocking with a switch; weather paused, later Open-Meteo from the phone; a coarse
  location asked once for the sun, else the time zone; Stay connected stays and stage 5 adds a push the owner runs,
  with no words; the lock screen never shows the words of money, deletion or account-security questions.
- PHN-412 weather paused; PHN-413 FERB controlling the phone later, not now; PHN-416 the glass lab is the design,
  the same on both phones.
- PHN-423 sign-ins: no always-on machine now; phones get a ChatGPT sign-in (OpenAI's Sign in with ChatGPT for
  open-source apps, against the Responses API, no Codex program) so users can talk with the laptop closed, API keys
  as the certain route; no Claude plan sign-in on phones (Anthropic allows a plan only through its own Claude Code),
  and the app says why (PHN-452's words). Refines T-1099; T-1212 builds it.
- SENSE-420 (for T-1130): device location city-level always, exact when a task needs it, asked then; location is a
  ninth permission in setup, and the phone asks its own.

**Agent rulings** (`by = "agent"`, each naming its slice): PHN-440 keep iroh, one crate `ferb-link` for both sides;
PHN-441 the link as its own signed helper, no permission, the daemon verifying everything; PHN-442 pairing as built;
PHN-443 the three keys; PHN-444 the public relay carries text, questions and Stop, bulk refused before its first byte
with stable codes (supersedes the lab's LIM-6); PHN-445 no timed start-overs on the public relay, the 30 s tries kept
on the own relay (supersedes NET-27); PHN-446 UniFFI and cargo-ndk (supersedes PHN-1); PHN-447 CameraX with
zxing-cpp, AVFoundation on iPhone (supersedes PHN-17); PHN-448 SQLCipher under a Keystore key with
dataExtractionRules (supersedes PHN-3, PHN-5); PHN-449 Compose now, the iPhone toolkit at stage 4; PHN-450 the sky
computed on the phone from the clock and a coarse place, no outside call; PHN-451 room for phone-side tools later;
PHN-452 the Claude row's words.

**Recommended, awaiting the owner** (ask these):
1. "Should the public relay carry only text, questions and Stop, with pictures, preview and files refused there?"
   (Item 2 of the twelve; PHN-444 builds it. LIM-12's shared-relay bulk figures, kept by PHN-419, then go.)
2. "Live sky computed on the phone from its clock and one coarse location reading, with no outside call, rather than
   fixed placements?" (PHN-450, the answer to his "live or hardcoded?")
3. "On the public relay, may the phone skip the every-30-seconds start-overs and leave the direct-path search to
   iroh's own once-a-minute try?" (PHN-445 narrows his NET-24 = PHN-224 there.)
4. "When your own relay is down, may the phone fall back to the public relay (text only) until it is back, or stay
   off?" (PHN-418 says never the public relay with an own relay set; the lab's NET-8 = PHN-208 falls back.)
5. "With your own relay set, does 'my relay always' mean your relay even when a direct path exists (the lab's
   'Always use my relay' setting, NET-19 = PHN-219), or direct first?" (PHN-418's reading says direct first.)
- No Cursor sign-in (PHN-424, owner). Open: whether OpenAI accepts Sign in with ChatGPT
  from a mobile app (to confirm with OpenAI; API keys are the certain route).
- The lab's own open questions, to ask when their slice comes: T-10 local discovery on the same Wi-Fi with no
  internet; T-25 a short typed code; T-28 how hard to keep trying for direct; T-29 the idle link's re-check and
  heartbeat; T-30 the 30-second tries' timings (partly answered by PHN-445); T-31 a second iroh patch to retry direct
  at once; T-32 (blocked) fixing what holds a session on the relay.

**The slices** (kb tasks; `after` gives the order). S1, S2, S6 and S10 need nothing and run at once.

| Slice | Task | What | After |
|---|---|---|---|
| S1 | T-1200 | `engine/crates/ferb-link` from the lab (with `relay/`'s setup script); the lab's suites pass; FFI defects fixed; PHN-444, PHN-445 | - |
| S2 | T-1201 | `Role::Phone` and the method classes in ferb-rpc; a test fails when a method has no class | - |
| S3 | (T-1202, done: journal 106) | The daemon's phone area and its migration: devices, challenges, unlock window, proofs, audit | S2 |
| S3b | (T-1203, done: journal 111) | Phone files through Protection | S3 |
| S4 | T-1204 | Start, sign and package the helper; its Keychain item through ferb-secrets | S1, S3 |
| S5 | (T-1205, done: journal 110, 113, 117) | FERB.app Settings → Phone: pairing, phones, own relay | S3, S4 |
| S6 | T-1206 | The transcript golden: `transcripts.json` from the Mac fold; gapped, sealed and legacy scenarios | - |
| S7 | T-1207 | `ferb-mobile`: store, sync, outbox, fold, reducer | S1, S6 |
| S8 | (T-1208, done: journal 112) | `ferb-mobile-ffi` (UniFFI); call cancellation in ferb-link left as T-1218 | S7 |
| S9 | (T-1209, done: journal 114) | `apps/android` :core, Gradle, the Rust build through cargo-ndk, the Vault and keys | S8 |
| S10 | (T-1210, done: journal 101) | `apps/android` :design: tokens, components, sky | - |
| S11 | T-1211 | `apps/android` :app screens, the lock, notifications, Stay connected (built: journal 115, 116, 117; speaking is the owner's decision) | S3b, S5, S9, S10 |
| - | (T-1221, done: journal 116) | ferb-mobile-ffi additions S11 needs: lock, stop everything, status, a read mark, files, preview, settings, tasks, the Mac's name | S8 |
| - | T-1212 | Phone sign-ins (ChatGPT, API keys; Claude's row says why) | S7 |
| - | (T-1214, done: journal 117) | The phone's device line and lookup (SENSE-421, SENSE-422 on the phone), its own Location permission | S3, S11 |

Live checks: LIVE-1100..1111 (pairing by QR and typed code, direct on the same Wi-Fi, own relay, the public relay's
text-only fallback, Wi-Fi to mobile data, the 5-minute lock, a notification answer, a fingerprint-signed Yes,
removing a phone, a phone turn on the Mac, the outbox with the Mac off).

Known before porting: the lab's library has no close call, no `catch_unwind` and one global client per process
(`rust/src/ffi.rs`); S1 fixes them. Stage 2's remaining items (the owner's live check of another client's turn, the
archive import) do not block S1, S2, S6 or S10.

## Stage 4: the iPhone app

- Built on the simulator (journal 118; rulings PHN-560..574): `apps/ios` with four Swift packages over the same Rust
  library (`tools/build-rust.sh`: an xcframework for `aarch64-apple-ios` and the Apple-silicon simulator), the glass
  lab's SwiftUI copied as FerbDesign, FerbCore (Secure Enclave keys in Android's wire form, Keychain vault), the app
  feature for feature with Android; `apps/ios/tools/build.sh test`. iOS has no Stay connected (PHN-566) and can't blank
  screenshots (PHN-565).
- Left: the owner's answers (T-1240 screenshots, T-1241 the background words, T-1242 bundle id, minimum iOS, team;
  T-1245 the words), a physical iPhone and provisioning (T-1243, then LIVE-1100..1111 with it), proof.rs's iOS vectors
  (T-1244), the gate (T-1248), the icon from make_logo.py (T-1246); closed-app notifications are stage 5 (T-1247).

## Stage 5: closed-app notifications

- The owner's words to another agent: "Include background notifications in the foundation" and "Follow the phone's
  notification privacy settings"; confirm and record them (they reverse the phone lab's PHN-6, "No hosted push
  sender is built").
- Owner, 2026-11-16 (PHN-421, superseding the lab's PHN-4 and PHN-6): Stay connected stays, and stage 5 adds
  closed-app notifications through a small push sender the owner runs, with no words in the push.
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

- **Pushing the branch.** GitHub's push protection refused `engine/fixtures/sanitize` (invented key-shaped test
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
| `ferb/orchestrator/verify.py` `Checks._action` (ORCH-561/576) | An action check proved "not done" from not finding a record of one kind of call, though a click, a key or an app's window can do the action; in 8 of 15 such verdicts the checker had looked and was right | replaced by code's facts (ORCH-664..675, journal 122) |
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
| `ferb/daemon/areas/browsers.py` `_pair_one` | A pairing page missed by the profile is never opened again; the wait runs out its minute | fixed in Rust (81) |
