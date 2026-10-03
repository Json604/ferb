# 0.22.1 — background click routing and coordinate correction

## Observed failure

The owner reported repeated desktop/focus shifts and cursor movement during the scaler Gmail assignment task.
The local task trace showed 12 `mac.click` calls, six with `allow_foreground=true` (one rejected as stale), three
`mac.act` calls and no `mac.pointer` calls. Chrome's scaler extension announced protocol 2 while the daemon needed
protocol 5. The unpacked extension points at this repository; its running service worker needs reloading.

The native fallback's `click_at` used AX and a legacy same-Space PID click, then told the agent that nothing else
could be done in the background and to retry in the foreground. It never used the newer window-targeted route.
Foreground delivery activates the app, may switch Spaces, and moves the shared cursor. Restoring focus afterwards
does not make that background input.

## Fix

- `mac.click` uses AX first, then the same screenshot-bound native transport as `mac.pointer`, including other
  Spaces when the transport is available. Window ownership and geometry are checked before dispatch.
- A dispatched click with no AX change is returned for inspection, never automatically replayed through the
  foreground. A successful event post is not proof of task success. The screenshot is consumed by pointer input.
- Native CGEvents retain screen coordinates, while the window-location field receives **window-local** coordinates.
  This applies to clicks, hover, drag and wheel input. Negative window origins are covered by regressions.
- Remove misleading fallback advice that background work is impossible. Tool descriptions and the worker prompt
  keep background tasks in background mode; agents still choose approaches. No new intent/permission gate.
- `ferb trace` shows `allow_foreground` so focus-taking choices are visible.
- Fix the disposable Chrome harness's close-on-exec bug when a pipe already occupies fd 3 or fd 4.

No new cursor overlay is needed for delivery. No global cursor warp, focus-transfer recipe, second event post or
desktop-switch operation was added. The existing explicit foreground option still describes a different delivery
mode; the agent is instructed not to choose it merely because a background method failed.

## Research, compared with implementation and measurements

| Primary source | What it establishes and how it affects this fix |
|---|---|
| [Apple Quartz cursor control](https://developer.apple.com/library/archive/documentation/GraphicsImaging/Conceptual/QuartzDisplayServicesConceptual/Articles/MouseCursor.html) | Hiding, warping or disassociating the shared pointer changes cursor behavior; it does not create an independent target-window input channel. Those techniques are not used. |
| [Apple CGEvent](https://developer.apple.com/documentation/coregraphics/cgevent) | Global event posting and process-directed posting are different routes. Neither an event-post return nor documentation alone establishes that a particular application accepted it. |
| [Chrome debugger transport](https://developer.chrome.com/docs/extensions/reference/api/debugger) | Browser commands can target a tab by `tabId`. FERB already uses this path for browser gestures; the stale scaler extension prevented it in the observed task. |
| [Chrome tabs](https://developer.chrome.com/docs/extensions/reference/api/tabs) and [windows](https://developer.chrome.com/docs/extensions/reference/api/windows) | Tab activation and native-window focus are separate properties. Task tabs should remain inactive and their windows unfocused; no reason to activate them merely to dispatch pointer input. |
| [Cua native mouse source](https://github.com/trycua/cua/blob/67c2f39af41edc832642178868c567a1afd8d240/libs/cua-driver/rust/crates/platform-macos/src/input/mouse.rs) and [SkyLight source](https://github.com/trycua/cua/blob/67c2f39af41edc832642178868c567a1afd8d240/libs/cua-driver/rust/crates/platform-macos/src/input/skylight.rs) | Some paths transfer focus without raising the window; some post through two transports. Neither is appropriate evidence of undisturbed concurrent work. Coordinate comments disagree, so FERB's receiving-app measurement determines the conversion. Existing attribution remains in THIRD_PARTY_NOTICES.md. |
| [Cua cursor overlay](https://github.com/trycua/cua/blob/67c2f39af41edc832642178868c567a1afd8d240/libs/cua-driver/rust/crates/platform-macos/src/cursor/overlay.rs) | The visible agent cursor is a transparent, click-through window. It visualizes activity; input delivery is separate. This does not establish how ChatGPT's dots are implemented. |
| [Peekaboo background input](https://github.com/openclaw/Peekaboo/blob/ed7f04d8ee773ef1dd4c40b519b41b731eea6710/Core/PeekabooAutomationKit/Sources/PeekabooAutomationKit/Services/UI/BackgroundInputDriver.swift) | Background AX and process/window input are distinct from shared-pointer input. Its implementation was reviewed, not installed or treated as a universal guarantee. |

## Live evidence and acceptance

An owner-requested single native click used a disposable Chrome profile, exact spawned PID and a canvas with an
event listener. The branch driver ran directly; the installed daemon and personal profiles were not changed.

The first check delivered one trusted canvas event. Cursor samples were identical; front PID was unchanged and
the front-app detector recorded no transitions. It exposed a coordinate error: intended canvas `(20,30)` became
`(42,85)` in a window at `(22,55)`. Passing screen coordinates as the window location applied the window origin
twice. The correction now supplies local `(screen_x-window_x, screen_y-window_y)` to the window route.

The owner confirmed the first check: “No disturbance—I kept working normally.” The corrected repeat delivered
exactly one trusted event at `(20,30)` in a window **on another Space**. The cursor samples matched, the front PID
was unchanged and the detector recorded no transitions. The real test passed in 1.98 s. The owner's second verdict
is pending; these readings do not replace that keyboard-focus/Space check. Broader app/gesture acceptance remains
separate from the Chrome click.

Reproducible owner-supervised check (one click, safe disposable Chrome, closes only its own process):

```sh
.venv/bin/python -m pytest -m real tests/test_mac_pointer_real.py -q -s
```

The test checks exactly one trusted event at `(20,30)`, records cursor/front-app samples and whether the target
was on the current Space, and checks that the lab PID never became frontmost. The owner must independently confirm
continued typing, pointer control and no desktop disturbance. Pause after each check for that answer.

## Task trace findings beyond pointer delivery

The task read the Gmail/form flow and downloaded the assignment PDF. It initially confused “Astra High” with a
project name. Its first Codex process actually used `gpt-6-astra` with `xhigh`, stalled, and was stopped. FERB then
generated the deliverable itself and ran another Codex process to verify it. The one-page PDF exists and nothing
was submitted. Thus there is real output, but the requested exact model-setting/delegation workflow was not fully
followed. The final checker could not reread Gmail through the stale extension. Those are remaining orchestration
acceptance findings, not claims that this pointer patch fixes them.

## Rollout

Install/restart only after the owner authorizes it. Reload FERB in `chrome://extensions` **in the scaler profile**
to load protocol 5, then check `ferb browsers` for a connected scaler profile before retrying the task. Do not switch
to an unrelated personal profile to hide the mismatch. No extension wire-protocol change is made in 0.22.1.

Owner-run update from this checkout (restarts FERB; finish or stop running work first):

```sh
cd /Users/kartikey/Desktop/FERB
.venv/bin/ferb setup --update
```

After reloading the scaler extension, use `.venv/bin/ferb browsers` to confirm it connects, then
`.venv/bin/ferb chat` to repeat the assignment task. Specify Codex model `gpt-6-astra`, reasoning effort `high`,
preserve existing output, and verify completion without submitting it. Observe focus, cursor and Space behavior
while continuing normal work. The daemon has not been updated by the pointer checks.
