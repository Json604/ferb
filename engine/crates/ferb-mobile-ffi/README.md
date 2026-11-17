# ferb-mobile-ffi

The phone library: ferb-mobile (the stored chat, sync, outbox, answers) and ferb-link (the link to the Mac, pairing)
behind one set of UniFFI bindings, so each app links ONE library (`libferb_mobile_ffi.so` on Android, the static
archive on iPhone). Stage 3 slice S8 (kb T-1208, PHN-446, PHN-492..494); `engine/JOURNAL.md` entry 112 says what was
done and why. Excluded from the workspace like ferb-mobile and ferb-link (built by its own manifest; edition 2024
because its real-link test shares ferb-link's test world).

## UniFFI

UniFFI 0.32.2 (Mozilla, MPL-2.0), proc-macro definitions only (no UDL). The bindings generator is a Rust program in
this crate (`src/bin/uniffi-bindgen.rs`, feature `bindgen`): no Python at build time. MPL-2.0 is file-level copyleft:
the phone library compiles in uniffi_core (and the generated Kotlin/Swift carry UniFFI's templates), so a release
names UniFFI and its licence in THIRD_PARTY_NOTICES.md with where its source is (crates.io, github.com/mozilla/uniffi-rs);
FERB's own files stay Apache-2.0, and only a change to UniFFI's own files would have to be published under MPL-2.0.
The notices file is the owner's (PROJ-504): to add, not added here. JNA (Apache-2.0/LGPL-2.1, the Kotlin bindings'
runtime) is S9's dependency and goes in the same list.

```sh
. ~/.cargo/env; cd engine/crates/ferb-mobile-ffi          # from here, so cargo metadata finds uniffi.toml
export CARGO_TARGET_DIR=../../target/agents/ffi
cargo build --lib                                          # the host library the generator reads
cargo run --features bindgen --bin uniffi-bindgen -- generate --library $CARGO_TARGET_DIR/debug/libferb_mobile_ffi.dylib \
  --language kotlin --language swift --out-dir <dir> --no-format
```

`uniffi.toml` names them: Kotlin package `com.ferb.mobile` (file `com/ferb/mobile/ferb_mobile_ffi.kt`, needs JNA
5.12+; `android = true`: Android's SystemCleaner from API 33, JNA's below), Swift module `FerbMobile` with the C module
`FerbMobileFFI` (`FerbMobile.swift`, `FerbMobileFFI.h`, `FerbMobileFFI.modulemap`). Generating from the phone's own
library (the Android `.so`) gives the same bindings: library mode reads the metadata UniFFI put in it.

## The API as Kotlin sees it

```kotlin
// the core
Mobile.open(path: String, key: ByteArray /* 32 */, config: MobileConfig, clock: Clock, ids: Ids, listener: Listener): Mobile
m.connect(link: Link); m.connectTransport(transport: Transport); m.linkEvent(eventJson: String); m.disconnect()
m.sync(); m.unlock(signer: Signer)
m.conversations(): List<ConversationInfo>; m.currentConversation(): String?; m.transcript(conversation: String): TranscriptView
m.outbox(): List<OutboxItem>; m.send(text: String): OutboxItem; m.flushOutbox(); m.cancel(outboxId: String): Cancelled
m.answer(approvalId: String, answer: AnswerInput, signer: Signer?): String   // the reply's JSON
m.stop(taskIds: List<String>); m.loadOlder(conversation: String): OlderState; m.markRead(conversation: String, pos: Long)
m.linkState(): LinkState; m.forgetThisMac(); m.shutdown(); m.isClosed(): Boolean; m.storePath(): String
// T-1221 (journal 116)
m.lock(): Boolean; m.unlocked(); m.stopEverything(); m.status(): PhoneStatus; m.lastStatus(): PhoneStatus?
m.markReadLatest(conversation); m.sendWith(text, attachments: List<Attachment>): OutboxItem
m.files(path: String?, signer: Signer?, after: String? = null): Folder; m.tree(path, signer?): Tree; m.putGroup(name): String
m.fetch(id, path, saveTo, offset: ULong, signer?, listener: TransferListener): Fetched
m.upload(id, localPath, name, resume?, group?, rel?, listener): Uploaded; m.cancelTransfer(id)  // -> FerbException.Cancelled
m.preview(taskId, have: Long?): PreviewFrame; m.watchLive(taskId, sink: PreviewSink): LivePreview  // .stop()
m.settings(): List<SettingValue>; m.setSetting(key, valueJson, signer): String
m.tasks(limit: UInt, before: String?): TaskPage; m.macInfo(): MacInfo; m.cachedMacInfo(): MacInfo?
m.registerKeys(devicePub?, strongPub?, signer)
// T-1214, T-1211 (journal 117)
m.reportDevice(report: DeviceReport): Boolean            // the phone's own record; again on each connection
m.locationWanted(): String?; m.answerLocation(requestId, fix: LocationFix?, whyNot: String?)  // Change.LocationWanted
m.recentFiles(limit: UInt): RecentFiles; m.searchFiles(query, path: String?, after: String? = null): FoundFiles
m.fetchTaskFile(id, taskId, index: ULong, saveTo, listener): Fetched
SecretInput()  // .push(unit: UShort) per UTF-16 unit, .len(), .isEmpty(), .clear()
m.answerHidden(approvalId, secret: SecretInput, signer: Signer): String   // the input is emptied whatever happens
// the owner's 2026-11-17 decisions (GATE-615, PHN-580/581, PHN-588/589)
m.answer(approvalId, AnswerInput.UntilBack, null)   // only where QuestionNeeds.untilBack; NeedsUnlock -> unlock, again
m.screenUse(): ScreenUse; m.endScreenUse(): ScreenUseEnd   // allowed locked; also kept in PhoneStatus.screenUse
m.beginGroup(name): GroupStarted   // {group, savedIn}; Uploaded.savedIn; MacInfo.uploadFolder: UploadFolder?
// load more: Folder/FoundFiles {from: ULong, next: String?, cut}: pass `next` back as `after` while it is non-null
interface TransferListener { fun progress(done: ULong, of: ULong); fun accepted(uploadId: String, offset: ULong) }
interface PreviewSink { fun frame(frame: PreviewFrame); fun ended(code: String?, detail: String?) }
// Android only: the one JNI export, com.ferb.core.NativeInit.init(context): ndk_context for iroh's DNS (PHN-514)
// the link
Link.start(config: LinkConfig, listener: LinkListener, deviceSigner: DeviceSigner?): Link
l.endpointId(): String; l.pair(code: String); l.status(): LinkStatus; l.setActive(on: Boolean)
l.networkChanged(kind: String?); l.retry(); l.forget(); l.shutdown(); l.isClosed(): Boolean
// what the app implements
interface Clock { fun nowIso(): String }                 interface Ids { fun newId(): String }
interface Listener { fun changed(change: Change) }       interface Signer { fun sign(payload: String): String? }
interface DeviceSigner { fun sign(payload: String): String? }
interface LinkListener { fun statusChanged(status: LinkStatus); fun persist(state: ByteArray) }
interface Transport { fun call(method: String, paramsJson: String, proofJson: String?): String
                      fun send(paramsJson: String): String; fun watch(paramsJson: String, sink: WatchSink): ULong
                      fun closeWatch(watchId: ULong) }   // throws TransportException.Failed(code, detail, allow, retryAfterS)
libraryVersion(): String
```

Records (Kotlin data classes): `TranscriptView {conversation, items: List<Item>, questions: List<QuestionItem>, live:
List<LiveView>, outbox, older: OlderState}`; `Item` is sealed (`Turn(turn: TurnItem)`, `Reply(reply: ReplyItem)`,
`Notice(notice: NoticeItem)`, `Reminder(notice: NoticeItem)`); TurnItem, AnswerView, CardView, RowView, CardRow,
ChecklistView, PlanStep, FirstReply, OwnerWords, ResultCard, ScheduleRow, RuntimeSwitch, RowQuestion, Sealed, Quote,
ReplyItem, NoticeItem, RowRef, LiveView, LiveRow, LiveLine are field-for-field copies of ferb-mobile's types (the
fields of `engine/fixtures/chat-entries/README.md`, "The phone's transcript", in camelCase; `ResultCard.kindOf` is the
card's `type`, `PlanStep.repeats` its `loop`, `NoticeItem.reference` its `ref`, `TurnItem.words` the owner's words
that the golden flattens into the turn). `QuestionItem.bodyJson` is the one JSON text: a question's body has no fixed
shape. Each conversion destructures the core's type completely, so a field added there fails the build here.
`Change` (with `LocationWanted(requestId)`), `OlderState`, `RelayStatus` sealed; `DeviceReport`, `DevicePlace`,
`LocationFix`, `FoundFile`, `RecentFiles`, `FoundFiles`, `ScreenUse`, `ScreenUseEnd`, `GroupStarted`, `UploadFolder`
records; `LinkPath`, `LinkPhase` enums; `AnswerInput` sealed (`Approve(granted, choice)`, `UntilBack` (GATE-615: the
screen-use question's Yes "until I'm back at my Mac", never signed), `Words(text)`, `Correct(text)`, `Hidden(secret)`:
kept for older apps; `answerHidden` with a `SecretInput` is the one whose secret crosses no buffer).

Errors: every call throws `FerbException`, sealed, one subclass per stable code, each with `detail` (the words) and
`linkCode` (the link's own code, ferb-link `errors.rs`): `Closed`, `BadKey`, `WrongKey`, `StoreNewer`, `Store`,
`Offline`, `NeedsBetterPath`, `NeedsUnlock`, `Refused`, `NeedsSigner`, `NotSigned`, `NotFound`, `InFlight`, `BadReply`
(the core's `ErrorCode`), `BadArgument` (the link's saved state is not the link's), `Internal` (a panic caught at the
boundary). Rust's `FerbError::code()` gives the string. `InternalException` (UniFFI's own) only if a panic got past
every guard (none known).

## Objects, close and panics

- No global state: `Mobile` and `Link` own what they use; a process may hold several of each (tests run two).
- `shutdown()` is the explicit close: idempotent, bounded; every later call throws `FerbException.Closed`. It is not
  named `close` because the bindings' own `close()` (Kotlin `AutoCloseable`, `use {}`; Swift deinit) frees the handle:
  that also shuts the object down (Rust `Drop`), and a call after it throws the bindings' `IllegalStateException`.
- Every exported call runs inside `catch_unwind` (`error::guard`): a panic becomes `FerbException.Internal` and the
  object stays usable (its mutexes recover from poisoning). UniFFI's scaffolding catches anything past that. Every
  callback the app supplies is called inside `catch_unwind` too: a Kotlin exception thrown from a `Listener` makes
  UniFFI panic on the calling thread, which is caught there (the pump and the core's threads go on). Panics print to
  stderr (logcat) through Rust's default hook; no secret is in any panic message of this crate.

## Threading

Every call is blocking and bounded (S7's model); none waits unboundedly. **No method of `Mobile` or `Link` is called
on the UI thread** (kb PHN-682): each takes a lock that store work, or a defect, can hold. The apps keep to it by
shape: every call goes through `FerbPhone.call` (its IO dispatcher or queue), what the app tells the link
(`setActive`, `setOpen`, `networkChanged`, `retry`) through one serial lane, and `MainGuard` at that door names any
call that still arrives on the main thread. The only objects touched on the UI thread are `SecretInput` (its own
lock, held for a push) and `LivePreview.stop` (a flag). Short (store or memory only, never the network): `transcript`,
`conversations`, `currentConversation`, `outbox`, `cancel`, `linkState`, `lastStatus` (its own lock, a copy),
`linkEvent`, `connect`, `connectTransport`, `disconnect`, `shutdown`, `isClosed`, `storePath`, and on `Link`:
`status`, `endpointId`, `pair`, `setActive`, `networkChanged`, `retry`, `forget`, `isClosed`. Long: `Mobile.open` (opens
and may upgrade the encrypted database), `sync`, `send` and `flushOutbox` (they send when online: up to the link's call
deadline), `answer` and `unlock` (network and the app's fingerprint prompt), `unlockWithStatement` (it waits up to 3 s
for the new connection's presence proof first, kb PHN-684), `stop`, `loadOlder`, `markRead`, `forgetThisMac`,
`Link.start` (up to 20 s), `Link.shutdown` (up to about 5 s). Callbacks: `Listener.changed` and the `LinkListener`
calls come on the library's threads (the link's pump, a sync's thread) with nothing locked: post to the UI thread
and return; never call a blocking method from inside one. `Signer.sign` runs on the thread that called `answer` or
`unlock` (it may show the prompt and wait); `DeviceSigner.sign` on a thread of its own and must answer within 3 s.

## The link

`Link.start` starts ferb-link's `LinkClient`; one pump thread (ferb-mobile `door::DoorTransport`) reads its events.
`Mobile.connect(link)` makes it the core's transport and forwards the link's `state`, `bulk` and `relay_state` events
to the core (the current state at once). A `relay_state` reaches the core as the words to show only while the owner's
relay is down and the public one is used (`own_fallback`: the link's own words, else a plain sentence); the full relay
state is `LinkStatus.relay`.

- **Pairing:** `pair(code)` takes a scanned QR's text or a typed `FERB-` code. `LinkStatus` then shows `Pairing`,
  `PairingWaitForMac` with `pairingCode` (the six digits both screens show, `123 456`), and `Online` with `paired`
  once the owner said Yes on the Mac. `forget()` forgets the Mac on the phone (the phone's link key stays).
- **Status:** `phase`, `path` (Direct, OwnRelay, PublicRelay, Offline), `tierWords`, `bulkAllowed` (PHN-444),
  `ownRelayUrl` and `ownRelayAlways` (the Mac's setting: the phone follows the Mac's relay, PHN-418/453; the owner
  changes it in FERB on the Mac), `relay` (`Shared`, `Own`, `OwnDown`, `OwnFallback`, with the problem), `problem`
  and `retryInS`, `slow`, `macOlder`/`macNewer`.
- **The phone's link key:** `LinkListener.persist(state)` hands over the link's state as opaque bytes (JSON inside):
  it holds the phone's PRIVATE link key and where the Mac is. S9 (and stage 4) must store it encrypted under a
  Keystore/Keychain key that never leaves the phone (non-exportable; Android `dataExtractionRules` and
  `fullBackupContent` exclude it, iOS `kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly` or a file excluded from
  backup), replace it at each `persist`, and pass it back as `LinkConfig.state`. A first start makes the key and calls
  `persist` at once. Bytes that are not the link's state are refused (`BadArgument`), never silently replaced by a
  new identity.
- **Presence and unlock (PHN-125):** with a `DeviceSigner`, the library proves each new session comes from this phone
  (`link.present` over the Mac's fresh challenge, the device key, no prompt) and answers the link's `prove` requests
  during a start-over. A locked phone's private reads are `NeedsUnlock`; `Mobile.unlock(signer)` signs the Mac's
  challenge for `link.unlock` (after the fingerprint prompt) and then syncs, which sends what waited.
- **Not here:** the app's own background and network signals (`setActive`, `networkChanged`) come from the app;
  files, previews and the stored-data viewer of PHN-403/404 are later slices.

## Checks

- `cargo test`: `tests/ffi.rs` (8) drives the exported objects with ferb-mobile's fake Mac behind an app-supplied
  `Transport`: every one-conversation scenario's transcript, as the records the app receives, equals
  `transcripts.json`; send and the outbox; signed and refused answers; the public relay; forget; shutdown and every
  call after it; two objects at once and from threads; a throwing listener. `tests/link.rs` (1) runs the real link on
  127.0.0.1 against ferb-link's test world (local relay, stand-in daemon, the Mac's bridge): start and persist,
  pairing by code with the six digits, the device key's proof, a locked refusal, Stop, unlock, a message through the
  door, forget, shutdown, a restart from the kept state, a bad state refused. The unit tests call UniFFI's generated
  C functions for the two panic hooks (`testing` feature or test builds only).
- The generated bindings (journal 112): `swiftc -typecheck` of `FerbMobile.swift` with its module map; a Swift program
  and a Kotlin/JVM program (Gradle, Kotlin 2.2.21, JNA 5.17.0) against the host library built with `--features
  testing`, each opening a core over a fake Mac written in that language, syncing, sending, answering, the panic hooks,
  shutdown. These ran from the scratch folder, not in the repo; S9 moves the Kotlin one into `apps/android` (:core's
  JVM tests).
- Builds: `cargo build --release --lib --locked` for `aarch64-linux-android` (NDK 28: `CC_`, `AR_`, `RANLIB_` and the
  linker `CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER` = the NDK's `aarch64-linux-android30-clang`), `aarch64-apple-ios`
  and `aarch64-apple-ios-sim` (with `IPHONEOS_DEPLOYMENT_TARGET=13.0`). Release profile (PHN-446): opt-level "s", LTO,
  one codegen unit, stripped, unwinding panics (the guard needs them).
