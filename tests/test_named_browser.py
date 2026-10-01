"""The browser the owner names is the one FERB uses (owner's report 2026-10-01: "my scaler gmail in my chrome" was
opened in their Brave instead). If it isn't connected, FERB says so and how to connect it — never another browser —
and `ferb browsers pair` walks them through installing the extension where it's missing."""

import io
import json
from pathlib import Path
from typing import Any

from ferb.act.browser.ext_protocol import FERB_EXTENSION_ID
from ferb.act.browser.profiles import BrowserProfile, Named, has_extension, named_in
from ferb.cli.admin import browsers
from tests.conftest import RunningDaemon
from tests.test_ext_bridge import OWNER, bd, call, pair, task  # noqa: F401  (bd is a fixture)

KNOWN = [BrowserProfile("chrome", "Default", "Kartikey", "kartikey@gmail.com"),
         BrowserProfile("chrome", "Profile 2", "scaler", "kartikey.24bcs@sst.scaler.com"),
         BrowserProfile("brave", "Default", "Work", "")]


def test_the_profile_and_browser_the_owner_names() -> None:
    n = named_in("open my scaler gmail in chrome and find the FM I assignment", KNOWN)
    assert n is not None and n.browser == "chrome" and [p.folder for p in n.profiles] == ["Profile 2"]
    assert named_in("check the sst.scaler.com mail", KNOWN).profiles[0].folder == "Profile 2"   # by its account
    assert named_in("check gmail in my chrome", KNOWN) == Named("chrome", ())                   # just the browser
    assert named_in("finish the work report", KNOWN) is None                  # "work" alone isn't the Work profile
    assert named_in("use my work profile", KNOWN).profiles[0].browser == "brave"
    assert named_in("search for budgeting tips", KNOWN) is None
    n = named_in("open my scaler gmail in chrome", KNOWN)
    assert n.allows("chrome", "Profile 2") and not n.allows("brave", "Default")
    assert Named("chrome", ()).allows("chrome", "Default") and not Named("chrome", ()).allows("brave", "Default")


def test_whether_the_extension_is_installed_is_read_from_the_profile(tmp_path: Path) -> None:
    (tmp_path / "Profile 2").mkdir()
    p = KNOWN[1]
    assert not has_extension(p, frozenset({FERB_EXTENSION_ID}), tmp_path)
    (tmp_path / "Profile 2" / "Secure Preferences").write_text(json.dumps(
        {"extensions": {"settings": {FERB_EXTENSION_ID: {"location": 4}}}}))
    assert has_extension(p, frozenset({FERB_EXTENSION_ID}), tmp_path)


def test_a_task_never_swaps_in_another_browser(bd: Any) -> None:     # noqa: F811
    d, launched = bd
    pair(d, launched, "Work")
    personal = next(p for p in d.daemon._profiles() if p.display_name == "Personal")
    d.daemon.router.named_for = lambda task_id: Named("chrome", (personal,))    # type: ignore[union-attr]
    g = task(d, "w", OWNER, {"research"})
    r = call(d, g, "browser.open", url="https://mail.google.com/", profile="Work")
    assert r["is_error"] and "the owner asked for chrome “Personal”" in r["content"]["error"]


def test_the_planner_is_told_to_use_it_or_say_how_to_connect_it(bd: Any) -> None:     # noqa: F811
    d, launched = bd
    pair(d, launched, "Work")
    daemon = d.daemon

    async def note_for(words: str) -> str:                                      # on the daemon's own thread
        return str(daemon._named_note(daemon._named_browser(words)))
    note = d.run(note_for("open my Work profile in chrome"))
    assert "use only that" in note and "“Work”" in note
    data = daemon.browser_data_dirs["chrome"] / "Default" / "Secure Preferences"
    data.unlink()                                                               # no extension in Personal
    note = d.run(note_for("open my Personal profile in chrome"))
    assert "can't use it yet: FERB's extension isn't installed there" in note
    assert "Don't use another browser or profile" in note and "`ferb browsers pair Personal`" in note


class FakeClient:
    def __init__(self) -> None:
        self.installed_after, self.calls = 2, []

    def call(self, method: str, params: dict[str, Any] | None = None) -> dict[str, Any]:
        self.calls.append(method)
        if method == "browsers.pair" and self.calls.count("browsers.pair") == 1:
            return {"paired": False, "needs_install": True, "browser": "chrome", "folder": "Profile 2",
                    "name": "scaler", "extension_dir": "/repo/extensions/chromium"}
        if method == "browsers.installed":
            self.installed_after -= 1
            return {"installed": self.installed_after <= 0}
        return {"paired": True, "browser": "chrome", "folder": "Profile 2", "name": "scaler"}


def test_pairing_walks_the_owner_through_installing_the_extension_then_pairs() -> None:
    c, out = FakeClient(), io.StringIO()
    assert browsers(c, "pair", "scaler", None, out, sleep=lambda s: None) == 0   # type: ignore[arg-type]
    text = out.getvalue()
    assert "isn't installed in Chrome “scaler”" in text and "Load unpacked" in text
    assert "/repo/extensions/chromium" in text and "Paired chrome profile “scaler”" in text
    assert c.calls == ["browsers.pair", "browsers.installed", "browsers.installed", "browsers.pair"]


def test_a_folder_named_in_the_owners_own_way_counts() -> None:
    """Owner's report (2026-10-01): "clg/uni folder in desktop" was asked about three times because the plan wrote
    Desktop/clg/uni. A path counts when its folder names are all in the owner's words (home folders may be implied)."""
    from ferb.sanitize.untrusted import targets_in_own_words
    said = "do that assignment in clg/uni fodler in desktop"
    assert targets_in_own_words(["Desktop/clg/uni", "~/Desktop/clg/uni", "clg/uni"], said) == []
    assert targets_in_own_words(["Documents/clg/uni"], "the uni folder inside clg") == []
    assert targets_in_own_words(["Desktop/clg/secret"], said) == ["Desktop/clg/secret"]     # never named: refused
    assert targets_in_own_words(["~/.ssh/id_rsa"], said) == ["~/.ssh/id_rsa"]
    assert targets_in_own_words(["Desktop/../clg"], "the clg folder") == []                  # ".." isn't a name
    import os
    home = os.path.expanduser("~")                                         # their home folder is never "a name"
    said2 = "make it complete the assignment in clg:uni folder in desktop"
    assert targets_in_own_words([f"{home}/Desktop/clg/uni", f"{home}/Desktop/clg:uni", "/Users/someone/Desktop/clg/uni"],
                                said2) == []


def test_naming_a_profile_counts_as_asking_for_their_own_browser(bd: Any) -> None:     # noqa: F811
    d, launched = bd
    pair(d, launched, "Work")

    async def owner_browser(words: str) -> bool:
        return bool(d.daemon._owner_browser(words))
    assert d.run(owner_browser("open my Personal gmail account in chrome and search it"))
    assert not d.run(owner_browser("search the web for budgeting tips"))


def test_finder_names_and_home_folders(tmp_path: Path, monkeypatch: Any) -> None:
    """macOS stores a Finder name "clg/uni" as clg:uni on disk; "Desktop/…" is the owner's Desktop, not FERB's
    workspace (owner's report 2026-10-01: FERB made a new empty clg/uni instead)."""
    from ferb.act.fs.files import normalize_path, owner_path
    monkeypatch.setattr(Path, "home", lambda: tmp_path)
    monkeypatch.setenv("HOME", str(tmp_path))                               # ~ too
    (tmp_path / "Desktop" / "clg:uni").mkdir(parents=True)
    (tmp_path / "Desktop" / "clg").mkdir()                                  # a plain clg folder doesn't hide it
    assert owner_path("Desktop/clg/uni", "/ws") == tmp_path / "Desktop" / "clg:uni"
    assert normalize_path("~/Desktop/clg/uni", "/ws") == str(tmp_path / "Desktop" / "clg:uni")
    assert owner_path("notes.md", "/ws") == Path("/ws/notes.md")            # other relative paths: the workspace
    assert owner_path("Desktop/new/x", "/ws") == tmp_path / "Desktop" / "new" / "x"   # nothing there: as written
