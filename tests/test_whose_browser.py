"""The owner's open tabs, and scrolling that isn't a loop.
- the owner's open tabs can be listed and taken over; FERB lets go of an attached tab and never closes it;
- the loop guard stops a cycle only when its results repeat too;
"""

import asyncio
from dataclasses import replace
from types import SimpleNamespace
from typing import Any

import pytest

from ferb.act.browser.ext_bridge import Router
from ferb.core.errors import Refused
from ferb.core.types import Identity, Task
from ferb.orchestrator.tree import TaskRecord


# ---------- the owner's open tabs ----------

class FakeBridge:
    def __init__(self) -> None:
        self.binding = SimpleNamespace(browser="brave", folder="Default", identity=Identity.OWNER)
        self.conn, self.device_id, self.gen, self.name = object(), "dev", 1, "brave/Default"
        self.calls: list[str] = []

    async def call(self, op: str, args: dict[str, Any], timeout: float = 30, cid: str | None = None) -> Any:
        self.calls.append(op)
        if op == "list_tabs":
            return {"tabs": [{"tab_id": 11, "host": "www.youtube.com", "url":"https://www.youtube.com/watch?v=fixture", "title":"Lesson", "active": True},
                             {"tab_id": 12, "host": "mail.google.com", "url":"https://mail.google.com/mail/", "title":"Inbox", "active": False}]}
        if op == "release_tab":
            return {"released": True, "kept_open": bool(args.get("keep_open")), "tab_id": args["tab_id"]}
        return {"closed": True}


def router() -> tuple[Router, FakeBridge]:
    br = FakeBridge()
    hub = SimpleNamespace(bridges={("brave", "Default"): br}, bridge=lambda b, f: br)
    r = Router(hub, resolver=SimpleNamespace(), profiles=lambda: [SimpleNamespace(browser="brave", folder="Default",
                                                                                  display_name="Work")])  # type: ignore
    return r, br


async def register_tasks(d: Any, *tasks: Task) -> None:
    for task in tasks:
        d.tasks.add(TaskRecord(task, d.clock.now(), "span", "tree"))


def test_the_owners_open_tabs_can_be_taken_over_and_are_never_closed() -> None:
    r, br = router()
    tabs = asyncio.run(r.open_tabs())
    assert tabs[0] == {"tab_ref": "brave:Default:11", "profile": "Brave “Work”", "site": "www.youtube.com",
                       "active": True,"profile_id":"brave/Default","url":"https://www.youtube.com/watch?v=fixture","title":"Lesson","window_id":None}
    ref = asyncio.run(r.attach("brave:Default:11", "task-1"))
    assert r.owned_by(ref.tab_id, "task-1") == ref and r.tab_url(ref.tab_id) == "https://www.youtube.com/watch?v=fixture"
    assert asyncio.run(r.attach("brave:Default:11", "task-1")) == ref                # same tab, same id
    assert [t["tab_ref"] for t in asyncio.run(r.open_tabs())] == ["brave:Default:12"]   # now in use by the task
    asyncio.run(r.release("task-1"))                                                 # the task ends
    assert "close" not in br.calls and ref.tab_id not in r._tabs                    # let go, never closed
    assert "release_tab" in br.calls


def test_handoff_preserves_task_tab_and_cleanup_releases_it_without_closing() -> None:
    r, br = router()
    ref = asyncio.run(r.attach("brave:Default:11", "task-1"))
    kept = asyncio.run(r.keep_open(ref))
    assert kept['kept_open'] and kept['released']
    assert r.owned_by(ref.tab_id, 'task-1') == ref  # checker can still inspect before cleanup
    result = asyncio.run(r.release('task-1'))
    assert result[0]['kept_open'] and not result[0]['closed']
    assert 'close' not in br.calls and ref.tab_id not in r._tabs


def test_failed_handoff_does_not_claim_success_or_allow_cleanup_to_close_tab() -> None:
    from ferb.core.errors import DriverError
    r, br = router()
    ref = asyncio.run(r.attach("brave:Default:11", "task-1"))
    r._tabs[ref.tab_id].attached = False
    br.unhold = lambda: None
    async def fails(op, args, **kwargs):
        br.calls.append(op)
        raise DriverError('disconnected during release')
    br.call = fails
    with pytest.raises(DriverError):
        asyncio.run(r.keep_open(ref))
    with pytest.raises(DriverError):
        asyncio.run(r.release('task-1'))
    assert ref.tab_id in r._tabs and 'close' not in br.calls


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


def test_registered_checker_can_inspect_parent_tab_without_taking_ownership(daemon: Any) -> None:
    d = daemon.daemon
    r, br = router()
    r.inspection_parent = d._inspection_parent
    parent = Task("worker", None, "read comments", "dev", Identity.OWNER,
                  {"browser:read", "browser:act"}, "worker", "request", [], None)
    checker = replace(parent, id="check", parent_id=parent.id, role="verifier", scopes={"browser:read"})
    daemon.run(register_tasks(d, parent, checker))
    ref = asyncio.run(r.attach("brave:Default:11", parent.id))
    assert r.owned_by(ref.tab_id, checker.id) == ref
    assert asyncio.run(r.open_tabs(task=checker.id))[0]["taken_over_as"] == ref.tab_id
    assert asyncio.run(r.attach("brave:Default:11", checker.id)) == ref
    assert r.owner_of(br, 11) == parent.id
    asyncio.run(r.release(checker.id))
    assert r.owned_by(ref.tab_id, parent.id) == ref
    d.tasks.remove(checker.id)
    with pytest.raises(Refused):
        r.owned_by(ref.tab_id, checker.id)
    asyncio.run(r.release(parent.id))
    assert "close" not in br.calls


@pytest.mark.parametrize("changes", [
    {"role": "worker"}, {"request_id": "other"}, {"identity": Identity.FERB},
    {"scopes": {"browser:read", "browser:act"}}, {"parent_id": "missing"},
    {"device_id": "other-device"},
])
def test_checker_tab_access_requires_matching_registered_parent(daemon: Any, changes: dict[str, Any]) -> None:
    d = daemon.daemon
    r, _ = router()
    r.inspection_parent = d._inspection_parent
    parent = Task("worker", None, "g", "dev", Identity.OWNER, {"browser:read"}, "worker", "req", [], None)
    checker = replace(replace(parent, id="worker.verify", parent_id=parent.id, role="verifier"), **changes)
    daemon.run(register_tasks(d, parent, checker))
    ref = asyncio.run(r.attach("brave:Default:11", parent.id))
    for tid in (checker.id, "worker.reconcile", "other-worker"):
        with pytest.raises(Refused):
            r.owned_by(ref.tab_id, tid)
        with pytest.raises(Refused):
            asyncio.run(r.attach("brave:Default:11", tid))
        assert len(asyncio.run(r.open_tabs(task=tid))) == 1


def test_chat_checker_reads_existing_worker_tab_without_navigating_or_taking_ownership(daemon: Any) -> None:
    """V3, W4: the checker reads the worker's borrowed tab where it is (find); it is never offered navigation of an
    owner's tab, so the page stays as the worker left it, and the tab stays the worker's, then the owner's."""
    import json
    from ferb.act.browser.tools import BrowserClose, BrowserFind, BrowserNavigate, OWNER_BROWSER
    from tests.conftest import chat, cond, done, install, outcome, sup, task, tool

    d = daemon.daemon
    r, br = router()
    r.inspection_parent = d._inspection_parent
    d.router = r
    d.registry.capabilities["browser.find"] = BrowserFind(r, OWNER_BROWSER)
    d.registry.capabilities["browser.close"] = BrowserClose(r, OWNER_BROWSER)
    d.registry.capabilities["browser.navigate"] = BrowserNavigate(r, OWNER_BROWSER)   # installed; not for a checker
    original_call = br.call

    async def page(op: str, args: dict[str, Any], **kwargs: Any) -> Any:
        if op == "find":
            return {"count": 1, "lines": 454, "examples": ["last author: a test comment"]}
        return await original_call(op, args, **kwargs)
    br.call = page

    def worker(token: str) -> str:
        grant = d.tokens.validate(token)
        daemon.run(r.attach("brave:Default:11", grant.task_id))
        return done("last author: a test comment")

    def checker(token: str) -> str:
        got = tool(daemon.sock, token, "browser.find", tab="t1", text="last author")
        assert not got["is_error"], got
        assert got["content"]["examples"] == ["last author: a test comment"]
        assert r.owner_of(br, 11) == d._inspection_parent(d.tokens.validate(token).task_id)
        assert r.tab_url("t1") == "https://www.youtube.com/watch?v=fixture"
        return json.dumps({"results": [{"index": 0, "met": True, "evidence": "fresh read matches"}]})

    spec = task("read current comments", "owner", conds=(cond("last comment matches"),))
    script = install(daemon, [sup("task", "reading", spec)], {"worker": worker, "worker.verify": checker,
                      "worker.resources": done("last author: a test comment; the borrowed tab remains open")})
    result = outcome(chat(daemon, "read the comments in my current Brave tab"))
    assert result["verified"] is True, result
    ctx = next(c for _, key, c, _ in script.contexts if key == "worker.verify")
    assert '"tool_prefix": "browser", "tab": "t1"' in ctx
    offered = next(t for _, key, _, t in script.contexts if key == "worker.verify")
    assert "browser.find" in offered and "browser.navigate" not in offered
    assert "navigate" not in br.calls
    assert not r._tabs and "close" not in br.calls


def test_find_searches_the_whole_page() -> None:
    r, br = router()
    ref = asyncio.run(r.attach("brave:Default:11", "task-1"))

    async def page(op: str, args: dict[str, Any], timeout: float = 30, cid: str | None = None) -> Any:
        if op == "find":
            return {"count": 23, "lines": 900, "examples": ["arpit is the best"] * 25}
        return {"tabs": []}
    br.call = page                                                                   # type: ignore[method-assign]
    got = asyncio.run(r.find(ref, "arpit"))
    assert got["matches"] == 23 and got["lines_on_page"] == 900 and len(got["examples"]) == 25


def test_a_held_tab_navigated_elsewhere_is_still_the_same_tab() -> None:
    """Owner live use 2026-10-18 (task 891b3eaf): the worker navigated t5 through problem pages and later reported "only
    14 of the required 15 tabs remain open; 'Monitoring, Observability & GitOps_1' is missing". A task's tabs are
    known by tab id: a held tab lists the page it held when the task took it over, whatever it shows now."""
    r, br = router()
    ref = asyncio.run(r.attach("brave:Default:11", "task-1"))

    async def moved(op: str, args: dict[str, Any], timeout: float = 30, cid: str | None = None) -> Any:
        if op == "list_tabs":                         # the held tab now shows a problem page
            return {"tabs": [{"tab_id": 11, "host": "www.scaler.com", "url": "https://www.scaler.com/problems/42",
                              "title": "Problem 42", "active": True},
                             {"tab_id": 12, "host": "mail.google.com", "url": "https://mail.google.com/mail/",
                              "title": "Inbox", "active": False}]}
        return {}
    br.call = moved                                                                  # type: ignore[method-assign]
    held = asyncio.run(r.open_tabs(task="task-1"))[0]
    assert held["taken_over_as"] == ref.tab_id and held["title"] == "Problem 42"
    assert held["taken_over_from"] == {"url": "https://www.youtube.com/watch?v=fixture", "title": "Lesson"}
    assert r.task_tabs("task-1") == [{"tab": ref.tab_id, "taken_over_from": {"url": "https://www.youtube.com/watch?v="
                                                                                 "fixture", "title": "Lesson"}}]


TITLE = "123456 is your Instagram code - kartikey@gmail.com - Gmail"


def test_the_page_a_taken_over_tab_held_is_stored_redacted() -> None:
    """Re-review 2 (I4, rule 14): the page held at takeover was stored raw and reached the final checker's model."""
    r, br = router()

    async def coded(op: str, args: dict[str, Any], timeout: float = 30, cid: str | None = None) -> Any:
        return {"tabs": [{"tab_id": 11, "host": "mail.google.com", "url": "https://mail.google.com/mail/u/0/",
                          "title": TITLE, "active": True}]} if op == "list_tabs" else {}
    br.call = coded                                                                  # type: ignore[method-assign]
    ref = asyncio.run(r.attach("brave:Default:11", "task-1"))
    held = r.task_tabs("task-1")[0]["taken_over_from"]
    assert "123456" not in held["title"] and "Instagram code" in held["title"]
    assert r._tabs[ref.tab_id].taken_over_from == held


def test_the_checkers_browser_context_is_redacted(daemon: Any, monkeypatch: pytest.MonkeyPatch) -> None:
    d = daemon.daemon
    monkeypatch.setattr(d, "_inspection_parent", lambda task_id: "parent")
    monkeypatch.setattr(d.router, "task_tabs", lambda parent: [{"tab": "t1", "taken_over_from": {"title": TITLE}}])
    if d.ferb_identity is not None:
        monkeypatch.setattr(d.ferb_identity, "task_tabs", lambda parent: [])
    context = d._browser_context("check-1")
    assert "t1" in context and "123456" not in context
