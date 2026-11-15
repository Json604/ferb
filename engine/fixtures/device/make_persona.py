"""Fixture for ferb/device/persona.py `discover_persona` (the scan behind `setup.persona` and the weekly Mac-profile
refresh) on a fake home: the invented app bundles of pure.json's scan (written again under
/private/tmp/ferb-device-fx/persona/Apps, the folder the Rust test uses too), the scan's clock and macOS version fixed,
`mdls` replaced by a stand-in that answers invented Spotlight records (nothing starts: a tripwire refuses every other
program), FERB's home a folder of its own. Nothing reads the owner's Library, Applications or browsers.

    cd <repo> && uv run python rust/fixtures/device/make_persona.py

Writes persona.json beside this file: each case (activity, excluded bundle ids, whether other apps' data is readable,
the browsers' profiles or a failure reading them), the mdls command it asked for, what it returned and the files it
left in FERB's home (`device/<id>.json`, `device/history.jsonl`). Invented data only; the same bytes on every run.
"""

import asyncio
import json
import os
import plistlib
import shutil
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[2]
sys.path.insert(0, str(REPO))
ROOT = Path("/private/tmp/ferb-device-fx/persona")
APPS = ROOT / "Apps"

CASES: list[dict[str, Any]] = [
    {"name": "readable", "activity": [{"bundle_id": "com.apple.Safari", "app": "Safari", "focus_events": 4,
                                       "last_used": "2026-11-12T08:00:00+00:00"},
                                      {"bundle_id": "com.example.web", "app": "Web", "focus_events": 2,
                                       "last_used": "2026-11-11T08:00:00+00:00"}],
     "excluded": [], "allowed": True,
     "profiles": [{"browser": "Google Chrome", "name": "Work", "email": "w@example.invalid", "last_used": True},
                  {"browser": "Google Chrome", "name": "Home", "email": "", "last_used": False}]},
    {"name": "no_full_disk_access", "activity": [], "excluded": [], "allowed": False,
     "profiles": [{"browser": "Google Chrome", "name": "Work", "email": "w@example.invalid", "last_used": True}]},
    {"name": "excluded_browser_and_app", "activity": [], "excluded": ["com.google.Chrome", "com.apple.Notes"],
     "allowed": True, "profiles": []},
    {"name": "profiles_unreadable", "activity": [], "excluded": [], "allowed": True, "profiles": "raise"},
]

LAUNCHED: list[str] = []


def _refuse(*a: Any, **k: Any) -> Any:
    LAUNCHED.append(repr(a))
    raise OSError("make_persona.py tripwire: refused to start a program")


def write_tree(root: Path, files: list[dict[str, Any]]) -> None:
    for e in files:
        p = root / e["path"]
        if e["kind"] == "dir":
            p.mkdir(parents=True, exist_ok=True)
            os.chmod(p, e["mode"])
        elif e["kind"] == "file":
            p.parent.mkdir(parents=True, exist_ok=True)
            p.write_bytes(bytes.fromhex(e["hex"]) if "hex" in e else e["text"].encode())
            os.chmod(p, e["mode"])
        elif e["kind"] == "link":
            os.symlink(e["target"], p)


def snap(root: Path) -> dict[str, str]:
    out = {}
    for p in sorted(root.rglob("*")):
        if p.is_file():
            out[str(p.relative_to(root))] = p.read_text()
    return out


class _Process:
    def __init__(self, out: bytes) -> None:
        self.out, self.returncode = out, 0

    async def communicate(self) -> tuple[bytes, bytes]:
        return self.out, b""


def main() -> None:
    import time
    os.environ["TZ"] = "UTC"
    time.tzset()
    import ferb.device.persona as persona
    import ferb.device.scan as scan_mod

    pure = json.loads((HERE / "pure.json").read_text())
    shutil.rmtree(ROOT, ignore_errors=True)
    APPS.mkdir(parents=True)
    write_tree(APPS, pure["scan"]["files"])

    # nothing starts but the mdls stand-in (bound as a default argument at import: replaced there too)
    subprocess.Popen.__init__ = _refuse                                  # type: ignore[method-assign]
    asked: list[list[str]] = []

    async def mdls(*argv: str, **k: Any) -> _Process:
        asked.append(list(argv))
        paths = argv[argv.index("kMDItemLastUsedDate") + 1:]
        records = []
        for n, path in enumerate(paths):
            if n % 2 == 0:
                records.append({"kMDItemPath": path, "kMDItemUseCount": 10 + n,
                                "kMDItemLastUsedDate": datetime(2026, 11, 1 + n, 8, 0, 0)})
        return _Process(plistlib.dumps(records))
    persona.app_usage_async.__defaults__ = (mdls,)
    asyncio.create_subprocess_exec = _refuse                             # type: ignore[assignment]

    original_scan = scan_mod.scan

    class FixedNow(datetime):
        @classmethod
        def now(cls, tz: Any = None) -> "FixedNow":
            return cls(2026, 11, 13, 9, 30, 5, tzinfo=timezone.utc)
    scan_mod.datetime = FixedNow                                         # type: ignore[misc]
    scan_mod.platform.mac_ver = lambda: ("15.4", ("", "", ""), "")       # type: ignore[assignment]
    scan_mod.scan = lambda device_id, roots=None: original_scan(device_id, [APPS])  # type: ignore[assignment]

    cases = []
    for case in CASES:
        home = ROOT / ("home-" + case["name"])
        home.mkdir()
        asked.clear()

        def profiles(c: dict[str, Any] = case) -> list[dict[str, Any]]:
            if c["profiles"] == "raise":
                raise OSError("Local State unreadable")
            return c["profiles"]
        facts = asyncio.run(persona.discover_persona(home, "dev-fx", case["activity"], frozenset(case["excluded"]),
                                                     profiles=profiles, allowed=lambda c=case: c["allowed"]))
        files = snap(home)
        cases.append({**case, "mdls": list(asked), "facts": facts, "files": files})
    assert not LAUNCHED, LAUNCHED
    (HERE / "persona.json").write_text(json.dumps({"made_by": "rust/fixtures/device/make_persona.py",
                                                   "apps": str(APPS), "cases": cases}, indent=1,
                                                  ensure_ascii=False) + "\n")


if __name__ == "__main__":
    main()
