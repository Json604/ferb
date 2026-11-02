"""FERB on a Mac that isn't a developer's (audit 2026-11-02): no developer tools, the first start, free space measured
like Finder, other Chromium browsers and quarantine on FERB's own bundle."""

import subprocess
from pathlib import Path
from typing import Any, NamedTuple

from ferb.device import devtools
from tests.conftest import RunningDaemon


def _xcode_select(folder: Path | None) -> Any:
    """`xcode-select -p` as a Mac with (folder) or without (None) a developer folder answers it."""
    def run(argv: list[str], **kw: Any) -> subprocess.CompletedProcess[str]:
        assert argv == ["/usr/bin/xcode-select", "-p"]
        if folder is None:
            return subprocess.CompletedProcess(argv, 2, "", "xcode-select: error: unable to get active developer "
                                                             "directory")
        return subprocess.CompletedProcess(argv, 0, f"{folder}\n", "")
    return run


def _tool(folder: Path, name: str) -> None:
    folder.mkdir(parents=True, exist_ok=True)
    (folder / name).write_text("#!/bin/sh\nexit 1\n")
    (folder / name).chmod(0o755)


# decision: DMN-511
def test_without_developer_tools_the_stubs_are_named_and_a_homebrew_tool_counts_as_installed(tmp_path: Path,
                                                                                            monkeypatch: Any) -> None:
    """Audit item 6: on a Mac without the Command Line Tools a worker's `python3 -c` reached Apple's stub, which opened
    the install-tools dialog in front of the owner. FERB reads `xcode-select -p` (no dialog) and names the stubs."""
    usr_bin, brew = tmp_path / "usr" / "bin", tmp_path / "brew" / "bin"
    for name in devtools.STUB_TOOLS:
        _tool(usr_bin, name)
    _tool(brew, "git")
    monkeypatch.setattr(devtools, "STUB_FOLDER", usr_bin)
    path = f"{brew}:{usr_bin}"
    missing = devtools.missing_tools(path, run=_xcode_select(None))
    assert missing == [n for n in devtools.STUB_TOOLS if n != "git"]           # Homebrew's git is a real one
    assert devtools.missing_tools(path, run=_xcode_select(tmp_path)) == []      # a developer folder: all real
    gone = tmp_path / "removed"                                                 # selected but deleted: no tools
    assert devtools.missing_tools(path, run=_xcode_select(gone)) == missing


# decision: DMN-511
def test_a_worker_is_told_which_developer_tools_are_missing(home: Path) -> None:
    def worker_instructions(d: RunningDaemon) -> str:
        async def built() -> str:
            return str(d.daemon.supervisor._system("worker.md"))                 # type: ignore[union-attr]
        return str(d.run(built()))
    d = RunningDaemon(home, developer_tools=lambda: ["python3", "git"])
    try:
        text = worker_instructions(d)
        assert "python3, git" in text and "Command Line Tools" in text
    finally:
        d.stop()
    d = RunningDaemon(home)                                                     # tools installed: nothing said
    try:
        assert "Command Line Tools" not in worker_instructions(d)
    finally:
        d.stop()


# ---------- free space as Finder counts it ----------

_GB = 1000 ** 3


class _Usage(NamedTuple):
    total: int
    used: int
    free: int


# decision: HAND-246
def test_the_free_disk_floor_counts_purgeable_space_as_finder_does(tmp_path: Path, monkeypatch: Any) -> None:
    """Audit item 7: statvfs free space leaves out purgeable space (local snapshots, optimised iCloud files), so a Mac
    whose Finder showed 40 GB available sat under the 10 GB floor and every download was refused. The floor reads the
    volume's capacity for important usage; the plain measure stays the fallback."""
    from ferb.act.browser import downloads
    monkeypatch.setattr(downloads.shutil, "disk_usage", lambda where: _Usage(500 * _GB, 494 * _GB, 6 * _GB))
    monkeypatch.setattr(downloads, "_important_usage", lambda where: 40 * _GB)
    d = downloads.Downloads(tmp_path / "workspace" / "downloads", None, lambda: None,  # type: ignore[arg-type,return-value]
                            lambda *a: a[-1], lambda text: text, lambda kind, data: None)
    assert d.free_bytes() == 40 * _GB
    assert d.limits().min_free_bytes == 10 * _GB                               # the owner's 10 GB is unchanged
    assert downloads.available_bytes(tmp_path, important=lambda where: None) == 6 * _GB

    def unsupported(where: Path) -> int | None:
        raise AttributeError("no such key")
    assert downloads.available_bytes(tmp_path, important=unsupported) == 6 * _GB
