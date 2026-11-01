# First-run intro (direction C) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the setup window with the eight-beat intro in notes/setup-intro.md: the notch speaks, a glass orb
flies to point, every grant is guided and confirmed by the system, and the demo shows a task's journey without acting.

**Architecture:** Pure decisions live in NotchCore (beats, resume, grant confirmation, orb motion, Settings anchoring,
sound policy and spatial mapping) with unit tests; AppKit/SwiftUI pieces in FERBNotch render them (orb panel, notch
narration, drag card, sound engine) and an `IntroController` drives the beats, reusing `SetupModel`'s existing actions
(service, sign-in, grants, browsers, identity, persona) as its service layer. The daemon gains stronger grant checks and
a plan-only preview run that issues no run token, so the demo cannot act by construction.

**Tech Stack:** Swift 6 / SwiftUI / AppKit (macOS 14+), AVAudioEngine, ScreenCaptureKit/CoreGraphics window lists;
Python 3.12 daemon (asyncio, RPC areas), pytest; XCTest.

**Spec:** notes/setup-intro.md (agreed 2026-11-01). Sound research: notes/sound.md. Code map: this plan's file paths
(S/ = ui/mac/FERBNotch/Sources/FERBNotch, C/ = …/Sources/NotchCore, T/ = …/Tests/NotchCoreTests).

## Global Constraints
- Beats, in order: meet, signIn, permissions, browsers, identity (optional), tryIt, control, home.
- Every permission stays required (`SetupModel.askedAtSetup`: seven after Task 3 folds hearing into the dictation row,
  the same Speech Recognition grant); Automation stays per app at first use.
- A grant counts only when the system confirms it; never inferred from a drop. Poll every 1.5 s during a grant.
- The demo never acts: the preview run issues no `RunGrant`, runs no worker, and emits only plan/part events marked
  `preview: true`.
- FERB's intro lines are text in the notch, never spoken; sound comes only from the sound interface, which plays the decided
  family (notes/sound.md; files rendered by `demo/sound-family/render.py`). Default on: needs you, error, done,
  notification, listening start/end and the tour; default off: summon and paused; Standard intensity by default,
  Subtle in Settings (owner, 2026-11-01).
- Motion: open springs duration 0.35–0.5 s, bounce 0.15–0.3; close/dismiss bounce 0; decorative animations ≤ 2 s; all
  interruptible. Reduce Motion: cross-fades, the orb fades in place beside its target.
- Spatial: pan = clamp((x / screenWidth − 0.5) × 1.2, −0.6, 0.6); distance from the notch lowers gain ≤ 3 dB and adds
  a low-pass; HRTF only on headphones. Honour "Play user interface sound effects" (`com.apple.sound.uiaudio.enabled`)
  and FERB's `feedback.enabled` / `feedback.volume`.
- Copy: warm, plain, short; macOS 27 names Accessibility "Device Control and Data Access" (say both where unsure).
- Repo general: invented example data only. Swift builds/tests use `--scratch-path` outside the repo; never run
  build.sh/build-app.sh. Python: `nice -n 10 uv run pytest -q`, `uv run mypy ferb/core`, `git diff --check`.
- Existing installs with setup done never see the tour; Settings → General gets "Replay intro".

## Review Focus
- Screen Recording's "Quit & Reopen" mid-tour: FERB relaunches into the same beat with the same grants (Task 1 test).
- System Settings moved, minimised, closed or covered while the orb is docked: the orb follows, hides, or flies home;
  never floats over another app (Task 6 test).
- Sign-in cancelled or failing (no network): the scan never starts; the Try beat uses the general task (Task 4 test).
- A prompt denied or a grant revoked mid-tour: the beat stays open with a plain way forward; finishing reports what's
  missing and setup returns next launch (Task 8 test).
- Skip pressed mid-flight: the orb, its sound and the drag card are torn down at once; the compact path shows only the
  required steps left (Task 8 test).
- Notchless Mac or FERB on a second display: the orb launches from the virtual notch on FERB's screen (Task 5 test).

---

### Task 1: Intro beats, resume and upgrade rules (NotchCore)

**Files:**
- Create: `C/IntroPlan.swift`
- Test: `T/IntroPlanTests.swift`

**Interfaces:**
- Produces:
  - `public enum IntroBeat: Int, CaseIterable { case meet, signIn, permissions, browsers, identity, tryIt, control, home }`
  - `public struct IntroNeeds { var setupDone: Bool; var signedIn: Bool; var missingPermissions: [String]; var unpairedBrowsers: Bool }`
  - `public enum IntroPlan { static func beats(_ needs: IntroNeeds, replay: Bool) -> [IntroBeat]; static func next(after: IntroBeat, in: [IntroBeat]) -> IntroBeat? }`
  - `public struct IntroProgress { static let beatKey = "FERBIntroBeat"; static let doneKey = "FERBIntroDone"; init(defaults: UserDefaults); var resumeBeat: IntroBeat? { get set }; var done: Bool { get set }; mutating func clear() }`

- [ ] **Step 1: Write the failing tests** in `T/IntroPlanTests.swift`:
  - `testNewInstallGetsEveryBeatInOrder`: `beats(IntroNeeds(setupDone:false, signedIn:false, missingPermissions: all8, unpairedBrowsers:true), replay:false)` == `IntroBeat.allCases`.
  - `testFinishedSetupGetsNoTourUnlessReplayed`: setupDone true → `[]`; with `replay:true` → all cases.
  - `testMissingGrantsOnlyGetsThePermissionsBeatAndHome`: setupDone false, signedIn true, missing `["screen_recording"]`, browsers paired → `[.permissions, .home]`.
  - `testResumeSurvivesARelaunch`: set `resumeBeat = .permissions` on a `ScratchDefaults` suite, new `IntroProgress` on the same suite reads `.permissions`; `clear()` → nil.
- [ ] **Step 2: Run** `swift test --filter IntroPlanTests --scratch-path <scratch>` → FAIL (types missing).
- [ ] **Step 3: Implement** `C/IntroPlan.swift` (pure logic; identity is offered whenever browsers are offered).
- [ ] **Step 4: Run** the same command → PASS.
- [ ] **Step 5: Commit** `git commit -m "Intro: beats, resume and upgrade rules"`.

### Task 2: Grant confirmation the system really reports (daemon)

**Files:**
- Modify: `ferb/device/permissions.py` (`check_all` ~179, `PANES`/`pane_url` ~145-158)
- Test: `tests/test_permissions.py`

**Interfaces:**
- Produces: `confirmed(name: str, probe: Callable[[str], bool] | None = None) -> bool` used by `check_all`;
  `pane_url(name)` returns the macOS 13+ form `x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?Privacy_<Pane>`.

- [ ] **Step 1: Write the failing tests**:
  - `test_accessibility_counts_only_when_a_real_ax_read_works`: fake `AXIsProcessTrusted` → True but the AX probe fails → status not granted.
  - `test_screen_recording_counts_only_when_another_apps_window_title_is_readable`: preflight True, probe False → not granted.
  - `test_input_monitoring_uses_iohid_check_access`: unchanged path still decides.
  - `test_pane_urls_use_the_privacy_security_extension_form`: `pane_url("accessibility")` == `"x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?Privacy_Accessibility"`.
- [ ] **Step 2: Run** `nice -n 10 uv run pytest -q tests/test_permissions.py` → FAIL.
- [ ] **Step 3: Implement** the probes: accessibility = `AXUIElementCopyAttributeValue` on the frontmost app's
  element succeeds; screen recording = `CGWindowListCopyWindowInfo` shows a `kCGWindowName` for a window owned by
  another process. Keep probes injectable for tests.
- [ ] **Step 4: Run** → PASS; then `uv run mypy ferb/core`.
- [ ] **Step 5: Commit** `git commit -m "Permissions: confirm grants with real reads; Privacy & Security deep links"`.

### Task 3: FERB Voice merges into FERB.app (owner, 2026-11-01: "ok lets merge it")

Why: `macapp/FERB` (FERB Voice, `Helpers/FERB Voice.app`, id `com.ferb.app`) is the 10-01 FERB.app kept as a helper
when the notch became the app on 10-14, "nested until it merges". It holds a second Speech Recognition grant that the
notch (`com.ferb.FERB`) already has for dictation, so the intro would guide one grant twice.

**Files:**
- Create: `S/Hearing.swift` (the three hearing modes and the lease protocol, ported from `macapp/FERB/main.swift`),
  `C/FerbHomeRule.swift` (`resolveFerbHome(_ given: String?, account: String?) -> FerbHome`, from `macapp/FERB/home.swift`)
- Modify: `S/Application.swift` (`main()`), `ferb/device/install.py` (`BUNDLE_VOICE_APP`, `voice_app`),
  `ferb/daemon/areas/voice.py` (`voice_bundle`, `_quit_voice_app`), `ferb/sense/transcribe.py`, `ferb/cli/setup.py`
  (`_build_app`, `_shipped_app`, `_voice_helper`), `ferb/device/permissions.py` (the hearing row),
  `S/SetupModel.swift` + `C/SetupModels.swift` (`askedAtSetup`, `allowAllSequence`, `answeredInPopup`, the "voice"
  process), `scripts/build-app.sh`, `scripts/signing.sh`, README.md (permissions table), notes/voice.md,
  notes/daemon.md, notes/test-tasks.md, and every test that names `macapp/` or `FERB Voice`
- Delete: `macapp/` (plain `rm -r`, then `git add macapp`)
- Test: the existing hearing, setup, packaging and permissions tests, moved to the new paths; `T/FerbHomeRuleTests.swift`

**Interfaces:**
- Produces: FERB.app's own executable answers `transcribe <audio> <out> [locale]`, `status <out>` and
  `request-speech` exactly as FERB Voice did (same arguments, same answer files, same lease, same exit codes), started by
  the daemon with `open -g -n -W --env FERB_HOME=… <FERB.app> --args <mode> …`. `install.voice_app(home, bundle)`
  returns the bundle itself.
- Rules for this task:
  - `main()` checks `CommandLine.arguments` first: a hearing mode runs and exits before `NSApplication`, the
    delegate, Sparkle, login items, the daemon connection or any window. No microphone in these modes. An unknown
    first argument starts the normal app.
  - The home rule and the lease behaviour are unchanged (a foreign `FERB_HOME` is refused before anything else;
    deleting the answer file stops the process with nothing written).
  - Quitting before an update targets only the hearing instances FERB itself launched (by process id), never by
    bundle id: `com.ferb.FERB` is also the running notch (`voice.py:77` terminates every `com.ferb.app` today). A
    transcription in flight is still spared (notes/voice.md decision 8).
  - Hearing's signature pin (`[hearing] requirement`) becomes FERB.app's own sealed requirement. An older pin naming
    `com.ferb.app` is replaced by the first app setup after the update, with no owner step (storage decision 5).
  - Permissions: hearing is the same Speech Recognition grant as dictation, so the separate "hearing" row goes. The
    microphone row (Microphone + Speech Recognition, `GrantStatus.both`) covers it and `askedAtSetup` drops to
    seven. Hearing's errors still say plainly when Speech Recognition is off.
  - Owner-facing text says "FERB", never "FERB Voice". `com.ferb.app` stays in the "is this FERB's own process"
    lists (`front_window.py` `FERB_APPS`, `sandbox.py` `APP_PREFERENCE_DOMAINS`), because an older build may still be
    running during an update.
  - A checkout install (`_build_app`) builds the notch with `ui/mac/FERBNotch/build.sh` (tests fake `run`; agents
    never run it).
  - notes/test-tasks.md's fresh-start list gains `tccutil reset SpeechRecognition com.ferb.app`, and Privacy &
    Security lists only FERB.

- [ ] **Step 1: Write the failing tests**:
  - `test_hearing_launches_the_app_itself`: `voice_app(home, bundle) == bundle`; the transcriber's `open` argv names
    the bundle with `--args transcribe`.
  - `test_update_quit_never_terminates_the_notch`: a running notch plus one idle hearing instance → only the hearing pid
    is ended; one transcribing → nothing is ended.
  - `test_an_old_voice_pin_is_replaced_by_app_setup`: config `[hearing] requirement` naming `com.ferb.app` → after
    app setup it is the app's sealed requirement.
  - `test_the_bundle_has_no_voice_helper`: packaging/bundle checks list no `Helpers/FERB Voice.app`; signing has
    no `voice` piece.
  - `test_setup_permissions_have_no_separate_hearing_row` (Python), plus `askedAtSetup` == seven ids (Swift).
  - `T/FerbHomeRuleTests.swift`: unset, empty or same-inode `FERB_HOME` → `.home`; another folder → `.refused`.
- [ ] **Step 2: Run** the focused pytest files and `swift test --filter 'FerbHomeRuleTests|SetupModelTests'
  --scratch-path <scratch>` → FAIL.
- [ ] **Step 3: Implement** the merge, then delete `macapp/` and sweep every reference (`git grep -n -e macapp -e
  "FERB Voice"` returns only CHANGELOG.md and the old-build lists above).
- [ ] **Step 4: Run** `swift build` and `swift test` (scratch path), then `nice -n 10 uv run pytest -q`,
  `uv run mypy ferb/core`, `git diff --check` → PASS.
- [ ] **Step 5: Commit** `git commit -m "FERB Voice merges into FERB.app: one app, one Speech Recognition grant"`.

### Task 4: Suggested task and plan-only preview (daemon)

**Files:**
- Create: `ferb/daemon/areas/intro.py`
- Modify: `ferb/orchestrator/supervisor.py` (add `preview`, next to `send` ~1814), `ferb/daemon/auth.py` (APP_ONLY),
  `ferb/daemon/areas/rpc.py` (register), `ferb/daemon/server.py` (`LONG_WAITS`)
- Test: `tests/test_intro_preview.py`

**Interfaces:**
- Consumes: `PersonaReview.status()` (ferb/daemon/persona.py:87), `Supervisor._supervise` (~2544).
- Produces:
  - RPC `intro.suggest {}` → `{"request": str, "fits": bool}` (`fits` false → the general task `GENERAL_TASK`).
  - Stream RPC `intro.preview {request}` → events `plan`, `part` (each with `"preview": true`), then `done`.
  - `Supervisor.preview(session_id: str, text: str) -> AsyncIterator[dict]`.
  - Constant `GENERAL_TASK = "Find three things on my Mac I could tidy up, and show me how you'd do it."` (copy may change; must stay general).

- [ ] **Step 1: Write the failing tests** (fake runtime, real daemon and gateway, temp FERB_HOME, `preload_secrets=False`):
  - `test_preview_runs_no_worker_and_issues_no_run_token`: after `intro.preview`, `RunTokens` issued count == 0, no `tool.called` bus events, every emitted event has `preview is True`.
  - `test_preview_shows_the_planned_steps_as_parts`: the fake planner returns 3 steps → 3 `part` events with those titles.
  - `test_suggest_falls_back_to_the_general_task_without_a_scan`: persona `not_started` or `failed` → `{"request": GENERAL_TASK, "fits": False}`.
  - `test_suggest_uses_the_scan_when_seen`: persona `seen` → `fits` True and a request derived by the fake model.
  - `test_intro_methods_refuse_any_other_program`: both are APP_ONLY.
- [ ] **Step 2: Run** `nice -n 10 uv run pytest -q tests/test_intro_preview.py` → FAIL.
- [ ] **Step 3: Implement**: `preview` runs the planner turn only (no `execute`/`run_tree`), maps its plan to part
  events, journals `intro.preview`; `intro.suggest` asks the model for one task from the persona summary (redacted),
  never acting, falling back to `GENERAL_TASK`.
- [ ] **Step 4: Run** → PASS; `uv run mypy ferb/core`.
- [ ] **Step 5: Commit** `git commit -m "Intro: suggested task and plan-only preview that cannot act"`.

### Task 5: Orb motion, spatial mapping and sound policy (NotchCore)

**Files:**
- Create: `C/OrbMotion.swift`, `C/SoundPolicy.swift`
- Test: `T/OrbMotionTests.swift`, `T/SoundPolicyTests.swift`

**Interfaces:**
- Produces:
  - `public struct SpringSpec { var duration: Double; var bounce: Double; static let open = SpringSpec(duration: 0.45, bounce: 0.2); static let close = SpringSpec(duration: 0.4, bounce: 0) }`
  - `public struct OrbMotion { init(origin: CGPoint); mutating func retarget(_ p: CGPoint, spec: SpringSpec); mutating func step(dt: Double) -> CGPoint; var velocity: CGVector { get }; var settled: Bool { get } }`
  - `public enum OrbOrigin { static func notchCentre(screenFrame: CGRect, cutout: CGRect?) -> CGPoint }` (virtual notch when `cutout` is nil, matching `NotchGeometry`'s 164-wide default).
  - `public enum FerbSound: CaseIterable { case firstLaunch, intro, summon, listeningStart, listeningEnd, done, needsYou, paused, error, notification, orbDrip, orbHop, orbMelt }` (the decided family, notes/sound.md: no flight or landing sound)
  - `public enum SoundPolicy { static func shouldPlay(_ s: FerbSound, uiSoundsEnabled: Bool, feedbackEnabled: Bool) -> Bool; static func pan(x: CGFloat, screenWidth: CGFloat) -> Float; static func distanceGainDB(fromNotch d: CGFloat, screenHeight: CGFloat) -> Float }`

- [ ] **Step 1: Write the failing tests**:
  - `testRetargetMidFlightKeepsVelocity`: step 10 frames toward A, retarget to B → velocity unchanged at the retarget instant.
  - `testCloseSpringNeverOvershoots`: `.close` toward a target → position never passes the target.
  - `testOrbLaunchesFromTheVirtualNotchOnANotchlessScreen`: `notchCentre(screenFrame: (1000,0,1440,900), cutout: nil)` == `(1720, 900 − menuBarHalfHeight)` per `NotchGeometry`.
  - `testPanIsClampedToPointSix`: x = 0 → −0.6; x = width → 0.6; x = width/2 → 0.
  - `testDistanceLowersGainAtMostThreeDB`: d = 0 → 0; d = screenHeight → −3.
  - `testNothingPlaysWhenSystemUISoundsAreOff`: `shouldPlay(.done, uiSoundsEnabled: false, feedbackEnabled: true)` == false.
- [ ] **Step 2: Run** `swift test --filter 'OrbMotionTests|SoundPolicyTests' --scratch-path <scratch>` → FAIL.
- [ ] **Step 3: Implement** (spring from duration/bounce per WWDC23 "Animate with springs": stiffness =
  (2π/duration)², damping = 4π(1 − bounce)/duration for bounce ≥ 0).
- [ ] **Step 4: Run** → PASS.
- [ ] **Step 5: Commit** `git commit -m "Intro: orb motion, spatial mapping and sound policy"`.

### Task 6: Docking beside System Settings (NotchCore + FERBNotch)

**Files:**
- Create: `C/SettingsAnchor.swift`, `S/SettingsWindowTracker.swift`
- Test: `T/SettingsAnchorTests.swift`

**Interfaces:**
- Produces:
  - `public struct WindowInfo { var owner: String; var bounds: CGRect; var onScreen: Bool; var isFrontmostApp: Bool }`
  - `public enum SettingsAnchor { static let bundleID = "com.apple.systempreferences"; static func dock(for windows: [WindowInfo], orb: CGSize, screen: CGRect) -> CGPoint?; static func settled(_ a: CGRect?, _ b: CGRect?) -> Bool }`
  - `final class SettingsWindowTracker { var onChange: ((CGPoint?) -> Void)?; func start(); func stop() }` (polls `CGWindowListCopyWindowInfo` every 0.25 s; bounds only).

- [ ] **Step 1: Write the failing tests**:
  - `testDocksToTheLeftOfSettingsWhenThereIsRoom` / `testDocksToTheRightWhenSettingsHugsTheLeftEdge`.
  - `testHidesWhenSettingsIsNotFrontmost`: `isFrontmostApp` false → nil.
  - `testHidesWhenSettingsClosedOrMinimised`: no on-screen window → nil.
  - `testSettledNeedsTwoMatchingFrames`.
- [ ] **Step 2: Run** `swift test --filter SettingsAnchorTests …` → FAIL.
- [ ] **Step 3: Implement** both files.
- [ ] **Step 4: Run** → PASS.
- [ ] **Step 5: Commit** `git commit -m "Intro: dock beside System Settings, follow it, hide when it isn't in front"`.

### Task 7: The orb, the notch narration and the sound engine (FERBNotch)

**Files:**
- Create: `S/OrbPanel.swift` (NSPanel, level `.mainMenu + 2`, `overlayBehavior`, click-through except the drag card),
  `S/OrbView.swift` (glass orb with the iridescent band; ripple while waiting; the frosted instruction bubble; the drag
  card holding an `NSDraggingSource` for the agent app's bundle URL, `PermissionSnapshot.grantTarget`),
  `S/IntroNarration.swift` (notch states: line + detail, swell/shrink using `NotchPresentation`), `S/FerbSoundEngine.swift`
  (AVAudioEngine; mono sources, `AVAudioEnvironmentNode` with `.headphones` → HRTFHQ else equal-power pan from
  `SoundPolicy`; plays the decided family's dry mono files through the room IR, picking a random variant where
  there are several; reads `com.apple.sound.uiaudio.enabled` and `feedback.*`), `S/Resources/Sounds/` (the rendered
  files every `FerbSound` needs, Standard and Subtle, plus `ferb-room-ir.wav`, copied from `render.py`'s `out/` and
  committed; lossless; listed in Package.swift resources)
- Modify: `S/NotchModel.swift` (an intro presentation mode: `introLine: IntroLine?`), `S/NotchView.swift` (render it),
  `ui/mac/FERBNotch/Package.swift` (the Sounds resources)
- Test: `T/IntroNarrationTests.swift` (model state only; no rendering tests)

**Interfaces:**
- Consumes: `OrbMotion`, `OrbOrigin`, `SpringSpec`, `SoundPolicy`, `FerbSound` (Task 5); `SettingsAnchor` (Task 6).
- Produces: `final class OrbController { func launch(to target: CGPoint, instruction: IntroLine?) async; func dockBesideSettings(dragging url: URL); func hop(); func flyHome() async; func tearDown() }`;
  `public struct IntroLine: Equatable { var text: String; var detail: String? }`;
  `final class FerbSoundEngine { func play(_ s: FerbSound, at point: CGPoint?) ; func stopAll() }`.

- [ ] **Step 1: Write the failing tests** in `T/IntroNarrationTests.swift`: setting `introLine` expands the notch and
  shows it; clearing it collapses; `tearDown` during a flight leaves no orb panel visible and calls `stopAll()` (fake engine);
  `testEverySoundHasItsFiles`: each `FerbSound` case resolves to at least one bundled Standard and one Subtle file.
- [ ] **Step 2: Run** → FAIL.
- [ ] **Step 3: Implement** the files above. Reduce Motion: `OrbController.launch` fades the orb in beside its target.
- [ ] **Step 4: Run** → PASS; `swift build`.
- [ ] **Step 5: Commit** `git commit -m "Intro: the orb, notch narration and one sound engine"`.

### Task 8: IntroController drives the eight beats (FERBNotch)

**Files:**
- Create: `S/IntroController.swift`
- Modify: `S/SetupModel.swift` (expose the per-row grant step used by `allowAll` as `grantAndConfirm(_ row:) async -> Bool`, polling 1.5 s), `T/Support/SetupFakes.swift` (fake orb, fake sound engine, fake shortcut feed)
- Test: `T/IntroControllerTests.swift`

**Interfaces:**
- Consumes: Tasks 1–7 (after Task 3, hearing is the dictation row's grant); `SetupModel` actions
  (`continueFromWelcome`, `signInWithChatGPT`, `signInWithClaude`, `saveAPIKey`, `grantAndConfirm`, `pairAll`, `useIdentity`, `refreshPersona`), RPCs `intro.suggest`, `intro.preview`.
- Produces: `@MainActor final class IntroController { init(setup: SetupModel, orb: OrbController, sound: FerbSoundEngine, notch: NotchModel, progress: IntroProgress); func start(beats: [IntroBeat]); func skip(); var onFinished: (() -> Void)? }`

- [ ] **Step 1: Write the failing tests** (SetupRig + ScriptedRPC fakes):
  - `testMeetInstallsTheServiceAndGuidesLoginItemsOnlyWhenAsked`: `serviceNeedsApproval` true → one orb trip to Login Items before sign-in; false → none.
  - `testFirstLaunchClipPlaysOnceThenSilenceThenTheLogo`: a new install plays `.firstLaunch` once at start; `.intro` plays
    only after the clip ended and Meet is ready, never during it; a replay or resume plays no clip.
  - `testPairAllRunsOnceAndIdentityCanBeSkipped`: browsers beat sends `setup.browsers_pair_all` once; identity "Later" advances with no `identity.*` call.
  - `testTheScanLineIsShownBeforeTheSuggestion`: persona `seen(summary)` → the notch shows it, then the suggestion.
  - `testSignInStartsTheScanInTheBackground`: after a successful sign-in, `setup.persona {}` is sent before the permissions beat starts.
  - `testEachListPermissionFliesTheOrbAndWaitsForTheSystem`: accessibility row fake-granted only on the 3rd poll → beat advances after it; the orb docked once.
  - `testADeniedPromptKeepsTheBeatOpen`: microphone denied → beat stays, line offers the Settings pane.
  - `testTryShowsThePreviewAndActsNever`: tryIt sends `intro.suggest` then `intro.preview`; preview parts land in the transcript marked preview; no `chat.send`.
  - `testDoItForRealStartsTheTaskAfterHome`: choosing it sends `chat.send {text: suggestion}` only after `home`.
  - `testControlBeatWaitsForEachShortcut`: fake double-tap right ⌥, hold right ⌘+⌥, ⌃⌥Space, stop on the replayed preview, and Pause FERB from the menu → advances once each was seen (pause is lifted at once).
  - `testSkipMidFlightTearsEverythingDown`: skip during an orb flight → `tearDown` and `stopAll` called; compact path lists only required steps left.
  - `testQuitAndReopenResumesTheBeat`: `IntroProgress.resumeBeat` = `.permissions` → `start` resumes there.
- [ ] **Step 2: Run** → FAIL.
- [ ] **Step 3: Implement** the controller (copy per beat from notes/setup-intro.md; FERB's lines via `IntroLine`).
- [ ] **Step 4: Run** → PASS; `swift test` (whole suite).
- [ ] **Step 5: Commit** `git commit -m "Intro: the eight beats, guided grants, preview demo, controls, skip and resume"`.

### Task 9: Launch, replay and notes (FERBNotch + notes)

**Files:**
- Modify: `S/AppSetupController.swift` (`startAtLaunch` ~50: `.window(.welcome)` and missing grants → `IntroController`
  with `IntroPlan.beats`; skip → the existing `SetupView` window on the remaining required step), `C/AppSetup.swift`
  (`atLaunch` returns `.intro([IntroBeat])` where it returned `.window`), `S/SettingsPages.swift` (General → "Replay intro"),
  `C/MenuBarState.swift` ("Set Up FERB…" → replay), notes/setup-intro.md (status: built), notes/test-tasks.md (sitting 1 lines below)
- Test: `T/AppSetupTests.swift`

- [ ] **Step 1: Write the failing tests**: `testNewInstallLaunchesTheIntro`, `testFinishedSetupLaunchesNothing`,
  `testMissingGrantLaunchesOnlyThatBeat`, `testReplayFromSettingsRunsEveryBeat`.
- [ ] **Step 2: Run** → FAIL. **Step 3: Implement.** **Step 4: Run** `swift test` → PASS; `nice -n 10 uv run pytest -q`.
- [ ] **Step 5: Notes**: add to notes/test-tasks.md sitting 1: "Fresh install: the intro runs all eight beats; quit at
  Screen Recording and reopen → same beat; skip mid-flight → compact path; Reduce Motion → orb fades in place; sound off
  → everything readable; notchless Mac → virtual notch; Replay intro from Settings."
- [ ] **Step 6: Commit** `git commit -m "Intro: launch for new installs, Replay intro, step-13 checks"`.

---

### Task 10: Everyday sounds: the family replaces today's system sounds (FERBNotch)

Owner, 2026-11-01: the decided family goes into this plan; big events only; the magnetic docking sound is removed.

**Files:**
- Modify: `S/SoundFeedback.swift` (its `Cue`s become `FerbSound`s played through `FerbSoundEngine`; per-sound on/off
  with the defaults below), `S/NotchModel.swift` (~549, 591, 638, 823), `S/DaemonConnection.swift` (~359, 381, 741-742:
  done → `.done`; a task that failed → `.error`; a question, approval or other wait on the owner → `.needsYou`;
  `receiveNotices` plays `.notification`), `S/NotificationPoster.swift` (banners use
  `UNNotificationSound(named:)` with the bundled notification file, not `.default`), `S/SettingsPages.swift` (one switch
  per everyday sound and the Standard/Subtle choice; the "Magnetic docking sound" switch goes), the `About` or
  acknowledgements view (the CC BY 4.0 credit from `demo/sound-family/CREDITS.md`)
- Delete: `S/MagneticDockingAudio.swift`, the `FERBMagneticSynth*` C sources and their wiring, `feedback.magneticEnabled`
- Test: `T/SoundFeedbackTests.swift` (or the nearest existing suite)

**Rules:**
- Defaults: on for needs you, error, done, notification, listening start and end, and the tour; off for summon and
  paused. Every one can be switched; `feedback.enabled` and `feedback.volume` keep working as the master switch and
  volume (existing values kept on upgrade).
- A notice shown both as a banner and in the notch sounds once (the banner carries it; the notch stays silent for it).
- During a call FERB stays quiet (owner, 2026-11-01): while a process other than FERB's own runs microphone input
  (Core Audio process objects, `kAudioProcessPropertyIsRunningInput`, macOS 14+), nothing plays except `.needsYou`, at
  Subtle. FERB's own dictation never counts as a call. No Focus, quiet-hours or screen-sharing rule.

- [ ] **Step 1: Write the failing tests**: `testAFailedTaskPlaysErrorNotNeedsYou`, `testAQuestionPlaysNeedsYou`,
  `testSummonAndPausedAreSilentByDefault`, `testANoticeShownAsBannerAndInTheNotchSoundsOnce`,
  `testTurningASoundOffSilencesOnlyThatSound`, `testDuringACallOnlyNeedsYouPlaysAtSubtle` (a fake mic-in-use
  feed: another app → only needs you, Subtle; FERB's own dictation → normal).
- [ ] **Step 2: Run** → FAIL. **Step 3: Implement**, deleting the magnetic sound and its setting.
- [ ] **Step 4: Run** `swift build`, `swift test` (scratch path) → PASS.
- [ ] **Step 5: Commit** `git commit -m "Sound: FERB's family replaces the system sounds; one notification cue; magnetic sound removed"`.
