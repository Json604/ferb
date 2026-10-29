# Magnetic trackpad haptics

## Goal
Owner approved 2026-10-30: replace the magnetic docking sound with a subtle
continuous trackpad vibration, native-tap fallback, then build/reload to try.
Keep work on ui/ux. Owner evaluates feel; do not audition haptics or inspect UI.

## Architecture
Replace PanelCoordinator's magnetic audio output with a small haptic controller
consuming the existing MagneticDocking.Sample and lifecycle cancellation hooks.
Runtime-load optional MultitouchSupport actuator and parametric waveform symbols;
use NSHapticFeedbackManager on unavailable hardware/framework or driver failure.
No microphone, audio output, new permission, or change to drag/fusion eligibility.
Keep the current field/physics. Ordinary completion/dictation sounds stay as-is.

## Spec
- Continuous low-amplitude resonance rises with proximity, tightens during fusion,
  recedes on retreat and resolves to one native alignment tap at completion.
- Render short whole-cycle segments on a serialized worker, with cancellation
  ownership checked before each segment. No indefinite device waveform or queued
  backlog; stop scheduling immediately on cancellation. Device resources must
  outlive in-flight calls and be released serially afterward.
- Public fallback uses sparse proximity-driven native taps and a final alignment
  tap. Do not claim the fallback is a true continuous motor waveform.
- Independent persisted “Magnetic docking haptics” toggle in Notch settings,
  enabled by default. Haptics do not depend on speaker volume or sound mute.
- Missing symbols, absent/disconnected actuator and waveform/play errors must
  leave dragging usable and fall back. Reprobe on a new interaction after failure.
- Re-grab, exit, explicit close, display changes, sleep and teardown stop output;
  stale samples/events cannot replay a final tap. No haptic output in tests.
- Implement fresh native code; retain MIT attribution where the public Tactile
  implementation informs private API declarations/waveform structure.

## Global Constraints
Owner's “ok do it, i wanna try” approves this follow-up and build/reload. Preserve
existing native geometry and reduced-motion behavior. One notch instance only;
do not restart daemon. Leave unrelated notes/ alone. Continue approved ui/ux
commit/push scope. No screenshots, sound/haptic audition or live automated drags.

## Review Focus
Private API ABI/lifetime, finite waveform scheduling, cancellation races, fallback
on play failure, completion exactly once, no magnetic sound and no test hardware
output. Distinguish code checks from owner's actual sensation.

## Task 1 — Haptic replacement and owner candidate
Files: new MagneticDockingHaptics.swift and native actuator adapter; coordinator,
SoundFeedback/preferences and SettingsPages; focused haptic tests; architecture,
changelog, owner acceptance and third-party notices as needed.
Interface: injectable haptic output reports continuous capability, accepts current
intensity/frequency and stop; public tap performer also injectable. Shared sample
is the only proximity/fusion source.
1. Failing tests for approach/retreat, completion once, mute toggle mid-effect,
   cancellation/stale output, missing/failed actuator fallback and resource lifetime.
2. Implement native controller/driver and replace magnetic sound invocation.
3. Run focused/full Swift using anonymous SwiftPM; mypy and diff check. Reuse
   unchanged full Python baseline (5506 passed) from preceding UI work.
4. Fresh read-only review and fix wave; build bundle; commit with L34 and push
   ui/ux. Restart only exact running development-notch process and launch bundle.
5. Owner tries actual drag on trackpad; record verdict separately when supplied.

## Self-Review
This is the approved haptic replacement, using the same sampled physics and
cleanup rather than a separate proximity effect. Private actuator compatibility
and tactile feel remain unverified until owner use; native fallback is explicit.
