"""HAND-411: Pair all names every owner profile it didn't pair, with why and the owner's step. Past bug (owner's
question 2026-11-04): Pair all paired only profiles whose extension was on and passed over the rest in silence, so the
tour said "Paired." when no browser had FERB's extension yet (the Web Store request installs only at the browser's
next start), when some hadn't, or when it was turned off, removed or too old."""

import json
from pathlib import Path
from typing import Any

from ferb.act.browser.autoinstall import STORE_EXTENSION_ID
from tests.conftest import RunningDaemon

ON: dict[str, Any] = {"state": 1}
OFF: dict[str, Any] = {"state": 1, "disable_reasons": [1]}


def _browser(support: Path, folder: str, profiles: dict[str, dict[str, Any] | None], *,
             removed: tuple[str, ...] = (), requested: bool = False) -> Path:
    """A browser's user-data folder: each profile with FERB's extension on, off, or absent (None)."""
    data = support / folder
    data.mkdir(parents=True)
    (data / "Local State").write_text(json.dumps({"profile": {"info_cache": {
        f: {"name": f"Name {f}"} for f in profiles}}}))
    for f, entry in profiles.items():
        (data / f).mkdir()
        ext: dict[str, Any] = {"settings": {STORE_EXTENSION_ID: entry} if entry is not None else {}}
        if f in removed:
            ext["external_uninstalls"] = [STORE_EXTENSION_ID]
        (data / f / "Preferences").write_text(json.dumps({"extensions": ext}))
    if requested:
        (data / "External Extensions").mkdir()
        (data / "External Extensions" / f"{STORE_EXTENSION_ID}.json").write_text("{}")
    return data


def _pair_all(daemon: RunningDaemon, dirs: dict[str, Path], support: Path) -> tuple[dict[str, Any], list[str]]:
    d = daemon.daemon
    d.browser_support, d.browser_data_dirs = support, dirs
    d.data_access = lambda: True
    d._connect_browsers = _no_files                     # type: ignore[method-assign]
    tried: list[str] = []

    async def pair_one(p: Any, timeout_s: float) -> dict[str, Any]:
        tried.append(f"{p.browser}/{p.folder}")
        return {"browser": p.browser, "folder": p.folder, "name": p.display_name, "paired": True}
    d._pair_one = pair_one                               # type: ignore[method-assign]
    return daemon.run(d._setup_browsers_pair_all(None, {})), tried


async def _no_files() -> dict[str, Any]:
    return {"state": "connected"}


def _reasons(reply: dict[str, Any]) -> dict[str, str]:
    return {f"{s['browser']}/{s['folder']}": s["reason"] for s in reply["skipped"]}


# decision: HAND-411
def test_pair_all_with_every_profile_on_pairs_them_all_and_skips_none(daemon: RunningDaemon, tmp_path: Path) -> None:
    support = tmp_path / "Support"
    chrome = _browser(support, "Google/Chrome", {"Default": ON, "Profile 1": ON})
    reply, tried = _pair_all(daemon, {"chrome": chrome}, support)
    assert sorted(tried) == ["chrome/Default", "chrome/Profile 1"]
    assert reply["profiles"] == 2 and reply["skipped"] == [] and reply["paired"] == 2


# decision: HAND-411
def test_pair_all_with_no_extension_yet_pairs_nothing_and_names_each_profile(
        daemon: RunningDaemon, tmp_path: Path) -> None:
    support = tmp_path / "Support"
    chrome = _browser(support, "Google/Chrome", {"Default": None, "Profile 1": None}, requested=True)
    vivaldi = _browser(support, "Vivaldi", {"Default": None})            # a browser FERB sends no request to
    reply, tried = _pair_all(daemon, {"chrome": chrome, "vivaldi": vivaldi}, support)
    assert tried == [] and reply["profiles"] == 3 and reply["paired"] == 0
    assert _reasons(reply) == {"chrome/Default": "requested", "chrome/Profile 1": "requested",
                               "vivaldi/Default": "not_added"}
    assert reply["store_url"] and reply["store_url"] in reply["skipped"][-1]["step"]
    assert all(s["step"] and s["name"] and s["label"] for s in reply["skipped"])


# decision: HAND-411
def test_pair_all_pairs_only_the_profiles_that_are_on_and_names_the_rest(
        daemon: RunningDaemon, tmp_path: Path) -> None:
    """Mixed: on, off, removed by the owner, not added yet, paired but too old, and FERB's own profile (left out)."""
    support = tmp_path / "Support"
    chrome = _browser(support, "Google/Chrome", {"Default": ON, "Profile 1": OFF, "Profile 2": None,
                                                 "Profile 3": None, "Profile 4": ON, "Profile 5": None},
                      removed=("Profile 3",), requested=True)
    d = daemon.daemon
    d._identity_profile = lambda: ("chrome", "Profile 5")                 # type: ignore[method-assign]

    async def paired_too_old() -> None:
        d.db.conn.execute("INSERT INTO ext_installs (install_id, browser, folder, identity, key, paired_at, "
                          "ext_version) VALUES ('i4', 'chrome', 'Profile 4', 'owner', x'00', 'now', '0.1.0')")
    daemon.run(paired_too_old())
    reply, tried = _pair_all(daemon, {"chrome": chrome}, support)
    assert tried == ["chrome/Default"] and reply["profiles"] == 5 and reply["paired"] == 1
    assert _reasons(reply) == {"chrome/Profile 1": "off", "chrome/Profile 2": "requested",
                               "chrome/Profile 3": "removed", "chrome/Profile 4": "problem"}
    assert "too old" in next(s["step"] for s in reply["skipped"] if s["reason"] == "problem")
    [row] = [e.data for e in daemon.run(_skips(d))]
    assert {s["folder"]: s["reason"] for s in row["skipped"]} == {
        "Profile 1": "off", "Profile 2": "requested", "Profile 3": "removed", "Profile 4": "problem"}


async def _skips(d: Any) -> list[Any]:
    return list(d.journal.of_kind("browsers.pair_all_skipped"))


def _status(daemon: RunningDaemon, support: Path, dirs: dict[str, Path]) -> Any:
    d = daemon.daemon
    d.browser_support, d.browser_data_dirs = support, dirs
    d.data_access = lambda: True
    d._connect_browsers = _no_files                     # type: ignore[method-assign]
    return d


# decision: HAND-412
def test_pair_all_opens_no_page_in_a_profile_whose_installed_copy_is_too_old(
        daemon: RunningDaemon, tmp_path: Path) -> None:
    """Past bug (owner's tour, 2026-11-04): profiles on 0.15.x got a pairing page each and held Pair all for the whole
    timeout, then said only "didn't answer". The version their files name is read first: too old is skipped."""
    support = tmp_path / "Support"
    chrome = _browser(support, "Google/Chrome", {
        "Default": {"state": 1, "path": f"{STORE_EXTENSION_ID}/0.15.13_0"},
        "Profile 1": {"state": 1, "manifest": {"version": "0.19.0"}},
        "Profile 2": {"state": 1, "disable_reasons": [4], "path": f"{STORE_EXTENSION_ID}/0.19.0_0"}})
    reply, tried = _pair_all(daemon, {"chrome": chrome}, support)
    assert tried == ["chrome/Profile 1"]
    assert _reasons(reply) == {"chrome/Default": "too_old", "chrome/Profile 2": "off"}
    assert "0.15.13" in next(s["step"] for s in reply["skipped"] if s["reason"] == "too_old")


# decision: HAND-412, HAND-413
def test_pair_all_streams_its_plan_each_result_and_the_reply(daemon: RunningDaemon, tmp_path: Path) -> None:
    support = tmp_path / "Support"
    chrome = _browser(support, "Google/Chrome", {"Default": ON, "Profile 1": ON, "Profile 2": None,
                                                 "Profile 3": OFF}, removed=("Profile 2",))
    d = _status(daemon, support, {"chrome": chrome})

    async def pair_one(p: Any, timeout_s: float) -> dict[str, Any]:
        return {"browser": p.browser, "folder": p.folder, "name": p.display_name, "paired": p.folder == "Default"}
    d._pair_one = pair_one                               # type: ignore[method-assign]

    async def events() -> list[dict[str, Any]]:
        return [e async for e in d._setup_browsers_pairing(None, {})]
    out = daemon.run(events())
    assert [e["type"] for e in out] == ["plan", "result", "result", "done"]
    assert {t["folder"] for t in out[0]["todo"]} == {"Default", "Profile 1"}
    adds = {s["folder"]: s["can_add"] for s in out[0]["skipped"]}
    assert adds == {"Profile 2": True, "Profile 3": False}        # the store page adds a removed copy, not an off one
    assert out[-1]["paired"] == 1 and len(out[-1]["results"]) == 2


# decision: HAND-412
def test_a_pairing_ends_at_once_when_the_extension_is_refused_or_turned_off(
        daemon: RunningDaemon, tmp_path: Path) -> None:
    """The wait no longer runs to its timeout when FERB already knows the answer: the page's key was used but the hello
    was refused (too old), or the profile's files now say the copy is off. A wait that ran on would hang here (the
    test clock never moves)."""
    support = tmp_path / "Support"
    chrome = _browser(support, "Google/Chrome", {"Default": ON, "Profile 1": ON})
    d = _status(daemon, support, {"chrome": chrome})
    d.pair_load_wait_s = 0.0
    [default] = [p for p in d._profiles() if p.folder == "Default"]
    [other] = [p for p in d._profiles() if p.folder == "Profile 1"]

    async def answered_then_refused(browser: str, folder: str, token: str) -> None:
        d.db.conn.execute("INSERT INTO ext_installs (install_id, browser, folder, identity, key, last_seq, paired_at) "
                          "VALUES ('install-old', ?, ?, 'owner', x'00', 0, 'now')", (browser, folder))
        d.pairings.paired.add(token)
        d.hub.note_refusal("extension_too_old", "the FERB extension in this profile is too old", "install-old")
    d._open_pairing = answered_then_refused              # type: ignore[method-assign]
    first = daemon.run(d._pair_one(default, 60.0))
    assert first["paired"] is False and first["reason"] == "problem"

    async def turned_off_meanwhile(browser: str, folder: str, token: str) -> None:
        (chrome / folder / "Preferences").write_text(json.dumps(
            {"extensions": {"settings": {STORE_EXTENSION_ID: OFF}}}))
    d._open_pairing = turned_off_meanwhile               # type: ignore[method-assign]
    second = daemon.run(d._pair_one(other, 60.0))
    assert second["paired"] is False and second["reason"] == "off"
    assert not d.pairings._tokens                         # neither page's key can pair later
