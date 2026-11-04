"""The owner's open tabs, and scrolling that isn't a loop.
- the owner's open tabs can be listed and taken over; FERB lets go of an attached tab and never closes it;
- the loop guard stops a cycle only when its results repeat too;
"""

import asyncio
from dataclasses import replace
from types import SimpleNamespace
from typing import Any

import pytest

from ferb.act.browser.ext_bridge import Hub, Router
from ferb.core.errors import Refused, StaleRef
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
    hub = SimpleNamespace(bridges={("brave", "Default"): br}, bridge=lambda b, f: br, publish=lambda *a: None)
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


def test_the_tabs_tool_says_which_tab_plays_sound_and_keys_each_tab_by_its_ref() -> None:
    """L31: a loop watches whether the owner's tab plays sound through browser_tabs (`tabs_by_ref.<ref>.audible`),
    cheaper than a snapshot. A tab_ref holds colons and a profile folder's space, never a dot, so the probe's dot path
    reaches it. An older extension sends no sound facts: they stay absent (never a guessed false)."""
    from ferb.act.browser.tools import OWNER_BROWSER, BrowserTabs
    from ferb.core.types import ToolContext
    from ferb.orchestrator.probes import MISSING, field_value
    r, br = router()
    br.binding = SimpleNamespace(browser="chrome", folder="Profile 1", identity=Identity.OWNER)

    async def call(op: str, args: dict[str, Any], timeout: float = 30, cid: str | None = None) -> Any:
        assert op == "list_tabs"
        return {"tabs": [{"tab_id": 11, "host": "www.instagram.com", "url": "https://www.instagram.com/reels/x/",
                          "title": "Reel", "active": False, "audible": True, "muted": False},
                         {"tab_id": 12, "host": "mail.google.com", "url": "https://mail.google.com/mail/",
                          "title": "Inbox", "active": True}]}
    br.call = call  # type: ignore[method-assign]
    tool = BrowserTabs(r, OWNER_BROWSER)
    ctx = ToolContext("task", "w", "r", Identity.OWNER, frozenset({"browser:read"}), "device", None, "/tmp", (), ())
    seen = asyncio.run(tool.run(tool.Input(), ctx)).content
    assert set(seen) == {"tabs"}                 # review M5: the model reads one list; only a probe's read gets the map
    out = asyncio.run(tool.run(tool.Input(), replace(ctx, probe=True))).content
    reel, inbox = out["tabs"]
    assert out["tabs"] == seen["tabs"]
    assert reel["tab_ref"] == "chrome:Profile 1:11" and (reel["audible"], reel["muted"]) == (True, False)
    assert "audible" not in inbox and "muted" not in inbox
    assert out["tabs_by_ref"] == {"chrome:Profile 1:11": reel, "chrome:Profile 1:12": inbox}
    assert field_value(out, "tabs_by_ref.chrome:Profile 1:11.audible") is True
    assert field_value(out, "tabs_by_ref.chrome:Profile 1:12.audible") is MISSING


def test_handoff_preserves_task_tab_and_cleanup_releases_it_without_closing() -> None:
    """L31, live cc5debe4 (2026-10-25): keep_open used to let go of the owner's borrowed tab at once; the extension's
    detach turned focus emulation off and Instagram paused 0.3 s later, while the task (a loop) still ran. A borrowed
    tab is marked kept and stays held until its task ends; the release then keeps it open."""
    r, br = router()
    ref = asyncio.run(r.attach("brave:Default:11", "task-1"))
    kept = asyncio.run(r.keep_open(ref))
    assert kept['kept_open'] and not kept['released'] and kept['borrowed']
    assert "release_tab" not in br.calls                           # still held: focus emulation stays on
    assert r.owned_by(ref.tab_id, 'task-1') == ref  # checker can still inspect before cleanup
    result = asyncio.run(r.release('task-1'))
    assert result[0]['kept_open'] and result[0]['released'] and not result[0]['closed']
    assert br.calls.count("release_tab") == 1 and 'close' not in br.calls and ref.tab_id not in r._tabs


def test_keep_open_on_a_tab_ferb_opened_still_hands_it_over_at_once() -> None:
    """Only the owner's borrowed tab is held on: a tab FERB opened is kept open and let go of at once, as before (the
    extension remembers it kept before it detaches, so a lost answer never closes it)."""
    r, br = router()
    ref = asyncio.run(r.attach("brave:Default:11", "task-1"))
    r._tabs[ref.tab_id].attached = False                           # as a tab FERB opened itself
    br.unhold = lambda: None  # type: ignore[attr-defined]
    kept = asyncio.run(r.keep_open(ref))
    assert kept['kept_open'] and kept['released'] and not kept['borrowed']
    assert br.calls.count("release_tab") == 1


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
            return {"tabs": [{"tab_id": 11, "host": "www.course.example", "url": "https://www.course.example/problems/42",
                              "title": "Problem 42", "active": True},
                             {"tab_id": 12, "host": "mail.google.com", "url": "https://mail.google.com/mail/",
                              "title": "Inbox", "active": False}]}
        return {}
    br.call = moved                                                                  # type: ignore[method-assign]
    held = asyncio.run(r.open_tabs(task="task-1"))[0]
    assert held["taken_over_as"] == ref.tab_id and held["title"] == "Problem 42"
    assert held["taken_over_from"] == {"url": "https://www.youtube.com/watch?v=fixture", "title": "Lesson"}
    assert r.task_tabs("task-1") == [{"tab": ref.tab_id, "tab_ref": "brave:Default:11",
                                      "taken_over_from": {"url": "https://www.youtube.com/watch?v=fixture",
                                                          "title": "Lesson"}}]


TITLE = "123456 is your Instagram code - sam@example.com - Gmail"


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


def test_a_held_tab_the_owner_closes_is_marked_closed() -> None:
    """Re-review 2 (R5): the inventory says the owner closed a held tab (the extension's removed event), so a task
    never reports it as missing or acts on it; reopening it is a new tab."""
    r, br = router()
    hub = SimpleNamespace(drivers=[r])
    ref = asyncio.run(r.attach("brave:Default:11", "task-1"))
    published: list[tuple[str, Any]] = []
    r.hub.publish = lambda topic, data: published.append((topic, data))           # type: ignore[attr-defined]
    Hub.tab_removed(hub, br, 11)                                                    # type: ignore[arg-type]
    assert r.task_tabs("task-1") == [{"tab": ref.tab_id, "closed": "the owner closed this tab",
                                      "tab_ref": "brave:Default:11",
                                      "taken_over_from": {"url": "https://www.youtube.com/watch?v=fixture",
                                                          "title": "Lesson"}}]
    with pytest.raises(StaleRef):
        r._tab(ref)
    assert published and published[0][0] == "browser.owner_stopped"              # HAND-201: for step 8
    assert r.task_tab(br, 11) is None                                       # no downloads are held for it
    br.calls.clear()
    assert asyncio.run(r.release("task-1"))[0]["closed"] is True
    assert br.calls == [] and r.task_tabs("task-1") == []                     # nothing to ask the browser to let go of


def test_keep_open_on_the_owners_tab_confirms_it_is_still_open() -> None:
    """Review M9: holding the owner's tab asked the browser nothing, so keep_open said "kept open" even for a tab the
    owner had just closed. It checks the browser still has the tab, and says so plainly when it hasn't."""
    from ferb.core.errors import DriverError
    r, br = router()
    ref = asyncio.run(r.attach("brave:Default:11", "task-1"))
    br.calls.clear()
    kept = asyncio.run(r.keep_open(ref))
    assert kept["kept_open"] and br.calls == ["list_tabs"]
    gone = asyncio.run(r.attach("brave:Default:12", "task-1"))

    async def without_12(op: str, args: dict[str, Any], timeout: float = 30, cid: str | None = None) -> Any:
        br.calls.append(op)
        return {"tabs": [{"tab_id": 11, "host": "www.youtube.com", "url": "https://www.youtube.com/", "title": "L"}]}
    br.call = without_12                                                             # type: ignore[method-assign]
    with pytest.raises(DriverError, match="isn't open"):
        asyncio.run(r.keep_open(gone))


@pytest.mark.parametrize("version, sent", [("0.16.0", True)])
def test_a_resting_owner_tab_gives_its_file_picker_back_only_on_an_extension_that_knows_how(version: str,
                                                                                           sent: bool) -> None:
    """Re-review N2: `rest` is new in extension 0.15.10; protocol 18's minimum (0.16.0) is past it, so every
    extension FERB accepts is sent it (the version gate went with the bump). Only the owner's borrowed tabs rest."""
    r, br = router()
    br.version = version                                                             # type: ignore[attr-defined]
    owned = asyncio.run(r.attach("brave:Default:11", "task-1"))
    r._tabs[owned.tab_id].attached = True
    br.calls.clear()
    sent_args: list[dict[str, Any]] = []
    original = br.call

    async def record(op: str, args: dict[str, Any], timeout: float = 30, cid: str | None = None) -> Any:
        if op == "rest":
            sent_args.append(args)
            return {"tab_id": args["tab_id"], "resting": args["resting"], "file_chooser_intercepted": not args["resting"]}
        return await original(op, args, timeout, cid)
    br.call = record                                                                 # type: ignore[method-assign]
    asyncio.run(r.rest("task-1", True))
    asyncio.run(r.rest("task-1", False))
    asyncio.run(r.rest("task-2", True))                                              # another task: nothing of its own
    assert sent_args == ([{"tab_id": 11, "resting": True}, {"tab_id": 11, "resting": False}] if sent else [])


@pytest.mark.parametrize("replies, raised, calls", [
    ([{"tab_id": 11, "resting": False, "file_chooser_intercepted": True}], False, 1),
    ([TimeoutError, {"tab_id": 11, "resting": False, "file_chooser_intercepted": True}], False, 2),    # retried once
    ([{"tab_id": 11, "resting": True}, {"tab_id": 11, "resting": False, "held": False}], False, 2),   # no session: ok
    ([TimeoutError, {"tab_id": 11, "resting": True, "file_chooser_intercepted": False}], True, 2),    # not confirmed
])
def test_taking_the_held_tab_back_for_a_round_is_confirmed_or_retried_once(replies: list[Any], raised: bool,
                                                                           calls: int) -> None:
    """Re-review fix round 3: a round's rest(false) that failed (a timeout under load) or whose reply didn't confirm
    the interception was swallowed. It is retried once and its reply checked; still unconfirmed, it says so (the
    supervisor journals it, and the extension also takes the picker back on the round's first acting command)."""
    from ferb.core.errors import DriverError
    r, br = router()
    br.version = "0.15.10"                                                           # type: ignore[attr-defined]
    asyncio.run(r.attach("brave:Default:11", "task-1"))
    sent: list[dict[str, Any]] = []
    queue = list(replies)

    async def answer(op: str, args: dict[str, Any], timeout: float = 30, cid: str | None = None) -> Any:
        assert op == "rest"
        sent.append(args)
        reply = queue.pop(0)
        if reply is TimeoutError:
            raise DriverError("rest timed out")
        return reply
    br.call = answer                                                                 # type: ignore[method-assign]
    if raised:
        with pytest.raises(DriverError, match="file picker"):
            asyncio.run(r.rest("task-1", False))
    else:
        asyncio.run(r.rest("task-1", False))
    assert len(sent) == calls and all(a == {"tab_id": 11, "resting": False} for a in sent)


# decision: HAND-419
def test_a_tab_the_cleanup_couldnt_let_go_of_is_named_plainly_for_the_owner() -> None:
    """HAND-419 (live 2026-11-04: "I couldn't tidy up everything after this task: DriverError: browser cleanup
    incomplete: t5: release_tab: timeout; outcome unknown"): the owner's line names the tab by browser, profile and
    title and says what to do; the raw error stays in the trace's record only."""
    from ferb.act.browser.ext_bridge import OutcomeUnknown
    from ferb.core.errors import DriverError
    from ferb.orchestrator.leftovers import leftover_lines
    r, br = router()
    ref = asyncio.run(r.attach("brave:Default:11", "task-1"))
    plain = br.call
    async def times_out(op: str, args: dict[str, Any], **kwargs: Any) -> Any:
        if op == "release_tab":
            raise OutcomeUnknown("release_tab: timeout; outcome unknown")
        return await plain(op, args, **kwargs)
    br.call = times_out                                                     # type: ignore[method-assign]
    with pytest.raises(DriverError) as failed:
        asyncio.run(r.release("task-1"))
    assert "release_tab: timeout" in str(failed.value)                      # the trace keeps the raw error
    record = {"resource": "browser", "released": False, "error": f"DriverError: {failed.value}",
              "tabs": failed.value.facts["tabs"]}
    [line] = leftover_lines([record], [record["error"]])
    assert "Brave “Work”, Lesson" in line and "still open" in line and ref.tab_id not in line
    assert "DriverError" not in line and "timeout" not in line and "release_tab" not in line
    opened = dict(record["tabs"][0], borrowed=False, title="History")      # a tab FERB opened itself
    [line] = leftover_lines([dict(record, tabs=[opened])], [record["error"]])
    assert line == "I couldn't close one tab I opened: Brave “Work”, History. Close it if it's still open."
    # a cleanup that stopped before any record came back still gets a plain line, never the raw text
    assert leftover_lines([], ["tree.worker: cleanup cancelled; resource outcome is unknown"]) == [
        "I couldn't finish tidying up after this task. The details are in the task's record."]
