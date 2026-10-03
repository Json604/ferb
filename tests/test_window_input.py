from types import SimpleNamespace
import sys

import pytest


def test_window_route_stamps_exact_target_and_posts_once():
    from ferb.act.mac.window_input import WindowRoute
    calls = []
    lib = SimpleNamespace(
        SLEventSetIntegerValueField=lambda *a: calls.append(('field', *a)),
        SLEventSetWindowLocation=lambda *a: calls.append(('point', *a)),
        SLEventPostToPid=lambda *a: calls.append(('post', *a)))
    event = SimpleNamespace(__c_void_p__=lambda: 123)
    route = WindowRoute(lib)
    route.post(event, 77, 900, (320, 240))
    assert ('field', 123, 40, 77) in calls
    for field in (51, 91, 92):
        assert ('field', 123, field, 900) in calls
    assert ('point', 123, 320, 240) in calls
    assert [c for c in calls if c[0] == 'post'] == [('post', 77, 123)]


def test_loader_fails_closed_when_symbols_unavailable(monkeypatch):
    import ctypes
    from ferb.act.mac.window_input import load_route
    load_route.cache_clear()
    monkeypatch.setattr(ctypes, 'CDLL', lambda *a: SimpleNamespace())
    assert load_route() is None
    load_route.cache_clear()


@pytest.mark.parametrize('kind', ['click', 'drag', 'scroll'])
def test_pointer_routing_separates_screen_and_window_coordinates(monkeypatch, kind):
    """Observed native regression: stamping screen coordinates as window-local adds the window offset twice."""
    from ferb.act.mac.input import pointer_steps, wheel_steps
    events = []
    q = SimpleNamespace(
        CGEventSourceCreate=lambda *_: None, kCGEventSourceStatePrivate=0,
        kCGMouseButtonRight=1, kCGMouseButtonLeft=0, kCGEventRightMouseDown=3, kCGEventRightMouseUp=4,
        kCGEventLeftMouseDown=1, kCGEventLeftMouseUp=2, kCGEventMouseMoved=5, kCGEventLeftMouseDragged=6,
        kCGMouseEventClickState=1, kCGScrollEventUnitPixel=0, kCGScrollEventUnitLine=1,
        CGEventCreateMouseEvent=lambda src, event, point, button: {'screen': point},
        CGEventCreateScrollWheelEvent=lambda *_: {},
        CGEventSetIntegerValueField=lambda *_: None,
        CGEventSetLocation=lambda ev, point: ev.update(screen=point))
    monkeypatch.setitem(sys.modules, 'Quartz', q)
    route = SimpleNamespace(post=lambda ev, pid, wid, point: events.append((ev['screen'], pid, wid, point)))
    kwargs = dict(window=900, route=route, window_origin=(-800, 55))
    if kind == 'scroll':
        steps, release = wheel_steps(-720, 205, -200, 77, **kwargs), lambda: None
    else:
        steps, release = pointer_steps((-720, 205), (-600, 255), kind, 77, **kwargs)
    for step in steps:
        step.post()
    release()
    assert events[0] == ((-720, 205), 77, 900, (80, 150))
    if kind == 'drag':
        assert events[-1] == ((-600, 255), 77, 900, (200, 200))
