"""DMN-507 (owner, 2026-11-02): nothing reads or writes another app's data (browser folders and profiles) before FERB
has Full Disk Access. Past bug (the owner's first run, macOS 27.2): FERB.app's setup read Chrome's folder before Allow
all, macOS blocked it with a "Data Access Blocked" banner and Chrome never got FERB's native host, unnoticed. Now the
daemon, once it holds Full Disk Access, writes FERB's files into every installed browser at Pair browsers and at start,
and reports a folder that refuses them; the Mac scan gathers the browsers' profiles only then."""

import asyncio
import json
import os
from pathlib import Path
from typing import Any

from ferb.config import Config
from ferb.daemon.persona import PersonaReview
from ferb.device import persona
from ferb.device.install import NATIVE_HOST_MANIFEST
from ferb.device.profile import DeviceProfile, ProfileStore
from tests.conftest import RunningDaemon


def _browser(support: Path, folder: str) -> Path:
    data = support / folder
    (data / "Default").mkdir(parents=True)
    (data / "Local State").write_text(json.dumps({"profile": {"info_cache": {"Default": {"name": "Me"}}}}))
    return data


def _files(root: Path) -> list[Path]:
    return sorted(p.relative_to(root) for p in root.rglob("*"))


# decision: DMN-507, DMN-508
def test_the_daemon_connects_browsers_only_with_full_disk_access_and_reports_a_refused_folder(
        daemon: RunningDaemon, tmp_path: Path) -> None:
    d = daemon.daemon
    support = tmp_path / "Support"
    chrome = _browser(support, "Google/Chrome")
    vivaldi = _browser(support, "Vivaldi")
    d.browser_support, d.browser_data_dirs = support, {"chrome": chrome}
    granted = [False]
    d.data_access = lambda: granted[0]
    before = _files(support)

    waiting = daemon.run(d._setup_browsers(None, {}))
    assert waiting["waiting"] == "full_disk_access" and waiting["browsers"] == [] and waiting["problem"]
    assert d._profiles() == [] and d._browser_data() == {}           # nothing looked at, tests' folders included
    daemon.run(d._with_full_disk_access())
    assert _files(support) == before                                 # nothing written either

    granted[0] = True
    os.chmod(vivaldi, 0o500)                                         # a browser folder that refuses FERB's files
    try:
        rows = daemon.run(d._setup_browsers(None, {}))
    finally:
        os.chmod(vivaldi, 0o700)
    manifest = json.loads((chrome / "NativeMessagingHosts" / NATIVE_HOST_MANIFEST).read_text())
    assert manifest["path"] == str(d.home / "bin" / "ferb-native-host")
    assert "Vivaldi" in rows["problem"] and not (vivaldi / "NativeMessagingHosts").exists()
    written = daemon.run(_journal(d, "browser.files_written"))
    assert [f["browser"] for f in written[-1]["failed"]] == ["Vivaldi"]
    assert [p.folder for p in d._profiles()] == ["Default"]

    daemon.run(d._with_full_disk_access())                           # at start: the refused folder is tried again
    assert (vivaldi / "NativeMessagingHosts" / NATIVE_HOST_MANIFEST).is_file()
    assert daemon.run(_journal(d, "browser.files_written"))[-1]["failed"] == []


async def _journal(d: Any, kind: str) -> list[dict[str, Any]]:
    return [e.data for e in d.journal.of_kind(kind)]


# decision: DMN-507, DMN-508, MEM-13
def test_the_mac_scan_gathers_browser_profiles_only_once_full_disk_access_is_granted(
        tmp_path: Path, monkeypatch: Any) -> None:
    import ferb.device.scan as scan_module
    monkeypatch.setattr(scan_module, "scan", lambda device_id: DeviceProfile(device_id, "macos", "27.2", "now", [], []))
    monkeypatch.setattr(ProfileStore, "save", lambda self, profile, why: True)

    async def no_usage(apps: Any) -> list[Any]:
        return []
    monkeypatch.setattr(persona, "app_usage_async", no_usage)
    granted, read = [False], []

    def profiles() -> list[dict[str, Any]]:
        read.append(True)
        return [{"browser": "Brave", "name": "Personal", "email": "", "last_used": True}]

    def collect() -> Any:
        return persona.discover_persona(tmp_path / "home", "dev", [], frozenset(), profiles=profiles,
                                        allowed=lambda: granted[0])
    built: list[dict[str, Any]] = []

    async def build(facts: dict[str, Any]) -> dict[str, Any]:
        built.append(facts)
        return {"summary": "s", "shown": "s", "facts": []}
    review = PersonaReview(Config(tmp_path / "home"), collect, build=build, record=lambda kind, data: None)

    async def go() -> tuple[bool, int, bool, bool]:
        review.start(shown_by="setup")
        assert review.task is not None
        await review.task
        before, reads = review.needs_browser_part(), len(read)
        granted[0] = True
        changed = await review.refresh()
        return before, reads, changed, review.needs_browser_part()
    before, reads, changed, after = asyncio.run(go())
    assert built[0]["profiles"] == [] and built[0]["profiles_read"] is False and reads == 0
    assert (before, changed, after) == (True, True, False) and read == [True]
    assert built[-1]["profiles"][0]["name"] == "Personal"
