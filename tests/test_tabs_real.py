"""The owner's current tab (owner decision, 2026-09-30), real (pytest -m real; Codex only; locked down: no Mac,
shell or AppleScript tools). A long local page is already open in the Chrome "Lab" profile, as a tab the owner
has open; "in my Lab profile, keep scrolling down the comments in the current tab" must scroll THAT tab (the
page reports its scroll position), not open a new one, and must leave it open afterwards.
"""

import time
from typing import Any

import pytest

from tests.conftest import RunningDaemon
from tests.chromelab import lab, rd, site, chat, problems  # noqa: F401

pytestmark = pytest.mark.real


def test_the_current_tab_is_scrolled_in_place(lab: Any, rd: RunningDaemon, site: tuple[str, list[str]]) -> None:  # noqa: F811
    _, chrome, _ = lab
    url, called = site
    called.clear()
    tab_id = chrome.owner_tab(f"{url}/comments")
    time.sleep(1)
    since = rd.daemon.clock.now().isoformat()
    evs = chat(rd, "in my Lab profile, keep scrolling down through the comments in the current tab until you "
                   "reach the last comment, then tell me what it says")
    scrolled = [c for c in called if c.startswith("scrolled:")]
    text = " ".join(str(e.get("answer") or e.get("text") or "") for e in evs if e.get("type") in ("outcome", "reply"))
    assert scrolled and max(int(c.split(":")[1]) for c in scrolled) > 2000, (called, text, problems(rd, since))
    assert "comment 60" in text.lower(), text                                       # it reached the end
    opened = [e for e in evs if e.get("type") == "event" and "open" in str(e.get("data", {}).get("tool", ""))]
    assert not opened, opened                                                       # no new tab: the owner's own one

    assert chrome.evaluate(chrome.sw, f"chrome.tabs.get({tab_id}).then(t => t.url)") == f"{url}/comments"
