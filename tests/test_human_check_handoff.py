"""Owner 2026-10-27: a human check FERB can't pass with one ordinary click goes to the owner as one notification that
opens the exact tab, and FERB carries on by itself once the page lets them through, with no "done" reply. FERB never
solves the check. The click brings the tab forward through extension 0.15.13's show_tab; an older extension gets the
profile's browser brought forward instead."""

import threading
import time
from typing import Any

from tests.browserkit import OWNER, bd, call, pair, task  # noqa: F401  (bd is a fixture)

CHECK = "Verify you are human by completing the action below."


def _start(d: Any, launched: list[list[str]]) -> tuple[Any, Any, str, int, list[tuple[str, str, dict[str, Any]]]]:
    ext = pair(d, launched, "Work")
    banners: list[tuple[str, str, dict[str, Any]]] = []

    async def notify(title: str, text: str, urgent: bool = False, **kw: Any) -> None:
        banners.append((title, text, {"urgent": urgent, **kw}))
    d.daemon._notify = notify
    d.daemon.handoff_poll_s = 0.05
    g = task(d, "w", OWNER, "use my Work profile")
    tab = call(d, g, "browser.open", url="https://www.shop.example/form")["content"]["tab"]
    chrome_id = max(ext.tabs)
    ext.page_text[chrome_id] = "Just a moment... " + CHECK
    return ext, g, tab, chrome_id, banners


def _handoff(d: Any, g: Any, tab: str, out: dict[str, Any]) -> threading.Thread:
    run = threading.Thread(target=lambda: out.update(call(d, g, "browser.handoff", tab=tab,
                                                          message="Verify you are human")))
    run.start()
    return run


def _wait(cond: Any) -> None:
    end = time.monotonic() + 10
    while not cond() and time.monotonic() < end:
        time.sleep(0.02)


def test_one_notification_opens_the_tab_and_ferb_carries_on_once_the_check_passes(bd: Any) -> None:  # noqa: F811
    d, launched = bd
    ext, g, tab, chrome_id, banners = _start(d, launched)
    out: dict[str, Any] = {}
    try:
        run = _handoff(d, g, tab, out)
        _wait(lambda: banners)
        title, text, kw = banners[0]
        assert kw["target"]["kind"] == "handoff" and kw["urgent"] is True and kw["category"] == "question"
        assert "shop.example wants to check you're human" in text and "www." not in text
        assert not out                                                   # still waiting for the owner
        shown = d.run(d.daemon._show_handoff(None, {"handoff_id": kw["target"]["handoff_id"]}))
        assert shown == {"shown": True, "tab": True} and ext.shown == [chrome_id]
        ext.page_text[chrome_id] = "Welcome back"                        # the owner passed the check
        run.join(10)
        assert out["content"]["passed"] is True and not out["is_error"] and len(banners) == 1
        assert d.run(d.daemon._show_handoff(None, {"handoff_id": kw["target"]["handoff_id"]}))["shown"] is False
    finally:
        ext.kill()


def test_a_tab_that_cant_be_shown_brings_the_profiles_browser_forward_instead(bd: Any) -> None:  # noqa: F811
    """Every extension FERB accepts knows show_tab (protocol 18's minimum); when the tab can't be shown (its profile
    disconnected), the click brings that profile's browser forward instead."""
    d, launched = bd
    ext, g, tab, chrome_id, banners = _start(d, launched)

    async def unshown(ref: Any) -> bool:
        return False
    d.daemon.router.show_tab = unshown                                   # type: ignore[union-attr,method-assign]
    out: dict[str, Any] = {}
    try:
        run = _handoff(d, g, tab, out)
        _wait(lambda: banners)
        before = len(launched)
        shown = d.run(d.daemon._show_handoff(None, {"handoff_id": banners[0][2]["target"]["handoff_id"]}))
        assert shown == {"shown": True, "tab": False} and not ext.shown
        assert launched[before:] and launched[-1][-1] == "--profile-directory=Profile 1"
        ext.tabs.pop(chrome_id)                                          # the owner closed the tab instead
        run.join(10)
        assert out["is_error"] and "closed" in out["content"]["error"]
    finally:
        ext.kill()


def test_a_page_that_no_longer_shows_the_words_needs_no_owner(bd: Any) -> None:  # noqa: F811
    d, launched = bd
    ext, g, tab, chrome_id, banners = _start(d, launched)
    ext.page_text[chrome_id] = "Welcome back"
    try:
        r = call(d, g, "browser.handoff", tab=tab, message="Verify you are human")
        assert r["content"]["passed"] is True and not banners
    finally:
        ext.kill()
