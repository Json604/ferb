import asyncio

import pytest

from ferb.act.mac.input import Step
from ferb.core.errors import Refused, StaleRef
from tests.test_mac_vision import vision


@pytest.fixture(autouse=True)
def no_native_route(monkeypatch):
    monkeypatch.setattr("ferb.act.mac.driver.load_route", lambda: None)


def test_exact_mac_pointer_maps_pixels_and_cleans_up_on_cancel(monkeypatch):
    d, _, _, apps, posted = vision(monkeypatch)
    import ferb.act.mac.driver as driver

    def gesture(start, end, kind, pid, **kwargs):
        posted.append((start, end, kind, pid))
        def cancel():
            raise asyncio.CancelledError()
        return [Step("down", lambda: posted.append("down")), Step("drag", cancel)], lambda: posted.append("released")

    monkeypatch.setattr(driver, "pointer_steps", gesture, raising=False)

    async def go():
        _, shot, _ = await d.screenshot("Notes")
        with pytest.raises(asyncio.CancelledError):
            await d.pointer_at("Notes", shot.id, 400, 300, "drag", (800, 600))
        assert posted[0][:3] == ((300.0, 200.0), (500.0, 350.0), "drag")
        assert posted[-1] == "released"
        with pytest.raises(StaleRef):
            await d.pointer_at("Notes", shot.id, 400, 300, "click")
        assert apps.activated == []
    asyncio.run(go())


def test_mac_pointer_refuses_changed_window_and_never_activates(monkeypatch):
    d, _, shots, apps, posted = vision(monkeypatch)

    async def go():
        _, shot, _ = await d.screenshot("Notes")
        shots.bounds = (200, 100, 800, 600)
        with pytest.raises(StaleRef):
            await d.pointer_at("Notes", shot.id, 400, 300, "hover")
        shots.bounds = shot.bounds
        shots.on_screen = lambda wid: False
        with pytest.raises(Refused, match="Space"):
            await d.pointer_at("Notes", shot.id, 400, 300, "drag", (800, 600))
        assert not posted and not apps.activated
    asyncio.run(go())


def test_native_drag_keeps_destination_policy_floor(monkeypatch):
    from ferb.act.mac.tools import MacPointer
    from ferb.policy.rules import Kind
    d, _, _, _, _ = vision(monkeypatch)
    monkeypatch.setattr(d, "label_at", lambda app, shot, x, y: "Delete" if x == 800 else "Document")
    cap = MacPointer(d)
    args = cap.Input(app="Notes", shot="shot1", x=400, y=300, action="drag", end_x=800, end_y=600)
    assert cap.describe(args).kind == Kind.DELETE_PERMANENT


def test_native_interruption_after_press_has_unknown_outcome(monkeypatch):
    from ferb.act.mac.input import PointerInterrupted
    import ferb.act.mac.driver as driver
    d, _, shots, _, posted = vision(monkeypatch)
    def gesture(*args, **kwargs):
        def press():
            posted.append("down")
            shots.bounds = (200, 100, 800, 600)
        return [Step("down", press), Step("drag", lambda: posted.append("drag"))], lambda: posted.append("released")
    monkeypatch.setattr(driver, "pointer_steps", gesture)
    async def go():
        _, shot, _ = await d.screenshot("Notes")
        with pytest.raises(PointerInterrupted):
            await d.pointer_at("Notes", shot.id, 400, 300, "drag", (800, 600))
        assert posted == ["down", "released"]
    asyncio.run(go())


def test_window_route_attempts_other_space_without_activation(monkeypatch):
    import ferb.act.mac.driver as driver
    d, _, shots, apps, posted = vision(monkeypatch)
    backend = object()
    monkeypatch.setattr(driver, 'load_route', lambda: backend)
    shots.on_screen = lambda wid: False
    def gesture(*args, **kwargs):
        assert kwargs['route'] is backend and kwargs['window'] == 110
        return [Step('hover', lambda: posted.append('hover'))], lambda: None
    monkeypatch.setattr(driver, 'pointer_steps', gesture)
    async def go():
        _, shot, _ = await d.screenshot('Notes')
        r = await d.pointer_at('Notes', shot.id, 100, 100, 'hover')
        assert r.ok and 'not confirmed changed' in r.detail
    asyncio.run(go())
    assert posted == ['hover'] and apps.activated == []


def test_pointer_scroll_stops_without_container_evidence(monkeypatch):
    import ferb.act.mac.driver as driver
    d, _, _, apps, posted = vision(monkeypatch)
    monkeypatch.setattr(driver, 'wheel_steps', lambda *a, **kw: [Step('wheel', lambda: posted.append(kw))])
    async def go():
        _, shot, _ = await d.screenshot('Notes')
        r = await d.pointer_at('Notes', shot.id, 100, 100, 'scroll', direction='right', amount=50)
        assert not r.ok and 'end not established' in r.detail
        assert len(posted) == 1 and posted[0]['horizontal']
        with pytest.raises(StaleRef):
            await d.pointer_at('Notes', shot.id, 100, 100, 'scroll')
    asyncio.run(go())
    assert not apps.activated
