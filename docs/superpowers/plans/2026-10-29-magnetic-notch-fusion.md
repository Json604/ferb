# Magnetic notch fusion

## Goal

Replace the current swallow transition with restrained, physically continuous
magnetic recognition, attraction and fusion, with original proximity-controlled
audio. Implement this in FERB's real native drag interaction, not a demo.
Owner request: 2026-10-29. Work exclusively on `ui/ux`; do not merge to main.
Status: owner approved 2026-10-29, with directional edge tension added; implementation underway.

## Architecture

Current code inspected:
- `FerbIndicator.swift:162–179`: global-pointer mouse-down/drag/up callbacks.
- `PanelCoordinator.swift:485–543`: tear-out and pointer-based placement; expanded
  chat uses `LiveDragLayout` and `DragChatMotion`.
- `PanelCoordinator.swift:545–595`: display-clock sampling, stable drag canvas,
  then compacting the native event surface at release.
- `PanelCoordinator.swift:597–695`: physical-notch release test, persisted docking,
  separate 240 ms layer scale/window translation/fade and final presentation reset.
- `NotchDocking.swift:14–20`: hardware-only drop target including lower pull strip.
- `NotchPresentation.swift:6–100`: shared native/SwiftUI presentation and spring clock.
- `SoundFeedback.swift:15–37`, `SettingsPages.swift:94–98`: persisted master sound
  enable/volume and system cue playback. No continuous synthesizer in this path.

Add a deterministic magnetic state model and a native coordinator-owned renderer
and synthesizer. Every visual and audio parameter consumes one timestamped sample
of that state. Pointer input remains authoritative; magnetic displacement is a
separate reversible physical offset, never cumulative modification of the grab
anchor. Keep the existing chat orientation spring independent of this offset.
Replace the old swallow's independent layer/window animations with shared sampled
fusion geometry; native frost, tint, controls and energy must agree each frame.

Prior evidence: `.superpowers/sdd/2026-10-29-ui-motion-drag-bubbles/progress.md`
Tasks 7–8 record an earlier generated entry cue and magnetic feedback, then owner
withdrawal pending this walkthrough. Do not restore the entry cue or enlarged
invisible drop target. That ledger reports offline tests/builds, not accepted sound.
Task 23 owner acceptance in `docs/OWNER_ACCEPTANCE.md` establishes drag text and
hover stability as regressions to protect. No new live result is claimed here.

## Spec

1. Use actual hardware-cutout geometry on the pointer's display. Measure distance
   from the unmagnetized pill rectangle to the reachable notch target; deriving
   influence from the attracted rectangle would create positive feedback.
   Start influence at 150 pt and strengthen near 40 pt (owner-suggested ranges).
   Smoothstep normalized distance drives field strength without an entry jump.
2. While held, retain direct pointer translation and add a small critically damped
   attraction offset toward the nearest target point. Carry offset velocity through
   reversal. Strong capture describes increased attraction, not an expanded release
   hit region or automatic docking while the button is held. Releasing still uses
   the existing physical-notch/pull-strip test.
3. Concentrate faint desaturated energy on the pill edge facing the notch, with
   a matching response in the drawable area beneath the notch. Subtle ripples or
   distortion suggest tension between these surfaces; that tension tightens as the
   gap closes and collapses into the join. Oscillation frequency increases
   continuously with proximity. No white outlines, particles, lightning or neon.
   Use native layers with enough drawing padding; no new interactive overlay area.
4. In the final approach, deform the surface slightly toward the target. Keep text
   legible and the joined pill/chat silhouette intact. Do not scale only the logo.
   Start with approximately 1–2% directional strain and tune with the owner.
5. Release on the existing target begins a damped fusion from the actually drawn
   geometry and velocity, aiming for 200 ms perceptual duration. Compress the field
   inward as geometry converges. Commit docked state once, at exact alignment.
   Expanded chat transitions into the existing docked presentation without a
   transparent/reset frame or a second independent entrance animation.
6. Original output-only synthesis: faint low hum plus softly frequency-modulated
   resonance, with frequency, modulation depth and amplitude following proximity.
   A short filtered descending resonance and soft transient resolve at actual
   fusion completion. No system notification sample or entry-triggered recording.
   Stereo positioning follows horizontal target direction subtly; no microphone
   access, recording permissions, network assets or new dependency.
7. Smooth audio parameters at sample rate; preserve oscillator phase between
   updates. Retreat reduces resonance continuously. The fusion tail ends in exact
   silence; exit, cancellation and teardown cannot leave a drone. Audio failure
   cannot prevent drag or docking. Render callbacks must not access AppKit,
   allocate, or wait on the main thread; review parameter transfer for races.
8. Add “Magnetic docking sound” in Settings → Sounds, enabled by default but
   subordinate to existing feedback mute and volume. Toggling either control takes
   effect during the current interaction, with a brief de-click ramp.
9. Reduce Motion removes spatial attraction, deformation, oscillation and traveling
   fusion; retain a static proximity affordance and immediate exact docking.
   Audio continues to follow proximity unless muted. Observe preference changes
   during an interaction, not only when a drag begins.
10. Rapid reversal, re-grab during fusion, tear-out, display changes/removal, sleep,
    explicit close and shutdown cancel safely. A stale completion cannot persist
    docking, play a completion transient or overwrite a newer drag. Tear-out is
    initially unarmed until the pill leaves the influence zone, avoiding immediate
    pull-back. No-notch screens retain their existing behavior.

## Global Constraints

- Owner explicitly selects `ui/ux`, overriding the supplied main-only rule for
  this work. No main checkout, merge or push.
- Follow L34 (native app), D17 (real acceptance), P27 (owner product decisions).
- Preserve camera avoidance, four-edge placement, pixel-aligned transcript width,
  focus, pin/hover state, current conversation, preferred sizes and persistence.
- Keep native frost/tint/content synchronized; do not flatten the glass through
  SwiftUI opacity or independently animate its mask.
- No daemon restart/install, personal-browser changes or release replacement.
  Never launch a development notch alongside the release notch.
- Native appearance and sound evaluation remain owner-led under `ui/AGENTS.md`.
  Automated tests exercise production views/controllers without unsolicited
  screenshots or screen-control loops. Offline success is not live acceptance.
- One fresh implementer per task and one fresh read-only reviewer, sequentially;
  fix Critical/Important findings before integration. Record dispatches and rulings
  in `.superpowers/sdd/2026-10-29-magnetic-notch-fusion/progress.md`.

## Review Focus

Continuous reversal and capture; geometry/velocity at interruption; stable native
canvas and text; shared audio/visual state; real-time audio safety; exact silence;
hardware-only targeting; completion ownership; reduced motion; resource cleanup.

## Task 1 — Magnetic state and native interaction

Files: new `Sources/FERBNotch/MagneticDocking.swift`, `PanelCoordinator.swift`,
`NotchDocking.swift`, `LiveDragLayout.swift` only where required; new
`Tests/NotchCoreTests/MagneticDockingTests.swift` and existing docking/drag tests.

Interface: a value state accepts raw pill/target geometry, pointer velocity,
monotonic time, drag/release/cancel and reduced-motion input. It emits proximity,
offset/velocity, strain, fusion progress, phase and a one-time completion event.
Inject time and geometry for tests; keep NSScreen and persistence in coordinator.

First failing tests: continuous approach/retreat; reversal without resetting
velocity; no force on external displays; unchanged release hit test; tear-out
arming; stale completion rejection; exact final pose; reduced-motion updates.
Then integrate state with actual compact and expanded drag paths. Test native
canvas compaction, preservation of pointer anchor and text geometry at 1x/2x.
Replace swallow animation ownership rather than running both clocks concurrently.
Commit: `feat(L34): drive notch docking with continuous magnetic state`.

## Task 2 — Shared field rendering and original synthesis

Files: new `MagneticFieldView.swift`, `MagneticDockingAudio.swift`; production
coordinator, backdrop/presentation and indicator integration; `SoundFeedback.swift`,
`SettingsPages.swift`; new renderer/audio tests in `NotchCoreTests`.

Interface: renderer and synthesizer consume Task 1 samples; neither independently
decides proximity or completion. Synthesizer owns output engine lifetime and exposes
deterministic PCM rendering for tests plus safe continuous parameter updates.

First failing tests: connected silhouette bounds and reduced-motion output; audio
gain/pitch progression, retreat, finite PCM, phase continuity, completion once,
mute/volume changes during playback and exact silence after exit/cancel. Verify
native production wiring, not just helper math. Then implement energy response,
subtle strain, shared fusion geometry, output-only synthesis and persisted toggle.
Test engine-start failure and route-change recovery without replaying completion.
Commit: `feat(L34): synchronize magnetic field and docking audio`.

## Task 3 — Lifecycle integration and acceptance preparation

Files: coordinator lifecycle hooks, relevant native integration tests,
`ARCHITECTURE.md`, `CHANGELOG.md`, `docs/OWNER_ACCEPTANCE.md`.

First failing tests: interruption/re-grab at multiple fusion phases; sleep/display
removal/close while resonating; reduced-motion toggle mid-fusion; screen crossing;
no stale persistence, effects, display links or sound after cancellation. Exercise
the real native indicator callbacks and compact/expanded/pinned presentation.
Implement any missing cleanup and document the resulting mechanism accurately.
Commit: `test(L34, D17): cover magnetic docking interruptions and acceptance`.

For each task: focused tests, full `swift test` after sourcing
`scripts/swiftpm-anonymous.sh`, `nice -n 10 uv run pytest -q`,
`uv run mypy ferb/core`, `git diff --check`. Never overlap full Python suites.
Record actual results, then fresh review and fixes before the next task. After all
tasks, whole-change review on the most capable available model and one fix wave.
Build the native app with anonymous SwiftPM configuration; arrange owner-led
activation without running two notch apps or replacing the release daemon.

Owner live acceptance: slowly approach from below and both sides; hover at several
distances; retreat; rush past; release outside/inside the physical target; interrupt
fusion; tear out; repeat with open/pinned chat and long scrolled transcript; cross
to a no-notch display; mute and change volume during resonance; toggle Reduce
Motion; sleep/wake. Confirm alignment, restrained energy, sound synchronization,
silence, no pointer jumps/text reflow and no focus theft. Record verdict separately
from automated results. Tune motion and sound from the owner's observed interaction.

## Self-Review

The user has specified the new visual/audio direction and branch. The prior
withdrawal is superseded by this request, not silently ignored. This plan preserves
drop eligibility while introducing the requested reversible magnetic force; 150/40
pt and 200 ms are tuning starts, not new product limits. Native output synthesis
and one shared state remove the old entry-cue synchronization problem. Remaining
subjective questions are resolved through owner-led real interaction and listening.
Owner approved this plan and explicitly instructed implementation on ui/ux only.
