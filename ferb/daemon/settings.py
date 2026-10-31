"""Own the settings FERB.app's window may change in config.toml through `settings.set` (L34 Phase E). Must not let
anything but the owner's connection write them, or let an unchecked value through.

Rule 10 (FERB never changes its own config): these writes come only through `settings.set`, a method of the CLI role
alone (auth.py). That role is proven by the owner's uid on the daemon's socket, which FERB's shell and AppleScript
can't reach (the sandbox names the socket last: sandbox.py, test_shell_sandbox), no tool calls, and FERB's Mac tools
never operate FERB.app (driver.OWN_APP). Each value is checked here (rule 5). A key not in SETTINGS can't be written
this way: [protection], secrets and API keys, models, the report relay address, exclude_paths and watch_folders stay in
config.toml; the runtimes have `runtimes.use` and the banner switches `notifications.categories`, each owner-only and
checked where it is defined. FERB's own account (`[mailbox] address`, `[identity] browser/profile`, daemon decision 7)
is written only by `write_identity`, which FERB.app's `identity.choose` calls after code found exactly that account
in that profile (auth.APP_ONLY)."""

import re
from dataclasses import dataclass
from typing import Any, Callable

from ferb.config import DEFAULT_EXCLUDE_APPS, Config
from ferb.core.errors import FerbError
from ferb.store.retention import MIN_DAYS, RULES

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


def _profile(value: Any) -> str:
    """`browser/folder` of one of the owner's browser profiles (ferb/act/browser/profiles.py BROWSERS), or "" for none."""
    from ferb.act.browser.profiles import BROWSERS
    if not isinstance(value, str):
        raise BadSetting("must be a browser profile as browser/folder, or empty")
    text = value.strip()
    browser, _, folder = text.partition("/")
    if text and (browser not in BROWSERS or not folder.strip() or len(text) > APP_CHARS
                 or any(ord(c) < 32 for c in text)):
        raise BadSetting(f"has {value!r}, which isn't a browser profile as browser/folder")
    return text


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
    # how long records are kept: every retention rule with a setting (ferb/store/retention.py RULES, one table)
    *(Setting(r.setting[0], r.setting[1], _days, r.default) for r in RULES if r.setting and r.default is not None),
    Setting("browsing", "owner_profile", _profile, ""),         # daemon decision 7: browsing as the owner, when chosen
)}


PRIVACY = frozenset({"privacy.exclude_apps", "privacy.exclude_domains"})   # the daemon applies these at once


def read(config: Config) -> dict[str, Any]:
    """Every listed setting's value: config.toml's, else its default."""
    values: dict[str, Any] = {}
    for name, s in SETTINGS.items():
        raw = config.get(s.section, s.key)
        values[name] = raw if raw is not None else s.default
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


EMAIL = re.compile(r"[^@\s]+@[^@\s]+\.[^@\s]+")
IDENTITY_KEYS = (("mailbox", "provider"), ("mailbox", "address"), ("identity", "browser"), ("identity", "profile"))


def write_identity(config: Config, address: str, browser: str, folder: str) -> dict[str, str]:
    """FERB's own Google account and the browser profile it is signed into (daemon decision 7), in one checked write
    each; or, with every value empty, none (the owner removed it: all four keys go). Returns what was stored."""
    from ferb.act.browser.profiles import BROWSERS
    import tomlkit
    address, browser, folder = address.strip().lower(), browser.strip(), folder.strip()
    doc = config.load()                          # one atomic write for the four keys, set or removed
    if not (address or browser or folder):
        present = [(section, key) for section, key in IDENTITY_KEYS if key in doc.get(section, {})]
        for section, key in present:
            del doc[section][key]
        if present:
            config.save(doc)
        return {"address": "", "browser": "", "profile": ""}
    if not EMAIL.fullmatch(address) or len(address) > 320:
        raise BadSetting(f"{address!r} isn't an email address")
    if browser not in BROWSERS or not folder or len(folder) > APP_CHARS or any(ord(c) < 32 for c in folder):
        raise BadSetting("FERB's profile must be one of your browsers' profiles")
    for (section, key), value in zip(IDENTITY_KEYS, ("gmail", address, browser, folder)):
        if section not in doc:
            doc[section] = tomlkit.table()
        doc[section][key] = value
    config.save(doc)
    return {"address": address, "browser": browser, "profile": folder}
