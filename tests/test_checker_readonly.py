"""The final checker is read-only, enforced by code, not its prompt (V3, W4). A click, send, write or shell is refused
even if the checker is told to do it; it returns unknown instead."""

import asyncio
from dataclasses import replace
from pathlib import Path
from types import SimpleNamespace
from typing import Any

from ferb.act.browser.ext_bridge import Router
from ferb.act.browser.tools import FERB_BROWSER, OWNER_BROWSER, BrowserFind, BrowserNavigate, BrowserOpen, Family
from ferb.core.types import Identity, TabRef
from ferb.daemon.auth import RunGrant
from ferb.daemon.gateway import observes
from ferb.orchestrator.tree import TaskRecord
from ferb.registry import (Registry, add_context_readers, add_evidence, add_identity_browser, add_mac,
                           add_owner_browsers, add_recording, add_shell, add_time, add_trail, build)
from ferb.trace.compose import compose, walk
from tests.conftest import FakeCap, RunningDaemon, gateway_request, gateway_setup, gateway_tool_call

# Every installed capability a checker may use (W4: snapshots, reads, screenshots, scrolling, opening a URL in FERB's
# own browser). Anything else, including a capability added later and left unmarked, is unavailable to a checker.
OBSERVATION = {
    "abilities.list", "abilities.read", "about.read",
    "actions.search", "chat.history", "memory.read", "task.evidence", "trail.days", "trail.query", "secrets.list",
    "fs.list", "fs.read", "fs.disk_usage", "recording.inspect", "shell.list", "shell.read", "shell.rules", "shell.wait",
    "mac.apps", "mac.screenshot", "mac.snapshot",
    "browser.downloads", "browser.find", "browser.html", "browser.point", "browser.profiles", "browser.read", "browser.screenshot",
    "browser.scroll", "browser.snapshot", "browser.tabs", "browser.wait",
    "ferb_browser.downloads", "ferb_browser.find", "ferb_browser.html", "ferb_browser.navigate", "ferb_browser.open", "ferb_browser.point",
    "ferb_browser.read", "ferb_browser.screenshot", "ferb_browser.scroll", "ferb_browser.snapshot", "ferb_browser.wait",
}


def _every_capability() -> Registry:
    """The full installed registry, built with stand-in drivers (construction only stores them)."""
    r = build()
    add_recording(r, object())
    add_identity_browser(r, object(), recordings=object())  # type: ignore[arg-type]
    add_owner_browsers(r, object(), recordings=object())    # type: ignore[arg-type]
    add_mac(r, object(), lambda: None, recordings=object(), sandbox=object())  # type: ignore[arg-type]
    add_shell(r, object(), object())                        # type: ignore[arg-type]
    add_trail(r, lambda: None)
    add_evidence(r, object(), lambda task_id: None)
    add_context_readers(r, object(), lambda task_id: None, lambda: None)
    add_time(r, lambda: None, lambda: None, lambda: None, lambda task_id: None)
    from ferb.identity.secret_tools import SecretsList
    r.add(SecretsList(lambda: Path('/nonexistent')))       # the daemon adds it beside the context readers (L36)
    from ferb.daemon.about import AboutRead
    r.add(AboutRead(lambda: None))                          # the daemon adds it too (DMN-6: read-only)
    return r


def _verifier_grant(daemon: RunningDaemon, *caps: Any) -> RunGrant:
    grant, _ = gateway_setup(daemon, {"probe"}, None, list(caps))
    return daemon.run(_observe(daemon, grant))      # type: ignore[no-any-return]


async def _observe(daemon: RunningDaemon, grant: RunGrant) -> RunGrant:
    # Re-issue the same grant as observe-only, the way the verifier's token is issued.
    tokens = daemon.daemon.tokens
    return tokens.validate(tokens.issue(grant.task_id, grant.worker_id, set(grant.scopes), grant.identity, ttl_s=600,
                                        observe_only=True))


# decision: DMN-504
def test_observation_tools_are_offered_acting_tools_are_not(daemon: RunningDaemon) -> None:
    read = FakeCap("probe.read", "read", {"probe"})
    read.observation = True                                 # type: ignore[attr-defined]
    send = FakeCap("probe.send", "send", {"probe"})
    grant = _verifier_grant(daemon, read, send)
    names = {t["name"] for t in daemon.daemon.gateway.tools_for(grant)}
    assert "probe.read" in names and "probe.send" not in names


# decision: DMN-504
def test_the_checker_cannot_act_even_when_asked(daemon: RunningDaemon) -> None:
    send = FakeCap("probe.send", "send", {"probe"})        # observation defaults False
    grant = _verifier_grant(daemon, send)
    r = gateway_tool_call(daemon, grant, "probe.send", to="x@y.com")
    assert r["is_error"] and "read-only" in r["content"]["error"].lower()
    assert "unknown" in r["content"]["error"]               # what the checker reports instead
    assert send.calls == 0                                  # the capability never ran


# decision: DMN-504
def test_an_observation_tool_runs_for_the_checker(daemon: RunningDaemon) -> None:
    read = FakeCap("probe.read", "read", {"probe"}, content={"seen": "the sent message"})
    read.observation = True                                 # type: ignore[attr-defined]
    grant = _verifier_grant(daemon, read)
    r = gateway_tool_call(daemon, grant, "probe.read")
    assert not r["is_error"] and r["content"]["seen"] == "the sent message" and read.calls == 1


# decision: DMN-504
def test_a_normal_worker_still_acts(daemon: RunningDaemon) -> None:
    send = FakeCap("probe.send", "send", {"probe"})
    grant, _ = gateway_setup(daemon, {"probe"}, gateway_request("send it to x@y.com"), [send])   # not observe-only
    assert not grant.observe_only
    r = gateway_tool_call(daemon, grant, "probe.send", to="x@y.com")
    assert not r["is_error"] and send.calls == 1


# decision: DMN-504
def test_only_reading_capabilities_are_marked_observation() -> None:
    """The checker's whole tool set, pinned: a change to it is a conscious change to this list. Opening an app
    (mac.open), a tab in the owner's profile (browser.open) or pressing a menu item (mac.menu) is a side effect."""
    caps = _every_capability().capabilities
    marked = {name for name, cap in caps.items() if observes(cap)}
    assert marked == OBSERVATION
    assert not {"mac.open", "browser.open", "browser.navigate", "mac.menu", "browser.dialog", "owner.ask",
                "ferb_browser.otp_candidates"} & marked
    assert all(getattr(caps[name], "reversible", True) for name in marked)   # none carries an effect annotation


# ---------- browser tabs: the checker reads the worker's tabs where they are (V3, W4) ----------

class TabBridge:
    """A paired browser for a Router: two of the owner's tabs; navigation is recorded."""

    def __init__(self) -> None:
        self.binding = SimpleNamespace(browser="brave", folder="Default", identity=Identity.OWNER)
        self.conn, self.device_id, self.gen, self.name = object(), "dev", 1, "brave/Default"
        self.calls: list[tuple[str, dict[str, Any]]] = []

    async def call(self, op: str, args: dict[str, Any], timeout: float = 30, cid: str | None = None) -> Any:
        self.calls.append((op, args))
        if op == "list_tabs":
            return {"tabs": [{"tab_id": 11, "host": "a.example", "url": "https://a.example/kept", "active": True},
                             {"tab_id": 12, "host": "b.example", "url": "https://b.example/", "active": False}]}
        if op == "navigate":
            return {"url": args["url"]}
        return {}


def _browser_checker(daemon: RunningDaemon, family: Family, *verbs: type) -> tuple[Router, TabBridge, RunGrant,
                                                                                    RunGrant]:
    """A worker (task-1) and its registered checker (task-1.verify, observe-only), over one Router of tabs."""
    d = daemon.daemon
    br = TabBridge()
    hub = SimpleNamespace(bridges={("brave", "Default"): br}, bridge=lambda b, f: br)
    r = Router(hub, resolver=SimpleNamespace(), profiles=lambda: [])   # type: ignore[arg-type]
    r.inspection_parent = d._inspection_parent
    caps = [verb(r, family) for verb in verbs]
    worker, _ = gateway_setup(daemon, {family.read}, None, caps, identity=family.identity)
    rec = d.tasks.get(worker.task_id)
    check = replace(rec.task, id=f"{worker.task_id}.verify", parent_id=worker.task_id, role="verifier")

    async def register() -> RunGrant:
        d.tasks.add(TaskRecord(check, d.clock.now(), rec.span_id, rec.tree_id))
        return d.tokens.validate(d.tokens.issue(check.id, "w2", set(check.scopes), check.identity, ttl_s=600,
                                                observe_only=True))
    return r, br, worker, daemon.run(register())


# decision: DMN-504
def test_the_checker_is_not_offered_navigation_of_the_owners_tabs(daemon: RunningDaemon) -> None:
    """The owner's borrowed or kept-open tab stays where it is: a checker can't load another URL in it (a signed-in
    request as the owner, e.g. an unsubscribe link). Its own browser's navigate is offered."""
    r, br, worker, checker = _browser_checker(daemon, OWNER_BROWSER, BrowserNavigate, BrowserFind)
    tab = asyncio.run(r.attach("brave:Default:11", worker.task_id))
    offered = {t["name"] for t in daemon.daemon.gateway.tools_for(checker)}
    assert "browser.find" in offered and "browser.navigate" not in offered
    got = gateway_tool_call(daemon, checker, "browser.navigate", tab=tab.tab_id, url="https://a.example/unsubscribe")
    assert got["is_error"] and "read-only" in got["content"]["error"]
    assert r.tab_url(tab.tab_id) == "https://a.example/kept" and "navigate" not in [op for op, _ in br.calls]
    found = gateway_tool_call(daemon, checker, "browser.find", tab=tab.tab_id, text="kept")
    assert "read-only" not in str(found["content"])            # reading it where it is stays open to the checker
    assert observes(BrowserNavigate(r, FERB_BROWSER))


# decision: DMN-504
def test_the_checker_navigates_only_ferb_tabs_it_opened(daemon: RunningDaemon) -> None:
    """The worker's FERB tabs keep their state for a retry (or the owner, after a handoff); the checker loads URLs
    only in tabs it opened. A normal worker still navigates its own tabs."""
    r, br, worker, checker = _browser_checker(daemon, FERB_BROWSER, BrowserNavigate)
    workers = asyncio.run(r.attach("brave:Default:11", worker.task_id))
    mine = asyncio.run(r.attach("brave:Default:12", checker.task_id))
    refused = gateway_tool_call(daemon, checker, "ferb_browser.navigate", tab=workers.tab_id, url="https://c.example/")
    assert refused["is_error"] and "this check opened" in refused["content"]["error"]
    assert r.tab_url(workers.tab_id) == "https://a.example/kept" and "navigate" not in [op for op, _ in br.calls]
    looked = gateway_tool_call(daemon, checker, "ferb_browser.navigate", tab=mine.tab_id, url="https://c.example/")
    assert not looked["is_error"] and r.tab_url(mine.tab_id) == "https://c.example/"
    acted = gateway_tool_call(daemon, worker, "ferb_browser.navigate", tab=workers.tab_id, url="https://d.example/")
    assert not acted["is_error"] and r.tab_url(workers.tab_id) == "https://d.example/"


# decision: DMN-504
def test_the_checker_never_opens_a_window_that_may_switch_spaces(daemon: RunningDaemon) -> None:
    """Rule 17: allow_foreground is ignored for a checker, whatever it was told; a normal worker keeps it."""
    opened: list[bool] = []

    class Driver:
        async def open_for(self, url: str, owner: str, profile: str | None = None, *, window_id: int | None = None,
                           allow_foreground: bool = False) -> TabRef:
            opened.append(allow_foreground)
            return TabRef("dev", "brave", "FERB", 1, f"t{len(opened)}")

        def tab_url(self, tab_id: str) -> str:
            return "https://a.example/"

    cap = BrowserOpen(Driver(), FERB_BROWSER)                # type: ignore[arg-type]
    worker, _ = gateway_setup(daemon, {FERB_BROWSER.read}, None, [cap], identity=Identity.FERB)
    checker = daemon.run(_observe(daemon, worker))
    assert not gateway_tool_call(daemon, checker, "ferb_browser.open", url="https://a.example/",
                                 allow_foreground=True)["is_error"]
    assert not gateway_tool_call(daemon, worker, "ferb_browser.open", url="https://a.example/",
                                 allow_foreground=True)["is_error"]
    assert opened == [False, True]


# ---------- a refused call shows in the trace ----------

async def _tree(daemon: RunningDaemon, task_id: str) -> list[dict[str, Any]]:
    return daemon.daemon.spans.tree_json(task_id)


# decision: DMN-504
def test_a_refused_call_is_recorded_for_the_trace(daemon: RunningDaemon) -> None:
    """A checker's acting call, an unknown tool and a tool outside the worker's scopes are each a hallucination or
    injection signal: a small tool.refused span (tool, worker, reason), and a line in the story."""
    send = FakeCap("probe.send", "send", {"probe"})
    other = FakeCap("other.send", "send", {"other"})
    grant = _verifier_grant(daemon, send, other)
    gateway_tool_call(daemon, grant, "probe.send", to="x@y.com")
    gateway_tool_call(daemon, grant, "no.such_tool")
    gateway_tool_call(daemon, grant, "other.send")
    tree = daemon.run(_tree(daemon, grant.task_id))
    spans = [n for n in walk(tree) if n["kind"] == "tool.refused"]
    assert [(n["input"]["tool"], n["input"]["worker_id"]) for n in spans] == [
        ("probe.send", "w1"), ("no.such_tool", "w1"), ("other.send", "w1")]
    assert ["read-only" in n["error"] for n in spans] == [True, False, False]
    assert spans[1]["error"] != spans[2]["error"] and all(n["ended_at"] for n in spans)
    assert not [n for n in walk(tree) if n["kind"] == "tool.call"]
    story = compose({"task_id": grant.task_id, "spans": tree, "outcome": {}})
    assert "⚠ 3 tool calls refused" in story["problems"]
