"""Owner 2026-10-27: automatic pairing removed ("so remove this your current implementation"); setup and Settings
pair by the owner's click: Pair all ("opens all pairs instantly and closes all too. SO THERES NO MANUAL STEP LATER")
and a Pair per profile ("single per profile too"); later, a task pairs the profile it needs with browser_pair, after
the owner's Yes. Profiles are chosen by exact folder: a friend's Chrome had three profiles named "Aditya Kumar"."""

import threading
import time
from typing import Any

from ferb.policy import confirm
from tests.browserkit import OWNER, bd, call, task  # noqa: F401  (bd is a fixture)
from tests.fake_extension import FakeExtension
from tests.support.confirm import FerbAnswers


def _pages(launched: list[list[str]], since: int) -> dict[str, str]:
    """folder -> token of each pairing page handed to a profile since `since`."""
    out = {}
    for argv in launched[since:]:
        if argv and "pair.html#token=" in argv[-1]:
            folder = next(a.split("=", 1)[1] for a in argv if a.startswith("--profile-directory="))
            out[folder] = argv[-1].split("#token=")[1]
    return out


def _answer_pages(launched: list[list[str]], since: int, exts: dict[str, FakeExtension], want: int) -> None:
    deadline = time.monotonic() + 10
    while len(_pages(launched, since)) < want and time.monotonic() < deadline:
        time.sleep(0.02)
    for folder, token in _pages(launched, since).items():
        exts[folder].pair(token)


def _unpaired(d: Any, folders: tuple[str, ...]) -> dict[str, FakeExtension]:
    exts = {}
    for folder in folders:
        ext = FakeExtension(d.daemon.home)
        ext.connect()
        exts[folder] = ext
    return exts


def test_pair_all_pairs_every_profile_with_the_extension_at_once_but_ferbs_own(bd: Any) -> None:  # noqa: F811
    d, launched = bd
    exts = _unpaired(d, ("Default", "Profile 1"))
    before = len(launched)
    out: dict[str, Any] = {}
    run = threading.Thread(target=lambda: out.update(d.run(d.daemon._setup_browsers_pair_all(None, {"timeout_s": 10}))))
    run.start()
    _answer_pages(launched, before, exts, 2)
    run.join(15)
    try:
        assert out["paired"] == 2, out
        assert {(r["folder"], r["paired"]) for r in out["results"]} == {("Default", True), ("Profile 1", True)}
        assert "Profile 22" not in _pages(launched, before)            # FERB's own profile is paired as FERB
    finally:
        for e in exts.values():
            e.kill()


def test_pair_one_profile_by_its_exact_folder(bd: Any) -> None:  # noqa: F811
    d, launched = bd
    exts = _unpaired(d, ("Default",))
    before = len(launched)
    out: dict[str, Any] = {}
    run = threading.Thread(target=lambda: out.update(d.run(d.daemon._setup_browser_pair(
        None, {"browser": "chrome", "folder": "Default", "timeout_s": 10}))))
    run.start()
    _answer_pages(launched, before, exts, 1)
    run.join(15)
    try:
        assert out.get("paired") is True and out["folder"] == "Default", out
        assert list(_pages(launched, before)) == ["Default"]
    finally:
        exts["Default"].kill()


def test_a_task_pairs_a_profile_only_after_the_owners_yes(bd: Any) -> None:  # noqa: F811
    d, launched = bd
    g = task(d, "w", OWNER, "check my inbox")                           # the owner's words don't name the profile
    with FerbAnswers(d, yes=False) as ferb:
        ferb.expect(confirm.PAIR)
        r = call(d, g, "browser.pair", profile="chrome/Default")
        assert r["is_error"] and not _pages(launched, 0)                  # No: no pairing page opened
        assert "“Personal”" in ferb.asked[0]
        ferb.verify()


def test_a_profile_the_owner_named_pairs_without_a_question(bd: Any) -> None:  # noqa: F811
    """Owner 2026-10-27, after "pair my online bits profile" asked "pair your Chrome profile “online bits”…?": "why
    did it ask me the question?" A profile named in the owner's own words is already authorized (rule 8 provenance)."""
    d, launched = bd
    exts = _unpaired(d, ("Default",))
    d.daemon.pair_timeout_s = 10
    g = task(d, "w", OWNER, "pair my personal profile with ferb")
    before = len(launched)
    out: dict[str, Any] = {}
    run = threading.Thread(target=lambda: out.update(call(d, g, "browser.pair", profile="chrome/Default")))
    run.start()
    _answer_pages(launched, before, exts, 1)
    run.join(15)
    try:
        assert out["content"]["paired"] is True, out

        async def asked() -> int:
            return int(d.daemon.db.conn.execute(
                "SELECT count(*) FROM spans WHERE kind = 'tool.call' AND json_extract(input_json, '$.tool') = "
                "'owner.ask'").fetchone()[0])
        assert d.run(asked()) == 0
    finally:
        exts["Default"].kill()
