"""Owner 2026-10-26: "ask when first needed, like Claude and ChatGPT". Setup asks only Accessibility and Screen
Recording; FERB asks for the rest the first time it needs each one."""
from ferb.act.fs import files
from ferb.device.permissions import Status


def test_a_file_tool_blocked_by_macos_says_full_disk_access_is_needed(monkeypatch) -> None:
    monkeypatch.setattr(files, "_full_disk_access", lambda: Status.DENIED)
    result = files._err("couldn't read /Users/me/Library/Mail/x: [Errno 1] Operation not permitted")
    assert result.is_error
    assert result.content["code"] == "needs_permission"
    assert result.content["permission"] == "Full Disk Access"
    assert "System Settings" in result.content["detail"]


def test_an_ordinary_file_error_stays_an_ordinary_error(monkeypatch) -> None:
    monkeypatch.setattr(files, "_full_disk_access", lambda: Status.DENIED)
    result = files._err("no such file: /tmp/missing.txt")
    assert result.content == {"error": "no such file: /tmp/missing.txt"}


def test_with_full_disk_access_on_a_blocked_read_stays_the_plain_error(monkeypatch) -> None:
    monkeypatch.setattr(files, "_full_disk_access", lambda: Status.GRANTED)
    result = files._err("couldn't read x: [Errno 1] Operation not permitted")
    assert "code" not in result.content


def test_the_first_applescript_for_an_app_brings_macos_own_prompt() -> None:
    """The check asks without a prompt first; only an app macOS hasn't asked about yet gets the prompt."""
    import inspect
    from ferb.act.mac import tools
    src = inspect.getsource(tools)
    assert "automation_async(a.bundle_id, ask=False)" in src and "automation_async(a.bundle_id, ask=True)" in src
    assert "if grant is Status.NOT_DETERMINED" in src


def test_input_monitoring_is_asked_once_with_the_first_request(monkeypatch) -> None:
    import asyncio
    from types import SimpleNamespace
    from ferb.daemon.main import Daemon
    from ferb.device import permissions as P
    asked: list[str] = []
    rows: list[tuple[str, dict]] = []
    fake = SimpleNamespace(_input_monitoring_asked=False, permission_request=asked.append,
                           _journal=lambda: SimpleNamespace(append=lambda kind, data: rows.append((kind, data))))
    monkeypatch.setattr(P, "input_monitoring", lambda: P.Status.NOT_DETERMINED)

    async def run() -> None:
        Daemon._ask_input_monitoring(fake)          # type: ignore[arg-type]
        Daemon._ask_input_monitoring(fake)          # type: ignore[arg-type]
        await asyncio.sleep(0.05)
    asyncio.run(run())
    assert asked == ["input_monitoring"] and rows[0][0] == "permission.asked"


def test_input_monitoring_already_decided_is_never_asked(monkeypatch) -> None:
    from types import SimpleNamespace
    from ferb.daemon.main import Daemon
    from ferb.device import permissions as P
    asked: list[str] = []
    fake = SimpleNamespace(_input_monitoring_asked=False, permission_request=asked.append, _journal=lambda: None)
    monkeypatch.setattr(P, "input_monitoring", lambda: P.Status.DENIED)
    Daemon._ask_input_monitoring(fake)              # type: ignore[arg-type]
    assert asked == []
