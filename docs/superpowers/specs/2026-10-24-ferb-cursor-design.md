# FERB's cursor — design (2026-10-24)

Status: design approved by the owner on 2026-10-24 ("yeah works"); this written design awaits the owner's review
before an implementation plan. Decision id: L33. Built after loops (L31) and the 2026-10-24 clarity fixes, which touch
the same gateway and app files.

## Why

The owner wants FERB to do things in the background and show what it's doing (audit list, 2026-10-24), and asked for
an on-screen cursor like other computer-use agents show ("why dont we have a cursor like codex dots and clicky???
does that additional cursor help??? … also add a cursor and good looking apple design principle cursor").

FERB acts on exact Accessibility and page elements in the background (`ferb/act/mac/driver.py`, the browser extension),
so the owner's real cursor never moves. The cost: nothing on screen shows where FERB is working. A FERB-only pointer adds
no capability; it makes supervised runs legible (CLAUDE.md rule 17: the owner watches live tests and must be able to
tie a disturbance to one action) and builds trust in background work.

## What the owner sees

- When FERB is about to act on an element in a window that is visible on screen, a small FERB pointer glides to that
  element. A click shows a soft press ripple at the point; typing shows a gentle shimmer along the field; a scroll
  shows a short drift in the scroll direction. About 1.5 seconds after the last action the pointer fades out.
- The owner's mouse, keyboard focus and Space are never touched. The pointer is click-through (mouse events pass to
  what's underneath) and never takes focus.
- No pointer for a window that isn't visible: FERB's own minimized browser window, another Space, a window fully
  behind others or under a full-screen app.
- Several tasks acting at once: one pointer per task, each a slightly different shade of the accent colour.
- A switch "Show FERB's cursor" in the notch's settings, on by default.

## Look (Apple Human Interface Guidelines)

- Shape: the macOS arrow's proportions, drawn as a vector, filled with FERB's accent colour, a thin white outline and a
  soft shadow, so it reads on light and dark content; sized from the display's backing scale like the system cursor.
- Motion: a spring (response ≈ 0.35 s, damping that settles without overshoot) along a short arc from the last point,
  or a fade-in at the target for the first action. Ripple and shimmer are short and low-contrast.
- Reduce Motion (System Settings → Accessibility): no glide or ripple; the pointer fades in at the target and out.
- No text label by default: the pointer itself is the signal.

## Design

### 1. The position event (daemon)
- Just before a capability marked `on_screen` or acting on an element runs (the gateway's `_held`, where the
  disturbance sentinel's `before_action` already runs, `ferb/daemon/gateway.py`), the gateway publishes
  `action.point`: `{task_id, kind: click|type|scroll|press|drag, rect: [x, y, w, h] in global screen points,
  display: <id>}`.
- The rect comes from code, never a model: a macOS element's Accessibility frame (from the snapshot the element id
  refers to, re-read when stale); a web element's page rect mapped onto the page's on-screen area with the existing
  `page_on_screen` (`ferb/act/mac/driver.py:212`), from the extension's `getBoundingClientRect` facts and visual
  viewport (`extensions/chromium/content.js`). A rect that can't be placed on a visible window publishes nothing.
- Geometry only: no labels, values, URLs or text. A secret field's position is shown like any other.
- Published only for windows on screen: code checks the window's on-screen state (CoreGraphics window list) before
  publishing.

### 2. The overlay (FERB.app)
- A borderless, transparent, click-through `NSPanel` (ignores mouse events, never key or main), level above normal
  windows and below the menu bar and system UI, joining all Spaces and full-screen spaces (`canJoinAllSpaces`,
  `fullScreenAuxiliary`), one per display.
- `DaemonConnection` receives `action.point` from the existing bus stream and drives a pointer view per task id:
  glide (or fade in) to the rect's centre for click/press, to the field's leading edge for type, drift for scroll;
  remove after 1.5 s idle or when the task ends (`task.result`).
- Respects `NSWorkspace.accessibilityDisplayShouldReduceMotion` and the settings switch (stored with the app's other
  preferences).

### 3. Not in this design
- Showing a pointer for FERB's own off-screen browser window (nothing visible to point at).
- Moving the real cursor or any foreground input: unchanged rules (D16, rule 17).

## Testing
- Python: `action.point` is published with the element's mapped rect for a mac AX action and for a browser action on a
  visible window; nothing for an off-screen window, an unplaceable rect, or an observation-only call; the payload has no
  text, label, value or URL.
- Swift: the pointer appears at the rect, one per task id, fades after idle, is removed on task end, honours Reduce
  Motion and the settings switch; the panel ignores mouse events and never becomes key.
- Live, owner-observed: a browser task and a native-app task while the owner watches; the owner's cursor and focus stay
  where they were; the pointer matches each action.
- L33 registered in `docs/0.27_DECISIONS.toml`; ARCHITECTURE.md, README.md and CHANGELOG.md updated as built.
