"""L34 Phase E: the FERB window's Settings change config.toml only through `settings.set`, a method of the owner's own
connection (the CLI role), only for listed keys (rule 10), with every value checked (rule 5)."""

import plistlib
import tomllib
from pathlib import Path
from typing import Any

import pytest

from ferb.cli.client import Client, RpcError
from ferb.core.types import Identity
from ferb.daemon import auth
from ferb.daemon.server import UNAUTHORIZED
from ferb.daemon.settings import SETTINGS
from tests.conftest import RunningDaemon

ROOT = Path(__file__).resolve().parent.parent


def _call(daemon: RunningDaemon, method: str, params: dict[str, Any] | None = None) -> dict[str, Any]:
    c = Client(daemon.sock)
    try:
        return c.call(method, params or {})
    finally:
        c.close()


def _config(daemon: RunningDaemon) -> dict[str, Any]:
    path = daemon.daemon.home / "config.toml"
    return tomllib.loads(path.read_text()) if path.exists() else {}


async def _of_kind(daemon: RunningDaemon, kind: str) -> list[Any]:
    return daemon.daemon.journal.of_kind(kind)       # type: ignore[union-attr]


def test_settings_get_lists_every_setting(daemon: RunningDaemon) -> None:
    values = _call(daemon, "settings.get")["values"]
    assert set(values) == set(SETTINGS)
    assert values["sense.window_titles"] is True
    assert values["retention.evidence_days"] == 7
    assert "com.1password.1password" in values["privacy.exclude_apps"]


def test_a_setting_is_checked_written_and_journaled(daemon: RunningDaemon) -> None:
    assert _call(daemon, "settings.set", {"key": "retention.trace_days", "value": 14}) == {
        "key": "retention.trace_days", "value": 14}
    assert _config(daemon)["retention"]["trace_days"] == 14
    rows = daemon.run(_of_kind(daemon, "settings.changed"))
    assert rows[-1].data == {"key": "retention.trace_days", "value": 14} and "owner" in rows[-1].origin


@pytest.mark.parametrize("key,value", [
    ("sense.window_titles", "yes"), ("retention.trace_days", 0), ("retention.trace_days", True),
    ("retention.trace_days", 2.5), ("privacy.exclude_apps", "com.x.y"), ("privacy.exclude_apps", ["ok", "bad\nline"]),
    ("privacy.exclude_apps", [""]), ("privacy.exclude_domains", "example.com"),
])
def test_a_value_of_the_wrong_kind_is_refused_and_nothing_changes(daemon: RunningDaemon, key: str, value: Any) -> None:
    before = _config(daemon)
    with pytest.raises(RpcError, match=key.replace(".", r"\.")):
        _call(daemon, "settings.set", {"key": key, "value": value})
    assert _config(daemon) == before


def test_sites_are_hostnames_and_a_url_is_refused(daemon: RunningDaemon) -> None:
    stored = _call(daemon, "settings.set", {"key": "privacy.exclude_domains",
                                            "value": [".Example.COM", "news.site.org", "example.com"]})
    assert stored["value"] == ["example.com", "news.site.org"]
    with pytest.raises(RpcError, match="no https:// or path"):
        _call(daemon, "settings.set", {"key": "privacy.exclude_domains", "value": ["https://www.example.com/x"]})
    assert _config(daemon)["privacy"]["exclude_domains"] == ["example.com", "news.site.org"]


@pytest.mark.parametrize("key,value", [
    ("protection.never_write", ["/tmp"]), ("feedback.address", "x@example.org"), ("secrets.github", "token"),
    ("models.codex", "gpt-x"), ("mailbox.address", "a@example.org"), ("privacy.exclude_paths", []),
    ("identity.profile", "Profile 9"), ("runtimes.worker", "claude"), ("banners.question", False),
    ("sense.watch_folders", ["/"]), ("", True),
])
def test_keys_outside_the_list_cannot_be_written_from_the_app(daemon: RunningDaemon, key: str, value: Any) -> None:
    before = _config(daemon)
    with pytest.raises(RpcError, match="can't be changed from FERB's window"):
        _call(daemon, "settings.set", {"key": key, "value": value})
    assert _config(daemon) == before


def test_only_the_owners_connection_may_read_or_change_settings() -> None:
    for method in ("settings.get", "settings.set"):
        assert auth.allowed(auth.Role.CLI, method)
        for role in (auth.Role.MCP_SHIM, auth.Role.VOICE, auth.Role.NATIVE_HOST):
            assert not auth.allowed(role, method), (role, method)


def test_a_worker_token_cannot_change_settings_over_the_socket(daemon: RunningDaemon) -> None:
    """Rule 10: a worker's MCP shim (a run token) is refused, as the shell is refused the socket (test_shell_sandbox)."""
    async def issue() -> str:
        return daemon.daemon.tokens.issue("t-settings", "w-settings", set(), Identity.OWNER, 60)
    token = daemon.run(issue())
    before = _config(daemon)
    shim = Client(daemon.sock, role="mcp_shim", token=token)
    try:
        with pytest.raises(RpcError) as refused:
            shim.call("settings.set", {"key": "sense.window_titles", "value": False})
        assert refused.value.code == UNAUTHORIZED
    finally:
        shim.close()
    assert _config(daemon) == before


def test_no_tool_is_named_for_settings_and_ferb_never_operates_the_window(daemon: RunningDaemon) -> None:
    """The window belongs to FERB.app, one of FERB's own apps: every Mac tool refuses it (driver.OWN_APP)."""
    from ferb.act.mac.driver import MacComputerDriver
    from ferb.device.install import APP_BUNDLE_ID
    assert not any(str(name).startswith("settings") for name in daemon.daemon.registry.capabilities)
    info = plistlib.loads((ROOT / "ui" / "mac" / "FERBNotch" / "Info.plist").read_bytes())
    assert info["CFBundleIdentifier"] == APP_BUNDLE_ID
    assert MacComputerDriver._own_app(APP_BUNDLE_ID, -1)
