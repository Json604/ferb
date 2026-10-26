"""L25 automatic pairing (owner 2026-10-24: "yes build", after OpenClaw's extension "pairs on its first native call"):
a FERB copy that says hello unpaired is paired by FERB itself, with no command. FERB opens its self-closing pairing page
only in the profile the owner is in: Chrome or Brave is the frontmost app and the profile is its last used, read twice
PREFS_SETTLE_S apart (Chromium writes it that late) and the front app read again just before. Never FERB's own profile
(by setting or by its account), a profile whose FERB is off, one already paired, one the owner unpaired, or one being
paired by hand; one try per profile while FERB runs."""

import json
import time
from typing import Any

from ferb.act.browser import profiles
from ferb.act.browser.ext_protocol import FERB_EXTENSION_ID
from ferb.cli.client import Client
from ferb.core.types import Identity
from ferb.daemon.main import AUTOPAIR_BROWSERS, PREFS_SETTLE_S
from ferb.sense.watchers.base import Focused
from tests.browserkit import bd, pair  # noqa: F401  (bd is a fixture)
from tests.fake_extension import FakeExtension

CHROME = "com.google.Chrome"


def setup(d: Any, front: str | None, last_used: str = "Profile 1") -> None:
    d.daemon.run_autopair = True                             # off in every other test (conftest)
    d.daemon.front_app = lambda: front
    use(d, last_used)


def use(d: Any, last_used: str) -> None:
    local_state = d.daemon.browser_data_dirs["chrome"] / "Local State"
    doc = json.loads(local_state.read_text())
    doc["profile"]["last_used"] = last_used
    local_state.write_text(json.dumps(doc))


def pair_pages(launched: list[list[str]]) -> list[list[str]]:
    return [a for a in launched if a and "pair.html#token=" in a[-1]]


def running(d: Any) -> bool:
    t = d.daemon._autopair_task
    return t is not None and not t.done()


def step(d: Any) -> None:
    """Chrome's write delay passes once (the fake clock), and the pass gets to run on it."""
    d.call_soon(d.daemon.clock.advance, PREFS_SETTLE_S + 0.5)
    time.sleep(0.05)


def until_page(d: Any, launched: list[list[str]], n: int = 1) -> list[str]:
    for _ in range(20):
        if len(pair_pages(launched)) >= n:
            return pair_pages(launched)[-1]
        step(d)
    raise AssertionError("no pairing page opened")


def settle(d: Any) -> None:
    """Until the pass a hello or an app switch started has finished (the fake clock moves past every wait)."""
    for _ in range(40):
        if not running(d):
            return
        step(d)
    raise AssertionError("the pairing pass never finished")


def kinds(d: Any) -> list[str]:
    async def read() -> list[str]:
        return [r[0] for r in d.daemon.db.conn.execute("SELECT kind FROM journal ORDER BY seq")]
    return list(d.run(read()))


def binding(d: Any, folder: str) -> Any:
    async def read() -> Any:
        return d.daemon.pairings.for_profile("chrome", folder)
    return d.run(read())


def unpaired_hello(d: Any) -> FakeExtension:
    ext = FakeExtension(d.daemon.home)
    n = len(d.daemon.hub.unpaired)
    ext.connect()
    ext.wait_for(lambda: len(d.daemon.hub.unpaired) > n)
    return ext


def test_an_unpaired_copy_in_the_profile_the_owner_is_in_is_paired_with_no_command(bd: Any) -> None:  # noqa: F811
    d, launched = bd
    setup(d, CHROME)
    ext = unpaired_hello(d)
    assert pair_pages(launched) == []                        # not before Chrome's write delay has passed
    page = until_page(d, launched)
    assert page[1] == "--profile-directory=Profile 1"
    assert page[-1].startswith(f"chrome-extension://{FERB_EXTENSION_ID}/pair.html#token=")
    assert len(launched) == 1                                # no wake page: the profile is the one in front
    ext.pair(page[-1].split("#token=")[1])
    ext.wait_for(lambda: binding(d, "Profile 1") is not None and "browsers.autopaired" in kinds(d))
    assert binding(d, "Profile 1").identity is Identity.OWNER


def test_a_stale_last_used_profile_waits_for_chromes_write_and_goes_to_the_profile_in_front(bd: Any) -> None:  # noqa: F811
    """The owner switched to "Personal" moments ago; Chrome's Local State still names "Work". The page must not pull
    Work's window forward: FERB waits until two readings agree, and they agree on Personal."""
    d, launched = bd
    setup(d, CHROME, last_used="Profile 1")
    unpaired_hello(d)
    use(d, "Default")                                        # Chrome writes the switch
    page = until_page(d, launched)
    assert page[1] == "--profile-directory=Default"
    assert len(pair_pages(launched)) == 1


def test_nothing_opens_while_the_browser_isnt_in_front_until_the_owner_switches_to_it(bd: Any) -> None:  # noqa: F811
    d, launched = bd
    setup(d, "com.apple.Terminal")
    unpaired_hello(d)
    settle(d)
    assert pair_pages(launched) == []
    d.daemon.front_app = lambda: CHROME
    d.call_soon(d.daemon._on_app_focus, Focused(1, "com.apple.Safari", "Safari"))   # not a FERB browser: nothing
    assert not running(d)
    d.call_soon(d.daemon._on_app_focus, Focused(2, CHROME, "Google Chrome"))
    assert until_page(d, launched)[1] == "--profile-directory=Profile 1"


def test_the_owner_leaving_the_browser_before_the_page_goes_stops_it(bd: Any) -> None:  # noqa: F811
    d, launched = bd
    setup(d, CHROME)
    unpaired_hello(d)
    d.daemon.front_app = lambda: "com.apple.Terminal"
    settle(d)
    assert pair_pages(launched) == []


def test_ferbs_own_profile_a_profile_with_ferb_off_and_a_paired_one_are_left_alone(bd: Any) -> None:  # noqa: F811
    d, launched = bd
    setup(d, CHROME, last_used="Profile 22")                 # FERB's identity profile: setup pairs it as FERB
    unpaired_hello(d)
    settle(d)
    assert pair_pages(launched) == []

    data = d.daemon.browser_data_dirs["chrome"]
    (data / "Default" / "Secure Preferences").write_text(json.dumps(
        {"extensions": {"settings": {FERB_EXTENSION_ID: {"location": 4, "disable_reasons": [1]}}}}))
    use(d, "Default")                                        # FERB is off there
    d.call_soon(d.daemon._on_app_focus, Focused(2, CHROME, "Google Chrome"))
    settle(d)
    assert pair_pages(launched) == []

    d.daemon.front_app = lambda: None                        # paired by hand, with nothing automatic in the way
    pair(d, launched, "Work")
    n = len(pair_pages(launched))
    d.daemon.front_app = lambda: CHROME
    use(d, "Profile 1")
    unpaired_hello(d)                                        # a second, unpaired copy says hello in Work
    settle(d)
    assert len(pair_pages(launched)) == n


def test_ferbs_account_profile_is_never_paired_as_the_owner_before_its_identity_step(bd: Any) -> None:  # noqa: F811
    d, launched = bd
    d.daemon.config.set("identity", "profile", "")           # `ferb setup --identity` hasn't run yet
    d.daemon.config.set("mailbox", "address", "FERB@gmail.com")
    setup(d, CHROME, last_used="Profile 22")                 # signed in as ferb@gmail.com
    unpaired_hello(d)
    settle(d)
    assert pair_pages(launched) == []
    assert binding(d, "Profile 22") is None


def test_a_profile_the_owner_unpaired_stays_unpaired_until_paired_by_hand(bd: Any) -> None:  # noqa: F811
    d, launched = bd
    d.daemon.front_app = lambda: None
    pair(d, launched, "Work")
    c = Client(d.sock, timeout=30)
    try:
        assert c.call("browsers.unpair", {"profile": "Work"})["unpaired"] is True
    finally:
        c.close()
    setup(d, CHROME, last_used="Profile 1")
    n = len(pair_pages(launched))
    unpaired_hello(d)                                        # e.g. after a restart
    settle(d)
    assert len(pair_pages(launched)) == n and binding(d, "Profile 1") is None

    d.daemon.front_app = lambda: None
    pair(d, launched, "Work")                                # by hand: automatic pairing applies again later

    async def off() -> bool:
        return bool(d.daemon.pairings.autopair_off("chrome", "Profile 1"))
    assert d.run(off()) is False


def test_a_pairing_by_hand_in_progress_is_left_to_finish(bd: Any) -> None:  # noqa: F811
    d, launched = bd
    setup(d, CHROME)

    async def issue() -> str:
        return str(d.daemon.pairings.issue_token("chrome", "Profile 1"))
    d.run(issue())                                           # `ferb browsers pair` has its page open
    unpaired_hello(d)
    settle(d)
    assert pair_pages(launched) == []


def test_one_try_per_profile_while_ferb_runs_and_a_failed_try_is_journaled(bd: Any) -> None:  # noqa: F811
    d, launched = bd
    setup(d, CHROME)
    ext = unpaired_hello(d)
    page = until_page(d, launched)
    settle(d)                                                # past pair_timeout_s: the page's key was never used
    assert "browsers.autopair_failed" in kinds(d)
    ext.pair(page[-1].split("#token=")[1])                   # too late: the key was revoked
    ext.hello()
    d.call_soon(d.daemon._on_app_focus, Focused(2, CHROME, "Google Chrome"))
    settle(d)
    assert len(pair_pages(launched)) == 1
    assert binding(d, "Profile 1") is None


def test_automatic_pairing_is_off_unless_enabled_and_only_for_chrome_and_brave(bd: Any) -> None:  # noqa: F811
    d, launched = bd
    d.daemon.front_app = lambda: CHROME
    unpaired_hello(d)                                        # conftest turns it off for every other test
    assert d.daemon._autopair_task is None
    assert AUTOPAIR_BROWSERS == ("chrome", "brave")          # where handing a page to one profile is verified


def test_a_missing_last_used_profile_reads_as_default_like_chromium(tmp_path: Any) -> None:
    (tmp_path / "Local State").write_text(json.dumps({"profile": {"info_cache": {"Default": {"name": "Me"}}}}))
    assert profiles.last_used("chrome", tmp_path) == "Default"
    (tmp_path / "Local State").write_text(json.dumps({"profile": {"info_cache": {"Profile 1": {"name": "Me"}}}}))
    assert profiles.last_used("chrome", tmp_path) is None
    (tmp_path / "Local State").write_text(json.dumps({"profile": {"last_used": "Profile 3", "info_cache": {}}}))
    assert profiles.last_used("chrome", tmp_path) == "Profile 3"


def test_a_reinstalled_copy_is_paired_again_when_the_old_one_is_gone(bd: Any) -> None:  # noqa: F811
    """Live 2026-10-26: the owner reinstalled FERB in "New_Personal"; the new copy connected unpaired and opening the
    profile did nothing, because the profile still held the old copy's pairing. Owner: "this is bad UX". A profile
    whose paired copy isn't connected is paired again the same way, with the owner in its window."""
    d, launched = bd
    old = pair(d, launched, "Work")                          # "Work" is Profile 1
    old.wait_for(lambda: old.gen is not None)
    old.kill()
    old.wait_for(lambda: d.daemon.hub.bridges[("chrome", "Profile 1")].conn is None)
    before = binding(d, "Profile 1").install_id
    pages = len(pair_pages(launched))                        # the hand pairing's page is already there
    setup(d, CHROME)
    ext = unpaired_hello(d)
    page = until_page(d, launched, pages + 1)
    assert page[1] == "--profile-directory=Profile 1"
    ext.pair(page[-1].split("#token=")[1])
    ext.wait_for(lambda: binding(d, "Profile 1") is not None and binding(d, "Profile 1").install_id != before)
    assert binding(d, "Profile 1").install_id == ext.install_id


def test_a_profile_whose_paired_copy_is_connected_is_never_paired_again(bd: Any) -> None:  # noqa: F811
    """No hijack: while the paired copy is live, a second unpaired copy never takes the profile over."""
    d, launched = bd
    old = pair(d, launched, "Work")
    old.wait_for(lambda: old.gen is not None)
    before = len(pair_pages(launched))
    setup(d, CHROME)
    unpaired_hello(d)
    settle(d)
    assert len(pair_pages(launched)) == before
    assert binding(d, "Profile 1").install_id == old.install_id
