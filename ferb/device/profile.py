"""Represent and persist device inventory and capability observations. Must not treat a historical probe as authorization.

DeviceProfile: load / save / diff, method resolution, and the Mac's change history.

Saved to ~/.ferb/device/<device_id>.json; each change appends one line to ~/.ferb/device/history.jsonl (when, why,
which apps were added, removed or changed). Plain files only: no git or any other developer tool, which a Mac without
the Command Line Tools doesn't have (its /usr/bin/git is Apple's install-tools stub). A device folder an older FERB
kept as a git repository is read as is: its profile JSON is the same file, and its .git folder is left untouched."""

import json
from dataclasses import asdict, dataclass, field
from pathlib import Path
from typing import Any


@dataclass(frozen=True)
class AppInfo:
    bundle_id: str
    name: str
    version: str
    path: str
    scriptable: bool
    ax_support: str = "untested"             # unknown until an explicit probe records evidence


@dataclass(frozen=True)
class BrowserInfo:
    name: str                                # "chrome", "brave", "safari"… (browsers.pair_key) or the app name
    bundle_id: str
    path: str
    drivers: dict[str, str] = field(default_factory=dict)   # driver -> status


@dataclass
class DeviceProfile:
    device_id: str
    os: str
    os_version: str
    scanned_at: str
    apps: list[AppInfo]
    browsers: list[BrowserInfo]

    def app(self, bundle_id: str) -> AppInfo | None:
        return next((a for a in self.apps if a.bundle_id == bundle_id), None)

    def to_json(self) -> str:
        return json.dumps(asdict(self), indent=2, sort_keys=True)

    @classmethod
    def from_json(cls, text: str) -> "DeviceProfile":
        d = json.loads(text)
        return cls(d["device_id"], d["os"], d["os_version"], d["scanned_at"],
                   [AppInfo(**a) for a in d["apps"]], [BrowserInfo(**b) for b in d["browsers"]])


@dataclass(frozen=True)
class ProfileDiff:
    added: list[str]
    removed: list[str]
    changed: list[str]

    @property
    def empty(self) -> bool:
        return not (self.added or self.removed or self.changed)


def diff(old: DeviceProfile, new: DeviceProfile) -> ProfileDiff:
    o = {a.bundle_id: a for a in old.apps}
    n = {a.bundle_id: a for a in new.apps}
    changed = sorted(b for b in o.keys() & n.keys()
                     if (o[b].version, o[b].scriptable, o[b].path) != (n[b].version, n[b].scriptable, n[b].path))
    return ProfileDiff(sorted(n.keys() - o.keys()), sorted(o.keys() - n.keys()), changed)


class ProfileStore:
    def __init__(self, home: Path) -> None:
        self.dir = home / "device"

    def path(self, device_id: str) -> Path:
        return self.dir / f"{device_id}.json"

    def load(self, device_id: str) -> DeviceProfile | None:
        p = self.path(device_id)
        return DeviceProfile.from_json(p.read_text()) if p.exists() else None

    @property
    def history_path(self) -> Path:
        return self.dir / "history.jsonl"

    def save(self, profile: DeviceProfile, message: str = "device scan") -> bool:
        """Write the profile and append its change to the history. Returns True if the profile changed."""
        self.dir.mkdir(parents=True, exist_ok=True)
        self.dir.chmod(0o700)
        old = self.load(profile.device_id)
        if old is not None and _same_except_time(old, profile):
            return False                     # only the scan time changed: no rewrite, no noise entry
        target = self.path(profile.device_id)
        staged = target.with_name(f".{target.name}.new")
        staged.write_text(profile.to_json() + "\n")
        staged.replace(target)               # a reader never sees half a profile
        d = diff(old, profile) if old is not None else ProfileDiff(sorted(a.bundle_id for a in profile.apps), [], [])
        entry = {"at": profile.scanned_at, "device_id": profile.device_id, "message": message,
                 "added": d.added, "removed": d.removed, "changed": d.changed}
        with self.history_path.open("a") as f:
            f.write(json.dumps(entry, sort_keys=True) + "\n")
        return True

    def history(self) -> list[str]:
        """The messages of the recorded changes, oldest first; a line that doesn't parse is skipped."""
        try:
            lines = self.history_path.read_text().splitlines()
        except FileNotFoundError:
            return []
        messages = []
        for line in lines:
            try:
                messages.append(str(json.loads(line)["message"]))
            except (ValueError, KeyError, TypeError):
                continue
        return messages


def _same_except_time(a: DeviceProfile, b: DeviceProfile) -> bool:
    return (a.device_id, a.os, a.os_version, a.apps, a.browsers) == (b.device_id, b.os, b.os_version, b.apps, b.browsers)


def summarize(profile: DeviceProfile) -> dict[str, Any]:
    return {"apps": len(profile.apps), "scriptable": sum(a.scriptable for a in profile.apps),
            "browsers": [b.name for b in profile.browsers]}
