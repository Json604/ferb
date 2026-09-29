"""Build step 10, real Mac (pytest -m real): a test daemon with the real watchers.
- app focus: a switch is recorded (and one FERB made is FERB's);
- window titles: a Finder window of a throwaway folder (closed through Accessibility afterwards);
- files: events in a temporary folder under the home, listed in [sense] watch_folders for this test only;
- `ferb pause`: afterwards a real app switch and a file change record nothing; `ferb resume` restores.
Tab events from the spare profile are covered by test_mac_vision_real / test_ext_real's daemon (the Hub path).
"""

import json
import secrets
import shutil
import tempfile
import time
from pathlib import Path
from typing import Any, Iterator

import pytest

from ferb.act.mac import apps as appsm
from ferb.cli.client import Client
from ferb.clock.clock import SystemClock
from tests.conftest import RunningDaemon

pytestmark = pytest.mark.real


@pytest.fixture(scope="module")
def live() -> Iterator[tuple[RunningDaemon, Path]]:
    base = Path(tempfile.mkdtemp(prefix="ferbs-"))
    watched = Path.home() / f"ferb-sense-test-{secrets.token_hex(3)}"
    watched.mkdir()
    home = base / "home"
    home.mkdir(mode=0o700)
    from ferb.config import Config
    Config(home).set("sense", "watch_folders", [str(watched)])
    d = RunningDaemon(home, clock=SystemClock(), watchers=True)
    time.sleep(1.0)
    try:
        yield d, watched
    finally:
        d.stop()
        shutil.rmtree(watched, ignore_errors=True)
        shutil.rmtree(base, ignore_errors=True)


def rows(d: RunningDaemon, source: str) -> list[dict[str, Any]]:
    async def go() -> list[dict[str, Any]]:
        return [dict(r) | {"data": json.loads(r["data_json"])} for r in d.daemon.db.conn.execute(  # type: ignore[union-attr]
            "SELECT * FROM trail WHERE source = ? ORDER BY seq", (source,)).fetchall()]
    return d.run(go())  # type: ignore[no-any-return]


def wait_for(fn: Any, timeout: float = 6.0) -> Any:
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        v = fn()
        if v:
            return v
        time.sleep(0.2)
    return fn()


def switch_away_and_back(bundle: str) -> None:
    front = appsm.frontmost()
    appsm.activate(bundle)
    time.sleep(1.2)
    if front is not None and front.bundle_id != bundle:
        appsm.activate(front.bundle_id)
        time.sleep(1.0)


def test_app_focus_and_ferbs_own_switch(live: tuple[RunningDaemon, Path]) -> None:
    d, _ = live
    switch_away_and_back("com.apple.finder")
    got = wait_for(lambda: [r for r in rows(d, "app_focus") if r["data"]["bundle_id"] == "com.apple.finder"])
    assert got and got[-1]["actor"] == "owner" and set(got[-1]["data"]) == {"app", "bundle_id"}
    d.daemon.attribution.ferb_activated("com.apple.finder", "t-real")         # as the Mac driver reports it
    switch_away_and_back("com.apple.finder")
    got = wait_for(lambda: [r for r in rows(d, "app_focus") if r["data"]["bundle_id"] == "com.apple.finder"
                            and r["actor"] == "ferb"])
    assert got and got[-1]["task_id"] == "t-real"


def test_a_window_title_is_recorded(live: tuple[RunningDaemon, Path]) -> None:
    import subprocess
    d, watched = live
    folder = watched / f"ferbtitle-{secrets.token_hex(3)}"
    folder.mkdir()
    front = appsm.frontmost()
    subprocess.run(["open", str(folder)], check=True)
    try:
        got = wait_for(lambda: [r for r in rows(d, "window") if r["data"]["title"] == folder.name])
        assert got and got[-1]["data"]["bundle_id"] == "com.apple.finder"
    finally:
        _close_finder_window(folder.name)
        if front is not None:
            appsm.activate(front.bundle_id)


def _close_finder_window(title: str) -> None:
    from ferb.act.mac.accessibility import _attr, press
    f = appsm.find("com.apple.finder")
    if f is None:
        return
    import ApplicationServices as AX
    for win in _attr(AX.AXUIElementCreateApplication(f.pid), "AXWindows") or []:
        if _attr(win, "AXTitle") == title:
            btn = _attr(win, "AXCloseButton")
            if btn is not None:
                press(btn)


def test_file_events_in_the_watched_folder_only(live: tuple[RunningDaemon, Path]) -> None:
    d, watched = live
    f = watched / "note.txt"
    f.write_text("contents never recorded")
    got = wait_for(lambda: [r for r in rows(d, "file") if r["data"]["path"].endswith("/note.txt")])
    assert got and got[0]["actor"] == "owner" and "contents never recorded" not in json.dumps(got)
    time.sleep(1.5)                                                            # past the 1 s coalescing window
    d.daemon.attribution.ferb_touched(str(f), "t-file")                       # as the gateway reports a fs tool
    f.write_text("changed by FERB")
    got = wait_for(lambda: [r for r in rows(d, "file") if r["actor"] == "ferb"])
    assert got and got[-1]["task_id"] == "t-file" and got[-1]["data"]["change"] == "modified"
    outside = Path(tempfile.mkdtemp(prefix="ferb-unwatched-"))
    (outside / "x.txt").write_text("x")
    time.sleep(1.5)
    assert not [r for r in rows(d, "file") if "ferb-unwatched-" in r["data"]["path"]]
    shutil.rmtree(outside, ignore_errors=True)


def test_pause_records_nothing_and_resume_restores(live: tuple[RunningDaemon, Path]) -> None:
    d, watched = live
    c = Client(d.sock)
    c.call("sense.pause")
    try:
        assert c.call("sense.status")["watchers"] == [] and not d.daemon.owner_input.running
        before = {s: len(rows(d, s)) for s in ("app_focus", "file", "window", "idle")}
        switch_away_and_back("com.apple.finder")
        (watched / "paused.txt").write_text("x")
        time.sleep(2.0)
        assert {s: len(rows(d, s)) for s in before} == before
    finally:
        r = c.call("sense.resume")
        c.close()
    assert {"app_focus", "file", "window", "idle"} <= set(r["watchers"])
    (watched / "resumed.txt").write_text("x")
    assert wait_for(lambda: [x for x in rows(d, "file") if x["data"]["path"].endswith("/resumed.txt")])
