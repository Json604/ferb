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


# decision: DMN-511, DMN-512
def test_a_worker_is_told_which_developer_tools_are_missing_until_they_are_installed(home: Path) -> None:
    """The worker never runs a stub and, when a task needs one, asks the owner once to install the tools (owner
    2026-11-02); once they are installed the fact is gone, so nothing asks again."""
    def worker_instructions(d: RunningDaemon) -> str:
        async def built() -> str:
            return str(d.daemon.supervisor._system("worker.md"))                 # type: ignore[union-attr]
        return str(d.run(built()))
    d = RunningDaemon(home, developer_tools=lambda: ["python3", "git"])
    installed = [False]
    d.daemon.dev_tools_installed = lambda: installed[0]                         # type: ignore[method-assign]
    try:
        text = worker_instructions(d)
        assert "python3, git" in text and "Command Line Tools" in text
        assert "owner_ask" in text and "xcode-select --install" in text         # asks once; Apple's installer
        installed[0] = True                                                     # the owner installed them
        assert "Command Line Tools" not in worker_instructions(d)
        installed[0] = False
        assert "Command Line Tools" not in worker_instructions(d)               # never comes back this run
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


# ---------- browsers other than Chrome and Brave ----------

STORE_ID = "plbfkifcmgkgnlfgikdeaaglgncdhkma"


# decision: HAND-247
def test_arc_vivaldi_opera_and_edge_add_ferb_from_the_web_store_not_load_unpacked(tmp_path: Path) -> None:
    """Audit item 8: Arc, Vivaldi and Opera were sent to Load unpacked (Developer mode), though each adds extensions
    from the Chrome Web Store. One table says how FERB reaches every browser it knows; Load unpacked is only for when
    no store id is set."""
    from ferb.act.browser import autoinstall, profiles
    from ferb.device.extensions import browser_rows
    assert set(autoinstall.INSTALL) == set(profiles.BROWSERS)                   # one row per known browser
    support = tmp_path / "Support"
    for name in ("arc", "vivaldi", "opera", "edge", "chrome"):
        data = profiles.data_dir(name, support)
        (data / "Default").mkdir(parents=True)
        (data / "Local State").write_text('{"profile": {"info_cache": {"Default": {"name": "Me"}}}}')
    states = autoinstall.prepare(support, STORE_ID, (STORE_ID,), None)
    requested = {s.browser for s in states if s.request is not None}
    assert requested == {"chrome", "edge"}                                     # no request where none is honoured
    rows = {r["browser"]: r for r in browser_rows(states, STORE_ID, tmp_path / "ext", None, set())["browsers"]}
    assert not any(r["manual"] for r in rows.values())                         # nobody is sent to Developer mode
    text = "\n".join(autoinstall.describe(states, STORE_ID, tmp_path / "ext"))
    assert "Load unpacked" not in text and text.count(f"chromewebstore.google.com/detail/{STORE_ID}") >= 4
    pending = browser_rows(states, "", tmp_path / "ext", None, set())["browsers"]
    assert all(r["manual"] for r in pending)                                   # no listing: by hand everywhere


# decision: HAND-247
def test_a_browser_is_found_by_its_bundle_id_wherever_it_is_installed(tmp_path: Path, monkeypatch: Any) -> None:
    """Audit item 8: the browser's executable was assumed to be in /Applications, so a Chrome in ~/Applications (common
    for a Standard user) never looked running and its profile launches failed. Launch Services finds it by bundle id."""
    from ferb.act.browser import profiles
    exe = tmp_path / "Applications" / "Google Chrome.app" / "Contents" / "MacOS" / "Google Chrome"
    exe.parent.mkdir(parents=True)
    exe.write_text("")
    asked: list[str] = []
    monkeypatch.setattr(profiles, "locate", lambda bundle_id: asked.append(bundle_id) or (
        exe if bundle_id == "com.google.Chrome" and exe.exists() else None))
    launched: list[list[str]] = []
    known = [profiles.BrowserProfile("chrome", "Default", "Me", "")]
    profiles.open_known_profile("chrome", "Default", None, known, launch=launched.append, window=True)
    assert launched == [[str(exe), "--profile-directory=Default"]]
    assert profiles.BROWSERS["chrome"].binary == str(exe) and asked == ["com.google.Chrome"]   # looked up once
    assert profiles.BROWSERS["vivaldi"].binary == "/Applications/Vivaldi.app/Contents/MacOS/Vivaldi"   # none known
    exe.unlink()                                                                # moved away: looked up again
    assert profiles.BROWSERS["chrome"].binary == "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
    assert asked.count("com.google.Chrome") == 2
