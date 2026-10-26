"""Permission checks (ported from spike 6): known states only, and nothing ever prompts."""

from ferb.device import permissions as P
import asyncio
import threading

import pytest

KNOWN = {s.value for s in P.Status}


def test_all_checks_return_known_states() -> None:
    result = P.check_all()
    assert set(result) == {"accessibility", "screen_recording", "input_monitoring", "full_disk_access"}
    assert set(result.values()) <= KNOWN


def test_automation_without_asking_for_an_app_that_isnt_running() -> None:
    assert P.automation("com.ferb.no-such-app", ask=False) is P.Status.TARGET_NOT_RUNNING


def test_every_permission_has_a_settings_pane() -> None:
    assert set(P.PANES) >= {"accessibility", "screen_recording", "input_monitoring", "full_disk_access", "automation"}


def test_cancelled_permission_wait_does_not_block_loop_exit_or_duplicate_native_check(monkeypatch):
    entered, release = threading.Event(), threading.Event()
    calls = []

    def blocked(bundle_id, ask=False):
        calls.append((bundle_id, ask, threading.current_thread().daemon))
        entered.set()
        release.wait()
        return P.Status.GRANTED

    monkeypatch.setattr(P, 'automation', blocked)

    async def cancel_wait():
        waiter = asyncio.create_task(P.automation_async('com.spotify.client'))
        while not entered.is_set():
            await asyncio.sleep(0)
        waiter.cancel()
        with pytest.raises(asyncio.CancelledError):
            await waiter

    try:
        asyncio.run(cancel_wait())  # must return while the native check is still blocked
        pending = P.automation_check('com.spotify.client')
        assert pending is P.automation_check('com.spotify.client')
        assert not pending.done() and not pending.cancelled()
        assert calls == [('com.spotify.client', False, True)]
        release.set()
        assert pending.result(timeout=2) is P.Status.GRANTED
    finally:
        release.set()


def test_explicit_permission_request_does_not_share_a_passive_check(monkeypatch):
    release = threading.Event()
    def blocked(bundle_id, ask=False):
        release.wait()
        return P.Status.GRANTED if ask else P.Status.NOT_DETERMINED
    monkeypatch.setattr(P, 'automation', blocked)
    try:
        passive = P.automation_check('com.ferb.permission-test')
        requested = P.automation_check('com.ferb.permission-test', ask=True)
        assert passive is not requested
        release.set()
        assert passive.result(timeout=2) is P.Status.NOT_DETERMINED
        assert requested.result(timeout=2) is P.Status.GRANTED
    finally:
        release.set()


def test_cancelled_check_consumes_late_error_and_new_check_can_retry(monkeypatch):
    entered, release = threading.Event(), threading.Event()
    def failed(bundle_id, ask=False):
        entered.set()
        release.wait()
        raise OSError('native check failed')
    monkeypatch.setattr(P, 'automation', failed)
    async def go():
        errors = []
        asyncio.get_running_loop().set_exception_handler(lambda loop, ctx: errors.append(ctx))
        waiter = asyncio.create_task(P.automation_async('com.ferb.error-test'))
        while not entered.is_set():
            await asyncio.sleep(0)
        pending = P.automation_check('com.ferb.error-test')
        waiter.cancel()
        with pytest.raises(asyncio.CancelledError):
            await waiter
        release.set()
        while not pending.done():
            await asyncio.sleep(0)
        with pytest.raises(OSError, match='native check failed'):
            pending.result()
        # Let the cancelled waiter's late completion callbacks run.
        for _ in range(4):
            await asyncio.sleep(0)
        assert not errors
        with pytest.raises(OSError, match='native check failed'):
            await P.automation_async('com.ferb.error-test')
    try:
        asyncio.run(go())
    finally:
        release.set()


def test_passive_watcher_skips_pending_permission_without_blocking_or_caching_denial(daemon, monkeypatch):
    from concurrent.futures import Future
    pending = Future()
    calls = []
    def check(bundle_id, ask=False):
        calls.append((bundle_id, ask))
        return pending
    monkeypatch.setattr(P, 'automation_check', check)
    d = daemon.daemon
    assert d._automation_granted('com.apple.Safari') is False
    assert d._automation_granted('com.apple.Safari') is False
    assert calls == [('com.apple.Safari', False)]
    assert 'com.apple.Safari' not in d._automation_cache
    pending.set_result(P.Status.GRANTED)
    assert d._automation_granted('com.apple.Safari') is True
    assert d._automation_granted('com.apple.Safari') is True
    assert calls == [('com.apple.Safari', False)]

# ---------- L34 C: the setup window's rows and requests ----------

import os
from pathlib import Path

from ferb.device.profile import AppInfo

PANE = "x-apple.systempreferences:com.apple.preference.security?"


def test_rows_name_whose_grant_and_the_exact_pane() -> None:
    rows = {r["id"]: r for r in P.rows({"accessibility": "granted", "screen_recording": "denied",
                                        "full_disk_access": "denied", "input_monitoring": "not_determined"},
                                       {"profile_ok": True, "shell": True, "proxy": True})}
    assert list(rows) == ["accessibility", "screen_recording", "full_disk_access", "input_monitoring"]
    assert {r["process"] for r in rows.values()} == {"daemon"}
    assert rows["accessibility"]["pane"] == PANE + "Privacy_Accessibility"
    assert rows["screen_recording"]["pane"] == PANE + "Privacy_ScreenCapture"
    assert rows["full_disk_access"]["pane"] == PANE + "Privacy_AllFiles"
    assert rows["input_monitoring"]["pane"] == PANE + "Privacy_ListenEvent"
    assert [i for i, r in rows.items() if r["required"]] == ["accessibility", "screen_recording", "full_disk_access"]
    assert [i for i, r in rows.items() if r["requestable"]] == ["accessibility", "screen_recording", "input_monitoring"]
    # Input Monitoring too (2026-10-27): a friend's switch never ticked until the service restarted
    assert [i for i, r in rows.items() if r["restart_to_check"]] == ["screen_recording", "full_disk_access",
                                                                    "input_monitoring"]
    assert rows["accessibility"]["status"] == "granted" and rows["input_monitoring"]["status"] == "not_determined"
    assert all(r["blocked"] == "" for r in rows.values())
    assert P.rows({}, {"profile_ok": True, "shell": True, "proxy": True})[0]["status"] == "unknown"   # not reported


def test_full_disk_access_waits_for_containment() -> None:
    for containment in ({}, {"profile_ok": True, "shell": True},
                        {"profile_ok": True, "shell": True, "proxy": True, "proxy_alive": False}):
        fda = next(r for r in P.rows({}, containment) if r["id"] == "full_disk_access")
        assert fda["blocked"] == P.FDA_WAITS, containment
    assert P.fda_ready({"profile_ok": True, "shell": True, "proxy": True})
    assert P.fda_ready({"profile_ok": True, "shell": True, "proxy": True, "proxy_alive": True})


def test_hearing_row_is_ferb_voices_and_only_when_probed() -> None:
    def hearing(**probe: object) -> dict[str, object]:
        return next(r for r in P.rows({}, {}, dict(probe)) if r["id"] == "hearing")
    assert hearing(installed=True, signed=True, speech="granted")["status"] == "granted"
    assert hearing(installed=True, signed=True, speech="restricted")["status"] == "denied"
    assert hearing(installed=True, signed=True, speech="not_determined")["status"] == "not_determined"
    unsigned = hearing(installed=True, signed=False)
    assert unsigned["status"] == "unknown" and "signature" in str(unsigned["blocked"])
    assert "FERB Voice" in str(hearing(installed=False)["blocked"])
    row = hearing(installed=True, signed=True, speech="denied")
    assert row["process"] == "voice" and row["pane"] == PANE + "Privacy_SpeechRecognition"
    assert row["requestable"] and not row["required"]
    assert all(r["id"] != "hearing" for r in P.rows({}, {}))


def test_clipboard_row_says_whether_macos_lets_ferb_read_what_you_copy() -> None:
    def row(state: str | None) -> dict[str, object]:
        return P.clipboard_row(state)
    assert row("allow")["status"] == row("unrestricted")["status"] == "granted"      # unrestricted: before 15.4
    assert row("default")["status"] == "not_determined" and row("default")["requestable"]   # macOS hasn't asked yet
    assert row("ask")["status"] == row("deny")["status"] == "denied"                  # the owner sets Allow in the pane
    assert not row("ask")["requestable"] and row(None)["status"] == "unknown"
    assert row("default")["pane"] == PANE + "Privacy_Pasteboard"
    assert row("default")["process"] == "daemon" and not row("default")["required"] and row("default")["blocked"] == ""


def test_app_panes_name_ferb_app_itself() -> None:
    assert P.app_panes("com.ferb.FERB") == {
        "notifications": "x-apple.systempreferences:com.apple.Notifications-Settings.extension?id=com.ferb.FERB",
        "microphone": PANE + "Privacy_Microphone", "speech_recognition": PANE + "Privacy_SpeechRecognition"}


def test_request_asks_macos_only_for_the_three_with_an_api(monkeypatch: pytest.MonkeyPatch) -> None:
    asked: list[tuple[str, object]] = []
    monkeypatch.setattr(P, "_ax_trusted", lambda prompt: asked.append(("ax", prompt)) or False)
    monkeypatch.setattr(P, "_request_screen_capture", lambda: asked.append(("screen", None)))
    monkeypatch.setattr(P, "_request_listen_event", lambda: asked.append(("hid", None)))
    monkeypatch.setattr(P, "screen_recording", lambda: P.Status.DENIED)
    monkeypatch.setattr(P, "input_monitoring", lambda: P.Status.GRANTED)
    assert P.request("accessibility") is P.Status.DENIED
    assert asked == [("ax", True), ("ax", False)]                 # the prompt, then a plain read
    assert P.request("screen_recording") is P.Status.DENIED
    assert P.request("input_monitoring") is P.Status.GRANTED
    assert asked[2:] == [("screen", None), ("hid", None)]
    for other in ("full_disk_access", "automation", "hearing", "clipboard", "nope"):
        with pytest.raises(ValueError):
            P.request(other)


def test_speech_request_is_ferb_voices_own_and_waits() -> None:
    app = Path("/Applications/FERB.app/Contents/Helpers/FERB Voice.app")
    assert P.speech_request_argv(app) == ["/usr/bin/open", "-n", "-W", str(app), "--args", "request-speech"]


def test_automation_candidates_are_the_running_scriptable_apps() -> None:
    apps = [AppInfo("com.apple.Notes", "Notes", "1", "/A/Notes.app", True),
            AppInfo("com.apple.Calculator", "Calculator", "1", "/A/Calculator.app", False),
            AppInfo("com.apple.mail", "Mail", "1", "/A/Mail.app", True)]
    assert P.automation_candidates(apps, {"com.apple.Notes", "com.apple.Calculator"}) == [("Notes", "com.apple.Notes")]


def test_grant_target_is_the_agent_app_or_the_checkout_interpreter(tmp_path: Path) -> None:
    assert P.grant_target(Path("/Applications/FERB.app"), "/x") == "/Applications/FERB.app/Contents/Helpers/FERB.app"
    real = tmp_path / "python3.12"
    real.write_text("")
    link = tmp_path / "python"
    link.symlink_to(real)
    assert P.grant_target(None, str(link)) == os.path.realpath(real)
