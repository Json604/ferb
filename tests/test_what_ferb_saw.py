"""Step 12e: "What FERB saw". FERB.app shows the screenshots FERB kept (the owner's front window as each message was
sent, and the images its tasks captured), exactly as the model got them: the stored redacted copies, never a capture
taken anew, never an image of an app or site on the owner's exclusion list, and to FERB.app's own process only
(auth.APP_ONLY; its code signature is stood in for here)."""
import base64
from typing import Any

import pytest

from ferb.cli.client import Client, RpcError
from tests.conftest import RunningDaemon, gateway_request, gateway_setup

JPEG = b"\xff\xd8\xff\xe0redacted-front-window"
SHOT = b"\xff\xd8\xff\xe0redacted-task-shot"


def _keep(daemon: RunningDaemon, task: str, kind: str, image: bytes | None, data: dict[str, Any],
          mime: str = "image/jpeg", origin: str = "private") -> str:
    async def go() -> str:
        return await daemon.daemon.evidence.put_async(task, None, kind, image=image, data=data, mime=mime,
                                                      origin=origin)
    return str(daemon.run(go()))


def _rows(daemon: RunningDaemon) -> int:
    async def go() -> int:
        return int(daemon.daemon.db.conn.execute("SELECT count(*) FROM evidence").fetchone()[0])
    return int(daemon.run(go()))


def _as_app(daemon: RunningDaemon) -> Client:
    daemon.daemon.server.is_app = lambda token: token is not None      # FERB.app's signature, stood in for
    return Client(daemon.sock)


def test_only_ferb_apps_own_process_may_list_or_see_the_screenshots(daemon: RunningDaemon) -> None:
    eid = _keep(daemon, "req-1", "front_window", JPEG, {"app": "Safari", "bundle_id": "com.apple.Safari"})
    c = Client(daemon.sock)
    try:
        for method, params in (("seen.list", {}), ("seen.image", {"id": eid})):
            with pytest.raises(RpcError) as refused:
                c.call(method, params)
            assert "FERB.app only" in str(refused.value)
    finally:
        c.close()


def test_a_messages_screenshots_are_its_front_window_and_its_tasks_images_newest_first(daemon: RunningDaemon) -> None:
    grant, _ = gateway_setup(daemon, {"shell"}, gateway_request("look at this"), [])    # task-1 of request req-1
    front = _keep(daemon, "req-1", "front_window", JPEG,
                  {"app": "Google Chrome", "bundle_id": "com.google.Chrome", "url": "https://docs.example/a?x=1",
                   "tab_title": "Quarterly plan"})
    daemon.call_soon(daemon.daemon.clock.advance, 5)
    shot = _keep(daemon, grant.task_id, "screenshot", SHOT, {"tool": "mac.screenshot", "app": "Notes"})
    _keep(daemon, grant.task_id, "screenshot", None, {"tool": "mac.screenshot", "image_withheld": "unmaskable"})
    _keep(daemon, grant.task_id, "recording", b"\x00\x00video", {"tool": "mac.record"}, mime="video/mp4")
    _keep(daemon, "req-2", "front_window", JPEG, {"app": "Mail"})                  # another message's
    before = _rows(daemon)
    c = _as_app(daemon)
    try:
        listed = c.call("seen.list", {"request_id": "req-1"})
        by_task = c.call("seen.list", {"task_id": grant.task_id})
        everything = c.call("seen.list", {})
        image = c.call("seen.image", {"id": front})
    finally:
        c.close()
    assert [x["id"] for x in listed["captures"]] == [shot, front]         # images only, newest first
    first = listed["captures"][1]
    assert first["kind"] == "front_window" and first["app"] == "Google Chrome" and first["site"] == "docs.example"
    assert first["title"] == "Quarterly plan" and first["at"]
    assert [x["id"] for x in by_task["captures"]] == [shot]
    assert everything["total"] == 3 and everything["days"] == 7
    assert base64.b64decode(image["image"]) == JPEG and image["mime"] == "image/jpeg"   # the stored copy, as kept
    assert _rows(daemon) == before                                       # nothing captured anew


def test_an_excluded_apps_image_is_never_listed_or_served_and_old_ones_age_out(daemon: RunningDaemon) -> None:
    secret = _keep(daemon, "req-1", "front_window", JPEG, {"app": "Vault", "bundle_id": "com.example.vault"})
    site = _keep(daemon, "req-1", "screenshot", SHOT, {"tool": "browser.screenshot", "url": "https://bank.example/x"})
    kept = _keep(daemon, "req-1", "screenshot", SHOT, {"tool": "browser.screenshot", "url": "https://news.example/"})
    daemon.daemon.config.set("privacy", "exclude_apps", ["com.example.vault"])      # excluded after it was kept
    daemon.daemon.config.set("privacy", "exclude_domains", ["bank.example"])
    c = _as_app(daemon)
    try:
        assert [x["id"] for x in c.call("seen.list", {"request_id": "req-1"})["captures"]] == [kept]
        for eid in (secret, site):
            with pytest.raises(RpcError):
                c.call("seen.image", {"id": eid})
        daemon.call_soon(daemon.daemon.clock.advance, 8 * 86400)                  # past the 7-day evidence window
        assert c.call("seen.list", {})["captures"] == []
        with pytest.raises(RpcError):
            c.call("seen.image", {"id": kept})
    finally:
        c.close()
