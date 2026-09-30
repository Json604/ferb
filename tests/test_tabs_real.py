"""The owner's current tab (owner decision, 2026-09-30), real (pytest -m real; Codex only; locked down: no Mac,
shell or AppleScript tools). A long local page is already open in the Chrome "FERB Test" profile, as a tab the owner
has open; "in my FERB Test profile, keep scrolling down the comments in the current tab" must scroll THAT tab (the
page reports its scroll position), not open a new one, and must leave it open afterwards.
"""

import time
from typing import Any

import pytest

from tests.conftest import RunningDaemon
from tests.test_contacts_real import chat, problems, rd, site  # noqa: F401  (fixtures: the test daemon on "FERB Test")

pytestmark = pytest.mark.real


def test_the_current_tab_is_scrolled_in_place(rd: RunningDaemon, site: tuple[str, list[str]]) -> None:  # noqa: F811
    from ferb.act.browser.profiles import discover_all, open_known_profile
    url, called = site
    called.clear()
    p = next(p for p in discover_all() if p.display_name == "FERB Test" and p.browser == "chrome")
    open_known_profile("chrome", p.folder, f"{url}/comments", discover_all())       # the owner's own tab
    time.sleep(3)
    since = rd.daemon.clock.now().isoformat()
    evs = chat(rd, "in my FERB Test profile, keep scrolling down through the comments in the current tab until you "
                   "reach the last comment, then tell me what it says")
    scrolled = [c for c in called if c.startswith("scrolled:")]
    text = " ".join(str(e.get("answer") or e.get("text") or "") for e in evs if e.get("type") in ("outcome", "reply"))
    assert scrolled and max(int(c.split(":")[1]) for c in scrolled) > 2000, (called, text, problems(rd, since))
    assert "comment 60" in text.lower(), text                                       # it reached the end
    opened = [e for e in evs if e.get("type") == "event" and "open" in str(e.get("data", {}).get("tool", ""))]
    assert not opened, opened                                                       # no new tab: the owner's own one
