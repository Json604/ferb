"""Represent and persist device inventory and capability observations. Must not treat a historical probe as authorization.

DeviceProfile: load / save / diff, method resolution, and the local git history.

Saved to ~/.ferb/device/<device_id>.json and committed to a local git repo in ~/.ferb/device/ (history of the
Mac's changes). Never pushed: no remote is ever added."""

import json
import subprocess
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

    def save(self, profile: DeviceProfile, message: str = "device scan") -> bool:
        """Write and commit to the local repo. Returns True if the profile changed (a commit was made)."""
        self.dir.mkdir(parents=True, exist_ok=True)
        self.dir.chmod(0o700)
        if not (self.dir / ".git").exists():
            self._git("init", "--quiet")
        old = self.load(profile.device_id)
        if old is not None and _same_except_time(old, profile):
            return False                     # only the scan time changed: no rewrite, no noise commit
        self.path(profile.device_id).write_text(profile.to_json() + "\n")
        self._git("add", "--", self.path(profile.device_id).name)
        if self._git("diff", "--cached", "--quiet", check=False).returncode == 0:
            return False
        self._git("commit", "--quiet", "-m", message)
        return True

    def history(self) -> list[str]:
        r = self._git("log", "--format=%s", check=False)
        return r.stdout.splitlines() if r.returncode == 0 else []

    def remotes(self) -> list[str]:
        return self._git("remote", check=False).stdout.split()

    def _git(self, *args: str, check: bool = True) -> "subprocess.CompletedProcess[str]":
        # Local identity only for this repo's commits; the owner's global git config is never touched.
        return subprocess.run(["git", "-C", str(self.dir), "-c", "user.name=FERB", "-c", "user.email=ferb@localhost",
                               "-c", "commit.gpgsign=false", *args], capture_output=True, text=True, check=check)


def _same_except_time(a: DeviceProfile, b: DeviceProfile) -> bool:
    return (a.device_id, a.os, a.os_version, a.apps, a.browsers) == (b.device_id, b.os, b.os_version, b.apps, b.browsers)


def summarize(profile: DeviceProfile) -> dict[str, Any]:
    return {"apps": len(profile.apps), "scriptable": sum(a.scriptable for a in profile.apps),
            "browsers": [b.name for b in profile.browsers]}
