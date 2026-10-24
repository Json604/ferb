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
