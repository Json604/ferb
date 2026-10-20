"""Captured state the checker can re-inspect, kept 7 days (V1, D20); app-window screenshots mask secret fields
(D26, D29): one picture of the app a worker starts on, never the desktop, never the owner's shot."""
import base64
import json
from datetime import timedelta
from typing import Any

import pytest

from ferb.act.evidence import mask
from ferb.act.evidence.store import EvidenceStore
from ferb.act.mac.accessibility import AxNode, AxTree
from ferb.act.mac.apps import RunningApp
from ferb.config import Config
from ferb.core.types import Identity, Task
from ferb.orchestrator.tree import TaskRecord
from ferb.policy.targets import Request
from ferb.sanitize.redact import Redactor
from tests.conftest import RunningDaemon, gateway_request, gateway_setup, gateway_tool_call
from tests.mackit import _fake_mac, gateway_call, grant_task
from tests.support.images import ImageTool as _ImageTool, black as _black, solid_jpeg as _solid_jpeg, white as _white


def test_put_and_read_round_trip(daemon: RunningDaemon) -> None:
    store = daemon.daemon.evidence
    eid = daemon.run(_put(store, "t1", "s1", "snapshot", data={"title": "Inbox"},
                          origin="external:web:mail.google.com"))
    rows = daemon.run(_list(store, "t1"))
    assert len(rows) == 1 and rows[0]["kind"] == "snapshot" and rows[0]["origin"] == "external:web:mail.google.com"
    assert rows[0]["has_image"] is False and rows[0]["path"] is None and rows[0]["span_id"] == "s1"
    meta, blob = daemon.run(_read(store, eid))
    assert meta["data"]["title"] == "Inbox" and blob is None


def test_a_stored_screenshot_masks_secret_fields(daemon: RunningDaemon) -> None:
    store = daemon.daemon.evidence
    img = _solid_jpeg(60, 40)                              # a known all-white image
    eid = daemon.run(_put(store, "t1", "s1", "app_window", image=img,
                          rects=[(10, 10, 20, 20)], origin="external:web:x"))
    _, blob = daemon.run(_read(store, eid))
    assert blob is not None and blob != img
    assert _black(mask.pixel(blob, 15, 15))                # inside the masked rect: opaque (JPEG: within a few levels)
    assert _white(mask.pixel(blob, 2, 2))                  # outside: untouched
    row = daemon.run(_list(store, "t1"))[0]
    on_disk = store.root / row["path"]
    assert row["path"] == f"{daemon.daemon.clock.now().astimezone().date().isoformat()}/{eid}.jpg"
    assert on_disk.read_bytes() == blob                     # only the masked image ever touched the disk


def test_masking_clamps_boxes_and_an_image_it_cannot_read_is_withheld(daemon: RunningDaemon) -> None:
    img = _solid_jpeg(60, 40)
    out = mask.mask_secret_fields(img, [(-10, -10, 20, 20), (55, 35, 50, 50), (100, 100, 5, 5), (5, 5, 0, 9)])
    assert _black(mask.pixel(out, 2, 2)) and _black(mask.pixel(out, 58, 38)) and _white(mask.pixel(out, 30, 20))
    with pytest.raises(ValueError):
        mask.mask_secret_fields(b"\xff\xd8\xff not a picture", [(0, 0, 4, 4)])
    store = daemon.daemon.evidence
    eid = daemon.run(_put(store, "t1", "s1", "app_window", image=b"\xff\xd8\xff not a picture",
                          rects=[(0, 0, 4, 4)], data={"app": "Notes"}, origin="private"))
    meta, blob = daemon.run(_read(store, eid))
    assert blob is None and meta["data"]["image_withheld"] and meta["data"]["app"] == "Notes"
    assert not [p for p in store.root.rglob("*") if p.is_file()]          # an unmasked image never touched the disk


def test_retention_purges_media_older_than_seven_days(daemon: RunningDaemon) -> None:
    store = daemon.daemon.evidence
    daemon.run(_put(store, "t1", "s1", "snapshot", data={"a": 1}, origin="ferb"))
    daemon.run(_put(store, "t1", "s2", "screenshot", image=_solid_jpeg(8, 8), origin="private"))
    files = [p for p in store.root.rglob("*.jpg")]
    assert len(files) == 1
    old = (daemon.daemon.clock.now() - timedelta(days=8)).isoformat()
    daemon.run(_age(daemon, old))
    cutoff = (daemon.daemon.clock.now() - timedelta(days=7)).isoformat()
    assert daemon.run(_purge(store, cutoff)) == 2
    assert daemon.run(_list(store, "t1")) == [] and not files[0].exists()


def test_the_retention_pass_purges_evidence_past_evidence_days_and_journals_it(daemon: RunningDaemon) -> None:
    store = daemon.daemon.evidence
    assert daemon.daemon.config.get("retention", "evidence_days") == 7         # the owner sees it in config.toml
    old = daemon.run(_put(store, "t1", "s1", "screenshot", image=_solid_jpeg(8, 8), origin="private"))
    kept = daemon.run(_put(store, "t1", "s2", "screenshot", image=_solid_jpeg(8, 8), origin="private"))
    now = daemon.daemon.clock.now()
    daemon.run(_age(daemon, (now - timedelta(days=8)).isoformat(), old))
    daemon.run(_age(daemon, (now - timedelta(days=6)).isoformat(), kept))
    result: dict[str, Any] = {}
    daemon.call_soon(lambda: result.update(daemon.daemon._purge_records()))
    assert result["evidence"] == 1 and [r["id"] for r in daemon.run(_list(store, "t1"))] == [kept]
    row = json.loads(daemon.run(_q(daemon, "SELECT data_json FROM journal WHERE kind = 'retention.purged'")))
    assert row["evidence"] == 1 and row["evidence_days"] == 7


def test_evidence_days_defaults_to_seven_and_the_owners_value_stays(tmp_path: Any) -> None:
    c = Config(tmp_path / "home")
    c.ensure_privacy_defaults()
    assert c.get("retention", "evidence_days") == 7
    c.set("retention", "evidence_days", 3)
    c.ensure_privacy_defaults()
    assert c.get("retention", "evidence_days") == 3


def test_the_checker_can_read_the_tasks_evidence(daemon: RunningDaemon) -> None:
    # A capture stored during the worker is listed back through task.evidence for the checker of that worker, as
    # facts with their origins; image bytes never come back, and another task's evidence is not readable.
    store = daemon.daemon.evidence
    worker, checker = _worker_and_checker(daemon)
    eid = daemon.run(_put(store, worker.id, "s1", "screenshot", image=_solid_jpeg(8, 8),
                          data={"title": "Order placed"}, origin="external:web:shop.example"))
    other = daemon.run(_put(store, "elsewhere", "s9", "snapshot", data={"title": "not yours"}, origin="private"))
    listed = gateway_call(daemon, checker, "task.evidence", kind="evidence")
    assert not listed["is_error"], listed
    rows = listed["content"]["records"]
    assert [(r["id"], r["kind"], r["has_image"], r["data"]["title"]) for r in rows] == \
        [(eid, "screenshot", True, "Order placed")]
    assert "Origin: external:web:shop.example" in listed["footer"] and "image" not in listed
    one = gateway_call(daemon, checker, "task.evidence", evidence_id=eid)
    assert not one["is_error"] and one["content"]["record"]["data"] == {"title": "Order placed"}
    assert "image" not in one and "path" not in one["content"]["record"]
    assert gateway_call(daemon, checker, "task.evidence", evidence_id=other)["is_error"]


def test_secret_text_in_evidence_data_is_redacted(daemon: RunningDaemon) -> None:
    store = daemon.daemon.evidence
    eid = daemon.run(_put(store, "t1", "s1", "snapshot",
                          data={"note": "my key is sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123456789"}, origin="ferb"))
    rows = daemon.run(_raw(daemon, eid))
    assert "sk-ant-api03" not in rows["data_json"]


# ---------- the gateway keeps the image a tool produced ----------

def test_a_tool_image_is_kept_as_evidence_and_the_trace_points_at_it(daemon: RunningDaemon) -> None:
    img = _solid_jpeg(16, 12)
    shot = _ImageTool("probe.screenshot", img,
                      {"shot": "bs-1", "width": 16, "height": 12, "url": "https://x.example/a"},
                      reads=(("web", "https://x.example/a"),))
    grant, _ = gateway_setup(daemon, {"probe"}, gateway_request("look at the page"), [shot])
    r = gateway_tool_call(daemon, grant, "probe.screenshot")
    assert not r["is_error"] and base64.b64decode(r["image"]["data"]) == img      # the model still gets it
    rows = daemon.run(_list(daemon.daemon.evidence, "task-1"))
    assert len(rows) == 1 and rows[0]["kind"] == "screenshot" and rows[0]["has_image"]
    assert rows[0]["origin"] == "external:web:x.example" and rows[0]["data"]["width"] == 16
    assert daemon.run(_read(daemon.daemon.evidence, rows[0]["id"]))[1] == img
    out = json.loads(daemon.run(_q(daemon, "SELECT output_json FROM spans WHERE kind = 'tool.call'")))
    assert out["evidence"] == rows[0]["id"] and rows[0]["span_id"] == daemon.run(
        _q(daemon, "SELECT id FROM spans WHERE kind = 'tool.call'"))
    snap = _ImageTool("probe.snapshot", img, {"elements": [{"id": "e1", "role": "button", "label": "Pay"}]})
    daemon.daemon.registry.capabilities[snap.name] = snap
    gateway_tool_call(daemon, grant, "probe.snapshot")
    assert [r["kind"] for r in daemon.run(_list(daemon.daemon.evidence, "task-1"))] == ["screenshot", "snapshot"]


def test_an_image_beside_a_secret_field_or_from_an_excluded_host_is_kept(daemon: RunningDaemon) -> None:
    """L13 (owner: "Yes, no exceptions"): a tool's image had its secret fields painted over where it was captured, so
    the evidence keeps what the model got, a page on the owner's excluded sites too (the list governs perception
    recording); the origin label still never names that host."""
    img = _solid_jpeg(16, 12)
    secret = _ImageTool("probe.snapshot", img, {"elements": [{"id": "e1", "role": "textbox", "label": "Password",
                                                              "sensitive": True, "filled": True}]})
    hidden = _ImageTool("probe.screenshot", img, {"shot": "bs-2"}, reads=(("web", "https://bank.example/login"),),
                        site="bank.example")
    grant, _ = gateway_setup(daemon, {"probe"}, gateway_request("look"), [secret, hidden])
    assert isinstance(daemon.daemon.redact, Redactor)
    daemon.daemon.redact.configure({"bank.example"}, set())
    gateway_tool_call(daemon, grant, "probe.snapshot")
    assert "image" in gateway_tool_call(daemon, grant, "probe.screenshot")
    rows = daemon.run(_list(daemon.daemon.evidence, "task-1"))
    assert [(r["kind"], r["has_image"]) for r in rows] == [("snapshot", True), ("screenshot", True)]
    assert daemon.run(_read(daemon.daemon.evidence, rows[0]["id"]))[1] == img
    assert rows[1]["origin"] == "external:web:excluded" and not rows[1]["data"].get("image_withheld")
    assert len([p for p in daemon.daemon.evidence.root.rglob("*") if p.is_file()]) == 2


# ---------- one picture of the app a worker starts on (D26, D29) ----------

def test_a_worker_starting_on_a_mac_app_keeps_one_picture_of_its_window(daemon: RunningDaemon,
                                                                        monkeypatch: pytest.MonkeyPatch) -> None:
    shots, _ = _fake_mac(daemon, monkeypatch)
    _menus(daemon)
    g = grant_task(daemon, "a", Identity.OWNER, {"mac:read", "screen"}, "look at my notes")
    first = gateway_call(daemon, g, "mac.menu", app="Notes")
    second = gateway_call(daemon, g, "mac.menu", app="Notes")
    assert not first["is_error"] and not second["is_error"]
    assert base64.b64decode(first["image"]["data"]) == shots.jpeg and "image" not in second
    assert shots.captured == [110]                                  # once, and only Notes' own window (pid 11)
    rows = daemon.run(_list(daemon.daemon.evidence, "a"))
    assert [(r["kind"], r["origin"], r["has_image"]) for r in rows] == [("app_window", "private", True)]
    assert daemon.run(_read(daemon.daemon.evidence, rows[0]["id"]))[1] == shots.jpeg
    assert rows[0]["data"]["bundle_id"] == "com.apple.Notes"
    span = daemon.run(_q(daemon, "SELECT output_json FROM spans WHERE id = ?", rows[0]["span_id"]))
    assert json.loads(span)["evidence"] == rows[0]["id"]
    assert daemon.daemon.mac._state["com.apple.Notes"].shot is None        # not a shot anyone can click on
    g2 = grant_task(daemon, "b", Identity.OWNER, {"mac:read", "screen"}, "look at my notes")
    gateway_call(daemon, g2, "mac.menu", app="Notes")
    assert len(shots.captured) == 2                                 # another worker: its own first look


def test_the_app_window_capture_leaves_another_workers_shot_alone(daemon: RunningDaemon,
                                                                  monkeypatch: pytest.MonkeyPatch) -> None:
    shots, posted = _fake_mac(daemon, monkeypatch)
    _menus(daemon)
    clicker = grant_task(daemon, "c", Identity.OWNER, {"screen", "mac:act"}, {"use_app"})
    r = gateway_call(daemon, clicker, "mac.screenshot", app="Notes")
    assert r["content"]["shot"] == "shot1"
    assert [x["kind"] for x in daemon.run(_list(daemon.daemon.evidence, "c"))] == ["screenshot"]   # its own image
    looker = grant_task(daemon, "l", Identity.OWNER, {"mac:read", "screen"}, "look at my notes")
    gateway_call(daemon, looker, "mac.menu", app="Notes")
    assert [x["kind"] for x in daemon.run(_list(daemon.daemon.evidence, "l"))] == ["app_window"]
    r = gateway_call(daemon, clicker, "mac.click", app="Notes", shot="shot1", x=400, y=300)
    assert not r["is_error"] and posted[-1] == "click 300.0,200.0 to pid", r
    assert len(shots.captured) == 2                                 # the clicker's screenshot was its first look


def test_the_app_window_masks_the_secret_fields_its_tree_shows(daemon: RunningDaemon,
                                                               monkeypatch: pytest.MonkeyPatch) -> None:
    shots, _ = _fake_mac(daemon, monkeypatch)
    _menus(daemon)
    shots.jpeg = _solid_jpeg(1600, 1200)                             # the fake window, captured at 2x
    ax = daemon.daemon.mac.ax                                        # type: ignore[union-attr]
    ax.tree = AxTree(nodes=[AxNode(None, "textbox", "Password", "filled", (150.0, 100.0, 100.0, 50.0), True, True)],
                     title="Sign in")                                # a tree that shows a field but didn't flag it
    g = grant_task(daemon, "m", Identity.OWNER, {"mac:read", "screen"}, "look at my notes")
    gateway_call(daemon, g, "mac.menu", app="Notes")
    rows = daemon.run(_list(daemon.daemon.evidence, "m"))
    _, blob = daemon.run(_read(daemon.daemon.evidence, rows[0]["id"]))
    assert blob is not None and _black(mask.pixel(blob, 200, 150))  # (150-100)*2 .. +200, (100-50)*2 .. +100
    assert _white(mask.pixel(blob, 20, 20)) and _white(mask.pixel(blob, 500, 400))


def test_no_app_window_for_a_shell_task_without_the_screen_scope_for_browsers_or_when_images_are_off(
        daemon: RunningDaemon, monkeypatch: pytest.MonkeyPatch) -> None:
    shots, _ = _fake_mac(daemon, monkeypatch)
    _menus(daemon)
    fs = grant_task(daemon, "f", Identity.OWNER, {"fs:read", "screen"}, "list my workspace")
    gateway_call(daemon, fs, "fs.list", path=".")
    plain = grant_task(daemon, "p", Identity.OWNER, {"mac:read"}, "look at my notes")
    gateway_call(daemon, plain, "mac.menu", app="Notes")
    browser = grant_task(daemon, "q", Identity.OWNER, {"mac:read", "screen"}, "look at brave")
    gateway_call(daemon, browser, "mac.menu", app="Brave")           # a browser window shows the owner's own tab
    daemon.daemon.mac.snapshot_images = lambda: False                # type: ignore[union-attr]
    off = grant_task(daemon, "o", Identity.OWNER, {"mac:read", "screen"}, "look at my notes")
    gateway_call(daemon, off, "mac.menu", app="Notes")
    assert shots.captured == []
    assert daemon.run(_q(daemon, "SELECT count(*) FROM evidence")) == 0
    assert daemon.run(_q(daemon, "SELECT count(*) FROM spans WHERE kind = 'evidence.app_window'")) == 0


def test_a_failed_app_window_capture_is_logged_and_never_fails_the_call(daemon: RunningDaemon,
                                                                        monkeypatch: pytest.MonkeyPatch) -> None:
    shots, _ = _fake_mac(daemon, monkeypatch)
    _menus(daemon)

    def broken() -> None:
        raise RuntimeError("the capture service didn't answer")
    shots.on_capture = broken
    g = grant_task(daemon, "x", Identity.OWNER, {"mac:read", "screen"}, "look at my notes")
    r = gateway_call(daemon, g, "mac.menu", app="Notes")
    assert not r["is_error"] and r["content"]["menus"]
    assert r["content"]["image_withheld"] == "initial window capture unavailable"
    assert daemon.run(_q(daemon, "SELECT count(*) FROM evidence")) == 1
    gateway_call(daemon, g, "mac.menu", app="Notes")
    assert daemon.run(_q(daemon, "SELECT count(*) FROM evidence")) == 1   # once; facts only


# ---------- fix round 1: what never gets pixels on disk, and store hardening ----------

def test_a_browser_window_a_hostless_page_and_a_recording_contact_sheet_keep_their_pixels(
        daemon: RunningDaemon, monkeypatch: pytest.MonkeyPatch) -> None:
    # L13 (D29 clarified): a browser's window, a page with no known host and a recording's contact sheet reach the
    # model and the evidence store like any other image, masked and sanitized. Formerly facts only.
    _fake_mac(daemon, monkeypatch)
    assert isinstance(daemon.daemon.redact, Redactor)
    daemon.daemon.redact.configure({"bank.example"}, set())
    g = grant_task(daemon, "b", Identity.OWNER, {"screen"}, {"use_app"})
    r = gateway_call(daemon, g, "mac.screenshot", app="Brave")
    assert not r["is_error"] and "image" in r
    img = _solid_jpeg(16, 12)
    sheet = _ImageTool("recording.inspect", img, {"path": "/tmp/r.mp4", "duration_s": 3.0},
                       reads=(("recording", "rec-1"),))
    hostless = _ImageTool("probe.screenshot", img, {"shot": "bs-3"}, reads=(("web", ""),))
    grant, _ = gateway_setup(daemon, {"probe"}, gateway_request("look"), [sheet, hostless])
    assert "image" in gateway_tool_call(daemon, grant, "recording.inspect")
    assert "image" in gateway_tool_call(daemon, grant, "probe.screenshot")
    rows = daemon.run(_list(daemon.daemon.evidence, "b")) + daemon.run(_list(daemon.daemon.evidence, "task-1"))
    assert [(r["kind"], r["has_image"]) for r in rows] == [("screenshot", True), ("recording", True),
                                                          ("screenshot", True)]
    assert not any(r["data"].get("image_withheld") for r in rows) and rows[1]["data"]["path"] == "/tmp/r.mp4"
    assert len([p for p in daemon.daemon.evidence.root.rglob("*") if p.is_file()]) == 3


def test_messaging_and_mail_apps_use_f5_sanitizer(daemon: RunningDaemon,
                                                          monkeypatch: pytest.MonkeyPatch) -> None:
    # OCR class masking is shared by mail and all other app-window pictures (fixture supplies empty OCR regions).
    shots, _ = _fake_mac(daemon, monkeypatch)
    _menus(daemon)
    daemon.daemon.mac.apps.apps.append(RunningApp("com.apple.mail", "Mail", 15, False))   # type: ignore[union-attr]
    shooter = grant_task(daemon, "s", Identity.OWNER, {"screen"}, {"use_app"})
    r = gateway_call(daemon, shooter, "mac.screenshot", app="Mail")
    assert not r["is_error"] and "image" in r
    looker = grant_task(daemon, "l", Identity.OWNER, {"mac:read", "screen"}, "look at my mail")
    gateway_call(daemon, looker, "mac.menu", app="Mail")
    assert shots.captured == [150, 150]
    rows = daemon.run(_list(daemon.daemon.evidence, "s")) + daemon.run(_list(daemon.daemon.evidence, "l"))
    assert [(r["kind"], r["has_image"]) for r in rows] == [("screenshot", True), ("app_window", True)]
    assert {r["origin"] for r in rows} == {"external:email:com.apple.mail,private"}
    assert len([p for p in daemon.daemon.evidence.root.rglob("*") if p.is_file()]) == 2


def test_a_first_look_waits_for_the_window_but_a_withheld_picture_counts(daemon: RunningDaemon,
                                                                         monkeypatch: pytest.MonkeyPatch) -> None:
    shots, _ = _fake_mac(daemon, monkeypatch)
    _menus(daemon)
    shots.front_window = lambda pid, like=None: None                # type: ignore[method-assign]   # no window yet
    g = grant_task(daemon, "w", Identity.OWNER, {"mac:read", "screen"}, "look at my notes")
    gateway_call(daemon, g, "mac.menu", app="Notes")
    assert daemon.run(_q(daemon, "SELECT count(*) FROM spans WHERE kind = 'evidence.app_window'")) == 0
    del shots.front_window                                          # the window appeared
    gateway_call(daemon, g, "mac.menu", app="Notes")
    assert [r["kind"] for r in daemon.run(_list(daemon.daemon.evidence, "w"))] == ["app_window"]
    daemon.daemon.mac.ax.tree = AxTree(secret_rects=[(150.0, 100.0, 100.0, 20.0)])   # type: ignore[union-attr]

    def broken(image: bytes, boxes: Any) -> bytes:                # a password field it can't paint over
        raise ValueError("not an image Quartz can decode")
    monkeypatch.setattr(mask, "mask_secret_fields", broken)
    p = grant_task(daemon, "p", Identity.OWNER, {"mac:read", "screen"}, "look at my notes")
    first = gateway_call(daemon, p, "mac.menu", app="Notes")
    second = gateway_call(daemon, p, "mac.menu", app="Notes")
    assert first["content"]["image_withheld"] == "the capture couldn't be painted over; pixels withheld"
    assert "image_withheld" not in second["content"]             # attempted only once


def test_masking_for_a_first_look_runs_off_the_event_loop(daemon: RunningDaemon,
                                                         monkeypatch: pytest.MonkeyPatch) -> None:
    import threading
    shots, _ = _fake_mac(daemon, monkeypatch)
    _menus(daemon)
    shots.jpeg = _solid_jpeg(1600, 1200)
    field = AxNode(None, "textbox", "Password", "filled", (150.0, 100.0, 100.0, 50.0), True, True)
    daemon.daemon.mac.ax.tree = AxTree(nodes=[field])               # type: ignore[union-attr]
    threads: list[int] = []
    real = mask.mask_secret_fields

    def recorded(image: bytes, rects: Any) -> bytes:
        threads.append(threading.get_ident())
        return real(image, rects)
    monkeypatch.setattr(mask, "mask_secret_fields", recorded)
    gateway_call(daemon, grant_task(daemon, "t", Identity.OWNER, {"mac:read", "screen"}, "notes"), "mac.menu",
                 app="Notes")
    assert threads and daemon.thread.ident not in threads


def test_the_first_look_is_in_the_call_reply_and_reserved_once(daemon: RunningDaemon,
                                                               monkeypatch: pytest.MonkeyPatch) -> None:
    import asyncio
    shots, _ = _fake_mac(daemon, monkeypatch)
    _menus(daemon)
    gateway = daemon.daemon.gateway
    real = gateway.app_window
    entered = __import__('threading').Event()
    gate = {}
    async def make():
        gate['open'] = asyncio.Event()
    daemon.run(make())
    async def slow(app, scopes):
        entered.set()
        await gate['open'].wait()
        return await real(app, scopes)
    gateway.app_window = slow
    g = grant_task(daemon, 'a', Identity.OWNER, {'mac:read', 'screen'}, 'look at my notes')
    running = asyncio.run_coroutine_threadsafe(gateway.call(g, 'mac.menu', {'app': 'Notes'}), daemon.loop)
    assert entered.wait(5) and not running.done()
    daemon.call_soon(gate['open'].set)
    result = running.result(timeout=5)
    assert not result['is_error'] and 'image' in result and shots.captured == [110]
    assert 'image' not in gateway_call(daemon, g, 'mac.menu', app='Notes')

def test_a_tool_image_is_written_off_the_event_loop(daemon: RunningDaemon, monkeypatch: pytest.MonkeyPatch) -> None:
    import threading
    from ferb.act.evidence import store as store_module
    threads: list[int] = []
    real = store_module._write_file

    def recorded(path: Any, image: bytes) -> None:
        threads.append(threading.get_ident())
        real(path, image)
    monkeypatch.setattr(store_module, "_write_file", recorded)
    grant, _ = gateway_setup(daemon, {"probe"}, gateway_request("look"),
                             [_ImageTool("probe.screenshot", _solid_jpeg(8, 8), {"shot": "bs-1"},
                                         reads=(("web", "https://x.example/a"),))])
    assert not gateway_tool_call(daemon, grant, "probe.screenshot")["is_error"]
    rows = daemon.run(_list(daemon.daemon.evidence, "task-1"))
    assert rows[0]["has_image"] and threads and daemon.thread.ident not in threads


def _slow_writes(monkeypatch: pytest.MonkeyPatch) -> Any:
    """The evidence file write takes 0.3 s in its worker thread; the returned event is set when one starts."""
    import threading
    import time as _time
    from ferb.act.evidence import store as store_module
    real, started = store_module._write_file, threading.Event()

    def slow(path: Any, image: bytes) -> None:
        started.set()
        _time.sleep(0.3)
        real(path, image)
    monkeypatch.setattr(store_module, "_write_file", slow)
    return started


def _files(daemon: RunningDaemon) -> list[Any]:
    return [p for p in daemon.daemon.evidence.root.rglob("*") if p.is_file()]


def test_a_cancel_during_the_async_write_never_leaves_a_file_without_its_row(daemon: RunningDaemon,
                                                                            monkeypatch: pytest.MonkeyPatch) -> None:
    import asyncio
    import time as _time
    started = _slow_writes(monkeypatch)
    store = daemon.daemon.evidence

    async def go() -> str:
        job = asyncio.ensure_future(store.put_async("t1", "s1", "screenshot", image=_solid_jpeg(8, 8),
                                                    data={"shot": "x"}, origin="private"))
        await asyncio.to_thread(started.wait, 5)
        job.cancel()
        try:
            await job
        except asyncio.CancelledError:
            return "cancelled"
        return "done"
    assert daemon.run(go()) == "cancelled"
    _time.sleep(0.5)                                                  # the write thread has long finished
    rows = daemon.run(_q(daemon, "SELECT count(*) FROM evidence"))
    assert (len(_files(daemon)), rows) in ((0, 0), (1, 1))            # never a file without its row
    daemon.run(_purge(store, "2100-01-01T00:00:00+00:00"))
    assert _files(daemon) == []                                       # nothing outlives retention


def test_a_stopped_initial_image_write_finishes_its_call_and_leaves_no_orphan(daemon: RunningDaemon,
                                                                          monkeypatch: pytest.MonkeyPatch) -> None:
    import asyncio
    import concurrent.futures
    import time
    _fake_mac(daemon, monkeypatch)
    _menus(daemon)
    started = _slow_writes(monkeypatch)
    gateway = daemon.daemon.gateway
    g = grant_task(daemon, 's', Identity.OWNER, {'mac:read', 'screen'}, 'look at my notes')
    running = asyncio.run_coroutine_threadsafe(gateway.call(g, 'mac.menu', {'app': 'Notes'}), daemon.loop)
    assert started.wait(5)
    daemon.call_soon(gateway.cancel_tasks, {'s'})
    with pytest.raises(concurrent.futures.CancelledError):
        running.result(timeout=5)
    time.sleep(0.5)
    ended, output = daemon.run(_row(daemon, "SELECT ended_at, output_json FROM spans WHERE kind='tool.call'"))
    assert ended is not None and json.loads(output)['evidence_error'] == 'cancelled'
    assert (len(_files(daemon)), daemon.run(_q(daemon, 'SELECT count(*) FROM evidence'))) in ((0, 0), (1, 1))

def test_a_stop_while_a_tool_image_is_written_still_finishes_the_call_span(daemon: RunningDaemon,
                                                                          monkeypatch: pytest.MonkeyPatch) -> None:
    import asyncio
    import concurrent.futures
    import time as _time
    started = _slow_writes(monkeypatch)
    gateway = daemon.daemon.gateway
    grant, _ = gateway_setup(daemon, {"probe"}, gateway_request("look"),
                             [_ImageTool("probe.screenshot", _solid_jpeg(8, 8), {"shot": "bs-1"},
                                         reads=(("web", "https://x.example/a"),))])
    running = asyncio.run_coroutine_threadsafe(gateway.call(grant, "probe.screenshot", {}), daemon.loop)
    assert started.wait(5)
    daemon.call_soon(gateway.cancel_tasks, {"task-1"})
    with pytest.raises(concurrent.futures.CancelledError):
        running.result(timeout=5)
    _time.sleep(0.5)
    ended, output = daemon.run(_row(daemon, "SELECT ended_at, output_json FROM spans WHERE kind = 'tool.call'"))
    assert ended is not None and json.loads(output)["evidence_error"] == "cancelled"
    assert (len(_files(daemon)), daemon.run(_q(daemon, "SELECT count(*) FROM evidence"))) in ((0, 0), (1, 1))


def test_task_end_forgets_its_initial_picture_marker(daemon: RunningDaemon, monkeypatch: pytest.MonkeyPatch) -> None:
    shots, _ = _fake_mac(daemon, monkeypatch)
    _menus(daemon)
    gateway = daemon.daemon.gateway
    g = grant_task(daemon, 'e', Identity.OWNER, {'mac:read', 'screen'}, 'look at my notes')
    assert 'image' in gateway_call(daemon, g, 'mac.menu', app='Notes')
    daemon.call_soon(gateway.forget_tasks, {'e'})
    assert not any(k[0] == 'e' for k in gateway._looked)
    assert [x['kind'] for x in daemon.run(_list(daemon.daemon.evidence, 'e'))] == ['app_window']

def test_an_initial_image_store_failure_does_not_fail_the_tool(daemon: RunningDaemon,
                                                                monkeypatch: pytest.MonkeyPatch) -> None:
    _fake_mac(daemon, monkeypatch)
    _menus(daemon)
    async def fail(*args, **kwargs):
        raise RuntimeError('private store diagnostic')
    monkeypatch.setattr(daemon.daemon.evidence, 'put_async', fail)
    r = gateway_call(daemon, grant_task(daemon, 'l', Identity.OWNER, {'mac:read', 'screen'}, 'notes'),
                     'mac.menu', app='Notes')
    assert not r['is_error'] and 'image' in r
    out = json.loads(daemon.run(_q(daemon, "SELECT output_json FROM spans WHERE kind='tool.call'")))
    assert out['evidence_error'] == 'RuntimeError' and 'private store diagnostic' not in str(out)

def test_a_store_error_in_the_trace_names_its_type_not_a_path(daemon: RunningDaemon,
                                                              monkeypatch: pytest.MonkeyPatch) -> None:
    async def full(*a: Any, **k: Any) -> str:
        raise PermissionError(13, "Permission denied", "/Users/someone/.ferb/evidence/2026-01-01/x.jpg")
    monkeypatch.setattr(daemon.daemon.evidence, "put_async", full)
    grant, _ = gateway_setup(daemon, {"probe"}, gateway_request("look"),
                             [_ImageTool("probe.screenshot", _solid_jpeg(8, 8), {"shot": "bs-1"})])
    assert not gateway_tool_call(daemon, grant, "probe.screenshot")["is_error"]
    out = json.loads(daemon.run(_q(daemon, "SELECT output_json FROM spans WHERE kind = 'tool.call'")))
    assert out["evidence_error"] == "PermissionError (errno 13)"


def test_the_store_needs_an_origin_writes_no_orphan_file_and_never_deletes_outside_itself(
        daemon: RunningDaemon) -> None:
    store = daemon.daemon.evidence
    with pytest.raises(TypeError):
        store.put("t1", "s1", "snapshot", data={"a": 1})            # type: ignore[call-arg]   # origin is required

    def boom(obj: Any) -> Any:
        raise RuntimeError("redactor broke")
    broken = EvidenceStore(store.root, store.clock, store.ids, boom, store.db)
    with pytest.raises(RuntimeError):
        daemon.run(_put(broken, "t1", "s1", "screenshot", image=_solid_jpeg(8, 8), data={"a": 1}, origin="ferb"))
    assert not [p for p in store.root.rglob("*") if p.is_file()]   # no file without its row
    outside = store.root.parent / "keep-me.txt"
    outside.write_text("not FERB's evidence")

    async def planted() -> None:
        daemon.daemon.db.conn.execute(
            "INSERT INTO evidence (id, task_id, kind, mime, path, origin, captured_at) VALUES "
            "('bad', 't1', 'screenshot', 'image/jpeg', '../keep-me.txt', 'ferb', '2000-01-01T00:00:00+00:00')")
    daemon.run(planted())
    assert daemon.run(_read(store, "bad"))[1] is None
    assert daemon.run(_purge(store, daemon.daemon.clock.now().isoformat())) == 1
    assert outside.read_text() == "not FERB's evidence" and daemon.run(_list(store, "t1")) == []


# ---------- helpers ----------

def _worker_and_checker(d: RunningDaemon) -> tuple[Task, Any]:
    """A registered worker and the checker of its result (same request, identity, device and tree)."""
    deadline = d.daemon.clock.now() + timedelta(minutes=10)
    worker = Task("r1.w", "r1", "buy it", "dev", Identity.OWNER, {"task:read"}, "worker", "req-r1", [], deadline)
    checker = Task("r1.w.verify", "r1.w", "check it", "dev", Identity.OWNER, {"task:read"}, "verifier", "req-r1", [],
                   deadline)

    async def go() -> Any:
        for t in (worker, checker):
            root = d.daemon.spans.start("task", task_id=t.id, input={})
            d.daemon.tasks.add(TaskRecord(t, d.daemon.clock.now(), root.id, "r1"))
        d.daemon.state.requests.record(Request("req-r1", "buy it"))
        return d.daemon.tokens.validate(d.daemon.tokens.issue(checker.id, "w-check", checker.scopes,
                                                              checker.identity, ttl_s=600))
    return worker, d.run(go())


def _menus(d: RunningDaemon) -> None:
    d.daemon.mac.ax.menus = lambda pid: [{"title": "File", "items": ["New Note"]}]   # type: ignore[union-attr]


async def _put(store: EvidenceStore, task_id: str, span_id: str, kind: str, *, image: bytes | None = None,
               data: dict[str, Any] | None = None, rects: list[tuple[int, int, int, int]] | None = None,
               origin: str) -> str:
    return store.put(task_id, span_id, kind, image=image, data=data, rects=rects, origin=origin)


async def _list(store: EvidenceStore, task_id: str) -> list[dict[str, Any]]:
    return store.list(task_id)


async def _read(store: EvidenceStore, eid: str) -> tuple[dict[str, Any], bytes | None]:
    return store.read(eid)


async def _purge(store: EvidenceStore, cutoff: str) -> int:
    return store.purge_before(cutoff)


async def _age(daemon: RunningDaemon, at: str, eid: str | None = None) -> None:
    if eid is None:
        daemon.daemon.db.conn.execute("UPDATE evidence SET captured_at = ?", (at,))
    else:
        daemon.daemon.db.conn.execute("UPDATE evidence SET captured_at = ? WHERE id = ?", (at, eid))


async def _q(daemon: RunningDaemon, sql: str, *values: Any) -> Any:
    return daemon.daemon.db.conn.execute(sql, values).fetchone()[0]


async def _row(daemon: RunningDaemon, sql: str) -> tuple[Any, ...]:
    return tuple(daemon.daemon.db.conn.execute(sql).fetchone())


async def _all(daemon: RunningDaemon, sql: str) -> list[Any]:
    return [r[0] for r in daemon.daemon.db.conn.execute(sql).fetchall()]


async def _raw(daemon: RunningDaemon, eid: str) -> dict[str, Any]:
    return dict(daemon.daemon.db.conn.execute("SELECT data_json FROM evidence WHERE id = ?", (eid,)).fetchone())
