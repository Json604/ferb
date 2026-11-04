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
