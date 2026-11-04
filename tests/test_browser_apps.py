"""HAND-414..416: which browsers FERB offers, and what a removed extension reads as.

Past bugs (owner's run of build 202611040536, 2026-11-04): Pair all listed "Arc “Your Arc” Add to Arc" on a Mac
without Arc, because a browser counted as installed once its user-data folder existed, and an uninstalled browser
leaves that folder behind. Five profiles whose FERB extension the owner had removed sat at "Pairing…": a settings
entry Chromium itself doesn't read as an installed extension still read as "on"."""

import json
import plistlib
from pathlib import Path
from typing import Any

import pytest

from ferb.act.browser import autoinstall, profiles
from ferb.act.browser.autoinstall import STORE_EXTENSION_ID
from ferb.device.extensions import browser_rows, profile_state
from tests.conftest import RunningDaemon

ON: dict[str, Any] = {"location": 1, "path": f"{STORE_EXTENSION_ID}/0.19.0_0"}


def _browser(support: Path, browser: str, entries: dict[str, dict[str, Any] | None],
             extra: dict[str, Any] | None = None) -> Path:
    data = profiles.data_dir(browser, support)
    data.mkdir(parents=True)
    (data / "Local State").write_text(json.dumps({"profile": {"info_cache": {
        f: {"name": f"Name {f}"} for f in entries}}}))
    for f, entry in entries.items():
        (data / f).mkdir()
        ext: dict[str, Any] = {"settings": {STORE_EXTENSION_ID: entry} if entry is not None else {}, **(extra or {})}
        (data / f / "Secure Preferences").write_text(json.dumps({"extensions": ext}))
    return data


def _only(monkeypatch: pytest.MonkeyPatch, *installed: str) -> None:
    """Launch Services knows only these browsers' apps."""
    ids = {profiles.BROWSERS[b].bundle_id for b in installed}
    monkeypatch.setattr(profiles, "has_app", lambda bundle_id: bundle_id in ids)


# decision: HAND-414
def test_a_browser_whose_app_is_gone_is_listed_nowhere(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    support = tmp_path / "Support"
    _browser(support, "arc", {"Default": None})                     # Arc's folder, left behind; Arc isn't installed
    chrome = _browser(support, "chrome", {"Default": ON})
    _only(monkeypatch, "chrome")
    assert profiles.installed(support) == ["chrome"]
    arc_dirs = {"arc": profiles.data_dir("arc", support), "chrome": chrome}
    assert [p.browser for p in profiles.discover_all(arc_dirs)] == ["chrome"]
    assert profiles.with_paired([], [("arc", "Default")]) == []      # a pairing record of Arc names nothing
    assert [s.browser for s in autoinstall.read_states(support, STORE_EXTENSION_ID, (STORE_EXTENSION_ID,), None)] \
        == ["chrome"]
    assert [s.browser for s in autoinstall.read_states(support, STORE_EXTENSION_ID, (STORE_EXTENSION_ID,), None,
                                                       arc_dirs)] == ["chrome"]
    states = autoinstall.prepare(support, STORE_EXTENSION_ID, (STORE_EXTENSION_ID,), None)
    assert [s.browser for s in states] == ["chrome"]
    assert not (profiles.data_dir("arc", support) / "External Extensions").exists()      # no Web Store request
    assert all("Arc" not in str(d) for d in autoinstall.native_host_dirs(support, profiles.installed(support)))
    assert "Arc" not in "\n".join(autoinstall.describe(states, STORE_EXTENSION_ID, tmp_path / "ext"))


# decision: HAND-414
def test_setup_and_pair_all_offer_only_installed_browsers(daemon: RunningDaemon, tmp_path: Path,
                                                         monkeypatch: pytest.MonkeyPatch) -> None:
    support = tmp_path / "Support"
    arc = _browser(support, "arc", {"Default": None})
    chrome = _browser(support, "chrome", {"Default": None})
    d = daemon.daemon
    d.browser_support, d.browser_data_dirs = support, {"arc": arc, "chrome": chrome}
    d.data_access = lambda: True

    async def no_files() -> dict[str, Any]:
        return {"state": "connected"}
    d._connect_browsers = no_files                       # type: ignore[method-assign]
    _only(monkeypatch, "chrome")
    rows = daemon.run(d._setup_browsers(None, {}))
    assert [b["browser"] for b in rows["browsers"]] == ["chrome"]
    reply = daemon.run(d._setup_browsers_pair_all(None, {}))
    assert {s["browser"] for s in reply["skipped"]} == {"chrome"} and reply["profiles"] == 1
    assert [p.browser for p in d._profiles()] == ["chrome"]
    _only(monkeypatch, "chrome", "arc")                  # Arc installed again: it is back
    assert {b["browser"] for b in daemon.run(d._setup_browsers(None, {}))["browsers"]} == {"arc", "chrome"}


def _app(root: Path, name: str, bundle_id: str, chromium: bool) -> Path:
    app = root / f"{name}.app"
    (app / "Contents").mkdir(parents=True)
    info: dict[str, Any] = {"CFBundleIdentifier": bundle_id, "CFBundleName": name}
    if chromium:
        info["CFBundleDocumentTypes"] = [{"LSItemContentTypes": [profiles.CHROMIUM_EXTENSION_TYPE]}]
        info["UTExportedTypeDeclarations"] = [{"UTTypeIdentifier": profiles.CHROMIUM_EXTENSION_TYPE}]
    with (app / "Contents" / "Info.plist").open("wb") as f:
        plistlib.dump(info, f)
    return app


# decision: HAND-415
def test_an_unknown_chromium_browser_is_named_as_not_supported(daemon: RunningDaemon, tmp_path: Path,
                                                               monkeypatch: pytest.MonkeyPatch) -> None:
    apps = tmp_path / "Applications"
    found = [_app(apps, "Example Browser", "com.example.browser", chromium=True),
             _app(apps, "Google Chrome", "com.google.Chrome", chromium=True),         # known: supported
             _app(apps, "Example Chat", "com.example.chat", chromium=False),          # an Electron-style app
             _app(tmp_path / ".Trash", "Old Browser", "com.example.old", chromium=True)]
    monkeypatch.setattr(profiles, "handlers", lambda: found)
    assert profiles.unsupported() == [{"app": "Example Browser", "bundle_id": "com.example.browser"}]
    lines = autoinstall.describe([], STORE_EXTENSION_ID, tmp_path / "ext", unsupported=profiles.unsupported())
    assert any("Example Browser isn't supported by FERB yet" in line for line in lines)
    d = daemon.daemon
    d.browser_support, d.browser_data_dirs = tmp_path / "Support", {}
    d.data_access = lambda: True

    async def no_files() -> dict[str, Any]:
        return {"state": "connected"}
    d._connect_browsers = no_files                       # type: ignore[method-assign]
    reply = daemon.run(d._setup_browsers(None, {}))
    assert [u["app"] for u in reply["unsupported"]] == ["Example Browser"]
    assert reply["unsupported"][0]["message"] == "Example Browser isn't supported by FERB yet"
    monkeypatch.setattr(profiles, "handlers", lambda: (_ for _ in ()).throw(OSError("no list")))
    assert profiles.unsupported() == []                  # Launch Services unavailable: nothing, never an error


# decision: HAND-416
@pytest.mark.parametrize("entry,extra,state", [
    ({"ack_external": True}, {}, "not_installed"),                       # keys left under the id: not installed
    ({"location": 1}, {}, "not_installed"),                              # no path: Chromium skips it too
    ({**ON, "state": 2}, {}, "removed"),                                 # an older build's "removed by you"
    (None, {"external_uninstalls": [STORE_EXTENSION_ID]}, "removed"),    # today's record of a removal
    ({"state": 1, "location": 1}, {"external_uninstalls": [STORE_EXTENSION_ID]}, "removed"),
    (ON, {}, "on"),
])
def test_an_extension_the_owner_removed_never_reads_as_on(tmp_path: Path, entry: dict[str, Any] | None,
                                                         extra: dict[str, Any], state: str) -> None:
    data = _browser(tmp_path, "chrome", {"Default": entry}, extra)
    [p] = profiles.discover("chrome", data)
    assert profiles.extension_state(p, frozenset({STORE_EXTENSION_ID}), data) == ("on" if state == "on" else "missing")
    [status] = [autoinstall.profile_status(p, (STORE_EXTENSION_ID,), data, set(), STORE_EXTENSION_ID)]
    assert profile_state(status) == state
    rows = browser_rows([autoinstall.BrowserState("chrome", None, (status,))], STORE_EXTENSION_ID, tmp_path, None, set())
    assert rows["browsers"][0]["profiles"][0]["state"] == state
