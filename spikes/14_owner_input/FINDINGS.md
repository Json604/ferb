# Spike 14 — Owner-input detection and GUI yield

Status: done (2026-09-28). 6/6 pass, 3 runs in a row. macOS 26.6.2.

Run: `uv run --python 3.12 --no-project --with pyobjc-framework-Quartz --with pytest pytest -q -s test_spike.py`
(~30 s; skips timing tests if the owner is actively using the Mac). Only harmless synthetic events: mouse-moved to
the pointer's current position. The tap records event type + timestamps only — never key codes or positions.

## Results

- **Detection path: listen-only `CGEventTap` + a private marker.** FERB's own synthetic events carry
  `kCGEventSourceUserData = "FERB"`; anything without it is the owner. 50/50 FERB events classified as FERB,
  none as owner.
- **The system idle timer can't be used:** `CGEventSourceSecondsSinceLastEventType(HIDSystemState, any)` is
  reset by FERB's own synthetic input (1.2 s → 0.05 s), so it would mistake FERB's actions for the owner.
- **Detection latency** (post → tap callback, synthetic owner events, 40 samples × 3 runs): median ~0.6 ms,
  p90 ~1.0 ms, max ~5 ms.
- **Yield latency is set by the action's step size**, since the action checks before every step: 20 ms steps →
  1–25 ms; 100 ms steps → 5–108 ms; never more than the one step already in flight after the owner's input.
- **Resume:** GUI work continues only after the owner has been quiet for the whole quiet period (1.5 s here).

## Requirements for build step 7 (`owner_input` watcher, `gui_lock`, `act/mac/input.py`)

- Every real input event FERB posts is marked; the watcher is a listen-only tap classifying by the marker.
- GUI actions are made of small steps (one keystroke / one mouse event) and check `owner_active()` before each;
  long text is typed per keystroke, not pasted in one event, so it can stop mid-way. A yielded action reports
  where it stopped (the ledger decides whether the partial effect is `unknown`).
- The tap needs Accessibility (worked here, via the terminal's grant) or Input Monitoring — for the daemon this is
  the dedicated interpreter's grant (section 8 step 6). If the tap is disabled by timeout/user input, re-enable it
  (handled in the callback).

## Real hardware (owner moved the mouse, 2026-09-28)

10 s of real input: 1,150 events (1,062 mouse-moved, 88 scroll). Event timestamp → tap callback: median 0.33 ms,
p90 0.45 ms, p99 0.72 ms, max 1.56 ms — faster than the synthetic path. 0 real events classified as FERB's.
