# FERB's cursor (L33) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A click-through FERB pointer in FERB.app that glides to each element FERB acts on in a window visible on
screen, fed by a geometry-only daemon event.

**Architecture:** An acting capability may declare `async point(args, ctx) -> Point | None` (screen rect in global
points of the element it will act on, or None). The gateway calls it just before running the action (in `_held`),
with a short timeout, and publishes `cursor.point` (not `action.*`: the chat stream forwards every `action.*` event).
macOS tools take the rect from the Accessibility snapshot node; browser tools map the element's page rect onto the
browser window's AX web area with the existing `page_on_screen`. FERB.app draws one pointer per task in a
transparent, mouse-ignoring overlay panel per display.

**Tech Stack:** Python 3.12 (pydantic, PyObjC Quartz/AX already used), Swift 6 / SwiftUI / AppKit (`swift test` in
`ui/mac/FERBNotch`).

**Spec:** `docs/superpowers/specs/2026-10-24-ferb-cursor-design.md` (approved 2026-10-24: "reviewd. start."). Read it
first. One deliberate change from the spec text: the event topic is `cursor.point`, not `action.point`, because
`chat.send` subscribes to `action.*` (`ferb/daemon/main.py`, `self.bus.subscribe({... "action.*"})`) and would forward
every pointer move into chat streams.

## Global Constraints
- Read `CLAUDE.md`. Never read, search or reference `vibed/`. Work on `main`; commit messages cite `L33`.
- Geometry only: the event payload is exactly `{task_id, kind, rect: [x, y, w, h], display}` — never a label, value,
  URL, title, path or text.
- Published only for a window visible on screen; nothing for off-screen windows, unplaceable rects, observation-only
  tools, or a failed/timed-out `point()`.
- `point()` can never block or fail an action: called with a 0.5 s timeout; any exception or timeout publishes nothing.
- The pointer never moves the owner's cursor, never takes focus, ignores mouse events (`ignoresMouseEvents = true`),
  never becomes key or main.
- Motion: spring (response 0.35 s, no overshoot); fade out 1.5 s after a task's last point; Reduce Motion → fade in at
  target, no glide or ripple.
- Settings: "Show FERB's cursor", default on, stored in `UserDefaults` key `cursor.enabled`.
- Each task: failing test first; focused tests; full `uv run pytest -q`, `uv run mypy ferb/core`, `git diff --check`
  and `swift test` before the plan's last commit.

## Review Focus
- A `point()` that hangs (AX unresponsive app): the action must start within 0.5 s anyway. Pinned in Task 1
  (`test_a_slow_point_never_delays_the_action`).
- A browser element inside an iframe (its rect is frame-relative): publish nothing rather than a wrong spot. Pinned in
  Task 3 (`test_an_element_in_a_frame_gets_no_point`).
- A stale element id (snapshot older than the action): publish nothing, never raise. Pinned in Task 2
  (`test_a_stale_element_gets_no_point`).
- Two tasks acting at once: two pointers, each removed on its own task's end. Pinned in Task 4
  (`testEachTaskHasItsOwnPointerAndEndsWithItsTask`).
- The settings switch off: no panel shown at all. Pinned in Task 4 (`testTheSwitchOffShowsNothing`).

---

### Task 1: The `cursor.point` event in the gateway

**Files:**
- Create: `ferb/act/point.py` (the `Point` type and its validation)
- Modify: `ferb/daemon/gateway.py` (`_held`, ~`:831-855`)
- Test: `tests/test_cursor.py`

**Interfaces:**
- Produces:
  - `ferb.act.point.Point` — frozen dataclass `kind: Literal['click','type','scroll','press','drag']`,
    `rect: tuple[float, float, float, float]` (x, y, w, h in global screen points, top-left origin), `display: int`.
    `Point.payload(task_id) -> dict` returns exactly `{'task_id', 'kind', 'rect', 'display'}`.
  - `ferb.act.point.display_of(rect) -> int | None` — the CoreGraphics display id containing the rect's centre
    (`Quartz.CGGetDisplaysWithPoint`), None off every display.
  - Capability protocol (duck-typed, like `locks`): `async def point(self, args, ctx) -> Point | None`.
  - Gateway: before `before_action` in `_held`, `await asyncio.wait_for(cap.point(args, ctx), 0.5)` when the cap has
    `point`; on a `Point`, `self.publish('cursor.point', p.payload(grant.task_id))`. `ctx` here is a minimal
    `ToolContext` (task id, worker id, identity, scopes) — build it as `_execute` does, or pass `grant` if simpler and
    adjust the protocol to `point(args, grant)`; record the choice in the report.

- [ ] **Step 1: Write the failing tests** (`tests/test_cursor.py`)

```python
"""L33: FERB's cursor — a geometry-only `cursor.point` event before each on-screen action."""
import asyncio

from pydantic import BaseModel

from ferb.act.point import Point
from ferb.core.types import ActionSpec, Identity, RiskLevel, ToolResult
from ferb.policy.rules import Kind
from tests.conftest import gateway_setup, gateway_tool_call


def _cap(point_result=None, delay=0.0, boom=False):
    class Pointed:
        name, description, scopes = 'test.click', 'click', set()
        identities = {Identity.OWNER, Identity.FERB}
        risk, reversible, on_screen = RiskLevel.LOW, True, True
        ran: list = []
        class Input(BaseModel):
            element: str = 'e1'
        def describe(self, args): return ActionSpec(Kind.READ)
        async def point(self, args, ctx):
            await asyncio.sleep(delay)
            if boom:
                raise RuntimeError('ax gone')
            return point_result
        async def run(self, args, ctx):
            Pointed.ran.append(asyncio.get_running_loop().time())
            return ToolResult({'ok': True})
    return Pointed()


def _published(daemon, monkeypatch):
    seen = []
    original = daemon.daemon.gateway.publish
    def spy(topic, data):
        seen.append((topic, data))
        return original(topic, data)
    monkeypatch.setattr(daemon.daemon.gateway, 'publish', spy)
    return seen


def test_an_action_publishes_its_point_with_geometry_only(daemon, monkeypatch):
    seen = _published(daemon, monkeypatch)
    grant, _ = gateway_setup(daemon, set(), None, [_cap(Point('click', (10.0, 20.0, 30.0, 40.0), 1))],
                             identity=Identity.FERB)
    gateway_tool_call(daemon, grant, 'test.click', element='e1')
    [(topic, data)] = [(t, d) for t, d in seen if t == 'cursor.point']
    assert data == {'task_id': grant.task_id, 'kind': 'click', 'rect': [10.0, 20.0, 30.0, 40.0], 'display': 1}


def test_no_point_no_event(daemon, monkeypatch):
    seen = _published(daemon, monkeypatch)
    grant, _ = gateway_setup(daemon, set(), None, [_cap(None)], identity=Identity.FERB)
    gateway_tool_call(daemon, grant, 'test.click', element='e1')
    assert not [t for t, _ in seen if t == 'cursor.point']


def test_a_failing_point_never_fails_the_action(daemon, monkeypatch):
    seen = _published(daemon, monkeypatch)
    cap = _cap(boom=True)
    grant, _ = gateway_setup(daemon, set(), None, [cap], identity=Identity.FERB)
    out = gateway_tool_call(daemon, grant, 'test.click', element='e1')
    assert out['is_error'] is False and cap.ran and not [t for t, _ in seen if t == 'cursor.point']


def test_a_slow_point_never_delays_the_action(daemon, monkeypatch):
    seen = _published(daemon, monkeypatch)
    cap = _cap(Point('click', (1.0, 1.0, 1.0, 1.0), 1), delay=5.0)
    grant, _ = gateway_setup(daemon, set(), None, [cap], identity=Identity.FERB)
    out = gateway_tool_call(daemon, grant, 'test.click', element='e1')
    assert out['is_error'] is False and cap.ran
    assert not [t for t, _ in seen if t == 'cursor.point']
```

(The 0.5 s timeout is what makes the last test pass quickly; if `gateway_tool_call`'s default timeout is shorter
than needed, pass `timeout=` as `tests/conftest.py:578` allows.)

- [ ] **Step 2: Run** `uv run pytest tests/test_cursor.py -q` → FAIL (`ferb.act.point` missing).

- [ ] **Step 3: Implement** `ferb/act/point.py`:

```python
"""L33: where FERB is about to act, for FERB's on-screen cursor. Geometry only: never a label, value, URL or text."""
from dataclasses import dataclass
from typing import Any, Literal

POINT_TIMEOUT_S = 0.5          # a point that takes longer is skipped; the action never waits for the cursor


@dataclass(frozen=True)
class Point:
    kind: Literal['click', 'type', 'scroll', 'press', 'drag']
    rect: tuple[float, float, float, float]          # x, y, w, h in global screen points, top-left origin
    display: int

    def payload(self, task_id: str) -> dict[str, Any]:
        return {'task_id': task_id, 'kind': self.kind, 'rect': [float(v) for v in self.rect], 'display': self.display}


def display_of(rect: tuple[float, float, float, float]) -> int | None:
    """The display holding the rect's centre, or None when it is on no display."""
    import Quartz
    x, y, w, h = rect
    err, ids, count = Quartz.CGGetDisplaysWithPoint((x + w / 2, y + h / 2), 1, None, None)
    return int(ids[0]) if err == 0 and count else None
```

In `gateway.py` `_held`, right before `if self.disturbance is not None: await self.disturbance.before_action(...)`:

```python
                await self._point(cap, args, grant)
```

and the method:

```python
    async def _point(self, cap: Any, args: Any, grant: RunGrant) -> None:
        """L33: the element this action is about to touch, for FERB's cursor (geometry only, `cursor.point`). Never
        delays or fails the action: skipped after POINT_TIMEOUT_S or on any error."""
        from ferb.act.point import POINT_TIMEOUT_S, Point
        if not hasattr(cap, "point"):
            return
        try:
            p = await asyncio.wait_for(cap.point(args, grant), POINT_TIMEOUT_S)
        except asyncio.CancelledError:
            raise
        except Exception:
            return
        if isinstance(p, Point):
            self.publish("cursor.point", p.payload(grant.task_id))
```

(Protocol choice: `point(args, grant)`; capabilities that need more context read it from their driver. Keep the
choice consistent in Tasks 2–3.)

- [ ] **Step 4: Run** `uv run pytest tests/test_cursor.py tests/test_gateway.py -q` → PASS.

- [ ] **Step 5: Commit** `git add ferb/act/point.py ferb/daemon/gateway.py tests/test_cursor.py` with
  `feat(L33): the gateway tells FERB's cursor where an action is about to land` (+ Co-Authored-By line).

---

### Task 2: macOS tools report their element's point

**Files:**
- Modify: `ferb/act/mac/tools.py` (`MacAct` ~`:262`, `MacClick` ~`:391`, `MacMenu` ~`:156` if it has an element,
  `MacSecret` ~`:542`)
- Modify: `ferb/act/mac/screenshot.py` or `ferb/act/mac/accessibility.py` (a `window_on_screen(pid, rect) -> bool`
  helper using `_info_windows()` from `screenshot.py:49-54`)
- Test: `tests/test_cursor.py`

**Interfaces:**
- Consumes: `Point`, `display_of` (Task 1); `driver.node(bundle_id, element) -> AxNode` with `.rect`
  (`ferb/act/mac/driver.py:421`, raises `StaleRef`); `_bundle(args)` (`tools.py:82`).
- Produces: `point(args, grant)` on those classes; kind from the action (`click`/`press` → 'click', type/set_value →
  'type', scroll → 'scroll'; a drag → 'drag'); None when the element id is absent (window-level scroll), stale, the
  rect is empty, the owning window isn't on screen, or `display_of` is None.

- [ ] **Step 1: Failing tests** (append): with a fake driver whose `node()` returns an `AxNode(rect=(100, 200, 50, 20))`
  and a monkeypatched `window_on_screen` → True and `display_of` → 1, `MacAct().point(Input(app='pid:1', element='s1e2',
  action='click'), grant)` returns `Point('click', (100,200,50,20), 1)`; `test_a_stale_element_gets_no_point`: `node()`
  raises `StaleRef` → None; off-screen window → None; type action → kind 'type'. Write these as real pytest functions
  using the classes' actual constructors (read `tests/test_mac_*` for how tools are built with fake drivers).
- [ ] **Step 2: Run** → FAIL.
- [ ] **Step 3: Implement** `point` on each class per the interface, catching `StaleRef` → None; the visibility check
  `window_on_screen(pid, rect)` returns True when an on-screen window of that pid contains the rect's centre.
- [ ] **Step 4: Run** `uv run pytest tests/test_cursor.py $(ls tests/test_mac*.py) -q` → PASS.
- [ ] **Step 5: Commit** `feat(L33): macOS actions report the element they touch for FERB's cursor`.

---

### Task 3: Browser tools report their element's point

**Files:**
- Modify: `ferb/act/browser/tools.py` (`BrowserAct` ~`:535`, `BrowserSecret` ~`:636`, `BrowserScroll` is an
  observation tool — no point)
- Modify: `ferb/act/browser/ext_bridge.py` (a `screen_rect(el: ElementRef) -> tuple | None` method)
- Modify: `ferb/act/mac/driver.py` (a small exported helper `place_on_web_area(rect_css, viewport_css, pid,
  window_bounds) -> Rect | None` that reads the window's AX tree and calls the existing `page_on_screen(rects,
  viewport, axm.outermost(tree.web_areas))`, `driver.py:212`; returns None on `Withheld`)
- Test: `tests/test_cursor.py`

**Interfaces:**
- Consumes: `PageElement.rect` (frame-viewport CSS px, `ferb/core/types.py:332`), the snapshot's viewport facts
  (`content.js:44`: `viewport`, `scroll`, `scale`), the tab's Chrome window bounds (the extension's
  `chrome.windows.get`), the browser's pid (driver `apps`), `page_on_screen`.
- Produces: `point(args, grant)` on `BrowserAct` (kind from the act: click → 'click', type/fill → 'type', press →
  'press') and `BrowserSecret` ('type'); `ExtBridge.screen_rect(el)`: None when the element is in a sub-frame
  (`frame != 0`), the tab is FERB's own minimized window or not visible, the window can't be matched by bounds
  (`accessibility.window_with_frame`), or placement is withheld.

- [ ] **Step 1: Failing tests** (append): `test_an_element_in_a_frame_gets_no_point` (fake resolve returns frame 1 →
  None); a top-frame element with rect (10, 20, 100, 30), viewport (0, 0, 1000, 800) on a fake web area
  (200, 100, 1000, 800) → Point rect (210, 120, 100, 30); FERB's own minimized window → None. Build fakes like the
  existing ext-bridge tests (`ls tests | grep -i ext_bridge`).
- [ ] **Step 2: Run** → FAIL.
- [ ] **Step 3: Implement.** If the extension doesn't already return the tab window's bounds to the bridge, add them to
  an existing reply the bridge already gets per action or snapshot (prefer the snapshot reply). If that changes the
  extension protocol, update the manifest version, the JS/Python protocol and the minimum versions together
  (CLAUDE.md), and note it.
- [ ] **Step 4: Run** `uv run pytest tests/test_cursor.py $(ls tests/test_*browser*.py tests/test_ext*.py 2>/dev/null) -q`
  → PASS.
- [ ] **Step 5: Commit** `feat(L33): browser actions report the element they touch, placed on the page's screen area`.

---

### Task 4: The overlay pointer in FERB.app

**Files:**
- Create: `ui/mac/FERBNotch/Sources/FERBNotch/CursorOverlay.swift` (panel per display + controller)
- Create: `ui/mac/FERBNotch/Sources/FERBNotch/CursorPointer.swift` (SwiftUI pointer view: arrow shape, ripple,
  shimmer)
- Modify: `ui/mac/FERBNotch/Sources/FERBNotch/DaemonConnection.swift` (`receiveBusEvent`: case `"cursor.point"`; on
  `"task.result"` also end that task's pointer)
- Modify: `ui/mac/FERBNotch/Sources/FERBNotch/SoundFeedback.swift` settings view (add the switch, key `cursor.enabled`,
  default true) and the app delegate/coordinator that owns panels (`PanelCoordinator.swift`) to create the controller
- Test: `ui/mac/FERBNotch/Tests/NotchCoreTests/CursorOverlayTests.swift`

**Interfaces:**
- Produces: `final class CursorController` with `func point(taskID: String, kind: String, rect: CGRect, display:
  CGDirectDisplayID)`, `func end(taskID: String)`, `var enabled: Bool`, `var reduceMotion: Bool`, and a test seam
  `pointers: [String: PointerState]` (`PointerState`: `target: CGPoint`, `kind: String`, `lastSeen: Date`, `hue:
  Double`); idle fade after 1.5 s via an injectable clock.
- Panel: `NSPanel(styleMask: [.borderless, .nonactivatingPanel])`, `isOpaque = false`, `backgroundColor = .clear`,
  `ignoresMouseEvents = true`, `level = .statusBar - 1` (above normal windows, below the menu bar), `collectionBehavior
  = [.canJoinAllSpaces, .fullScreenAuxiliary, .stationary, .ignoresCycle]`, `canBecomeKey/Main` false (subclass),
  frame = the display's frame; screen coordinates converted from top-left global points to AppKit's bottom-left.
- Target point: click/press → rect centre; type → (minX + 12, midY); scroll → centre, with a short drift.
- Look: arrow path in the macOS cursor's proportions, `Color.accentColor` fill, 1.5 pt white stroke, shadow radius 3
  opacity 0.35; per-task hue offset of 0, 0.08, −0.08, 0.16 … ; spring `.spring(response: 0.35, dampingFraction: 1.0)`;
  Reduce Motion (`NSWorkspace.shared.accessibilityDisplayShouldReduceMotion`) → no glide/ripple, `.easeOut(0.15)` fade.

- [ ] **Step 1: Failing Swift tests** (`CursorOverlayTests.swift`):

```swift
import XCTest
@testable import FERBNotch

@MainActor
final class CursorOverlayTests: XCTestCase {
    func testAPointShowsAPointerAtTheElement() {
        let c = CursorController(clock: { Date(timeIntervalSince1970: 100) }, makePanels: false)
        c.point(taskID: "t1", kind: "click", rect: CGRect(x: 100, y: 200, width: 40, height: 20), display: 1)
        XCTAssertEqual(c.pointers["t1"]?.target, CGPoint(x: 120, y: 210))
    }

    func testEachTaskHasItsOwnPointerAndEndsWithItsTask() {
        let c = CursorController(clock: { Date(timeIntervalSince1970: 100) }, makePanels: false)
        c.point(taskID: "t1", kind: "click", rect: CGRect(x: 0, y: 0, width: 10, height: 10), display: 1)
        c.point(taskID: "t2", kind: "type", rect: CGRect(x: 50, y: 50, width: 100, height: 20), display: 1)
        XCTAssertEqual(Set(c.pointers.keys), ["t1", "t2"])
        XCTAssertNotEqual(c.pointers["t1"]?.hue, c.pointers["t2"]?.hue)
        XCTAssertEqual(c.pointers["t2"]?.target, CGPoint(x: 62, y: 60))   // type: the field's leading edge
        c.end(taskID: "t1")
        XCTAssertEqual(Set(c.pointers.keys), ["t2"])
    }

    func testAPointerFadesAfterIdle() {
        var now = Date(timeIntervalSince1970: 100)
        let c = CursorController(clock: { now }, makePanels: false)
        c.point(taskID: "t1", kind: "click", rect: CGRect(x: 0, y: 0, width: 10, height: 10), display: 1)
        now = now.addingTimeInterval(1.6)
        c.sweep()
        XCTAssertNil(c.pointers["t1"])
    }

    func testTheSwitchOffShowsNothing() {
        let c = CursorController(clock: { Date() }, makePanels: false)
        c.enabled = false
        c.point(taskID: "t1", kind: "click", rect: CGRect(x: 0, y: 0, width: 10, height: 10), display: 1)
        XCTAssertTrue(c.pointers.isEmpty)
    }

    func testTheOverlayPanelNeverTakesTheMouseOrFocus() {
        let panel = CursorPanel(screenFrame: CGRect(x: 0, y: 0, width: 800, height: 600))
        XCTAssertTrue(panel.ignoresMouseEvents)
        XCTAssertFalse(panel.canBecomeKey)
        XCTAssertFalse(panel.canBecomeMain)
        XCTAssertTrue(panel.collectionBehavior.contains(.canJoinAllSpaces))
    }

    func testABusEventReachesTheController() {
        let model = NotchModel(rpc: QuietCursorRPC(), preferences: UserDefaults(suiteName: UUID().uuidString)!)
        let c = CursorController(clock: { Date() }, makePanels: false)
        model.cursor = c
        model.receiveBusEvent(["topic": "cursor.point", "data": ["task_id": "t9", "kind": "click",
                               "rect": [10.0, 10.0, 20.0, 20.0], "display": 1] as [String: Any]])
        XCTAssertNotNil(c.pointers["t9"])
    }
}

private final class QuietCursorRPC: DaemonRPCTransport {
    func call(_ method: String, params: [String: Any]) async throws -> [String: Any] { [:] }
    func stream(_ method: String, params: [String: Any]) -> AsyncThrowingStream<[String: Any], Error> {
        AsyncThrowingStream { _ in }
    }
}
```

- [ ] **Step 2: Run** `cd ui/mac/FERBNotch && swift test --filter CursorOverlayTests` → FAIL.
- [ ] **Step 3: Implement** per the interface; `NotchModel.cursor: CursorController?` set by the coordinator at launch;
  `receiveBusEvent` case `"cursor.point"` parses `rect` (4 numbers) and `display` and calls `cursor?.point(...)`;
  `"task.result"` calls `cursor?.end(taskID:)`. A 0.25 s repeating timer calls `sweep()` while any pointer exists.
- [ ] **Step 4: Run** `swift test` (all) and `./build.sh` → PASS / builds.
- [ ] **Step 5: Commit** `feat(L33): FERB's pointer: a click-through overlay that glides to each action`.

---

### Task 5: Docs, decision record, validation

**Files:** `docs/0.27_DECISIONS.toml` (`[L33]`), `docs/0.27_PLAN.md` (one line in "Launch-week clarifications" naming
L33 and its spec), `ARCHITECTURE.md` (the `cursor.point` event and the overlay; debugging: `ferb logs` shows the
event), `README.md` (Use: what the pointer means; the settings switch), `CHANGELOG.md` (one entry),
`docs/OWNER_ACCEPTANCE.md` (pending live check: browser + native task watched; cursor matches actions; owner's cursor
and focus untouched).

- [ ] **Step 1:** `[L33]` entry: milestone "L", tests `["tests/test_cursor.py"]`, note quoting the owner ("also add a
  cursor and good looking apple design principle cursor"; "yeah works"; "reviewd. start."), summary of the design.
- [ ] **Step 2:** the doc edits above.
- [ ] **Step 3:** `uv run pytest -q`, `uv run mypy ferb/core`, `git diff --check`, `cd ui/mac/FERBNotch && swift test`;
  record counts.
- [ ] **Step 4:** commit `docs(L33): FERB's cursor recorded` and push only after every commit underneath is reviewed.
