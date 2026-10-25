"""Own the settings FERB.app's window may change in config.toml through `settings.set` (L34 Phase E). Must not let
anything but the owner's connection write them, or let an unchecked value through.

Rule 10 (FERB never changes its own config): these writes come only through `settings.set`, a method of the CLI role
alone (auth.py). That role is proven by the owner's uid on the daemon's socket, which FERB's shell and AppleScript
can't reach (the sandbox names the socket last: sandbox.py, test_shell_sandbox), no tool calls, and FERB's Mac tools
never operate FERB.app (driver.OWN_APP). Each value is checked here (rule 5). A key not in SETTINGS can't be written
this way: [protection], secrets and API keys, models, the mailbox and identity, the feedback address, exclude_paths
and watch_folders stay in config.toml and the CLI; the runtimes have `runtimes.use` and the banner switches
`notifications.categories`, each owner-only and checked where it is defined."""

import re
from dataclasses import dataclass
from typing import Any, Callable

from ferb.config import DEFAULT_EXCLUDE_APPS, Config
from ferb.core.errors import FerbError
from ferb.store.retention import MIN_DAYS

# A site as [privacy] exclude_domains holds it (sense/exclusions.py matches the host and its subdomains)
HOST = re.compile(r"[a-z0-9]([a-z0-9-]*[a-z0-9])?(\.[a-z0-9]([a-z0-9-]*[a-z0-9])?)*")
APP_CHARS = 200          # an app's bundle id or name, as [privacy] exclude_apps holds it


class BadSetting(FerbError, ValueError):
    """A key the app may not write, or a value of the wrong kind."""


def _bool(value: Any) -> bool:
    if not isinstance(value, bool):
        raise BadSetting("must be true or false")
    return value


def _days(value: Any) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or value < MIN_DAYS:
        raise BadSetting(f"must be a whole number of days, at least {MIN_DAYS}")
    return value


def _apps(value: Any) -> list[str]:
    if not isinstance(value, list):
        raise BadSetting("must be a list of apps")
    out: list[str] = []
    for item in value:
        text = item.strip() if isinstance(item, str) else ""
        if not text or len(text) > APP_CHARS or any(ord(c) < 32 for c in text):
            raise BadSetting(f"has {item!r}, which isn't an app's bundle id or name")
        if text not in out:
            out.append(text)
    return out


def _hosts(value: Any) -> list[str]:
    if not isinstance(value, list):
        raise BadSetting("must be a list of sites")
    out: list[str] = []
    for item in value:
        host = item.strip().lower().strip(".") if isinstance(item, str) else ""
        if not HOST.fullmatch(host):
            raise BadSetting(f"has {item!r}, which isn't a site name like example.com (no https:// or path)")
        if host not in out:
            out.append(host)
    return out


@dataclass(frozen=True)
class Setting:
    section: str
    key: str
    check: Callable[[Any], Any]
    default: Any

    @property
    def name(self) -> str:
        return f"{self.section}.{self.key}"


SETTINGS: dict[str, Setting] = {s.name: s for s in (
    Setting("privacy", "exclude_apps", _apps, list(DEFAULT_EXCLUDE_APPS)),
    Setting("privacy", "exclude_domains", _hosts, []),
    Setting("sense", "window_titles", _bool, True),
    Setting("sense", "trail_retention_days", _days, 30),
    Setting("retention", "trace_days", _days, 30),
    Setting("retention", "decision_days", _days, 30),
    Setting("retention", "evidence_days", _days, 7),
    Setting("retention", "recovery_days", _days, 7),
    Setting("retention", "resource_days", _days, 30),
)}


def _plain(value: Any) -> Any:
    """A tomlkit item as a plain value (its strings, numbers, booleans and arrays)."""
    return value.unwrap() if hasattr(value, "unwrap") else value


def read(config: Config) -> dict[str, Any]:
    """Every listed setting's value: config.toml's, else its default."""
    doc = config.load()
    values: dict[str, Any] = {}
    for name, s in SETTINGS.items():
        table = doc.get(s.section)
        raw = table.get(s.key) if table is not None else None
        values[name] = _plain(raw) if raw is not None else s.default
    return values


def write(config: Config, name: str, value: Any) -> Any:
    """Check one value and write it in one atomic save (Config.set); returns what was stored."""
    setting = SETTINGS.get(name)
    if setting is None:
        raise BadSetting(f"{name or '(no key)'} can't be changed from FERB's window")
    try:
        checked = setting.check(value)
    except BadSetting as e:
        raise BadSetting(f"{name} {e}") from None
    config.set(setting.section, setting.key, checked)
    return checked
