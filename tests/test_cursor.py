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
    assert got == Point('click', (196.0, 96.0, 8.0, 8.0), 2) and seen == [(3, (196.0, 96.0, 8.0, 8.0))]
    assert asyncio.run(tool.point(tools.MacClick.Input(app='x', shot='old', x=200, y=100), None)) is None
    assert asyncio.run(tool.point(tools.MacClick.Input(app='x', shot='shot1', x=900, y=100), None)) is None
    monkeypatch.setattr(tools, 'window_on_screen', lambda pid, r: False)
    assert asyncio.run(tool.point(tools.MacClick.Input(app='x', shot='shot1', x=200, y=100), None)) is None
    monkeypatch.setattr(tools, 'window_on_screen', lambda pid, r: True)
    monkeypatch.setattr(tools, 'display_of', lambda r: None)
    assert asyncio.run(tool.point(tools.MacClick.Input(app='x', shot='shot1', x=200, y=100), None)) is None
    pt = tools.MacPointer(d)
    assert asyncio.run(pt.point(tools.MacPointer.Input(app='x', shot='shot1', x=1, y=1, action='click'), None)) is None


# ---- browser tools report their element's point, placed on the page's screen area ----

WEB_AREA = (200.0, 100.0, 1000.0, 800.0)          # where the browser window draws the tab's page, screen points
TAB_WINDOW = {'bounds': [200, 25, 1000, 875], 'active': True, 'minimized': False, 'title': 'Window'}


def _browser(monkeypatch, *, frame=0, rect=(10, 20, 100, 30), window=TAB_WINDOW, scroll=None, container=None,
             areas=(WEB_AREA,), on_screen=True, display=1, layout=(1000, 800), visual=None):
    """A real ext-bridge tab driver after one snapshot (a fake bridge answers it), placed by a real Mac driver whose
    Accessibility and window list are fakes: one element `s1e1`, the tab's window as the extension reported it."""
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
    monkeypatch.setattr(screenshot, 'window_on_screen', lambda pid, r: asked.append(('cg', pid, r)) or on_screen)
    monkeypatch.setattr(btools, 'display_of', lambda r: display)
    raw = {'id': 0, 'role': 'button', 'label': 'Go', 'rect': list(rect), **({'container': container} if container else {})}
    top = {'frame_id': 0, 'url': 'https://example.test/', 'title': 'Window', 'text': '', 'elements': [] if frame else [raw],
           'viewport': list(layout), 'visual': list(visual or (0, 0, *layout)),
           'scroll': scroll or {'y': 0, 'height': layout[1], 'viewport_h': layout[1], 'at_end': True}}
    child = {'frame_id': frame, 'url': 'https://pay.example/', 'title': '', 'text': '', 'elements': [raw],
             'viewport': [300, 200], 'visual': [0, 0, 300, 200], 'scroll': {'y': 0, 'height': 200, 'viewport_h': 200}}
    reply = {'frames': [top] + ([child] if frame else []), 'dialogs': [], **({'window': window} if window is not None else {})}

    async def call(op, args, **kw):
        return reply
    bridge = SimpleNamespace(gen=1, conn=SimpleNamespace(browser_pid=12), call=call, navs={})
    d = _TabDriver(None, lambda: [])
    ref = TabRef('device', 'brave', 'Default', 1, 't1')
    d._tabs['t1'] = _Tab(ref, bridge, 19, 'task', 'https://example.test/')
    asyncio.run(d.snapshot(ref))
    d.native_place = mac.place_on_web_area
    return d, mac, ax, asked


def _bpoint(d, action='click', tool='act', task='task', **kw):
    from types import SimpleNamespace
    from ferb.act.browser.tools import OWNER_BROWSER, BrowserAct, BrowserLoginCode, BrowserSecret
    cls = {'act': BrowserAct, 'secret': BrowserSecret, 'code': BrowserLoginCode}[tool]
    args = cls.Input(tab='t1', element='s1e1', **({'action': action} if tool == 'act' else {}), **kw)
    return asyncio.run(cls(d, OWNER_BROWSER).point(args, SimpleNamespace(task_id=task)))


def test_a_browser_act_points_at_its_element_on_the_pages_screen_area(monkeypatch):
    d, _, _, asked = _browser(monkeypatch)
    assert _bpoint(d) == Point('click', (210.0, 120.0, 100.0, 30.0), 1)
    # the browser's window at the bounds the extension reported, showing the tab (its title); the spot on screen
    assert asked == [('ax', 12, (200.0, 25.0, 1000.0, 875.0), 'Window'), ('cg', 12, (210.0, 120.0, 100.0, 30.0))]


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
    d, *_ = _browser(monkeypatch, on_screen=False)             # minimized since, or on another Space
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


def test_a_click_points_where_its_act_scrolls_the_element(monkeypatch):
    """The act centres its element in the viewport first (content.js scrollIntoView, block "center"): the point is
    where the element will be, from the snapshot's scroll facts, as far as the page can scroll."""
    long_page = {'y': 0, 'height': 3000, 'viewport_h': 800}
    d, *_ = _browser(monkeypatch, rect=(10, 700, 100, 30), scroll=long_page)
    assert _bpoint(d).rect == (210.0, 485.0, 100.0, 30.0)     # scrolled by 715 - 400 = 315
    assert _bpoint(d, 'select', text='A').rect == (210.0, 800.0, 100.0, 30.0)   # select scrolls nothing
    d, *_ = _browser(monkeypatch, rect=(10, 700, 100, 30), scroll=dict(long_page, y=2100))
    assert _bpoint(d).rect == (210.0, 700.0, 100.0, 30.0)     # only 100 more to the page's end
    d, *_ = _browser(monkeypatch, rect=(10, 900, 100, 30), scroll=long_page)          # below the viewport
    assert _bpoint(d, 'type', text='x').rect == (210.0, 485.0, 100.0, 30.0)         # real typing clicks it first
    assert _bpoint(d, 'type', text='x', page_mode=True) is None   # page events focus it: where it lands is unknown
    assert _bpoint(d, tool='secret') is None
    d, *_ = _browser(monkeypatch, rect=(1200, 20, 100, 30))    # right of the viewport: a sideways scroll FERB can't tell
    assert _bpoint(d) is None
    d, *_ = _browser(monkeypatch, rect=(10, 700, 100, 30), scroll=long_page)
    d._tabs['t1'].points['s1e1'] = 'pointer-token'             # a screenshot point: pressed where it is, no scroll
    assert _bpoint(d).rect == (210.0, 800.0, 100.0, 30.0)
    assert _bpoint(d, 'drag', end_element='s1e2') == Point('drag', (210.0, 800.0, 100.0, 30.0), 1)


def test_a_centred_act_in_a_scroll_container_gets_no_point(monkeypatch):
    d, *_ = _browser(monkeypatch, container='c1')
    assert _bpoint(d) is None                                  # where scrolling its container puts it isn't known
    assert _bpoint(d, 'select', text='A') == Point('type', (210.0, 120.0, 100.0, 30.0), 1)


def test_a_pinch_zoomed_page_is_placed_from_its_visual_viewport(monkeypatch):
    d, *_ = _browser(monkeypatch, rect=(200, 150, 50, 20), visual=(100, 50, 500, 400))
    assert _bpoint(d, 'select', text='A').rect == (400.0, 300.0, 100.0, 40.0)      # 2x: (200-100)*2, (150-50)*2
    assert _bpoint(d) is None                                  # how a centring scroll moves a zoomed view: unknown


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
    monkeypatch.setattr(screenshot, 'window_on_screen', lambda pid, r: True)
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
    """Extension 0.15.8 (optional fields, protocol 17 unchanged): the snapshot reply carries the tab's window (bounds,
    whether it shows the tab, minimized, title), and the top frame its visual viewport."""
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
const root = {setAttribute() {}, getAttribute() { return null; }, removeAttribute() {}, scrollTop: 0, scrollHeight: 900};
const doc = {documentElement: root, scrollingElement: root, body: {}, title: 'Go', querySelectorAll: () => [],
  createTreeWalker: () => ({nextNode: () => null}), createRange: () => ({})};
const ctx = vm.createContext({document: doc, window: {visualViewport: {offsetLeft: 100, offsetTop: 50, width: 500,
  height: 400}}, top: null, location: {href: 'https://example.test/'}, crypto: globalThis.crypto, WeakRef, URL,
  setTimeout, innerWidth: 1000, innerHeight: 800, addEventListener() {}, MutationObserver: class { observe() {} },
  ShadowRoot: class {}, NodeFilter: {SHOW_TEXT: 4, FILTER_REJECT: 2, FILTER_ACCEPT: 1}});
ctx.top = ctx.window;
vm.runInContext(fs.readFileSync(process.argv[1], 'utf8'), ctx);
const s = ctx.window.__ferb.snapshot(1);
console.log(JSON.stringify({viewport: s.viewport, visual: s.visual}));
'''
    result = subprocess.run([_node(), '-e', harness, str(EXT / 'content.js')], capture_output=True, text=True, timeout=10)
    assert result.returncode == 0, result.stderr
    assert json.loads(result.stdout) == {'viewport': [1000, 800], 'visual': [100, 50, 500, 400]}
