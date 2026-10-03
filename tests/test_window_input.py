from types import SimpleNamespace


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
