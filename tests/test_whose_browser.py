"""Whose browser (owner decision, 2026-09-30), the owner's open tabs, and scrolling that isn't a loop.
- the owner's browser only when their words point at it ("in my brave … the current youtube tab"); a plain "do X on
  YouTube" is FERB's own browser; plan validation refuses an owner-browser plan without it;
- "Brave" is the owner's Brave profile, never an app route; a service is announced once;
- the owner's open tabs can be listed and taken over; FERB lets go of an attached tab and never closes it;
- the loop guard stops a cycle only when its results repeat too.
"""

import asyncio
from datetime import datetime, timezone
from types import SimpleNamespace
from typing import Any

from ferb.act.browser.ext_bridge import Router
from ferb.core.types import Identity
from ferb.orchestrator import dag
from ferb.orchestrator.guards import TreeGuard
from ferb.orchestrator.schemas import Plan
from ferb.policy import services as svc
from ferb.policy.services import Environment, owner_browser_named
from tests.test_supervisor import plan, sub

NOW = datetime(2026, 9, 30, 12, 0, tzinfo=timezone.utc)
OWNER_SAID = "ok, in my brave keep scorlling for comments in the current youtube tab"


# ---------- whose browser ----------

def test_the_owners_words_decide_whose_browser() -> None:
    for words in (OWNER_SAID, "summarize this tab", "open it in my browser", "check my linkedin messages",
                  "open example.com in my Work profile", "use brave to open youtube", "in my chrome, open x.com"):
        assert owner_browser_named(words, ["Work"]), words
    for words in ("play some lofi on youtube", "post this on linkedin", "find the latest AI news on x.com",
                  "keep scrolling the comments", "in chrome open x.com"):
        assert not owner_browser_named(words, ["Work"]), words


def env(owner_browser: bool, **kw: Any) -> Environment:
    base: dict[str, Any] = {"apps": [("com.brave.Browser", "Brave Browser"), ("net.whatsapp.WhatsApp", "WhatsApp")],
                            "paired": [("brave", "Default", "Work")], "owner_browser": owner_browser}
    return Environment(**(base | kw))


def yt() -> svc.Service:
    return next(s for s in svc.CATALOGUE if s.name == "YouTube")


def test_a_plain_request_on_a_site_is_ferbs_own_browser() -> None:
    r = svc.route(yt(), env(False, usage_web={("www.youtube.com", "brave", "Default"): (40, NOW)}))
    assert r is not None and r.browser == svc.FERB                                   # the owner's usage doesn't pull it
    assert r.say() == "Using YouTube on the web (youtube.com) in FERB's own browser"
    mine = svc.route(yt(), env(True))
    assert mine is not None and (mine.browser, mine.profile_name) == ("brave", "Work")
    wa = next(s for s in svc.CATALOGUE if s.name == "WhatsApp")
    assert svc.route(wa, env(False)).kind == "app"                                   # type: ignore[union-attr]


def test_brave_is_never_an_app_route_and_a_service_is_announced_once() -> None:
    e = env(True, usage_web={("www.youtube.com", "brave", "Default"): (3, NOW)})
    named = [s.name for s in svc.services_named(OWNER_SAID, e)]
    assert named == ["YouTube"], named                                               # no "Brave Browser", no "Youtube"


def test_plan_validation_refuses_the_owners_browser_unless_they_asked() -> None:
    p = Plan.model_validate(plan([sub("a", identity="owner", scopes=("browser:read", "browser:act"))],
                                 verbs=("summarize",)) | {})
    avail = {"browser:read", "browser:act", "identity:browse"}
    problems = dag.validate(p, "summarize the comments on youtube", avail, owner_browser=False)
    assert any("didn't ask for their own browser" in x for x in problems), problems
    assert not any("own browser" in x for x in dag.validate(p, OWNER_SAID, avail, owner_browser=True))


# ---------- the owner's open tabs ----------

class FakeBridge:
    def __init__(self) -> None:
        self.binding = SimpleNamespace(browser="brave", folder="Default", identity=Identity.OWNER)
        self.conn, self.device_id, self.gen, self.name = object(), "dev", 1, "brave/Default"
        self.calls: list[str] = []

    async def call(self, op: str, args: dict[str, Any], timeout: float = 30, cid: str | None = None) -> Any:
        self.calls.append(op)
        if op == "list_tabs":
            return {"tabs": [{"tab_id": 11, "host": "www.youtube.com", "active": True},
                             {"tab_id": 12, "host": "mail.google.com", "active": False}]}
        return {"closed": True}


def router() -> tuple[Router, FakeBridge]:
    br = FakeBridge()
    hub = SimpleNamespace(bridges={("brave", "Default"): br}, bridge=lambda b, f: br)
    r = Router(hub, resolver=SimpleNamespace(), profiles=lambda: [SimpleNamespace(browser="brave", folder="Default",
                                                                                  display_name="Work")])  # type: ignore
    return r, br


def test_the_owners_open_tabs_can_be_taken_over_and_are_never_closed() -> None:
    r, br = router()
    tabs = asyncio.run(r.open_tabs())
    assert tabs[0] == {"tab_ref": "brave:Default:11", "profile": "Brave “Work”", "site": "www.youtube.com",
                       "active": True}
    ref = asyncio.run(r.attach("brave:Default:11", "task-1"))
    assert r.owned_by(ref.tab_id, "task-1") == ref and r.tab_url(ref.tab_id) == "https://www.youtube.com/"
    assert asyncio.run(r.attach("brave:Default:11", "task-1")) == ref                # same tab, same id
    assert [t["tab_ref"] for t in asyncio.run(r.open_tabs())] == ["brave:Default:12"]   # now in use by the task
    asyncio.run(r.release("task-1"))                                                 # the task ends
    assert "close" not in br.calls and ref.tab_id not in r._tabs                    # let go, never closed


def test_another_task_cant_grab_an_attached_tab() -> None:
    r, _ = router()
    asyncio.run(r.attach("brave:Default:11", "task-1"))
    try:
        asyncio.run(r.attach("brave:Default:11", "task-2"))
        raise AssertionError("should have been refused")
    except Exception as e:
        assert "another task" in str(e)


def test_the_tools_exist_only_for_the_owners_browser(daemon: Any) -> None:
    caps = daemon.daemon.registry.capabilities
    assert {"browser.tabs", "browser.use_tab"} <= set(caps)
    assert caps["browser.tabs"].identities == {Identity.OWNER}
    assert not [n for n in caps if n.startswith("ferb_browser.") and n.endswith(("tabs", "use_tab"))]


# ---------- scrolling isn't a loop ----------

def test_scrolling_through_a_changing_page_is_not_a_loop_but_a_stuck_one_is() -> None:
    g = TreeGuard(0.0)
    for i in range(12):                                                              # new comments load each time
        assert g.call("w", "browser.act", {"action": "scroll", "text": "down"}, False, f"ok{i}") is None
        assert g.call("w", "browser.snapshot", {"tab": "t1"}, False, f"page{i}") is None
    stuck = TreeGuard(0.0)
    reasons = []
    for _ in range(6):                                                               # the page never changes
        reasons.append(stuck.call("w", "browser.act", {"action": "scroll", "text": "down"}, False, "ok"))
        reasons.append(stuck.call("w", "browser.snapshot", {"tab": "t1"}, False, "same page"))
    assert any(r and "loop" in r for r in reasons)


def test_a_page_scrolls_without_an_element_and_an_old_extension_says_reload() -> None:
    from ferb.core.errors import DriverError, Refused
    r, br = router()
    ref = asyncio.run(r.attach("brave:Default:11", "task-1"))

    async def scroll_ok(op: str, args: dict[str, Any], timeout: float = 30, cid: str | None = None) -> Any:
        return {"y": 1800, "height": 5000, "at_end": False} if op == "scroll" else {"tabs": []}
    br.call = scroll_ok                                                              # type: ignore[method-assign]
    assert asyncio.run(r.scroll(ref, "down")) == {"y": 1800, "page_height": 5000, "at_end": False}

    async def old(op: str, args: dict[str, Any], timeout: float = 30, cid: str | None = None) -> Any:
        raise DriverError("unknown op scroll")
    br.call = old                                                                    # type: ignore[method-assign]
    try:
        asyncio.run(r.scroll(ref, "down"))
        raise AssertionError("should have been refused")
    except Refused as e:
        assert "reload it" in str(e)


def test_scroll_is_a_read_in_both_browsers(daemon: Any) -> None:
    caps = daemon.daemon.registry.capabilities
    assert caps["browser.scroll"].scopes == {"browser:read"}
    spec = caps["browser.scroll"].describe(caps["browser.scroll"].Input(tab="t9"))
    assert spec.kind == "read"


def test_a_tab_the_task_took_over_stays_in_its_list() -> None:
    r, _ = router()
    ref = asyncio.run(r.attach("brave:Default:11", "task-1"))
    mine = asyncio.run(r.open_tabs(task="task-1"))
    assert mine[0]["tab_ref"] == "brave:Default:11" and mine[0]["taken_over_as"] == ref.tab_id
    assert [t["tab_ref"] for t in asyncio.run(r.open_tabs(task="task-2"))] == ["brave:Default:12"]   # others: hidden


def test_find_searches_the_whole_page() -> None:
    r, br = router()
    ref = asyncio.run(r.attach("brave:Default:11", "task-1"))

    async def page(op: str, args: dict[str, Any], timeout: float = 30, cid: str | None = None) -> Any:
        if op == "find":
            return {"count": 23, "lines": 900, "examples": ["arpit is the best"] * 25}
        return {"tabs": []}
    br.call = page                                                                   # type: ignore[method-assign]
    got = asyncio.run(r.find(ref, "arpit"))
    assert got["matches"] == 23 and got["lines_on_page"] == 900 and len(got["examples"]) == 20


def test_an_app_alias_counts_as_naming_the_app() -> None:
    from ferb.policy.names import app_named
    assert app_named("WhatsApp", "now call aditya scaler in wa")                     # the owner's session
    assert not app_named("WhatsApp", "call aditya")
