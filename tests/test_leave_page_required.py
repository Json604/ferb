"""HAND-447/448: after FERB's own input in a tab (typed, filled, uploaded) since its page last loaded, leaving that tab
(browser.navigate, browser.close) needs the worker's leave_page; without it the call is refused at once, nothing
sent. A page that asks before closing at the task's end keeps its dialog (it doesn't come forward) and is closed once,
never asked again. The real daemon and gateway with the fake extension.

Owner 2026-11-10 (via the coordinator): the "Leave site?" dialog itself is the concern: it shifts the tab and maximizes
Chrome. The extension can keep it from being raised only when the worker said leave (HAND-446)."""

import json
from typing import Any

import pytest

from tests.browserkit import bd, call  # noqa: F401  (the fixture and its helpers)
from tests.chromelab import SHOP, _ready


def _cmds(ext: Any, op: str) -> list[dict[str, Any]]:
    out = [json.loads(json.loads(m)["body"]) for m in ext.wire if '"type": "cmd"' in m]
    return [c for c in out if c["op"] == op]


def _typed(d: Any, g: Any) -> str:
    tab = call(d, g, "browser.open", url=f"{SHOP}/form")["content"]["tab"]
    snap = call(d, g, "browser.snapshot", tab=tab)["content"]
    name = next(e["id"] for e in snap["elements"] if e.get("label") == "Name")
    typed = call(d, g, "browser.act", tab=tab, element=name, action="type", text="Kai")
    assert not typed["is_error"], typed
    return str(tab)


# decision: HAND-447
def test_leaving_a_tab_typed_into_needs_the_workers_leave_or_stay(bd: Any) -> None:  # noqa: F811
    d, launched = bd
    ext, g = _ready(d, launched)
    tab = _typed(d, g)
    navs = len(_cmds(ext, "navigate"))                                       # opening the tab loaded it once
    refused = call(d, g, "browser.navigate", tab=tab, url=f"{SHOP}/next")
    assert refused["is_error"] and "leave_page" in json.dumps(refused["content"])
    assert len(_cmds(ext, "navigate")) == navs                               # nothing was sent
    closed = call(d, g, "browser.close", tab=tab)
    assert closed["is_error"] and "leave_page" in json.dumps(closed["content"]) and not _cmds(ext, "close")
    left = call(d, g, "browser.navigate", tab=tab, url=f"{SHOP}/next", leave_page="leave")
    assert not left["is_error"], left
    assert _cmds(ext, "navigate")[-1]["args"]["leave_page"] == "leave"
    ext.wait_for(lambda: not d.daemon.router.typed_since_load(d.daemon.router.owned_by(tab, g.task_id)))
    again = call(d, g, "browser.navigate", tab=tab, url=f"{SHOP}/form")        # a fresh page: optional again
    assert not again["is_error"], again
    assert "leave_page" not in _cmds(ext, "navigate")[-1]["args"]


# decision: HAND-447
def test_an_untouched_tab_and_a_borrowed_tab_leave_without_a_choice(bd: Any) -> None:  # noqa: F811
    d, launched = bd
    ext, g = _ready(d, launched)
    tab = call(d, g, "browser.open", url=f"{SHOP}/form")["content"]["tab"]
    assert not call(d, g, "browser.navigate", tab=tab, url=f"{SHOP}/next")["is_error"]
    ext.tabs[41], ext.tab_windows[41] = f"{SHOP}/form", 7
    borrowed = call(d, g, "browser.use_tab", tab_ref="chrome:Profile 1:41")["content"]["tab"]
    snap = call(d, g, "browser.snapshot", tab=borrowed)["content"]
    name = next(e["id"] for e in snap["elements"] if e.get("label") == "Name")
    assert not call(d, g, "browser.act", tab=borrowed, element=name, action="type", text="Kai")["is_error"]
    assert not call(d, g, "browser.close", tab=borrowed)["is_error"]          # only let go of: the page stays


# decision: HAND-448
def test_a_page_that_asks_at_the_tasks_end_keeps_its_dialog_and_is_closed_once(bd: Any) -> None:  # noqa: F811
    d, launched = bd
    ext, g = _ready(d, launched)
    tab = _typed(d, g)
    chrome_id = d.daemon.router._tabs[tab].chrome_id
    ext.asks_before_leaving.add(chrome_id)
    with pytest.raises(Exception) as ended:
        d.run(d.daemon.router.release(g.task_id))
    assert len(_cmds(ext, "close")) == 1 and "leave_page" not in _cmds(ext, "close")[0]["args"]   # code never picks
    [left] = getattr(ended.value, "facts")["tabs"]
    assert left["tab"] == tab and "dialog" in left["error"]
