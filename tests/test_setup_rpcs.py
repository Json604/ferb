"""L34 C: the setup window's permission RPCs. Rows are the daemon's own grants with their exact panes; the owner's Grant
becomes a macOS request made by the daemon; Done is recorded as `ferb setup` records it. Nothing here prompts: every
macOS call is replaced."""
import asyncio
from typing import Any, Callable

import pytest

from ferb.cli.client import Client, RpcError
from ferb.device import permissions as P
from ferb.device.profile import AppInfo, DeviceProfile
from tests.conftest import RunningDaemon


def call(daemon: RunningDaemon, method: str, params: dict[str, Any] | None = None) -> dict[str, Any]:
    client = Client(daemon.sock)
    try:
        return client.call(method, params or {})
    finally:
        client.close()


def on_loop(daemon: RunningDaemon, fn: Callable[[], Any]) -> Any:
    async def go() -> Any:
        return fn()
    return daemon.run(go())


@pytest.fixture
def grants(daemon: RunningDaemon) -> dict[str, str]:
    """macOS as the daemon reads it, faked; a request grants the permission, as the owner would in System Settings."""
    state = {"accessibility": "denied", "screen_recording": "granted", "input_monitoring": "not_determined",
             "full_disk_access": "denied"}
    d = daemon.daemon

    def requested(permission: str) -> P.Status:
        state[permission] = "granted"
        return P.Status.GRANTED
    d.permission_check = lambda: dict(state)                   # type: ignore[method-assign]
    d.permission_request = requested                           # type: ignore[method-assign]
    d._setup_containment = lambda: {"profile_ok": True, "shell": True, "proxy": True}   # type: ignore[method-assign]
    d._device_profile = lambda: None                           # type: ignore[method-assign]
    d._clipboard_access = lambda: "default"                    # type: ignore[method-assign]   # macOS hasn't asked
    return state


def notes_profile() -> DeviceProfile:
    return DeviceProfile("dev", "macOS", "15.0", "2026-10-25T10:00:00", [
        AppInfo("com.apple.Notes", "Notes", "1", "/A/Notes.app", True),
        AppInfo("com.apple.Calculator", "Calculator", "1", "/A/Calculator.app", False),
        AppInfo("com.apple.mail", "Mail", "1", "/A/Mail.app", True)], [])


def test_permission_rows_are_the_daemons_own_with_their_panes(daemon: RunningDaemon, grants: dict[str, str]) -> None:
    reply = call(daemon, "setup.permissions")
    rows = {r["id"]: r for r in reply["rows"]}
    assert list(rows) == ["accessibility", "screen_recording", "full_disk_access", "input_monitoring", "clipboard"]
    assert rows["accessibility"]["status"] == "denied" and rows["screen_recording"]["status"] == "granted"
    assert rows["clipboard"]["status"] == "not_determined" and rows["clipboard"]["pane"].endswith("?Privacy_Pasteboard")
    assert rows["accessibility"]["pane"].endswith("?Privacy_Accessibility")
    assert reply["app_panes"]["notifications"].endswith("?id=com.ferb.FERB")
    assert reply["automation_pane"].endswith("?Privacy_Automation")
    assert reply["grant_target"] and reply["automation"] == [] and reply["setup_done"] is False


def test_full_disk_access_row_waits_for_the_containment_the_daemon_started_with(
        daemon: RunningDaemon, grants: dict[str, str]) -> None:
    daemon.daemon._setup_containment = lambda: {"profile_ok": False, "shell": False, "proxy": True}   # type: ignore[method-assign]
    fda = next(r for r in call(daemon, "setup.permissions")["rows"] if r["id"] == "full_disk_access")
    assert fda["blocked"] == P.FDA_WAITS


def test_automation_lists_running_scriptable_apps_with_their_state(daemon: RunningDaemon, grants: dict[str, str]) -> None:
    d = daemon.daemon
    d._device_profile = notes_profile                         # type: ignore[method-assign]
    d._running_apps = lambda: [("com.apple.Notes", "Notes", 11), ("com.apple.Calculator", "Calculator", 12)]  # type: ignore[method-assign]

    async def status(bundle: str) -> P.Status:
        return P.Status.NOT_DETERMINED
    d.automation_check = status                                # type: ignore[method-assign]
    assert call(daemon, "setup.permissions")["automation"] == [
        {"name": "Notes", "bundle_id": "com.apple.Notes", "status": "not_determined"}]


def test_a_hung_automation_check_reads_unknown_within_the_reply(daemon: RunningDaemon, grants: dict[str, str]) -> None:
    d = daemon.daemon
    d._device_profile = notes_profile                         # type: ignore[method-assign]
    d._running_apps = lambda: [("com.apple.Notes", "Notes", 11)]   # type: ignore[method-assign]

    async def hang(bundle: str) -> P.Status:
        await asyncio.sleep(30)
        return P.Status.GRANTED
    d.automation_check = hang                                  # type: ignore[method-assign]
    assert call(daemon, "setup.permissions")["automation"][0]["status"] == "unknown"


def test_hearing_is_probed_only_when_asked(daemon: RunningDaemon, grants: dict[str, str]) -> None:
    async def probe(asked: bool) -> dict[str, Any]:
        return {"installed": True, "signed": True, "speech": "not_determined"} if asked else {"installed": True}
    daemon.daemon._hearing_status = probe                      # type: ignore[method-assign]
    assert all(r["id"] != "hearing" for r in call(daemon, "setup.permissions")["rows"])
    hearing = next(r for r in call(daemon, "setup.permissions", {"hearing": True})["rows"] if r["id"] == "hearing")
    assert hearing["status"] == "not_determined" and hearing["process"] == "voice"


def test_grant_asks_macos_in_the_daemon_and_journals_it(daemon: RunningDaemon, grants: dict[str, str]) -> None:
    assert call(daemon, "setup.permission_request", {"permission": "accessibility"}) == {
        "permission": "accessibility", "status": "granted"}
    entries = on_loop(daemon, lambda: daemon.daemon._journal().of_kind("setup.permission_requested"))
    assert [e.data for e in entries] == [{"permission": "accessibility", "status": "granted"}]
    for refused in ("full_disk_access", "automation", "microphone", ""):
        with pytest.raises(RpcError):
            call(daemon, "setup.permission_request", {"permission": refused})


def test_clipboard_grant_lets_macos_ask_once_and_journals_what_it_answered(daemon: RunningDaemon,
                                                                          grants: dict[str, str]) -> None:
    daemon.daemon.clipboard_request = lambda: "ask"            # type: ignore[method-assign]   # macOS: ask each time
    assert call(daemon, "setup.permission_request", {"permission": "clipboard"}) == {
        "permission": "clipboard", "status": "denied"}         # the owner still has to set Allow in the pane
    access = on_loop(daemon, lambda: daemon.daemon._journal().of_kind("clipboard.access"))
    assert [e.data for e in access] == [{"access": "ask", "requested": True}]


def test_hearing_grant_runs_ferb_voices_own_request(daemon: RunningDaemon, grants: dict[str, str]) -> None:
    ran: list[Any] = []
    d = daemon.daemon
    d.speech_request = ran.append                              # type: ignore[method-assign]

    async def probe(asked: bool) -> dict[str, Any]:
        return {"installed": True, "signed": True, "speech": "granted"}
    d._hearing_status = probe                                  # type: ignore[method-assign]
    assert call(daemon, "setup.permission_request", {"permission": "hearing"})["status"] == "granted"
    assert ran == [d.voice_bundle]


def test_setup_is_recorded_done_only_when_every_permission_is_granted(daemon: RunningDaemon,
                                                                      grants: dict[str, str]) -> None:
    """Every FERB permission is needed (owner, 2026-10-26): Done records `permissions_version` only when the daemon's
    own rows, FERB Voice's and FERB.app's are all granted; otherwise it says what's missing and records nothing."""
    d = daemon.daemon

    async def probe(asked: bool) -> dict[str, Any]:
        return {"installed": True, "signed": True, "speech": "granted"}
    d._hearing_status = probe                                  # type: ignore[method-assign]
    assert call(daemon, "setup.permissions")["setup_done"] is False
    reply = call(daemon, "setup.finish", {"app": {"notifications": "granted", "microphone": "granted"}})
    assert reply["setup_done"] is False
    assert {m["id"] for m in reply["missing"]} == {"accessibility", "input_monitoring", "full_disk_access", "clipboard"}
    assert d.config.get("setup", "permissions_version", 0) == 0
    grants.update(accessibility="granted", input_monitoring="granted", full_disk_access="granted")
    d._clipboard_access = lambda: "allow"                      # type: ignore[method-assign]
    reply = call(daemon, "setup.finish", {"app": {"notifications": "denied", "microphone": "granted"}})
    assert reply["setup_done"] is False and [m["id"] for m in reply["missing"]] == ["notifications"]
    assert d.config.get("setup", "permissions_version", 0) == 0
    assert call(daemon, "setup.finish", {"app": {"notifications": "granted", "microphone": "granted"}})["setup_done"]
    assert d.config.get("setup", "permissions_version") == 27
    assert call(daemon, "setup.permissions")["setup_done"] is True
    d.config.set("setup", "permissions_version", 31)          # a later setup never goes backwards
    call(daemon, "setup.finish", {"app": {"notifications": "granted", "microphone": "granted"}})
    assert d.config.get("setup", "permissions_version") == 31


def test_every_permission_row_is_needed_none_optional(daemon: RunningDaemon, grants: dict[str, str]) -> None:
    async def probe(asked: bool) -> dict[str, Any]:
        return {"installed": True, "signed": True, "speech": "not_determined"}
    daemon.daemon._hearing_status = probe                      # type: ignore[method-assign]
    rows = call(daemon, "setup.permissions", {"hearing": True})["rows"]
    assert {r["id"] for r in rows} >= {"hearing", "clipboard"} and all(r["required"] for r in rows)
