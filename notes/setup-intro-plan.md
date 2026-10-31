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
- All eight permissions stay required (`SetupModel.askedAtSetup`); Automation stays per app at first use.
- A grant counts only when the system confirms it; never inferred from a drop. Poll every 1.5 s during a grant.
- The demo never acts: the preview run issues no `RunGrant`, runs no worker, and emits only plan/part events marked
  `preview: true`.
- FERB's intro lines are text in the notch, never spoken; sound comes only from the sound interface (phase 1: neutral
  placeholders; the family arrives after the audition, a later plan).
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
  never floats over another app (Task 5 test).
- Sign-in cancelled or failing (no network): the scan never starts; the Try beat uses the general task (Task 3 test).
- A prompt denied or a grant revoked mid-tour: the beat stays open with a plain way forward; finishing reports what's
  missing and setup returns next launch (Task 7 test).
- Skip pressed mid-flight: the orb, its sound and the drag card are torn down at once; the compact path shows only the
  required steps left (Task 7 test).
- Notchless Mac or FERB on a second display: the orb launches from the virtual notch on FERB's screen (Task 4 test).

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

### Task 3: Suggested task and plan-only preview (daemon)

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

### Task 4: Orb motion, spatial mapping and sound policy (NotchCore)

**Files:**
- Create: `C/OrbMotion.swift`, `C/SoundPolicy.swift`
- Test: `T/OrbMotionTests.swift`, `T/SoundPolicyTests.swift`

**Interfaces:**
- Produces:
  - `public struct SpringSpec { var duration: Double; var bounce: Double; static let open = SpringSpec(duration: 0.45, bounce: 0.2); static let close = SpringSpec(duration: 0.4, bounce: 0) }`
  - `public struct OrbMotion { init(origin: CGPoint); mutating func retarget(_ p: CGPoint, spec: SpringSpec); mutating func step(dt: Double) -> CGPoint; var velocity: CGVector { get }; var settled: Bool { get } }`
  - `public enum OrbOrigin { static func notchCentre(screenFrame: CGRect, cutout: CGRect?) -> CGPoint }` (virtual notch when `cutout` is nil, matching `NotchGeometry`'s 164-wide default).
  - `public enum FerbSound: CaseIterable { case intro, summon, listeningStart, listeningEnd, done, needsYou, paused, error, orbDrip, orbLand, orbMelt }`
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

### Task 5: Docking beside System Settings (NotchCore + FERBNotch)

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

### Task 6: The orb, the notch narration and the sound engine (FERBNotch)

**Files:**
- Create: `S/OrbPanel.swift` (NSPanel, level `.mainMenu + 2`, `overlayBehavior`, click-through except the drag card),
  `S/OrbView.swift` (glass orb with the iridescent band; ripple while waiting; the frosted instruction bubble; the drag
  card holding an `NSDraggingSource` for the agent app's bundle URL, `PermissionSnapshot.grantTarget`),
  `S/IntroNarration.swift` (notch states: line + detail, swell/shrink using `NotchPresentation`), `S/FerbSoundEngine.swift`
  (AVAudioEngine; mono sources, `AVAudioEnvironmentNode` with `.headphones` → HRTFHQ else equal-power pan from
  `SoundPolicy`; neutral placeholder buffers; reads `com.apple.sound.uiaudio.enabled` and `feedback.*`)
- Modify: `S/NotchModel.swift` (an intro presentation mode: `introLine: IntroLine?`), `S/NotchView.swift` (render it),
  `S/SoundFeedback.swift` (route through `FerbSoundEngine`, same cues as today in phase 1)
- Test: `T/IntroNarrationTests.swift` (model state only; no rendering tests)

**Interfaces:**
- Consumes: `OrbMotion`, `OrbOrigin`, `SpringSpec`, `SoundPolicy`, `FerbSound` (Task 4); `SettingsAnchor` (Task 5).
- Produces: `final class OrbController { func launch(to target: CGPoint, instruction: IntroLine?) async; func dockBesideSettings(dragging url: URL); func hop(); func flyHome() async; func tearDown() }`;
  `public struct IntroLine: Equatable { var text: String; var detail: String? }`;
  `final class FerbSoundEngine { func play(_ s: FerbSound, at point: CGPoint?) ; func stopAll() }`.

- [ ] **Step 1: Write the failing tests** in `T/IntroNarrationTests.swift`: setting `introLine` expands the notch and
  shows it; clearing it collapses; `tearDown` during a flight leaves no orb panel visible and calls `stopAll()` (fake engine).
- [ ] **Step 2: Run** → FAIL.
- [ ] **Step 3: Implement** the files above. Reduce Motion: `OrbController.launch` fades the orb in beside its target.
- [ ] **Step 4: Run** → PASS; `swift build`.
- [ ] **Step 5: Commit** `git commit -m "Intro: the orb, notch narration and one sound engine"`.

### Task 7: IntroController drives the eight beats (FERBNotch)

**Files:**
- Create: `S/IntroController.swift`
- Modify: `S/SetupModel.swift` (expose the per-row grant step used by `allowAll` as `grantAndConfirm(_ row:) async -> Bool`, polling 1.5 s), `T/Support/SetupFakes.swift` (fake orb, fake sound engine, fake shortcut feed)
- Test: `T/IntroControllerTests.swift`

**Interfaces:**
- Consumes: Tasks 1–6; `SetupModel` actions (`continueFromWelcome`, `signInWithChatGPT`, `signInWithClaude`,
  `saveAPIKey`, `grantAndConfirm`, `pairAll`, `useIdentity`, `refreshPersona`), RPCs `intro.suggest`, `intro.preview`.
- Produces: `@MainActor final class IntroController { init(setup: SetupModel, orb: OrbController, sound: FerbSoundEngine, notch: NotchModel, progress: IntroProgress); func start(beats: [IntroBeat]); func skip(); var onFinished: (() -> Void)? }`

- [ ] **Step 1: Write the failing tests** (SetupRig + ScriptedRPC fakes):
  - `testMeetInstallsTheServiceAndGuidesLoginItemsOnlyWhenAsked`: `serviceNeedsApproval` true → one orb trip to Login Items before sign-in; false → none.
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

### Task 8: Launch, replay and notes (FERBNotch + notes)

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

Phase 2 (a separate plan after the sound audition): the chosen family replaces the placeholders and today's system
sounds, with the context rules (owner thresholds) and per-sound settings, per notes/sound.md.
