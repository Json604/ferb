import asyncio

from ferb.act.mac.accessibility import AxNode, AxTree
from ferb.core.types import ComputerAction
from tests.test_background_input import ScrollAx, setup, ids


def test_reveal_row_is_not_directional_scrolling(monkeypatch):
    class Rows(ScrollAx):
        def read_tree(self, *args, **kwargs):
            return AxTree([AxNode('row', 'row', 'Row', None, (0, 0, 10, 10), False, True,
                                 'AXRow', ('scroll_to_visible',))], '', 'Untitled')
        def perform(self, el, action):
            if action == 'AXScrollToVisible':
                raise AssertionError('reveal is not scrolling')
            return super().perform(el, action)
    ax = Rows(keys_scroll=False)
    d, apps, posted = setup(monkeypatch, ax)
    r = asyncio.run(d.act('WhatsApp', ComputerAction('scroll', ids(d)['Row'], direction='down', amount=4)))
    assert r.ok and ax.position == 4
    assert posted == [] and apps.activated == []


def test_scroll_stops_batch_at_observed_boundary(monkeypatch):
    class Bounded(ScrollAx):
        def perform(self, el, action):
            self.position = min(10, self.position + 1)
            return True
    ax = Bounded(position=8)
    d, apps, posted = setup(monkeypatch, ax)
    r = asyncio.run(d.act('WhatsApp', ComputerAction('scroll', ids(d)['Document'], direction='down', amount=40)))
    assert r.ok and 'boundary' in r.detail and '2' in r.detail
    assert ax.position == 10 and posted == [] and apps.activated == []


def test_unknown_scroll_position_is_not_a_boundary(monkeypatch):
    class Unknown(ScrollAx):
        def scroll_position(self, *args):
            return (None, None, None)
        def perform(self, *args):
            return True
    ax = Unknown()
    d, _, _ = setup(monkeypatch, ax)
    r = asyncio.run(d.act('WhatsApp', ComputerAction('scroll', ids(d)['Document'], amount=40)))
    assert not r.ok and 'not confirmed' in r.detail and 'boundary reached' not in r.detail


def test_scroll_container_discovery_handles_lists_and_ambiguity(monkeypatch):
    from ferb.act.mac import accessibility as ax
    nodes = {'win': {'AXChildren': ['list', 'other']}, 'list': {'AXRole': 'AXList'},
             'other': {'AXRole': 'AXScrollArea'}, 'row': {'AXRole': 'AXRow', 'AXParent': 'list'}}
    monkeypatch.setattr(ax, '_attr', lambda el, key: nodes.get(el, {}).get(key))
    assert ax.scroll_area_of('row') == 'list'
    assert ax.scroll_area_in('win') is None  # don't silently pick the wrong pane


def test_trace_shows_scroll_direction_and_amount():
    from ferb.cli.admin import _span_text
    label, _ = _span_text({'kind': 'tool.call', 'error': None,
                          'input': {'tool': 'mac.pointer', 'args': {'action': 'scroll', 'direction': 'down',
                                    'amount': 12, 'x': 20, 'y': 30}}, 'output': {}}, False)
    assert '↓' in label and 'amount=12' in label and 'x=20' in label


def test_container_progress_tracks_visible_children_without_scrollbar_values(monkeypatch):
    from ferb.act.mac import accessibility as ax
    state = {'offset': 0}
    attrs = {'area': {'AXChildren': ['row', 'offscreen']}, 'row': {'AXRole': 'AXButton'},
             'offscreen': {'AXRole': 'AXGroup'}}
    monkeypatch.setattr(ax, '_attr', lambda el, key: attrs.get(el, {}).get(key))
    monkeypatch.setattr(ax, 'frame', lambda el: {'area': (0,0,300,600),
        'row': (0,100-state['offset'],300,60), 'offscreen': (0,5000,300,60)}.get(el))
    before = ax.scroll_position('area')
    state['offset'] = 40
    after = ax.scroll_position('area')
    assert before != after


def test_pixel_scroll_uses_viewport_sized_steps(monkeypatch):
    import ferb.act.mac.driver as driver
    from ferb.act.mac.input import Step
    from tests.test_mac_vision import vision
    d, _, _, _, _ = vision(monkeypatch)
    monkeypatch.setattr(driver, 'load_route', lambda: None)
    calls = []
    monkeypatch.setattr(driver, 'wheel_steps', lambda *a, **kw: [Step('wheel', lambda: calls.append((a,kw)))])
    async def go():
        _, shot, _ = await d.screenshot('Notes')
        await d.pointer_at('Notes', shot.id, 100, 100, 'scroll', amount=1)
    asyncio.run(go())
    assert calls[0][1]['pixels'] is True
    assert calls[0][0][2] == -480  # 80% of a 600-point viewport, down


def test_non_numeric_scrollbar_value_is_unknown(monkeypatch):
    from ferb.act.mac import accessibility as ax
    monkeypatch.setattr(ax, '_attr', lambda el, key: 'bar' if key == 'AXVerticalScrollBar' else
                        'not a numeric position' if el == 'bar' and key == 'AXValue' else None)
    monkeypatch.setattr(ax, 'frame', lambda el: None)
    assert ax.scroll_position('area')[0] is None


def test_pointer_scroll_supports_small_viewport_fraction(monkeypatch):
    import ferb.act.mac.driver as driver
    from ferb.act.mac.input import Step
    from ferb.act.mac.tools import MacPointer
    from tests.test_mac_vision import vision
    d, _, _, _, _ = vision(monkeypatch)
    monkeypatch.setattr(driver, 'load_route', lambda: None)
    distances = []
    monkeypatch.setattr(driver, 'wheel_steps', lambda *a, **kw: [Step('wheel', lambda: distances.append(a[2]))])
    args = MacPointer.Input(app='Notes', shot='shot1', x=100, y=100, action='scroll', distance=0.05)
    async def go():
        _, shot, _ = await d.screenshot('Notes')
        await d.pointer_at('Notes', shot.id, 100, 100, 'scroll', distance=args.distance)
    asyncio.run(go())
    assert distances == [-30]  # a small adjustment, not a forced full page
