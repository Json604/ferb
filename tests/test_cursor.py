"""L33: FERB's cursor — a geometry-only `cursor.point` event before each on-screen action."""
import asyncio
import time

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
        pointed: list = []
        class Input(BaseModel):
            element: str = 'e1'
        def describe(self, args): return ActionSpec(Kind.READ)
        async def point(self, args, ctx):
            Pointed.pointed.append(1)
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
    assert data == {'task_id': grant.task_id, 'worker_id': grant.worker_id, 'kind': 'click',
                    'rect': [10.0, 20.0, 30.0, 40.0], 'display': 1}


def test_a_steps_point_names_its_root_task_so_one_task_keeps_one_pointer(daemon, monkeypatch):
    """L33 final review M1: each step runs under its own task id (`<tree>.worker`, `<tree>.step.N`), and the app ends a
    pointer on the root task's `task.result`: the point names the root task (one pointer and one shade per task,
    across its steps) and the worker that acted (as `tool.called` names it)."""
    from datetime import timedelta
    from ferb.core.types import Task
    from ferb.orchestrator.tree import TaskRecord
    seen = _published(daemon, monkeypatch)
    cap = _cap(Point('click', (10.0, 20.0, 30.0, 40.0), 1))
    daemon.daemon.registry.capabilities[cap.name] = cap

    async def step(tid, worker):
        now = daemon.daemon.clock.now()
        task = Task(tid, 'tree-1', 'g', 'dev', Identity.FERB, set(), 'worker', 'req-1', [], now + timedelta(minutes=10))
        span = daemon.daemon.spans.start('task', task_id=tid, input={})
        daemon.daemon.tasks.add(TaskRecord(task, now, span.id, tree_id='tree-1'))
        return daemon.daemon.tokens.validate(daemon.daemon.tokens.issue(tid, worker, set(), Identity.FERB, ttl_s=600))
    for tid, worker in [('tree-1.step.a', 'w-a'), ('tree-1.step.b', 'w-b')]:
        gateway_tool_call(daemon, daemon.run(step(tid, worker)), 'test.click', element='e1')
    assert [(d['task_id'], d['worker_id']) for t, d in seen if t == 'cursor.point'] == [('tree-1', 'w-a'),
                                                                                         ('tree-1', 'w-b')]


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
    assert cap.pointed
    assert out['is_error'] is False and cap.ran and not [t for t, _ in seen if t == 'cursor.point']


def test_a_slow_point_never_delays_the_action(daemon, monkeypatch):
    seen = _published(daemon, monkeypatch)
    cap = _cap(Point('click', (1.0, 1.0, 1.0, 1.0), 1), delay=5.0)
    grant, _ = gateway_setup(daemon, set(), None, [cap], identity=Identity.FERB)
    t0 = time.monotonic()
    out = gateway_tool_call(daemon, grant, 'test.click', element='e1')
    assert cap.pointed
    assert out['is_error'] is False and cap.ran and cap.ran[0] - t0 < 1.0
    assert not [t for t, _ in seen if t == 'cursor.point']


def test_a_bad_point_is_refused_at_construction_and_the_action_runs(daemon, monkeypatch):
    import pytest
    seen = _published(daemon, monkeypatch)
    with pytest.raises(ValueError):
        Point('click', (float('nan'), 0, 10, 10), 1)
    for bad in [('hover', (0, 0, 1, 1)), ('click', (0, 0, 0, 1)), ('click', (0, 0, 1))]:
        with pytest.raises(ValueError):
            Point(bad[0], bad[1], 1)
    cap = _cap(boom=True)           # a capability that builds a bad Point raises inside point()
    grant, _ = gateway_setup(daemon, set(), None, [cap], identity=Identity.FERB)
    out = gateway_tool_call(daemon, grant, 'test.click', element='e1')
    assert cap.pointed and out['is_error'] is False and cap.ran and not [t for t, _ in seen if t == 'cursor.point']


def test_display_of_reads_the_display_under_the_centre(monkeypatch):
    import Quartz
    from ferb.act.point import display_of
    seen = []
    def fake(pt, n, ids, count):
        seen.append(pt)
        return (0, [7], 1)
    monkeypatch.setattr(Quartz, 'CGGetDisplaysWithPoint', fake)
    assert display_of((10.0, 20.0, 30.0, 40.0)) == 7 and seen == [(25.0, 40.0)]
    monkeypatch.setattr(Quartz, 'CGGetDisplaysWithPoint', lambda *a: (0, [], 0))
    assert display_of((10.0, 20.0, 30.0, 40.0)) is None


# ---- macOS tools report their element's point ----

def _mac(monkeypatch, rect=(100, 200, 50, 20), stale=False, on_screen=True, display=1, tool='act', live='same',
         secret=False, bundle='com.x'):
    """A Mac tool over a fake driver: the element's snapshot rect is `rect`, its Accessibility frame now is `live`
    (the same by default; None: gone or no size)."""
    from types import SimpleNamespace
    from ferb.act.mac import tools
    from ferb.core.errors import StaleRef
    from ferb.act.mac.accessibility import AxNode
    now = rect if live == 'same' else live
    class Driver:
        apps = SimpleNamespace(find=lambda q: SimpleNamespace(bundle_id=bundle, pid=1, name='X'))
        ax = SimpleNamespace(frame=lambda ref: now)
        def app(self, name):
            return SimpleNamespace(bundle_id=bundle, pid=1, name='X')
        def node(self, bundle, element):
            if stale:
                raise StaleRef('old')
            return AxNode(ref='el', role='button', label='', value=None, rect=rect, secret=secret, enabled=True)
    monkeypatch.setattr(tools, 'window_on_screen', lambda pid, r: on_screen)
    monkeypatch.setattr(tools, 'display_of', lambda r: display)
    cls = {'act': tools.MacAct, 'secret': tools.MacSecret}[tool]
    return cls(Driver()), cls


def _point(tool, cls, **kw):
    return asyncio.run(tool.point(cls.Input(app='pid:1', **kw), None))


def test_a_mac_act_reports_the_element_it_touches(monkeypatch):
    tool, cls = _mac(monkeypatch)
    assert _point(tool, cls, element='s1e2', action='click') == Point('click', (100, 200, 50, 20), 1)
    assert _point(tool, cls, element='s1e2', action='press').kind == 'click'
    assert _point(tool, cls, element='s1e2', action='type', text='x').kind == 'type'
    assert _point(tool, cls, element='s1e2', action='set_value', text='x').kind == 'type'
    assert _point(tool, cls, element='s1e2', action='scroll', direction='down').kind == 'scroll'


def test_a_stale_element_gets_no_point(monkeypatch):
    tool, cls = _mac(monkeypatch, stale=True)
    assert _point(tool, cls, element='s1e2', action='click') is None


def test_no_element_an_empty_rect_a_hidden_window_or_no_display_get_no_point(monkeypatch):
    tool, cls = _mac(monkeypatch)
    assert _point(tool, cls, action='scroll', direction='down') is None            # window-level scroll
    assert _point(*_mac(monkeypatch, rect=(1, 2, 0, 5)), element='s1e2', action='click') is None
    assert _point(*_mac(monkeypatch, on_screen=False), element='s1e2', action='click') is None
    assert _point(*_mac(monkeypatch, display=None), element='s1e2', action='click') is None


def test_mac_secret_points_at_its_field_as_typing(monkeypatch):
    tool, cls = _mac(monkeypatch, tool='secret')
    assert _point(tool, cls, element='s1e2') == Point('type', (100, 200, 50, 20), 1)


def test_the_element_lookup_runs_off_the_event_loop(monkeypatch):
    import threading
    tool, cls = _mac(monkeypatch)
    seen, framed = [], []
    orig, orig_frame = tool.driver.node, tool.driver.ax.frame
    tool.driver.node = lambda b, e: (seen.append(threading.current_thread()), orig(b, e))[1]
    tool.driver.ax.frame = lambda ref: (framed.append(threading.current_thread()), orig_frame(ref))[1]
    main = threading.current_thread()
    assert _point(tool, cls, element='s1e2', action='click') is not None
    assert seen and seen[0] is not main and framed and framed[0] is not main


def test_a_mac_point_is_where_the_element_is_now_not_where_the_snapshot_saw_it(monkeypatch):
    """L33 final review I3: a list scrolled or a window moved since the snapshot leaves its rect stale (AXPress still
    works on the element); the point is the element's Accessibility frame read now, and the window check is there."""
    from ferb.act.mac import tools
    tool, cls = _mac(monkeypatch, live=(300.0, 450.0, 50.0, 20.0))
    checked = []
    monkeypatch.setattr(tools, 'window_on_screen', lambda pid, r: checked.append(r) or True)
    assert _point(tool, cls, element='s1e2', action='click') == Point('click', (300.0, 450.0, 50.0, 20.0), 1)
    assert checked == [(300.0, 450.0, 50.0, 20.0)]
    assert _point(*_mac(monkeypatch, live=None), element='s1e2', action='click') is None       # gone, or no size
    assert _point(*_mac(monkeypatch, live=(300.0, 450.0, 0.0, 20.0)), element='s1e2', action='click') is None


def test_native_acts_the_driver_refuses_or_sends_elsewhere_publish_no_point(monkeypatch):
    """L33 final review M5: as on the browser side, no pointer for an act `act` refuses before any input, and none for
    keys (they go to the app's focus, not the named element)."""
    from types import SimpleNamespace
    tool, cls = _mac(monkeypatch, secret=True)
    for action in ('type', 'set_value', 'edit'):                                  # FERB never types a password
        assert _point(tool, cls, element='s1e2', action=action, text='x') is None, action
    assert _point(tool, cls, element='s1e2', action='click') is not None          # pressing it is fine
    tool, cls = _mac(monkeypatch)
    assert _point(tool, cls, element='s1e2', action='type') is None               # no text: refused
    assert _point(tool, cls, element='s1e2', action='key', text='cmd+s') is None  # keys go to the app
    assert _point(tool, cls, element='s1e2', action='click', mechanism='foreground') is None   # no permission
    assert _point(tool, cls, element='s1e2', action='click', mechanism='foreground', allow_foreground=True) is not None
    assert _point(tool, cls, element='s1e2', action='click', mechanism='pid') is None          # a route it lacks
    assert _point(tool, cls, element='s1e2', action='type', text='x', mechanism='ax') is None
    assert _point(tool, cls, element='s1e2', action='set_value', text='x', mechanism='pid') is None
    assert _point(tool, cls, element='s1e2', action='focus') is not None
    tool.protection = SimpleNamespace(terminal_apps={'com.x'})                    # FERB never types into a terminal
    assert _point(tool, cls, element='s1e2', action='type', text='x') is None
    assert _point(tool, cls, element='s1e2', action='click') is not None
    assert _point(*_mac(monkeypatch, tool='secret', bundle='com.brave.Browser'), element='s1e2') is None   # web page


def test_the_native_pointer_check_agrees_with_what_act_refuses(monkeypatch):
    """Drift guard for M5: whenever `act` refuses an element action before any input, `acts_on` says no pointer;
    for the actions whose refusals all come first, `acts_on` also never hides one `act` goes on to do."""
    import itertools
    from ferb.act.mac.driver import acts_on
    from ferb.core.errors import Refused
    from ferb.core.types import ComputerAction
    from tests.mackit import mac
    import ferb.act.mac.driver as drv
    d, ax, *_ = mac(monkeypatch)
    ax.has_focus = lambda pid, el: True

    async def no_wait(s: float) -> None:              # select waits for its menu to open
        return None
    monkeypatch.setattr(drv.asyncio, 'sleep', no_wait)
    asyncio.run(d.snapshot('Notes'))
    kinds = ('press', 'click', 'cancel', 'set_value', 'type', 'focus', 'select', 'edit', 'scroll', 'key')
    seen = {True: 0, False: 0}

    async def check() -> None:
        for kind, mechanism, element, text in itertools.product(
                kinds, (None, 'ax', 'pid', 'ax_keyboard', 'ax_value', 'foreground'), ('s1e1', 's1e2', 's1e3'),
                ('x', None)):
            action = ComputerAction(kind, element, text, mechanism=mechanism)
            try:
                r = await d.act('Notes', action)
                refused = r.facts.get('code') == 'needs_foreground'
            except Refused:
                refused = True
            except Exception:                             # past act's own checks, into what these fakes lack
                refused = False
            pointed = acts_on(action, d.node('com.apple.Notes', element))
            seen[refused] += 1
            if refused:
                assert not pointed, action
            elif kind not in ('edit', 'scroll', 'key'):
                assert pointed, action
    asyncio.run(check())
    assert seen[True] > 100 and seen[False] > 50                  # both sides exercised


def test_window_on_screen_wants_a_normal_window_of_the_pid_holding_the_centre(monkeypatch):
    from ferb.act.mac import screenshot
    def win(pid=1, layer=0, alpha=1.0, b=(0, 0, 400, 300)):
        return {'kCGWindowOwnerPID': pid, 'kCGWindowLayer': layer, 'kCGWindowAlpha': alpha, 'kCGWindowNumber': 9,
                'kCGWindowBounds': {'X': b[0], 'Y': b[1], 'Width': b[2], 'Height': b[3]}}
    rect = (100, 100, 20, 20)
    def on(*ws):
        monkeypatch.setattr(screenshot, '_info_windows', lambda everywhere=False: list(ws))
        return screenshot.window_on_screen(1, rect)
    assert on(win()) is True
    assert on(win(pid=2)) is False
    assert on(win(b=(500, 500, 100, 100))) is False
    assert on(win(alpha=0)) is False
    assert on(win(layer=25)) is False                  # a menu-bar item or overlay is not an app window
    assert on(win(layer=25), win()) is True


def test_window_on_screen_with_bounds_wants_the_matched_window(monkeypatch):
    """L33 review (7): the spot must be in the very window placement matched (its bounds and the tab's title), not
    in any on-screen window of the browser that holds it."""
    from ferb.act.mac import screenshot
    def win(b=(0, 0, 400, 300), name='Page'):
        return {'kCGWindowOwnerPID': 1, 'kCGWindowLayer': 0, 'kCGWindowAlpha': 1.0, 'kCGWindowNumber': 9,
                'kCGWindowName': name, 'kCGWindowBounds': {'X': b[0], 'Y': b[1], 'Width': b[2], 'Height': b[3]}}
    def on(*ws, rect=(100, 100, 20, 20), title='Page'):
        monkeypatch.setattr(screenshot, '_info_windows', lambda everywhere=False: list(ws))
        return screenshot.window_on_screen(1, rect, (0.0, 0.0, 400.0, 300.0), title)
    assert on(win()) is True
    assert on(win(b=(0, 0, 800, 600))) is False                # another window of the browser holds the spot
    assert on(win(name='Other tab')) is False                  # the window there shows another page
    assert on(win(), win(name='Other tab')) is True            # two at those bounds: the title tells them apart
    assert on(win(), win()) is False                           # two showing the same page: which one can't be told
    assert on(win(), rect=(500, 100, 20, 20)) is False         # outside the matched window
    assert on(win(name='Untitled tab'), title=None) is True    # no title to tell by: the one window there


def test_a_window_behind_another_window_at_the_spot_gets_no_point(monkeypatch):
    """L33 final review I1: the spot counts only where the target window is the frontmost normal window there, in the
    window server's front-to-back order. The owner typing in another app over FERB's window sees no arrow on top."""
    from ferb.act.mac import screenshot
    def win(pid=1, b=(0, 0, 400, 300), name='Page', number=9, layer=0, alpha=1.0):
        return {'kCGWindowOwnerPID': pid, 'kCGWindowLayer': layer, 'kCGWindowAlpha': alpha, 'kCGWindowNumber': number,
                'kCGWindowName': name, 'kCGWindowBounds': {'X': b[0], 'Y': b[1], 'Width': b[2], 'Height': b[3]}}
    def on(*ws, rect=(100, 100, 20, 20), **kw):
        monkeypatch.setattr(screenshot, '_info_windows', lambda everywhere=False: list(ws))
        return screenshot.window_on_screen(1, rect, **kw)
    editor = win(pid=2, b=(50, 50, 300, 300), name='main.py', number=3)          # another app's window over the spot
    assert on(editor, win()) is False
    assert on(win(), editor) is True                                             # the target in front
    assert on(win(pid=2, b=(500, 0, 300, 300), number=3), win()) is True         # another app's window beside the spot
    assert on(win(pid=99, b=(0, 0, 2000, 2000), layer=24, number=4), win()) is True   # FERB's own overlay is no window
    assert on(win(pid=2, alpha=0, number=3), win()) is True                      # nor is a transparent one
    like = (0.0, 0.0, 400.0, 300.0)                                              # a browser's matched window
    assert on(editor, win(), like=like, title='Page') is False
    assert on(win(), editor, like=like, title='Page') is True
    assert on(win(b=(0, 0, 800, 600), name='Other', number=8), win(), like=like, title='Page') is False   # its own
    assert on(win(number=8), win(), window=9) is False                           # a screenshot's window, by id
    assert on(win(), win(number=8), window=9) is True
    assert on(win(), window=7) is False                                          # that window isn't on screen


def test_mac_click_points_at_where_its_pixels_land(monkeypatch):
    from types import SimpleNamespace
    from ferb.act.mac import tools
    from ferb.act.mac.driver import MacComputerDriver
    from ferb.act.mac.screenshot import Window
    shot = SimpleNamespace(id='shot1', window=5, bounds=(100.0, 50.0, 400.0, 200.0), size=(800, 400))
    d = MacComputerDriver.__new__(MacComputerDriver)
    d.app = lambda name: SimpleNamespace(bundle_id='com.x', pid=3, name='X')
    d._state = {'com.x': SimpleNamespace(shot=shot)}
    d.shots = SimpleNamespace(window=lambda wid: Window(wid, 3, shot.bounds, 'Doc') if wid == 5 else None)
    seen = []
    monkeypatch.setattr(tools, 'window_on_screen', lambda pid, r, window=None: seen.append((pid, r, window)) or True)
    monkeypatch.setattr(tools, 'display_of', lambda r: 2)
    tool = tools.MacClick(d)
    got = asyncio.run(tool.point(tools.MacClick.Input(app='x', shot='shot1', x=200, y=100), None))
    assert got == Point('click', (196.0, 96.0, 8.0, 8.0), 2) and seen == [(3, (196.0, 96.0, 8.0, 8.0), 5)]
    assert asyncio.run(tool.point(tools.MacClick.Input(app='x', shot='old', x=200, y=100), None)) is None
    assert asyncio.run(tool.point(tools.MacClick.Input(app='x', shot='shot1', x=900, y=100), None)) is None
    monkeypatch.setattr(tools, 'window_on_screen', lambda pid, r, window=None: False)
    assert asyncio.run(tool.point(tools.MacClick.Input(app='x', shot='shot1', x=200, y=100), None)) is None
    monkeypatch.setattr(tools, 'window_on_screen', lambda pid, r, window=None: True)
    monkeypatch.setattr(tools, 'display_of', lambda r: None)
    assert asyncio.run(tool.point(tools.MacClick.Input(app='x', shot='shot1', x=200, y=100), None)) is None
    pt = tools.MacPointer(d)
    assert asyncio.run(pt.point(tools.MacPointer.Input(app='x', shot='shot1', x=1, y=1, action='click'), None)) is None


def test_mac_click_points_nowhere_once_its_window_moved(monkeypatch):
    """L33 final review I3: a pixel click maps through the shot's recorded window bounds; once the window moved,
    resized, closed or changed owner (click_at then refuses with StaleRef), that spot is stale: no point."""
    from types import SimpleNamespace
    from ferb.act.mac import tools
    from ferb.act.mac.driver import MacComputerDriver
    from ferb.act.mac.screenshot import Window
    shot = SimpleNamespace(id='shot1', window=5, bounds=(100.0, 50.0, 400.0, 200.0), size=(800, 400))
    d = MacComputerDriver.__new__(MacComputerDriver)
    d.app = lambda name: SimpleNamespace(bundle_id='com.x', pid=3, name='X')
    d._state = {'com.x': SimpleNamespace(shot=shot)}
    monkeypatch.setattr(tools, 'window_on_screen', lambda pid, r, window=None: True)
    monkeypatch.setattr(tools, 'display_of', lambda r: 2)
    tool = tools.MacClick(d)
    for now in [Window(5, 3, (300.0, 50.0, 400.0, 200.0), 'Doc'),     # moved
                Window(5, 3, (100.0, 50.0, 600.0, 200.0), 'Doc'),     # resized
                Window(5, 4, (100.0, 50.0, 400.0, 200.0), 'Doc'),     # another process's window now
                None]:                                                # closed
        d.shots = SimpleNamespace(window=lambda wid, now=now: now)
        assert asyncio.run(tool.point(tools.MacClick.Input(app='x', shot='shot1', x=200, y=100), None)) is None, now


# ---- browser tools report their element's point, placed on the page's screen area ----

WEB_AREA = (200.0, 100.0, 1000.0, 800.0)          # where the browser window draws the tab's page, screen points
TAB_WINDOW = {'bounds': [200, 25, 1000, 875], 'active': True, 'minimized': False, 'title': 'Window'}
BOUNDS = (200.0, 25.0, 1000.0, 875.0)
LONG_PAGE = {'y': 0, 'height': 3000, 'viewport_h': 800, 'x': 0, 'width': 1000}


def _browser(monkeypatch, *, frame=0, rect=(10, 20, 100, 30), window=TAB_WINDOW, scroll=None, container=None,
             areas=(WEB_AREA,), on_screen=True, display=1, layout=(1000, 800), visual=None, pinned=None,
             scroll_box=False, body_scrolls=False, secret=False):
    """A real ext-bridge tab driver after one snapshot (a fake bridge answers it, and acts, scrolls and gestures with
    ok), placed by a real Mac driver whose Accessibility and window list are fakes: the element `e1` (and `e2`, below
    it), the tab's window as the extension reported it."""
    from types import SimpleNamespace
    from ferb.act.browser import tools as btools
    from ferb.act.browser.ext_bridge import _Tab, _TabDriver
    from ferb.act.mac import screenshot
    from ferb.core.types import TabRef
    from tests.mackit import vision
    mac, ax, *_ = vision(monkeypatch)
    ax.tree.web_areas = list(areas)
    asked = []                                        # every look at the browser's windows, Accessibility's or Quartz's
    ax.window_with_frame = lambda pid, bounds, title=None, *a: asked.append(('ax', pid, bounds, title)) or 'win'
    monkeypatch.setattr(screenshot, 'window_on_screen',
                        lambda pid, r, like=None, title=None: asked.append(('cg', pid, r, like, title)) or on_screen)
    monkeypatch.setattr(btools, 'display_of', lambda r: display)
    raw = {'id': 0, 'role': 'button', 'label': 'Go', 'rect': list(rect), **({'container': container} if container else {}),
           **({'pinned': pinned} if pinned else {}), **({'scroll_box': True} if scroll_box else {}),
           **({'secret': True, 'role': 'textbox', 'input_type': 'password'} if secret else {})}
    other = {'id': 1, 'role': 'button', 'label': 'Next', 'rect': [10, 60, 100, 30]}
    top = {'frame_id': 0, 'url': 'https://example.test/', 'title': 'Window', 'text': '',
           'elements': [] if frame else [raw, other], 'viewport': list(layout),
           'visual': list(visual or (0, 0, *layout, 1)), **({'body_scrolls': True} if body_scrolls else {}),
           'scroll': scroll or {'y': 0, 'height': layout[1], 'viewport_h': layout[1], 'x': 0, 'width': layout[0]}}
    child = {'frame_id': frame, 'url': 'https://pay.example/', 'title': '', 'text': '', 'elements': [raw],
             'viewport': [300, 200], 'visual': [0, 0, 300, 200, 1], 'scroll': {'y': 0, 'height': 200, 'viewport_h': 200}}
    reply = {'frames': [top] + ([child] if frame else []), 'dialogs': [], **({'window': window} if window is not None else {})}

    async def call(op, args, **kw):
        return {'snapshot': reply, 'scroll': {'y': 700, 'height': 3000, 'viewport_h': 800, 'frame_id': 0}}.get(
            op, {'ok': True, 'detail': 'done'})
    bridge = SimpleNamespace(gen=1, conn=SimpleNamespace(browser_pid=12), call=call, navs={})
    d = _TabDriver(None, lambda: [])
    ref = TabRef('device', 'brave', 'Default', 1, 't1')
    d._tabs['t1'] = _Tab(ref, bridge, 19, 'task', 'https://example.test/')
    asyncio.run(d.snapshot(ref))
    d.native_place = mac.place_on_web_area
    return d, mac, ax, asked


def _el(d, n=1):
    return f"s{d._tabs['t1'].snap_gen}e{n}"          # an element of the latest snapshot


def _bpoint(d, action='click', tool='act', task='task', **kw):
    from types import SimpleNamespace
    from ferb.act.browser.tools import OWNER_BROWSER, BrowserAct, BrowserLoginCode, BrowserSecret
    cls = {'act': BrowserAct, 'secret': BrowserSecret, 'code': BrowserLoginCode}[tool]
    args = cls.Input(tab='t1', element=_el(d), **({'action': action} if tool == 'act' else {}), **kw)
    return asyncio.run(cls(d, OWNER_BROWSER).point(args, SimpleNamespace(task_id=task)))


def _visual(d):
    """A fresh screenshot whose points e1 and e2 resolved (browser_point)."""
    t = d._tabs['t1']
    t.points.update({_el(d, 1): 'p1', _el(d, 2): 'p2'})
    t.visual = ('bs-1', t.snap_gen, t.snap_nav, time.monotonic(), 1000, 800)
    return t


def _act(d, action, n=2):
    from ferb.core.types import BrowserAction
    ref = d._tabs['t1'].ref
    return asyncio.run(d.act(d.element_ref(ref, _el(d, n)), BrowserAction(action, 'A' if action == 'select' else None),
                             ''))


def test_a_browser_act_points_at_its_element_on_the_pages_screen_area(monkeypatch):
    d, _, _, asked = _browser(monkeypatch)
    assert _bpoint(d) == Point('click', (210.0, 120.0, 100.0, 30.0), 1)
    # the browser's window at the bounds the extension reported, showing the tab (its title); the spot in that very
    # window, on screen
    assert asked == [('ax', 12, BOUNDS, 'Window'), ('cg', 12, (210.0, 120.0, 100.0, 30.0), BOUNDS, 'Window')]


def test_an_element_in_a_frame_gets_no_point(monkeypatch):
    d, _, _, asked = _browser(monkeypatch, frame=1)
    assert _bpoint(d) is None and asked == []          # its rect is its frame's: never placed as if it were the page's


def test_ferbs_own_minimized_window_or_a_tab_not_shown_gets_no_point(monkeypatch):
    for window in [dict(TAB_WINDOW, minimized=True),           # FERB's own window: always minimized
                   dict(TAB_WINDOW, active=False),             # another tab is the one its window shows
                   dict(TAB_WINDOW, emulated=True),            # laid out at FERB's own size, not the window's
                   dict(TAB_WINDOW, bounds=[0, 0, 10]),        # malformed
                   None]:                                      # an older extension says nothing about the window
        d, _, _, asked = _browser(monkeypatch, window=window)
        assert _bpoint(d) is None and asked == [], window   # decided from the snapshot's facts: no Accessibility read


def test_a_browser_point_needs_its_window_its_page_area_and_the_screen(monkeypatch):
    d, mac, ax, _ = _browser(monkeypatch)
    ax.window_with_frame = lambda *a: None                     # no single window of the browser at those bounds
    assert _bpoint(d) is None
    d, mac, ax, _ = _browser(monkeypatch)
    ax.window_title = lambda win: 'Another tab'                # that window shows another tab now
    assert _bpoint(d) is None
    for areas in [(), ((200.0, 100.0, 500.0, 800.0), (700.0, 100.0, 500.0, 800.0))]:   # no page area; a split view
        d, *_ = _browser(monkeypatch, areas=areas)
        assert _bpoint(d) is None
    d, *_ = _browser(monkeypatch, on_screen=False)             # minimized since, on another Space, or moved
    assert _bpoint(d) is None
    d, *_ = _browser(monkeypatch, display=None)
    assert _bpoint(d) is None
    d, mac, *_ = _browser(monkeypatch)
    mac.excluded = lambda: {'com.brave.Browser'}               # an app the owner keeps FERB from looking at
    assert _bpoint(d) is None
    d, *_ = _browser(monkeypatch)
    assert _bpoint(d, task='another-task') is None             # not this task's tab
    d._tabs['t1'].bridge.navs[19] = 1                          # the page changed since the snapshot: a stale element
    assert _bpoint(d) is None
    d, *_ = _browser(monkeypatch, rect=(10, 20, 0, 30))
    assert _bpoint(d) is None


def test_browser_points_have_the_kind_of_their_act(monkeypatch):
    d, *_ = _browser(monkeypatch)
    for action in ('click', 'dblclick', 'rightclick', 'hover'):
        assert _bpoint(d, action).kind == 'click', action
    assert _bpoint(d, 'type', text='x').kind == 'type' and _bpoint(d, 'select', text='A').kind == 'type'
    assert _bpoint(d, 'press', text='Enter').kind == 'press'
    assert _bpoint(d, 'scroll', text='down').kind == 'scroll'
    assert _bpoint(d, 'upload', files=['/tmp/a.txt']) is None  # a file field: often hidden, nothing to point at
    assert _bpoint(d, tool='secret') == Point('type', (210.0, 120.0, 100.0, 30.0), 1)
    assert _bpoint(d, tool='code') == Point('type', (210.0, 120.0, 100.0, 30.0), 1)
    _visual(d)
    assert _bpoint(d, 'drag', end_element=_el(d, 2)) == Point('drag', (210.0, 120.0, 100.0, 30.0), 1)


def test_a_click_points_where_its_act_scrolls_the_element(monkeypatch):
    """The act centres its element in the viewport first (content.js scrollIntoView, block "center"): the point is
    where the element will be, from the snapshot's scroll facts, as far as the page can scroll."""
    d, *_ = _browser(monkeypatch, rect=(10, 700, 100, 30), scroll=LONG_PAGE)
    assert _bpoint(d).rect == (210.0, 485.0, 100.0, 30.0)     # scrolled by 715 - 400 = 315
    assert _bpoint(d, 'select', text='A').rect == (210.0, 800.0, 100.0, 30.0)   # select scrolls nothing
    d, *_ = _browser(monkeypatch, rect=(10, 700, 100, 30), scroll=dict(LONG_PAGE, y=2100))
    assert _bpoint(d).rect == (210.0, 700.0, 100.0, 30.0)     # only 100 more to the page's end
    d, *_ = _browser(monkeypatch, rect=(10, 900, 100, 30), scroll=LONG_PAGE)          # below the viewport
    assert _bpoint(d, 'type', text='x').rect == (210.0, 485.0, 100.0, 30.0)         # real typing clicks it first
    assert _bpoint(d, 'type', text='x', page_mode=True) is None   # page events focus it: where it lands is unknown
    assert _bpoint(d, tool='secret') is None
    d, *_ = _browser(monkeypatch, rect=(1200, 20, 100, 30))    # right of the viewport: a sideways scroll FERB can't tell
    assert _bpoint(d) is None
    d, *_ = _browser(monkeypatch, rect=(10, 700, 100, 30), scroll=LONG_PAGE)
    _visual(d)                                                 # a screenshot point: pressed where it is, no scroll
    assert _bpoint(d).rect == (210.0, 800.0, 100.0, 30.0)


def test_a_typing_point_is_the_part_of_its_field_the_page_shows(monkeypatch):
    """L33 final review M4: the app aims a typing pointer at the field's leading edge, so a field wider than the page's
    visible part (partly scrolled sideways, or built so) is clipped to that part and the arrow stays on the page. A
    click's point keeps the element whole: its centre is where the click lands."""
    d, *_ = _browser(monkeypatch, rect=(-300, 20, 1000, 30))
    assert _bpoint(d, 'type', text='x') == Point('type', (200.0, 120.0, 700.0, 30.0), 1)
    assert _bpoint(d, 'select', text='A').rect == (200.0, 120.0, 700.0, 30.0)
    assert _bpoint(d, tool='secret').rect == (200.0, 120.0, 700.0, 30.0)
    assert _bpoint(d, tool='code').rect == (200.0, 120.0, 700.0, 30.0)
    assert _bpoint(d).rect == (-100.0, 120.0, 1000.0, 30.0)
    d, *_ = _browser(monkeypatch, rect=(400, 20, 1000, 30), visual=(0, 0, 985, 785, 1))   # up to a classic scrollbar
    assert _bpoint(d, tool='secret').rect == (600.0, 120.0, 585.0, 30.0)


def test_a_centred_act_in_a_scroll_container_gets_no_point(monkeypatch):
    d, *_ = _browser(monkeypatch, container='c1')
    assert _bpoint(d) is None                                  # where scrolling its container puts it isn't known
    assert _bpoint(d, 'select', text='A') == Point('type', (210.0, 120.0, 100.0, 30.0), 1)


def test_a_fixed_or_sticky_element_is_pointed_only_where_it_stays(monkeypatch):
    d, *_ = _browser(monkeypatch, pinned='fixed', scroll=LONG_PAGE)
    assert _bpoint(d) is None                                  # a centring act: where it ends up isn't known
    assert _bpoint(d, 'select', text='A').rect == (210.0, 120.0, 100.0, 30.0)
    _act(d, 'click')                                           # the page scrolled since the snapshot; a fixed one didn't
    assert _bpoint(d, 'select', text='A').rect == (210.0, 120.0, 100.0, 30.0)
    d, *_ = _browser(monkeypatch, pinned='sticky', scroll=LONG_PAGE)
    assert _bpoint(d) is None and _bpoint(d, 'select', text='A') is None
    d, *_ = _browser(monkeypatch, pinned='unknown', scroll=LONG_PAGE)   # fixed in a transformed box: it scrolls too
    assert _bpoint(d) is None and _bpoint(d, 'select', text='A') is None
    d, *_ = _browser(monkeypatch, pinned='fixed', scroll_box=True, scroll=LONG_PAGE)   # in a fixed panel that scrolls
    assert _bpoint(d, 'select', text='A') is not None
    _act(d, 'scroll')
    assert _bpoint(d, 'select', text='A') is None


def test_a_page_or_a_box_that_scrolls_sideways_gets_no_centred_point(monkeypatch):
    d, *_ = _browser(monkeypatch, scroll=dict(LONG_PAGE, width=1500))   # the page itself scrolls sideways
    assert _bpoint(d) is None and _bpoint(d, 'select', text='A') is not None
    d, *_ = _browser(monkeypatch, scroll_box=True)             # a carousel, or a box whose overflow is hidden
    assert _bpoint(d) is None and _bpoint(d, 'select', text='A') is not None
    d, *_ = _browser(monkeypatch, scroll={'y': 0, 'height': 800, 'viewport_h': 800})   # no sideways facts: unknown
    assert _bpoint(d) is None and _bpoint(d, 'select', text='A') is not None


def test_a_body_that_scrolls_by_itself_gets_no_centred_point(monkeypatch):
    d, *_ = _browser(monkeypatch, body_scrolls=True)           # html{overflow:hidden} body{overflow:auto}
    assert _bpoint(d) is None and _bpoint(d, 'select', text='A') is not None


def test_after_ferbs_own_scroll_only_centred_points_hold_until_the_next_snapshot(monkeypatch):
    d, *_ = _browser(monkeypatch, rect=(10, 700, 100, 30), scroll=LONG_PAGE)
    _act(d, 'click')                                           # it centred e2: the page moved under e1's snapshot rect
    assert _bpoint(d, 'select', text='A') is None
    assert _bpoint(d).rect == (210.0, 485.0, 100.0, 30.0)     # a centring act lands it by its place in the page
    asyncio.run(d.snapshot(d._tabs['t1'].ref))                 # a new look: its rects are current again
    assert _bpoint(d, 'select', text='A').rect == (210.0, 800.0, 100.0, 30.0)
    asyncio.run(d.scroll(d._tabs['t1'].ref, 'down'))
    assert _bpoint(d, 'select', text='A') is None
    asyncio.run(d.snapshot(d._tabs['t1'].ref))
    _act(d, 'select')                                          # a select scrolls nothing
    assert _bpoint(d, 'select', text='A') is not None
    _visual(d)
    _act(d, 'click')                                           # a screenshot gesture (`_pointer`) counts too
    assert d._tabs['t1'].moved


def test_classic_scrollbars_map_with_the_layout_viewport(monkeypatch):
    """With scrollbars always shown, the visual viewport leaves out the scrollbar (985 of 1000 CSS px); the page area
    holds the whole layout viewport, so placement uses it (pinch zoom 1), never a stretch by 1000/985."""
    d, *_ = _browser(monkeypatch, rect=(900, 20, 50, 30), visual=(0, 0, 985, 785, 1))
    assert _bpoint(d, 'select', text='A').rect == (1100.0, 120.0, 50.0, 30.0)
    assert _bpoint(d).rect == (1100.0, 120.0, 50.0, 30.0)


def test_a_pinch_zoomed_page_is_placed_from_its_visual_viewport(monkeypatch):
    d, *_ = _browser(monkeypatch, rect=(200, 150, 50, 20), visual=(100, 50, 500, 400, 2))
    assert _bpoint(d, 'select', text='A').rect == (400.0, 300.0, 100.0, 40.0)      # 2x: (200-100)*2, (150-50)*2
    assert _bpoint(d) is None                                  # how a centring scroll moves a zoomed view: unknown


def test_acts_the_driver_refuses_publish_no_point(monkeypatch):
    d, *_ = _browser(monkeypatch)
    assert _bpoint(d, 'drag', end_element=_el(d, 2)) is None   # a drag needs two points from one screenshot
    t = _visual(d)
    assert _bpoint(d, 'type', text='x') is None                # typing on a screenshot point is refused
    assert _bpoint(d, page_mode=True) is None                  # so are page events
    t.visual = ('bs-1', t.snap_gen, t.snap_nav, time.monotonic() - 31, 1000, 800)
    assert _bpoint(d) is None                                  # a screenshot older than 30 s
    t.visual = None
    assert _bpoint(d) is None                                  # one an act used up
    d, *_ = _browser(monkeypatch, secret=True)
    assert _bpoint(d, 'type', text='x') is None                # FERB never types into a password or code field
    assert _bpoint(d, tool='secret').kind == 'type'            # its hidden fill does point there


def test_the_browser_placement_reads_accessibility_off_the_event_loop(monkeypatch):
    import threading
    d, mac, ax, _ = _browser(monkeypatch)
    seen = []
    ax.window_with_frame = lambda *a: seen.append(threading.current_thread()) or 'win'
    assert _bpoint(d) is not None and seen and seen[0] is not threading.main_thread()


def test_web_areas_of_reads_the_browsers_own_controls_only(monkeypatch):
    from ferb.act.mac import accessibility as axm
    tree = {'win': ['toolbar', 'view'], 'toolbar': ['back'], 'back': [], 'view': ['page', 'tools'],
            'page': ['deep'], 'tools': []}
    roles = {'win': 'AXWindow', 'page': 'AXWebArea', 'tools': 'AXWebArea', 'back': 'AXButton'}
    frames = {'page': (200.0, 100.0, 1000.0, 600.0), 'tools': (200.0, 700.0, 1000.0, 200.0)}
    read = []

    def attr(el, name):
        read.append((el, name))
        return {'AXRole': roles.get(el, 'AXGroup'), 'AXChildren': tree.get(el)}.get(name)
    monkeypatch.setattr(axm, '_attr', attr)
    monkeypatch.setattr(axm, '_frame_of', lambda el: frames[el])
    assert sorted(axm.web_areas_of('win')) == sorted(frames.values())
    assert not [r for r in read if r[0] in ('page', 'deep', 'tools') and r[1] == 'AXChildren']   # never walked into
    tree['view'] = ['view', 'page']                            # a cycle: it can't say, never loops
    assert axm.web_areas_of('win') is None


def test_the_daemon_wires_the_cursor_placement(daemon, monkeypatch):
    from tests.mackit import _fake_mac
    from ferb.act.mac import screenshot
    assert daemon.daemon.router.native_place == daemon.daemon._native_place
    _fake_mac(daemon, monkeypatch)
    daemon.daemon.mac.ax.tree.web_areas = [WEB_AREA]
    monkeypatch.setattr(screenshot, 'window_on_screen', lambda pid, r, like=None, title=None: True)
    got = daemon.run(daemon.daemon._native_place((10.0, 20.0, 100.0, 30.0), (0.0, 0.0, 1000.0, 800.0), 12,
                                                 (200.0, 25.0, 1000.0, 875.0), 'Window'))
    assert got == (210.0, 120.0, 100.0, 30.0)
    mac, daemon.daemon.mac = daemon.daemon.mac, None
    try:
        assert daemon.run(daemon.daemon._native_place((10.0, 20.0, 100.0, 30.0), (0.0, 0.0, 1000.0, 800.0), 12,
                                                      (200.0, 25.0, 1000.0, 875.0), 'Window')) is None
    finally:
        daemon.daemon.mac = mac


def test_the_extensions_snapshot_says_where_the_tab_is_shown():
    """Extension 0.15.9 (optional fields, protocol 17 unchanged): the snapshot reply carries the tab's window (bounds,
    whether it shows the tab, minimized, title), and each frame its visual viewport with its pinch zoom and its own
    scroll sideways too."""
    import json
    import subprocess
    from tests.chromelab import EXT, _node, worker
    out = worker('''(async () => {
      inject = async () => {}; attach = async () => { throw new Error('no debugger'); };
      chrome.scripting = {executeScript: async () => [{frameId: 0, result: {elements: [], text: '', iframes: [],
                                                                          viewport: [1000, 800]}}]};
      chrome.tabs.get = async id => ({id, windowId: 3, active: true, title: 'Go'});
      chrome.windows = {get: async id => ({id, left: 200, top: 25, width: 1000, height: 875, state: 'normal'})};
      return await commands.snapshot({tab_id: 19, snap: 1});
    })()''')
    assert out['window'] == {'bounds': [200, 25, 1000, 875], 'active': True, 'minimized': False, 'title': 'Go'}
    harness = r'''
const vm = require('vm'), fs = require('fs');
const root = {setAttribute() {}, getAttribute() { return null; }, removeAttribute() {}, scrollTop: 0, scrollHeight: 900,
  scrollLeft: 0, scrollWidth: 1000};
const doc = {documentElement: root, scrollingElement: root, body: {}, title: 'Go', querySelectorAll: () => [],
  createTreeWalker: () => ({nextNode: () => null}), createRange: () => ({})};
const ctx = vm.createContext({document: doc, window: {visualViewport: {offsetLeft: 100, offsetTop: 50, width: 500,
  height: 400, scale: 2}}, top: null, location: {href: 'https://example.test/'}, crypto: globalThis.crypto, WeakRef, URL,
  setTimeout, innerWidth: 1000, innerHeight: 800, addEventListener() {}, MutationObserver: class { observe() {} },
  ShadowRoot: class {}, NodeFilter: {SHOW_TEXT: 4, FILTER_REJECT: 2, FILTER_ACCEPT: 1}});
ctx.top = ctx.window;
vm.runInContext(fs.readFileSync(process.argv[1], 'utf8'), ctx);
const s = ctx.window.__ferb.snapshot(1);
console.log(JSON.stringify({viewport: s.viewport, visual: s.visual, scroll: s.scroll, body: s.body_scrolls}));
'''
    result = subprocess.run([_node(), '-e', harness, str(EXT / 'content.js')], capture_output=True, text=True, timeout=10)
    assert result.returncode == 0, result.stderr
    assert json.loads(result.stdout) == {'viewport': [1000, 800], 'visual': [100, 50, 500, 400, 2],
                                         'scroll': {'y': 0, 'height': 900, 'viewport_h': 800, 'at_end': False,
                                                    'x': 0, 'width': 1000}}


def test_the_extension_scrolls_instantly_before_reading_where_to_aim():
    """Review (6): under `scroll-behavior: smooth` a bare scrollIntoView animates, so the rect read right after it was
    the one before the scroll and real input aimed at the wrong place. Both centring scrolls are instant."""
    import json
    import subprocess
    from tests.chromelab import EXT, _node
    script = r'''
const vm = require('vm'), fs = require('fs'); const seen = [];
const el = {isConnected: true, tagName: 'BUTTON', disabled: false, isContentEditable: false,
  scrollIntoView(o) { seen.push(o); }, getAttribute() { return null; }, getRootNode() { return doc; },
  getBoundingClientRect() { return {x: 10, y: 20, width: 30, height: 10, left: 10, top: 20, right: 40, bottom: 30}; },
  contains(n) { return n === el; }, click() {}, dispatchEvent() {}};
const doc = {documentElement: {}, body: {}, elementFromPoint: () => el};
const ctx = vm.createContext({document: doc, window: {}, crypto: globalThis.crypto, WeakRef, innerWidth: 800,
  innerHeight: 600, addEventListener() {}, MutationObserver: class { observe() {} }, ShadowRoot: class {},
  MouseEvent: class {}, PointerEvent: class {}, el});
const source = fs.readFileSync(process.argv[1], 'utf8').replace('let els = new Map();',
  'let els = new Map([[1, new WeakRef(el)]]);').replace('let snapGen = 0;', 'let snapGen = 1;');
vm.runInContext(source, ctx);
ctx.window.__ferb.rect(1, 1);
ctx.window.__ferb.act(1, 1, 'click');
console.log(JSON.stringify(seen));
'''
    result = subprocess.run([_node(), '-e', script, str(EXT / 'content.js')], capture_output=True, text=True, timeout=10)
    assert result.returncode == 0, result.stderr
    assert json.loads(result.stdout) == [{'block': 'center', 'inline': 'center', 'behavior': 'instant'}] * 2


def test_the_extension_says_what_a_scroll_into_view_moves_besides_the_page():
    """Review (1-3; round 2): an element that is fixed or sticky (itself or an ancestor as the page is drawn, through
    the shadow tree a slotted element is drawn in; sticky wins), "unknown" for a fixed one inside a containing block
    (transform, filter, contain, will-change...: it scrolls with the page after all), one inside a box that scrolls on
    either axis (a carousel, hidden overflow, also in a shadow tree), and a body that scrolls by itself."""
    import json
    import subprocess
    from tests.chromelab import EXT, _node
    source = (EXT / 'content.js').read_text()
    block = source[source.index('  // L33: what a scroll into view moves'):source.index('  function hitTest(')]
    flat = (source[source.index('  const shadowOf = (el) =>'):source.index('  function eachElement(')]
            + source[source.index('  function drawnParent(el) {'):source.index('  // Whether the element is drawn again')])
    script = r'''
const chrome = {}, styles = new Map(), page = {};
const getComputedStyle = n => styles.get(n);
const node = (parent, style = {}, size = {}) => {
  const n = {parentElement: parent, scrollWidth: 100, clientWidth: 100, scrollHeight: 100, clientHeight: 100,
             getRootNode: () => page, ...size};
  styles.set(n, {position: 'static', overflowX: 'visible', overflowY: 'visible', transform: 'none', filter: 'none',
                 perspective: 'none', contain: 'none', willChange: 'auto', ...style});
  return n;
};
const shadow = (host, style, size) => {           // host's shadow root: a wrapper around the <slot> its child is drawn in
  const sr = {host, querySelectorAll: () => []};
  host.shadowRoot = sr;
  const wrap = node(null, style, size); wrap.getRootNode = () => sr;
  const slot = node(wrap); slot.getRootNode = () => sr;
  const light = node(host); light.assignedSlot = slot;
  return light;
};
const root = node(null), body = node(root, {}, {scrollHeight: 3000});
const document = {documentElement: root, body, scrollingElement: root};
''' + flat + block + r'''
const header = node(body, {position: 'fixed'}), inHeader = node(node(header));
const bar = node(body, {position: 'sticky'}), inBar = node(node(bar, {position: 'fixed'}));
const slotted = shadow(node(body), {position: 'fixed'});
const turned = node(node(body, {transform: 'matrix(1, 0, 0, 1, 0, 0)'}), {position: 'fixed'}), inTurned = node(turned);
const willTurn = node(node(body, {willChange: 'transform'}), {position: 'fixed'});
const contained = node(node(body, {contain: 'paint'}), {position: 'fixed'});
const turnedInside = node(node(node(body, {position: 'fixed'}), {transform: 'matrix(1, 0, 0, 1, 0, 0)'}));
const inTransformed = node(node(body, {transform: 'matrix(1, 0, 0, 1, 0, 0)'}));
const carousel = node(body, {overflowX: 'auto'}, {scrollWidth: 900}), slide = node(node(carousel));
const clipped = node(body, {overflowX: 'hidden'}, {scrollWidth: 600}), past = node(clipped);
const short = node(body, {overflowY: 'auto'}), inShort = node(short);      // nothing more to show: it can't scroll
const shown = node(body, {overflowX: 'clip'}, {scrollWidth: 600}), inShown = node(shown);   // clip never scrolls
const shadowSlide = shadow(node(body), {overflowX: 'auto'}, {scrollWidth: 900});
const plain = node(body), inPlain = node(plain);
const out = {pins: [pinnedOf(inHeader), pinnedOf(inBar), pinnedOf(slotted), pinnedOf(inTurned), pinnedOf(willTurn),
                    pinnedOf(contained), pinnedOf(turnedInside), pinnedOf(inTransformed), pinnedOf(inPlain)],
             boxes: [boxedBy(drawnParent(slide)), boxedBy(drawnParent(past)), boxedBy(drawnParent(inShort)),
                     boxedBy(drawnParent(inShown)), boxedBy(drawnParent(shadowSlide)), boxedBy(drawnParent(inPlain))],
             body: [bodyScrolls()]};
styles.get(body).overflowY = 'auto';                    // body{overflow:auto} with html's overflow visible: the page's
out.body.push(bodyScrolls());
styles.get(root).overflowY = 'hidden'; styles.get(root).overflowX = 'hidden';   // html{overflow:hidden}: body's own
out.body.push(bodyScrolls());
document.scrollingElement = body;                       // quirks mode: the body is the page's scroller
out.body.push(bodyScrolls());
console.log(JSON.stringify(out));
'''
    result = subprocess.run([_node(), '-e', script], capture_output=True, text=True, timeout=10)
    assert result.returncode == 0, result.stderr
    assert json.loads(result.stdout) == {
        'pins': ['fixed', 'sticky', 'fixed', 'unknown', 'unknown', 'unknown', 'fixed', None, None],
        'boxes': [True, True, False, False, True, False], 'body': [False, False, True, False]}


def test_the_extensions_page_scroll_is_instant_so_it_reports_where_it_ended():
    """Review round 2 (3): on a `scroll-behavior: smooth` page a bare scrollBy animates, so the position it reported
    was the one before the scroll. The scroll act's scrollBy is instant, for a container and for the page."""
    import json
    import subprocess
    from tests.chromelab import EXT, _node
    script = r'''
const vm = require('vm'), fs = require('fs'); const seen = [];
const box = (overflowY, name) => ({isConnected: true, tagName: 'DIV', scrollTop: 0, scrollHeight: 2000, clientHeight: 400,
  parentElement: null, style: {overflowY}, getRootNode() { return doc; }, scrollBy(...a) { seen.push([name, ...a]); }});
const el = box('auto', 'box'), plain = box('visible', 'plain');
const root = {scrollTop: 0, scrollHeight: 3000};
const doc = {documentElement: root, scrollingElement: root, body: {}};
const ctx = vm.createContext({document: doc, window: {scrollBy(...a) { seen.push(['page', ...a]); }},
  crypto: globalThis.crypto, WeakRef, innerWidth: 800, innerHeight: 600, addEventListener() {},
  MutationObserver: class { observe() {} }, ShadowRoot: class {}, getComputedStyle: n => n.style, el, plain});
const source = fs.readFileSync(process.argv[1], 'utf8').replace('let els = new Map();',
  'let els = new Map([[1, new WeakRef(el)], [2, new WeakRef(plain)]]);').replace('let snapGen = 0;', 'let snapGen = 1;');
vm.runInContext(source, ctx);
ctx.window.__ferb.act(1, 1, 'scroll', 'down');           // it scrolls itself
ctx.window.__ferb.act(1, 2, 'scroll', 'up');             // no scroller around it: the page
console.log(JSON.stringify(seen));
'''
    result = subprocess.run([_node(), '-e', script, str(EXT / 'content.js')], capture_output=True, text=True, timeout=10)
    assert result.returncode == 0, result.stderr
    assert json.loads(result.stdout) == [['box', {'top': 340, 'behavior': 'instant'}],
                                         ['page', {'top': -510, 'behavior': 'instant'}]]
