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

def _mac(monkeypatch, rect=(100, 200, 50, 20), stale=False, on_screen=True, display=1, tool='act'):
    from types import SimpleNamespace
    from ferb.act.mac import tools
    from ferb.core.errors import StaleRef
    from ferb.act.mac.accessibility import AxNode
    class Driver:
        def app(self, name):
            return SimpleNamespace(bundle_id='com.x', pid=1, name='X')
        def node(self, bundle, element):
            if stale:
                raise StaleRef('old')
            return AxNode(ref=None, role='button', label='', value=None, rect=rect, secret=False, enabled=True)
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
    seen = []
    orig = tool.driver.node
    tool.driver.node = lambda b, e: (seen.append(threading.current_thread()), orig(b, e))[1]
    main = threading.current_thread()
    assert _point(tool, cls, element='s1e2', action='click') is not None
    assert seen and seen[0] is not main


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


def test_mac_click_points_at_where_its_pixels_land(monkeypatch):
    from types import SimpleNamespace
    from ferb.act.mac import tools
    from ferb.act.mac.driver import MacComputerDriver
    shot = SimpleNamespace(id='shot1', bounds=(100.0, 50.0, 400.0, 200.0), size=(800, 400))
    d = MacComputerDriver.__new__(MacComputerDriver)
    d.app = lambda name: SimpleNamespace(bundle_id='com.x', pid=3, name='X')
    d._state = {'com.x': SimpleNamespace(shot=shot)}
    seen = []
    monkeypatch.setattr(tools, 'window_on_screen', lambda pid, r: seen.append((pid, r)) or True)
    monkeypatch.setattr(tools, 'display_of', lambda r: 2)
    tool = tools.MacClick(d)
    got = asyncio.run(tool.point(tools.MacClick.Input(app='x', shot='shot1', x=200, y=100), None))
    assert got == Point('click', (296.0, 96.0, 8.0, 8.0), 2) and seen == [(3, (296.0, 96.0, 8.0, 8.0))]
    assert asyncio.run(tool.point(tools.MacClick.Input(app='x', shot='old', x=200, y=100), None)) is None
    assert asyncio.run(tool.point(tools.MacClick.Input(app='x', shot='shot1', x=900, y=100), None)) is None
    monkeypatch.setattr(tools, 'window_on_screen', lambda pid, r: False)
    assert asyncio.run(tool.point(tools.MacClick.Input(app='x', shot='shot1', x=200, y=100), None)) is None
    monkeypatch.setattr(tools, 'window_on_screen', lambda pid, r: True)
    monkeypatch.setattr(tools, 'display_of', lambda r: None)
    assert asyncio.run(tool.point(tools.MacClick.Input(app='x', shot='shot1', x=200, y=100), None)) is None
    pt = tools.MacPointer(d)
    assert asyncio.run(pt.point(tools.MacPointer.Input(app='x', shot='shot1', x=1, y=1, action='click'), None)) is None
