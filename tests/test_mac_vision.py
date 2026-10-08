"""Build step 9, offline: screenshot + click on fake window / capture layers. Only apps on [privacy] exclude_apps
are refused (default: password managers; the owner may edit or empty it); a password field the Accessibility tree
shows, before or after capture, drops the frame; when the tree can't tell, the capture goes ahead. Clicks map image
pixels to the window, refuse a stale shot / moved window / something on top (checked before each event), and yield
to the owner. The image reaches the model and FERB's evidence store (V1, ~/.ferb/evidence, 7 days) and nothing else:
not spans, journal, logs or any other file.
"""

import asyncio
import base64
from dataclasses import field
from pathlib import Path
from typing import Any

import pytest

from ferb.act.mac.accessibility import AxTree
from ferb.act.mac.apps import RunningApp
from ferb.act.mac.input import Step, Yielded
from ferb.config import DEFAULT_EXCLUDE_APPS, Config
from ferb.core.errors import Refused, StaleRef
from ferb.core.types import Identity
from tests.conftest import RunningDaemon
from tests.mackit import gateway_call as call, grant_task as task
from tests.mackit import BOUNDS, IMG, _fake_mac, vision


# ---------- what may be captured ----------

def test_only_excluded_apps_are_refused_and_the_list_is_the_owners(monkeypatch: pytest.MonkeyPatch,
                                                                  tmp_path: Path) -> None:
    d, _, shots, *_ = vision(monkeypatch)
    with pytest.raises(Refused, match="exclusion"):
        asyncio.run(d.screenshot("Passwords"))
    assert shots.captured == []                                          # refused before any capture
    for app in ("Notes", "Brave"):                                       # no built-in refusals: browsers too
        asyncio.run(d.screenshot(app))
    assert len(shots.captured) == 2
    emptied, *_ = vision(monkeypatch, excluded=lambda: set())
    asyncio.run(emptied.screenshot("Passwords"))                         # the owner emptied the list: allowed

    c = Config(tmp_path / "home")
    c.ensure_privacy_defaults()
    assert c.get("privacy", "exclude_apps") == DEFAULT_EXCLUDE_APPS
    assert {"com.apple.Passwords", "com.apple.keychainaccess", "com.bitwarden.desktop"} <= c.exclude_apps()
    c.set("privacy", "exclude_apps", [])
    c.ensure_privacy_defaults()
    assert c.exclude_apps() == frozenset()                               # an emptied list stays empty


def test_a_detectable_password_field_drops_the_frame_and_unknown_is_allowed(monkeypatch: pytest.MonkeyPatch) -> None:
    d, ax, shots, *_ = vision(monkeypatch)
    ax.secure = [True]
    with pytest.raises(Refused, match="password"):
        asyncio.run(d.screenshot("Notes"))
    assert shots.captured == []                                          # seen before: never captured
    ax.secure = [False, True]
    with pytest.raises(Refused, match="password"):
        asyncio.run(d.screenshot("Notes"))                               # appeared during capture: dropped
    assert d._state["com.apple.Notes"].shot is None                      # no shot recorded to click on
    ax.secure = [None, None]
    _, shot, jpeg = asyncio.run(d.screenshot("Notes"))                   # the tree can't tell: allowed
    assert jpeg == shots.jpeg and shot.id == "shot1" and shot.size == IMG


def test_a_browser_tab_the_extension_reports_with_a_password_field_drops_the_frame(
        monkeypatch: pytest.MonkeyPatch) -> None:
    d, ax, shots, *_ = vision(monkeypatch)
    answers: list[bool | None] = []
    asked: list[tuple[str, Any]] = []

    async def web(browser: str, bounds: Any) -> bool | None:
        asked.append((browser, bounds))
        return answers.pop(0) if answers else None
    d.web_password = web
    answers[:] = [True]
    with pytest.raises(Refused, match="password"):
        asyncio.run(d.screenshot("Brave"))
    assert shots.captured == [] and asked == [("brave", BOUNDS)]
    answers[:] = [False, True]                                           # appeared while capturing
    with pytest.raises(Refused, match="password"):
        asyncio.run(d.screenshot("Brave"))
    answers[:] = [None, None]                                            # no profile could tell: captured
    asyncio.run(d.screenshot("Brave"))
    asked.clear()
    asyncio.run(d.screenshot("Notes"))
    assert asked == []                                                   # only browsers are asked


# ---------- clicks ----------

def test_clicks_map_image_pixels_and_require_fresh_refs_for_each_attempt(monkeypatch):
    d, _, shots, apps, posted = vision(monkeypatch)
    _, shot, _ = asyncio.run(d.screenshot("Notes"))
    r = asyncio.run(d.click_at("Notes", shot.id, 400, 300))
    assert r.ok and posted == ["move 300.0,200.0 to pid", "click 300.0,200.0 to pid"]
    assert apps.activated == []
    with pytest.raises(StaleRef):
        asyncio.run(d.click_at("Notes", shot.id, 400, 300))
    shots.takes_pid_clicks = False
    _, shot, _ = asyncio.run(d.screenshot("Notes"))
    r = asyncio.run(d.click_at("Notes", shot.id, 400, 300, allow_foreground=True))
    assert r.ok and not r.facts['observed_change'] and apps.activated == []
    _, shot, _ = asyncio.run(d.screenshot("Notes"))
    r = asyncio.run(d.click_at("Notes", shot.id, 400, 300, allow_foreground=True, mechanism="foreground"))
    assert r.ok and posted[-2:] == ["move 300.0,200.0", "click 300.0,200.0"]
    assert apps.activated == ["com.apple.Notes"]
    with pytest.raises(Refused, match="outside"):
        asyncio.run(d.click_at("Notes", shot.id, 1600, 10))
    posted.clear()
    shots.bounds = (120.0, 50.0, 800.0, 600.0)
    with pytest.raises(StaleRef, match="moved"):
        asyncio.run(d.click_at("Notes", shot.id, 10, 10))
    shots.bounds = BOUNDS
    shots.top = 999
    with pytest.raises(StaleRef, match="another window"):
        asyncio.run(d.click_at("Notes", shot.id, 10, 10, allow_foreground=True, mechanism="foreground"))
    assert posted == []


def test_an_explicit_ax_hit_test_never_falls_back(monkeypatch: pytest.MonkeyPatch) -> None:
    """The point becomes an element (AXUIElementCopyElementAtPosition on the app element) and is pressed through
    Accessibility: no mouse event at all, in front or behind."""
    d, ax, shots, apps, posted = vision(monkeypatch)
    _, shot, _ = asyncio.run(d.screenshot("Notes"))
    ax.hit_el = "Export"
    import ferb.act.mac.driver as drv
    monkeypatch.setattr(drv.axm, "_attr", lambda el, name: "win")
    r = asyncio.run(d.click_at("Notes", shot.id, 400, 300, mechanism="ax"))
    assert r.ok and r.facts["method"] == "ax"
    assert posted == [] and apps.activated == [] and ax.calls[-1] == "ax.press"
    ax.press_ok = False
    r = asyncio.run(d.click_at("Notes", shot.id, 400, 300, mechanism="ax"))
    assert not r.ok and posted == []


def test_the_accessibility_hit_test_decides_when_it_can(monkeypatch: pytest.MonkeyPatch) -> None:
    d, ax, shots, _, posted = vision(monkeypatch)
    shots.takes_pid_clicks = False                                       # the foreground path (by leave)
    asyncio.run(d.screenshot("Notes"))
    shots.top = 999                                                      # the window list sees an overlay...
    ax.hit_answer = (11, BOUNDS)                                         # ...that ignores clicks: Notes gets it
    assert asyncio.run(d.click_at("Notes", "shot1", 10, 10, allow_foreground=True, mechanism="foreground")).ok and posted
    posted.clear()
    shots.top = None
    for answer in ((12, BOUNDS), (11, (110.0, 60.0, 160.0, 160.0)), (11, None)):   # another app / window / a menu
        ax.hit_answer = answer
        with pytest.raises(StaleRef):
            asyncio.run(d.click_at("Notes", "shot1", 10, 10, allow_foreground=True, mechanism="foreground"))
    assert not [p for p in posted if not p.endswith("to pid")]


def test_the_topmost_check_runs_before_every_event(monkeypatch: pytest.MonkeyPatch) -> None:
    d, _, shots, _, posted = vision(monkeypatch)
    shots.takes_pid_clicks = False
    asyncio.run(d.screenshot("Notes"))
    import ferb.act.mac.driver as drv

    def cover() -> None:
        posted.append("move")
        shots.top = 999                                                  # a window pops up after the move
    monkeypatch.setattr(drv, "click_steps", lambda x, y, pid=None: [Step("move", cover if pid is None else
                                                                         lambda: posted.append("move to pid")),
                                                                    Step("click", lambda: posted.append("click"))])
    with pytest.raises(StaleRef):
        asyncio.run(d.click_at("Notes", "shot1", 10, 10, allow_foreground=True, mechanism="foreground"))
    assert posted == ["move"]                    # the real click itself never happened


def test_clicks_yield_to_the_owner(monkeypatch: pytest.MonkeyPatch) -> None:
    d, _, shots, apps, posted = vision(monkeypatch, owner=lambda: True)
    shots.takes_pid_clicks = False
    asyncio.run(d.screenshot("Notes"))
    with pytest.raises(Yielded):
        asyncio.run(d.click_at("Notes", "shot1", 10, 10, allow_foreground=True, mechanism="foreground"))
    assert not [p for p in posted if not p.endswith("to pid")] and apps.activated == []   # the owner never moved


# ---------- through the gateway ----------


def test_the_image_reaches_the_model_and_the_evidence_store_only(daemon: RunningDaemon,
                                                                 monkeypatch: pytest.MonkeyPatch) -> None:
    shots, posted = _fake_mac(daemon, monkeypatch)
    ferb = task(daemon, "f", Identity.FERB, {"fs:read"}, None)
    assert not [t for t in daemon.daemon.gateway.tools_for(ferb) if t["name"] in ("mac.screenshot", "mac.click")]
    owner = task(daemon, "v", Identity.OWNER, {"screen", "mac:act"}, {"use_app"})
    r = call(daemon, owner, "mac.screenshot", app="Notes")
    assert not r["is_error"] and base64.b64decode(r["image"]["data"]) == shots.jpeg
    assert r["image"]["mime"] == "image/jpeg" and r["content"]["shot"] == "shot1"
    r = call(daemon, owner, "mac.click", app="Notes", shot="shot1", x=400, y=300)
    assert not r["is_error"] and posted[-1] == "click 300.0,200.0 to pid", r
    b64 = base64.b64encode(shots.jpeg)
    daemon.run(_checkpoint(daemon))
    kept = _evidence_files(daemon)
    assert [p.read_bytes() for p in kept] == [shots.jpeg]               # one capture, kept as evidence (V1)
    for p in daemon.daemon.home.rglob("*"):                             # database, WAL, logs, anything else
        if p.is_file() and not p.is_socket() and p not in kept:
            data = p.read_bytes()
            assert shots.jpeg[3:] not in data and b64 not in data, p


def test_screenshots_of_browsers_and_messages_expose_the_worker(daemon: RunningDaemon,
                                                                monkeypatch: pytest.MonkeyPatch) -> None:
    _fake_mac(daemon, monkeypatch)
    daemon.daemon.mac.apps.apps.append(RunningApp("com.apple.mail", "Mail", 15, False))   # type: ignore[union-attr]
    for tid, app in (("b", "Brave"), ("m", "Mail")):
        g = task(daemon, tid, Identity.OWNER, {"screen", "browser:read"}, {"use_app"})
        assert not call(daemon, g, "mac.screenshot", app=app)["is_error"]


async def _checkpoint(d: RunningDaemon) -> None:
    d.daemon.db.conn.execute("PRAGMA wal_checkpoint(FULL)")


def _evidence_files(d: RunningDaemon) -> list[Path]:
    return sorted(p for p in (d.daemon.home / "evidence").rglob("*") if p.is_file())


def test_native_browser_controls_do_not_need_a_fallback_permission(daemon, monkeypatch):
    shots, _ = _fake_mac(daemon, monkeypatch)
    shots.top = 120
    g = task(daemon, "e", Identity.OWNER, {"screen", "mac:act", "browser:read"}, {"use_app"})
    r = call(daemon, g, "mac.screenshot", app="Brave")
    assert not r["is_error"], r
    r = call(daemon, g, "mac.click", app="Brave", shot="shot1", x=5, y=5)
    assert not r["is_error"], r


async def _span_inputs(d: RunningDaemon) -> list[str]:
    return [str(r[0]) for r in d.daemon.db.conn.execute("SELECT input_json FROM spans").fetchall()]


# ---------- the image with the tree (owner request, 2026-10-01) ----------

def test_marks_map_controls_onto_the_image() -> None:
    from ferb.act.mac.screenshot import to_pixels
    bounds, size = (100.0, 50.0, 400.0, 300.0), (800, 600)                  # a window captured at 2x
    got = to_pixels([("7", (110, 60, 100, 30)), ("9", (450, 300, 100, 100)), ("off", (2000, 0, 10, 10)),
                     ("flat", (150, 100, 0, 20))], bounds, size)
    assert got == [("7", (20.0, 20.0, 200.0, 60.0)), ("9", (700.0, 500.0, 100.0, 100.0))]   # clipped to the window


def _tree_with_controls(ax: Any) -> None:
    from ferb.act.mac.accessibility import AxNode, AxTree
    ax.tree = AxTree(nodes=[AxNode(None, "button", "Send", None, (120.0, 70.0, 60.0, 20.0), False, True),
                            AxNode(None, "button", "", None, (200.0, 100.0, 30.0, 30.0), False, True)],
                     title="Chat")


def test_a_snapshot_with_the_screen_scope_carries_a_marked_image(daemon: RunningDaemon,
                                                                monkeypatch: pytest.MonkeyPatch) -> None:
    shots, _ = _fake_mac(daemon, monkeypatch)
    _tree_with_controls(daemon.daemon.mac.ax)                                   # type: ignore[union-attr]
    g = task(daemon, "i", Identity.OWNER, {"mac:read", "screen"}, {"use_app"})
    r = call(daemon, g, "mac.snapshot", app="Notes")
    assert not r["is_error"] and base64.b64decode(r["image"]["data"]) == shots.jpeg
    ids = [e["id"] for e in r["content"]["elements"]]
    assert shots.marks == [(i.split("e", 1)[1], rect) for i, rect in
                           zip(ids, [(120.0, 70.0, 60.0, 20.0), (200.0, 100.0, 30.0, 30.0)])]
    assert r["content"]["shot"] == "shot1" and r["content"]["image"].startswith("attached")
    daemon.run(_checkpoint(daemon))
    kept = _evidence_files(daemon)
    assert [p.read_bytes() for p in kept] == [shots.jpeg]                      # the evidence store holds it (V1)
    for p in daemon.daemon.home.rglob("*"):                                    # nowhere else
        if p.is_file() and not p.is_socket() and p not in kept:
            assert base64.b64encode(shots.jpeg) not in p.read_bytes() and shots.jpeg[3:] not in p.read_bytes(), p


def test_no_image_without_the_screen_scope_for_browsers_or_when_switched_off(daemon: RunningDaemon,
                                                                         monkeypatch: pytest.MonkeyPatch) -> None:
    shots, _ = _fake_mac(daemon, monkeypatch)
    _tree_with_controls(daemon.daemon.mac.ax)                                   # type: ignore[union-attr]
    plain = task(daemon, "p", Identity.OWNER, {"mac:read"}, {"use_app"})
    assert "image" not in call(daemon, plain, "mac.snapshot", app="Notes")
    g = task(daemon, "q", Identity.OWNER, {"mac:read", "screen"}, {"use_app"})
    assert "image" not in call(daemon, g, "mac.snapshot", app="Brave")        # browsers: the extension's job
    daemon.daemon.mac.snapshot_images = lambda: False                          # type: ignore[union-attr]
    assert "image" not in call(daemon, g, "mac.snapshot", app="Notes")
    assert shots.captured == []


def test_a_password_field_still_means_no_image_but_the_tree_comes_back(daemon: RunningDaemon,
                                                                     monkeypatch: pytest.MonkeyPatch) -> None:
    shots, _ = _fake_mac(daemon, monkeypatch)
    ax = daemon.daemon.mac.ax                                                   # type: ignore[union-attr]
    _tree_with_controls(ax)
    ax.secure = [True, True]                    # the field is still there when FERB looks for its evidence (D26)
    g = task(daemon, "s", Identity.OWNER, {"mac:read", "screen"}, {"use_app"})
    r = call(daemon, g, "mac.snapshot", app="Notes")
    assert not r["is_error"] and "image" not in r and "password" in r["content"]["image"]
    assert len(r["content"]["elements"]) == 2 and shots.captured == []
    assert _evidence_files(daemon) == []
