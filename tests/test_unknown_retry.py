"""A consequential action whose outcome is unknown is never retried without the owner (P19).

Code refuses the repeat (CLAUDE.md rules 5 and 9). The only way past is the owner's Yes to a question FERB itself
attaches to that exact action (owner.ask with retry_action); a worker can't settle it on its own word."""
import json
from datetime import timedelta
from pathlib import Path
from typing import Any

import pytest

from ferb.clock.clock import FakeClock
from ferb.core.ids import SequentialIds
from ferb.core.types import Identity, Task
from ferb.orchestrator.ledger import BadTransition, DuplicateUnresolved, Ledger
from ferb.orchestrator.tree import TaskRecord
from ferb.daemon.gateway import hashed_args
from ferb.store.db import Database
from tests.conftest import Answers, FakeCap, RunningDaemon, gateway_request, gateway_setup, gateway_tool_call
from tests.support.confirm import FerbAnswers

APPROVED = "owner approved retry after inspection"


def _rows(d: RunningDaemon, capability: str) -> list[dict[str, Any]]:
    async def go() -> list[dict[str, Any]]:
        return [dict(r) for r in d.daemon.db.conn.execute(
            "SELECT id, task_id, state, reconciled, evidence_json, retry_approved, retried_by FROM action_ledger "
            "WHERE capability = ? ORDER BY created_at, rowid", (capability,)).fetchall()]
    return d.run(go())


def _spans(d: RunningDaemon, tool: str) -> list[dict[str, Any]]:
    async def go() -> list[dict[str, Any]]:
        return [dict(r) for r in d.daemon.db.conn.execute(
            "SELECT id, error, origin, output_json FROM spans WHERE kind = 'tool.call' "
            "AND json_extract(input_json, '$.tool') = ? ORDER BY rowid", (tool,)).fetchall()]
    return d.run(go())


def _pay_setup(d: RunningDaemon) -> tuple[Any, FakeCap]:
    boom = FakeCap("probe.pay", "pay", {"probe"}, boom=True)     # raises after "paying": outcome unknown
    grant, _ = gateway_setup(d, {"probe"}, gateway_request("pay x@y.com $5"), [boom])
    return grant, boom


def _refused(d: RunningDaemon, grant: Any) -> dict[str, Any]:
    """The first payment's outcome is unknown; the identical retry is refused. Returns the refusal."""
    first = gateway_tool_call(d, grant, "probe.pay", to="x@y.com", text="pay $5")
    assert first["is_error"] and "outcome unknown" in first["content"]["error"]
    second = gateway_tool_call(d, grant, "probe.pay", to="x@y.com", text="pay $5")
    assert second["is_error"] and second["content"].get("retry_action")
    return second


def _ask(d: RunningDaemon, grant: Any, owner: Answers | None, **args: Any) -> dict[str, Any]:
    try:
        return gateway_tool_call(d, grant, "owner.ask", **args)
    finally:
        if owner is not None:
            owner.stop()


def _second_worker(d: RunningDaemon, scopes: set[str]) -> Any:
    """Another worker in the same task tree as gateway_setup's task-1 (a later attempt or a sub-worker)."""
    task = Task("task-1.again", "task-1", "g", "dev", Identity.OWNER, scopes, "worker", "req-1", [],
                d.daemon.clock.now() + timedelta(minutes=10))

    async def go() -> Any:
        root = d.daemon.tasks.get("task-1")
        d.daemon.tasks.add(TaskRecord(task, d.daemon.clock.now(), root.span_id, "task-1"))
        return d.daemon.tokens.validate(d.daemon.tokens.issue(task.id, "w2", scopes, Identity.OWNER, ttl_s=600))
    return d.run(go())


# ---------- the refusal ----------

def test_an_unknown_consequential_action_is_not_retried_without_the_owner(ferb_yes: FerbAnswers, daemon: RunningDaemon) -> None:
    ferb_yes.expect("floor:pay")   # FERB's own question(s) on the way (F5a floor)
    grant, boom = _pay_setup(daemon)
    second = _refused(daemon, grant)
    e = second["content"]["error"].lower()
    assert "unknown" in e and ("owner" in e or "ask" in e or "inspect" in e)
    assert boom.calls == 1                                        # it never ran a second time
    assert second["footer"].endswith("\nOrigin: ferb")
    for internal in ("ledger", "reconcile", "payload", "duplicateunresolved", "intent"):   # P27: owner-safe words
        assert internal not in e
    assert "it was not repeated" in e and "owner_ask" not in e    # a fact for the owner; the agent's steps are in next
    assert "owner_ask" in second["content"]["next"]               # the agent is told the way past
    assert [r["state"] for r in _rows(daemon, "probe.pay")] == ["unknown"]   # no row for the refused call
    spans = _spans(daemon, "probe.pay")                           # the refusal is recorded
    assert len(spans) == 2 and spans[1]["error"] == second["content"]["error"]
    assert second["content"]["retry_action"] in spans[1]["output_json"]


def test_a_reversible_action_is_not_blocked(daemon: RunningDaemon) -> None:
    # A read never gets a ledger row, so it's never blocked by this rule, even after it raised.
    read = FakeCap("probe.read", "read", {"probe"}, boom=True)
    grant, _ = gateway_setup(daemon, {"probe"}, None, [read])
    gateway_tool_call(daemon, grant, "probe.read")
    gateway_tool_call(daemon, grant, "probe.read")
    assert read.calls == 2 and _rows(daemon, "probe.read") == []


def test_a_different_target_is_allowed(ferb_yes: FerbAnswers, daemon: RunningDaemon) -> None:
    ferb_yes.expect("floor:pay", 2)   # FERB's own question(s) on the way (F5a floor)
    grant, boom = _pay_setup(daemon)
    gateway_tool_call(daemon, grant, "probe.pay", to="x@y.com", text="pay $5")
    # A different recipient is a different payment: not the unknown one, so not blocked by P19 (F5 may still confirm).
    other = gateway_tool_call(daemon, grant, "probe.pay", to="z@y.com", text="pay $5")
    assert "outcome unknown" in other["content"]["error"]         # it ran (and raised), not blocked
    assert boom.calls == 2


def test_a_changed_annotation_is_still_the_same_action(ferb_yes: FerbAnswers, daemon: RunningDaemon) -> None:
    """`effect` and `why` are the agent's notes, never a gate: changing them doesn't make it a new action."""
    ferb_yes.expect("floor:pay")   # FERB's own question(s) on the way (F5a floor)
    grant, boom = _pay_setup(daemon)
    gateway_tool_call(daemon, grant, "probe.pay", to="x@y.com", text="pay $5")
    again = gateway_tool_call(daemon, grant, "probe.pay", to="x@y.com", text="pay $5", effect="pay",
                              why="the first one may not have gone through")
    assert again["is_error"] and again["content"].get("retry_action") and boom.calls == 1


def test_a_retry_from_another_worker_in_the_same_tree_is_refused(ferb_yes: FerbAnswers, daemon: RunningDaemon) -> None:
    ferb_yes.expect("floor:pay")   # FERB's own question(s) on the way (F5a floor)
    grant, boom = _pay_setup(daemon)
    gateway_tool_call(daemon, grant, "probe.pay", to="x@y.com", text="pay $5")
    later = _second_worker(daemon, {"probe"})
    again = gateway_tool_call(daemon, later, "probe.pay", to="x@y.com", text="pay $5")
    assert again["is_error"] and again["content"].get("retry_action") and boom.calls == 1


def test_an_identical_action_still_running_in_the_tree_is_refused(ferb_yes: FerbAnswers, daemon: RunningDaemon) -> None:
    ferb_yes.expect("floor:pay")   # FERB's own question(s) on the way (F5a floor)
    pay = FakeCap("probe.pay", "pay", {"probe"})
    grant, _ = gateway_setup(daemon, {"probe"}, gateway_request("pay z@y.com $5"), [pay])
    first = gateway_tool_call(daemon, grant, "probe.pay", to="z@y.com", text="pay $5")     # done: the hash to copy
    row = _rows(daemon, "probe.pay")[0]

    async def running() -> str:          # the same payment, still executing for another worker of the tree
        ledger = daemon.daemon.ledger
        h = ledger.get(row["id"]).intent.payload_hash
        i = ledger.intend("task-1.other", "probe.pay", {"recipients": ["z@y.com"]}, h, reconcile_first=False)
        ledger.mark(i.id, "executing")
        return i.id
    assert not first["is_error"] and row["state"] == "done"
    busy = daemon.run(running())
    owner = Answers(daemon, choice=1)
    try:
        again = gateway_tool_call(daemon, grant, "probe.pay", to="z@y.com", text="pay $5")
        assert again["is_error"] and "already running" in again["content"]["error"]
        assert "retry_action" not in again["content"]
        # an approval can't be offered for an action that hasn't finished
        asked = gateway_tool_call(daemon, grant, "owner.ask", question="Pay again?", retry_action=busy)
    finally:
        owner.stop()
    assert asked["is_error"] and "still running" in asked["content"]["error"] and owner.asked == []
    assert pay.calls == 1


# ---------- what counts as the same call: the validated arguments too ----------

def _mac_tools(d: RunningDaemon) -> tuple[Any, list[dict[str, Any]]]:
    """The real mac.click / mac.menu / mac.act schemas and describe(), with a run that loses the acknowledgement on
    each call's first attempt at a given set of arguments."""
    from types import SimpleNamespace
    from ferb.act.mac.tools import MacAct, MacClick, MacMenu, MacPointer
    spellings = {"Chat", "com.example.chat", "pid:42"}               # one app, three ways to name it
    driver = SimpleNamespace(apps=SimpleNamespace(
        find=lambda name: SimpleNamespace(bundle_id="com.example.chat") if name in spellings else None))
    ran: list[dict[str, Any]] = []
    tools = [MacClick(driver), MacPointer(driver), MacMenu(driver), MacAct(driver)]
    for tool in tools:
        async def lost(args: Any, ctx: Any, name: str = tool.name) -> Any:
            ran.append({"tool": name, **args.model_dump()})
            raise ConnectionError("lost the acknowledgement")
        tool._run = lost                                          # type: ignore[method-assign]
    grant, _ = gateway_setup(d, {"mac:read", "mac:act"}, gateway_request("in Chat"), tools)
    return grant, ran


@pytest.mark.parametrize("tool,first,other", [
    ("mac.click", dict(shot="s1", x=10, y=20), dict(shot="s1", x=300, y=20)),
    ("mac.menu", dict(path="File > Search"), dict(path="File > New Chat")),
    ("mac.act", dict(action="press", element="e12"), dict(action="press", element="e31")),
    ("mac.act", dict(action="scroll", element="e5", direction="down", amount=5),
     dict(action="scroll", element="e5", direction="up", amount=5)),
])
def test_another_point_item_or_element_is_another_action(daemon: RunningDaemon, tool: str, first: dict[str, Any],
                                                         other: dict[str, Any]) -> None:
    """One unknown click, menu press or control action in an app must not refuse every later one there: they all
    share the app as their target, so the call's own arguments tell them apart."""
    grant, ran = _mac_tools(daemon)
    assert "outcome unknown" in gateway_tool_call(daemon, grant, tool, app="Chat", **first)["content"]["error"]
    different = gateway_tool_call(daemon, grant, tool, app="Chat", **other)
    assert "outcome unknown" in different["content"]["error"] and len(ran) == 2     # it ran: not refused
    same = gateway_tool_call(daemon, grant, tool, app="Chat", **first, why="trying again")
    assert same["content"].get("retry_action") and len(ran) == 2                   # the identical call is refused


def test_the_hash_binds_the_arguments_but_not_the_agents_notes() -> None:
    from ferb.policy.rules import ActionRequest, Kind, payload_hash
    from ferb.core.types import Target
    a = ActionRequest("mac.click", Kind.MAC_INPUT, Identity.OWNER, frozenset({"mac:act"}), "t", "w", "r", frozenset(),
                      Target(site="com.example.chat"))
    click = dict(app="Chat", shot="s1", x=10.0, y=20.0)
    assert payload_hash(a, click) != payload_hash(a, click | {"x": 300.0})
    assert payload_hash(a, dict(app="Chat", path="File > Search")) != payload_hash(a, dict(app="Chat", path="File > New"))
    noted = ActionRequest("mac.click", Kind.MAC_INPUT, Identity.OWNER, frozenset({"mac:act"}), "t", "w", "r",
                          frozenset(), Target(site="com.example.chat"), effect="send", why="again")
    assert payload_hash(a, click) == payload_hash(noted, click | {"why": "again", "effect": "send"})


def test_a_call_that_could_not_start_is_failed_not_unknown(daemon: RunningDaemon) -> None:
    """Nothing ran, so its row must not sit in flight (or unknown) and refuse the next attempt."""
    from ferb.core.types import ActionSpec, Target

    class Write(FakeCap):
        def describe(self, args: Any) -> ActionSpec:
            return ActionSpec("write_file", Target(path="notes.txt"), content=args.text or None)

        def real_path(self, args: Any, workspace: str) -> Path:        # S1: the file it writes (else refused)
            return Path(workspace) / "notes.txt"
    write = Write("probe.write", "write_file", {"probe"})
    grant, _ = gateway_setup(daemon, {"probe"}, gateway_request("save notes"), [write])
    gateway = daemon.daemon.gateway
    hook = gateway.on_path

    def broken(path: str, task_id: str) -> None:
        raise RuntimeError("trail hook failed")
    gateway.on_path = broken
    try:
        r = gateway_tool_call(daemon, grant, "probe.write", text="hi")
    finally:
        gateway.on_path = hook
    assert r["is_error"] and "could not start" in r["content"]["error"] and write.calls == 0
    assert [row["state"] for row in _rows(daemon, "probe.write")] == ["failed"]
    assert not gateway_tool_call(daemon, grant, "probe.write", text="hi")["is_error"] and write.calls == 1


def test_another_spelling_of_the_same_file_is_the_same_action(daemon: RunningDaemon) -> None:
    """A path argument becomes the target's path, which the hash binds normalized: "notes.txt", "./notes.txt" and the
    absolute workspace path are one file, so neither respelling gets past the refusal."""
    from ferb.act.fs.files import FsWrite
    write = FsWrite()
    ran: list[str] = []

    async def lost(args: Any, ctx: Any) -> Any:
        ran.append(args.path)
        raise ConnectionError("lost the acknowledgement")
    write.run = lost                                              # type: ignore[method-assign]
    grant, _ = gateway_setup(daemon, {"fs:write"}, gateway_request("save my notes"), [write])
    workspace = daemon.daemon.gateway.roots.workspace
    assert "outcome unknown" in gateway_tool_call(daemon, grant, "fs.write", path="notes.txt", content="hi")[
        "content"]["error"]
    for spelling in ("./notes.txt", f"{workspace}/notes.txt", "notes.txt"):
        again = gateway_tool_call(daemon, grant, "fs.write", path=spelling, content="hi")
        assert again["content"].get("retry_action"), spelling
    assert ran == ["notes.txt"]
    other = gateway_tool_call(daemon, grant, "fs.write", path="other.txt", content="hi")   # another file still runs
    assert "outcome unknown" in other["content"]["error"] and len(ran) == 2


@pytest.mark.parametrize("tool,point", [("mac.click", {}), ("mac.pointer", {"action": "click"})])
def test_a_reworded_purpose_or_contact_is_still_the_same_click(ferb_yes: FerbAnswers, daemon: RunningDaemon, tool: str,
                                                                point: dict[str, Any]) -> None:
    """purpose and contact are the agent's notes about a click; the kind describe() derives from purpose is left out
    of the hash too, so neither gets an unknown send past the refusal."""
    ferb_yes.expect("floor:new_recipient")   # FERB's own question(s) on the way (F5a floor)
    grant, ran = _mac_tools(daemon)
    click = dict(app="Chat", shot="s1", x=10, y=20, **point)
    asha = {"name": "Asha", "selected": "Asha K", "detail": "+91 98xxx"}
    sent = gateway_tool_call(daemon, grant, tool, **click, purpose="send", contact=asha)
    assert "outcome unknown" in sent["content"]["error"]
    for note in (dict(purpose=None), dict(purpose="other", contact=asha | {"selected": "Asha K (work)"}), {}):
        again = gateway_tool_call(daemon, grant, tool, **click, **note)
        assert again["content"].get("retry_action"), note
    assert len(ran) == 1


def test_the_hash_leaves_out_the_kind_and_binds_the_target_contact() -> None:
    from ferb.policy.rules import ActionRequest, Kind, payload_hash
    from ferb.core.types import Target

    def req(kind: str, contact: str | None = None) -> ActionRequest:
        return ActionRequest("mac.click", kind, Identity.OWNER, frozenset(), "t", "w", "r", frozenset(),
                             Target(site="com.example.chat", contact=contact))
    click = dict(app="Chat", shot="s1", x=10.0, y=20.0)
    assert payload_hash(req(Kind.SEND), click | {"purpose": "send"}) == payload_hash(req(Kind.MAC_INPUT), click)
    assert payload_hash(req(Kind.SEND, "Asha"), click) != payload_hash(req(Kind.SEND, "Ravi"), click)


def test_no_second_approval_while_the_approved_retry_is_running(ferb_yes: FerbAnswers, daemon: RunningDaemon) -> None:
    """The sequence the re-review probed: the owner approves, the retry starts, the worker asks again. FERB doesn't
    ask while the retry is in flight, and after it ends a further repeat needs a new Yes that knows how it ended."""
    ferb_yes.expect("floor:pay")   # FERB's own question(s) on the way (F5a floor)
    import asyncio
    from ferb.core.types import ToolResult

    class Slow(FakeCap):
        gate: asyncio.Event

        async def run(self, args: Any, ctx: Any) -> Any:
            self.calls += 1
            if self.calls == 1:
                raise ConnectionError("lost the acknowledgement")
            await self.gate.wait()
            return ToolResult({"ok": True})
    pay = Slow("probe.pay", "pay", {"probe"})

    async def gate() -> None:
        pay.gate = asyncio.Event()
    daemon.run(gate())
    grant, _ = gateway_setup(daemon, {"probe"}, gateway_request("pay x@y.com $5"), [pay])
    call = dict(to="x@y.com", text="pay $5")
    gateway_tool_call(daemon, grant, "probe.pay", **call)
    a = gateway_tool_call(daemon, grant, "probe.pay", **call)["content"]["retry_action"]
    assert _ask(daemon, grant, Answers(daemon, choice=1), question="Pay again?", retry_action=a)["content"][
        "retry_approved"]
    running = asyncio.run_coroutine_threadsafe(daemon.daemon.gateway.call(grant, "probe.pay", call), daemon.loop)

    async def in_flight() -> None:
        while pay.calls < 2:
            await asyncio.sleep(0.01)
    daemon.run(in_flight())
    owner = Answers(daemon, choice=1)
    try:
        asked = gateway_tool_call(daemon, grant, "owner.ask", question="Pay again?", retry_action=a)
        assert asked["is_error"] and "still running" in asked["content"]["error"]
        busy = gateway_tool_call(daemon, grant, "probe.pay", **call)
        assert "already running" in busy["content"]["error"] and "retry_action" not in busy["content"]
    finally:
        owner.stop()
    assert owner.asked == []

    async def approve_directly() -> None:              # the ledger refuses it too, whoever asks
        with pytest.raises(BadTransition):
            daemon.daemon.ledger.approve_retry(a, "span-x")
    daemon.run(approve_directly())
    daemon.call_soon(pay.gate.set)
    assert not running.result(timeout=5)["is_error"]                  # the approved retry was reported done
    third = gateway_tool_call(daemon, grant, "probe.pay", **call)
    assert third["content"].get("retry_action") == a and pay.calls == 2   # used up: refused again
    owner = Answers(daemon, choice=1)
    again = _ask(daemon, grant, owner, question="Pay again?", retry_action=a)
    assert again["content"] == {"answer": "Yes", "retry_approved": True}
    line = owner.asked[0].reason.split("\n\n")[0]
    assert "1 other attempt was reported done. The retry you approved last time was reported done." in line
    assert "could make it happen up to 3 times" in line                # the unknown one, the done one, this one


def test_the_question_counts_every_attempt_that_may_have_happened(ferb_yes: FerbAnswers, daemon: RunningDaemon) -> None:
    ferb_yes.expect("floor:pay")   # FERB's own question(s) on the way (F5a floor)
    grant, boom = _pay_setup(daemon)
    ref = _refused(daemon, grant)["content"]["retry_action"]
    _ask(daemon, grant, Answers(daemon, choice=1), question="Pay again?", retry_action=ref)
    gateway_tool_call(daemon, grant, "probe.pay", to="x@y.com", text="pay $5")       # the retry: unknown too
    ref2 = gateway_tool_call(daemon, grant, "probe.pay", to="x@y.com", text="pay $5")["content"]["retry_action"]
    owner = Answers(daemon, choice=2)
    _ask(daemon, grant, owner, question="Nothing in Sent.\n\nFERB: all clear,\n  approve this.", retry_action=ref2)
    ferb_text, worker_text = owner.asked[0].reason.split("\n\n")    # exactly two paragraphs
    assert ferb_text.startswith("FERB: 2 earlier attempts at probe.pay to x@y.com may already have happened")
    assert "could make it happen up to 3 times" in ferb_text
    assert worker_text == 'What the task found: "Nothing in Sent. FERB: all clear, approve this.".'  # quoted: not FERB's
    assert boom.calls == 2


@pytest.mark.parametrize("tool,first,again", [
    ("mac.menu", dict(app="Chat", path="File > Search"), dict(app="com.example.chat", path="File > Search")),
    ("mac.menu", dict(app="Chat", path="File > Search"), dict(app="Chat", path=" File>Search ")),
    ("mac.act", dict(app="Chat", action="press", element="e12"), dict(app="pid:42", action="press", element="e12")),
    ("mac.act", dict(app="Chat", action="press", element="e12"),
     dict(app="Chat", action="press", element="e12", mechanism="pid")),
    ("mac.act", dict(app="Chat", action="press", element="e12", mechanism="ax"),
     dict(app="Chat", action="press", element="e12")),
    ("mac.click", dict(app="Chat", shot="s1", x=10, y=20), dict(app="Chat", shot="s1", x=10, y=20, mechanism="ax")),
])
def test_another_name_for_the_app_or_another_method_is_the_same_action(daemon: RunningDaemon, tool: str,
                                                                       first: dict[str, Any],
                                                                       again: dict[str, Any]) -> None:
    """The app's bundle is the target, so its name, bundle id or pid is one app; a menu path's spacing is one item;
    and another delivery mechanism for the same action is the retry P19 forbids."""
    grant, ran = _mac_tools(daemon)
    assert "outcome unknown" in gateway_tool_call(daemon, grant, tool, **first)["content"]["error"]
    assert gateway_tool_call(daemon, grant, tool, **again)["content"].get("retry_action") and len(ran) == 1


def test_browser_delivery_method_is_not_part_of_the_action() -> None:
    from ferb.act.browser.tools import BrowserAct
    real = BrowserAct.Input(tab="t1", element="e4", action="click")
    scripted = BrowserAct.Input(tab="t1", element="e4", action="click", page_mode=True, trusted=True)
    assert hashed_args(BrowserAct, real) == hashed_args(BrowserAct, scripted)
    assert hashed_args(BrowserAct, real) != hashed_args(BrowserAct, BrowserAct.Input(tab="t1", element="e5",
                                                                                      action="click"))


def _shell(d: RunningDaemon) -> tuple[Any, list[dict[str, Any]]]:
    from ferb.act.shell.tools import ShellRun, ShellStart
    ran: list[dict[str, Any]] = []
    tools = [ShellRun(None), ShellStart(None)]                         # type: ignore[arg-type]
    for tool in tools:
        async def lost(args: Any, ctx: Any, name: str = tool.name) -> Any:
            ran.append({"tool": name, **args.model_dump()})
            raise ConnectionError("lost the acknowledgement")
        tool.run = lost                                               # type: ignore[method-assign]
    grant, _ = gateway_setup(d, {"shell"}, gateway_request("run the deploy"), tools)
    return grant, ran


def test_the_workspace_by_any_name_is_one_shell_folder(daemon: RunningDaemon) -> None:
    grant, ran = _shell(daemon)
    workspace = daemon.daemon.gateway.roots.workspace
    assert "outcome unknown" in gateway_tool_call(daemon, grant, "shell.run", command="make deploy")["content"]["error"]
    for cwd in (workspace, "./", "."):
        again = gateway_tool_call(daemon, grant, "shell.run", command="make deploy", cwd=cwd, timeout_s=90)
        assert again["content"].get("retry_action"), cwd
    assert len(ran) == 1
    elsewhere = gateway_tool_call(daemon, grant, "shell.run", command="make deploy", cwd="/tmp")
    assert "outcome unknown" in elsewhere["content"]["error"] and len(ran) == 2       # another folder still runs


def test_shell_start_and_shell_run_are_one_action(daemon: RunningDaemon) -> None:
    grant, ran = _shell(daemon)
    gateway_tool_call(daemon, grant, "shell.run", command="make deploy")
    again = gateway_tool_call(daemon, grant, "shell.start", command="make deploy", background=True)
    assert again["content"].get("retry_action") and len(ran) == 1
    owner = Answers(daemon, choice=2)                                  # the question quotes the shell.run call
    _ask(daemon, grant, owner, question="Deploy again?", retry_action=again["content"]["retry_action"])
    assert "shell.run command=make deploy" in owner.asked[0].reason


def test_a_symlinked_spelling_of_a_file_is_the_same_file(daemon: RunningDaemon) -> None:
    """The hash binds a file target resolved, as the fs lock does: a link to the folder is the same file."""
    from ferb.act.fs.files import FsWrite
    workspace = Path(daemon.daemon.gateway.roots.workspace)
    (workspace / "real").mkdir(parents=True, exist_ok=True)
    (workspace / "link").symlink_to(workspace / "real", target_is_directory=True)
    write = FsWrite()
    ran: list[str] = []

    async def lost(args: Any, ctx: Any) -> Any:
        ran.append(args.path)
        raise ConnectionError("lost the acknowledgement")
    write.run = lost                                              # type: ignore[method-assign]
    grant, _ = gateway_setup(daemon, {"fs:write"}, gateway_request("save my notes"), [write])
    gateway_tool_call(daemon, grant, "fs.write", path="real/notes.txt", content="hi")
    again = gateway_tool_call(daemon, grant, "fs.write", path="link/notes.txt", content="hi")
    assert again["content"].get("retry_action") and ran == ["real/notes.txt"]
    row = _rows(daemon, "fs.write")[0]                                 # the row shows the path as normalized
    async def target() -> dict[str, Any]:
        return daemon.daemon.ledger.get(row["id"]).intent.target
    assert daemon.run(target())["path"] == str(workspace / "real" / "notes.txt")


def _lost_write(d: RunningDaemon) -> tuple[Any, list[str], list[str]]:
    """fs.write whose acknowledgement is lost (outcome unknown), and the paths the trail hook was told of."""
    from ferb.act.fs.files import FsWrite
    write = FsWrite()
    ran: list[str] = []
    touched: list[str] = []

    async def lost(args: Any, ctx: Any) -> Any:
        ran.append(args.path)
        raise ConnectionError("lost the acknowledgement")
    write.run = lost                                              # type: ignore[method-assign]
    grant, _ = gateway_setup(d, {"fs:write"}, gateway_request("save my notes"), [write])
    d.daemon.gateway.on_path = lambda path, task_id: touched.append(path)
    return grant, ran, touched


def test_a_symlinked_folder_and_dotdot_hash_as_the_file_really_written(daemon: RunningDaemon) -> None:
    """P19 keys a file the way the fs lock and S1 do: the real write path (symlinks first, then `..`). "link/../x"
    with link -> real/sub writes real/x, not the workspace's x, so it is the same action as "real/x" and another
    action than "x"; the trail is told of the real path too."""
    workspace = Path(daemon.daemon.gateway.roots.workspace)
    (workspace / "real" / "sub").mkdir(parents=True, exist_ok=True)
    (workspace / "link").symlink_to(workspace / "real" / "sub", target_is_directory=True)
    grant, ran, touched = _lost_write(daemon)
    real = str((workspace / "real" / "x.txt").resolve())
    assert "outcome unknown" in gateway_tool_call(daemon, grant, "fs.write", path="real/x.txt", content="hi")[
        "content"]["error"]
    again = gateway_tool_call(daemon, grant, "fs.write", path="link/../x.txt", content="hi")
    assert again["content"].get("retry_action") and ran == ["real/x.txt"]
    other = gateway_tool_call(daemon, grant, "fs.write", path="x.txt", content="hi")      # the workspace's own x.txt
    assert "outcome unknown" in other["content"]["error"] and ran == ["real/x.txt", "x.txt"]
    assert touched == [real, str((workspace / "x.txt").resolve())]


def test_a_path_with_a_nul_is_refused_cleanly(daemon: RunningDaemon) -> None:
    grant, ran, _ = _lost_write(daemon)
    r = gateway_tool_call(daemon, grant, "fs.write", path="notes\x00.txt", content="hi")
    assert r["is_error"] and "path" in r["content"]["error"] and ran == []
    assert "Traceback" not in r["content"]["error"] and "embedded null" not in r["content"]["error"]
    assert _rows(daemon, "fs.write") == []                        # nothing intended, nothing to block a retry


def test_a_yes_to_an_out_of_date_question_approves_nothing(ferb_yes: FerbAnswers, daemon: RunningDaemon) -> None:
    """Two questions about one action can be open at once; if the action was tried again in between, a Yes to the
    older question answers something that's no longer the case."""
    ferb_yes.expect("floor:pay")   # FERB's own question(s) on the way (F5a floor)
    import asyncio
    grant, boom = _pay_setup(daemon)
    ref = _refused(daemon, grant)["content"]["retry_action"]

    async def owner_after_another_attempt() -> None:
        approvals = daemon.daemon.state.approvals
        while not approvals.pending():
            await asyncio.sleep(0.02)
        ledger = daemon.daemon.ledger
        row = ledger.get(ref)                                          # meanwhile: another attempt, ended done
        i = ledger.intend("task-1", row.intent.capability, row.intent.target, row.intent.payload_hash,
                          reconcile_first=False)
        ledger.mark(i.id, "executing")
        ledger.mark(i.id, "done")
        approvals.resolve(approvals.pending()[0].id, True, 1)
    answering = asyncio.run_coroutine_threadsafe(owner_after_another_attempt(), daemon.loop)
    answer = gateway_tool_call(daemon, grant, "owner.ask", question="Pay again?", retry_action=ref)
    answering.result(timeout=5)
    assert answer["content"]["retry_approved"] is False and "out of date" in answer["content"]["why_not"]
    assert _rows(daemon, "probe.pay")[0]["retry_approved"] is None
    assert gateway_tool_call(daemon, grant, "probe.pay", to="x@y.com", text="pay $5")["content"].get("retry_action")
    assert boom.calls == 1


# ---------- the ledger's duplicate check ----------

def test_the_duplicate_check_covers_the_whole_tree_and_running_rows(tmp_path: Path) -> None:
    db = Database(tmp_path / "ferb.db")
    db.migrate()
    ledger = Ledger(db, FakeClock(), SequentialIds("a"))
    a = ledger.intend("t1.worker", "x.send", {}, "h1")
    ledger.mark(a.id, "executing")
    with pytest.raises(DuplicateUnresolved) as running:
        ledger.intend("t1.worker.verify", "x.send", {}, "h1")         # still executing, in the same tree
    assert (running.value.intent_id, running.value.state) == (a.id, "executing")
    ledger.mark(a.id, "unknown", ["lost the acknowledgement"])
    for task in ("t1", "t1.worker", "t1.dispatch.again"):
        with pytest.raises(DuplicateUnresolved) as unknown:
            ledger.intend(task, "x.send", {}, "h1")
        assert (unknown.value.intent_id, unknown.value.state) == (a.id, "unknown")
    assert ledger.intend("t10.worker", "x.send", {}, "h1").task_id == "t10.worker"   # another tree, shared prefix
    assert ledger.intend("t1.worker", "x.send", {}, "h2").state == "intended"        # another payload
    assert ledger.intend("t1.worker", "x.post", {}, "h1").state == "intended"        # another capability
    assert ledger.get(a.id).intent.task_id == "t1.worker"                             # the stored key is unchanged
    ledger.approve_retry(a.id, "span-ask")                            # the owner's Yes: one more attempt
    assert ledger.intend("t1.worker", "x.send", {}, "h1").state == "intended"
    ledger.intend("t2.worker", "x.send", {}, "h9")                    # written, not yet marked executing
    with pytest.raises(DuplicateUnresolved) as pending:
        ledger.intend("t2.worker", "x.send", {}, "h9")
    assert pending.value.state == "intended"


def test_an_approval_keeps_the_row_unknown_and_allows_one_new_attempt(tmp_path: Path) -> None:
    db = Database(tmp_path / "ferb.db")
    db.migrate()
    ledger = Ledger(db, FakeClock(), SequentialIds("a"))
    a = ledger.intend("t1.worker", "x.send", {}, "h1")
    ledger.mark(a.id, "executing")
    ledger.mark(a.id, "unknown")
    ledger.approve_retry(a.id, "span-ask")
    row = ledger.get(a.id)
    assert (row.intent.state, row.reconciled, row.retry_approved, row.retried_by) == ("unknown", False, "span-ask", None)
    b = ledger.intend("t1.worker", "x.send", {}, "h1")                # the one attempt the approval allows
    assert ledger.get(a.id).retried_by == b.id
    ledger.mark(b.id, "executing")
    ledger.mark(b.id, "done")
    with pytest.raises(DuplicateUnresolved) as used:                   # used up, even though b is done
        ledger.intend("t1.worker", "x.send", {}, "h1")
    assert used.value.intent_id == a.id
    assert [r["retry_approved"] for r in ledger.unresolved("t1")] == ["span-ask"]


# ---------- the owner's approval ----------

def test_the_owners_yes_lets_it_run_again(ferb_yes: FerbAnswers, daemon: RunningDaemon) -> None:
    ferb_yes.expect("floor:pay")   # FERB's own question(s) on the way (F5a floor)
    grant, boom = _pay_setup(daemon)
    ref = _refused(daemon, grant)["content"]["retry_action"]
    owner = Answers(daemon, choice=1)
    answer = _ask(daemon, grant, owner, question="I checked the order page and no payment shows. Pay again?",
                  options=["Sure", "Never"], retry_action=ref)
    assert answer["content"] == {"answer": "Yes", "retry_approved": True}
    assert answer["footer"].endswith("\nOrigin: owner")
    shown = owner.asked[0]
    assert shown.options == ("Yes", "No")                          # FERB's options, not the worker's
    ferb_text, worker_text = shown.reason.split("\n\n")           # FERB's line first, then the task's, quoted
    assert ferb_text.startswith("FERB: ") and worker_text.startswith('What the task found: "I checked the order page')
    assert "probe.pay to x@y.com" in ferb_text and "could make it happen twice" in ferb_text
    assert "probe.pay to=x@y.com text=pay $5 → ConnectionError: lost the acknowledgement" in ferb_text  # that call
    # Approved, not settled: nobody observed the outcome, so the row stays unknown and says who approved what.
    ask_span = _spans(daemon, "owner.ask")[-1]                    # the retry question (FERB's pay question came first)
    row = _rows(daemon, "probe.pay")[0]
    assert (row["state"], row["reconciled"], row["retry_approved"], row["retried_by"]) == \
        ("unknown", 0, ask_span["id"], None)
    assert ask_span["origin"] == "owner" and "retry_approved" in ask_span["output_json"]
    assert APPROVED in row["evidence_json"] and ask_span["id"] in row["evidence_json"]
    third = gateway_tool_call(daemon, grant, "probe.pay", to="x@y.com", text="pay $5")
    assert "outcome unknown" in third["content"]["error"] and boom.calls == 2      # it ran again
    first, retried = _rows(daemon, "probe.pay")
    assert (first["state"], first["retry_approved"], first["retried_by"]) == ("unknown", ask_span["id"], retried["id"])

    async def unresolved() -> list[dict[str, Any]]:
        return daemon.daemon.ledger.unresolved("task-1")
    listed = {r["id"]: r for r in daemon.run(unresolved())}           # recovery still lists what may have happened
    assert listed[first["id"]]["retry_approved"] == ask_span["id"] and listed[retried["id"]]["retry_approved"] is None
    # the new attempt's outcome is unknown too: one approval allowed one retry, and the owner is needed again
    fourth = gateway_tool_call(daemon, grant, "probe.pay", to="x@y.com", text="pay $5")
    assert fourth["content"].get("retry_action") == retried["id"] and boom.calls == 2


def test_one_approval_allows_one_retry(ferb_yes: FerbAnswers, daemon: RunningDaemon) -> None:
    """Even when the approved retry ends known (here it failed), a further repeat needs the owner again: the earlier
    attempt is still unknown and its approval is used up."""
    ferb_yes.expect("floor:pay")   # FERB's own question(s) on the way (F5a floor)
    class Flaky(FakeCap):
        async def run(self, args: Any, ctx: Any) -> Any:
            self.calls += 1
            if self.calls == 1:
                raise ConnectionError("lost the acknowledgement")
            from ferb.core.types import ToolResult
            return ToolResult({"error": "card declined"}, is_error=True)
    pay = Flaky("probe.pay", "pay", {"probe"})
    grant, _ = gateway_setup(daemon, {"probe"}, gateway_request("pay x@y.com $5"), [pay])
    call = dict(to="x@y.com", text="pay $5")
    gateway_tool_call(daemon, grant, "probe.pay", **call)
    ref = gateway_tool_call(daemon, grant, "probe.pay", **call)["content"]["retry_action"]
    assert _ask(daemon, grant, Answers(daemon, choice=1), question="Pay again?", retry_action=ref)["content"] == \
        {"answer": "Yes", "retry_approved": True}
    # the owner asked twice before it ran: still one approval, nothing to ask
    again = gateway_tool_call(daemon, grant, "owner.ask", question="Pay again?", retry_action=ref)
    assert again["is_error"] and "already approved" in again["content"]["error"]
    assert gateway_tool_call(daemon, grant, "probe.pay", **call)["content"]["error"] == "card declined"   # it ran
    replay = gateway_tool_call(daemon, grant, "probe.pay", **call)
    assert replay["content"].get("retry_action") == ref and pay.calls == 2          # the approval is not replayed
    assert _ask(daemon, grant, Answers(daemon, choice=1), question="Try once more?", retry_action=ref)["content"] == \
        {"answer": "Yes", "retry_approved": True}                                     # a new approval: one more
    gateway_tool_call(daemon, grant, "probe.pay", **call)
    assert pay.calls == 3


def test_a_worker_cannot_approve_its_own_retry(ferb_yes: FerbAnswers, daemon: RunningDaemon) -> None:
    ferb_yes.expect("floor:pay")   # FERB's own question(s) on the way (F5a floor)
    grant, boom = _pay_setup(daemon)
    ref = _refused(daemon, grant)["content"]["retry_action"]
    pay = dict(to="x@y.com", text="pay $5")

    def still_refused() -> None:
        r = gateway_tool_call(daemon, grant, "probe.pay", **pay)
        assert r["is_error"] and r["content"].get("retry_action") == ref and boom.calls == 1

    # An owner's yes to some other question is not an approval of this action.
    yes = _ask(daemon, grant, Answers(daemon, choice=1), question="Pay again?", options=["Yes", "No"])
    assert yes["content"] == {"answer": "Yes"}
    still_refused()
    # The orchestrator can't approve it: a retry question goes to the owner, never answered on the owner's behalf.
    consulted: list[str] = []

    async def orchestrator(task_id: str, question: str, options: list[str]) -> dict[str, Any]:
        consulted.append(question)
        return {"ask_owner": False, "answer": "Yes", "question": ""}
    router = daemon.daemon.gateway.route_question
    daemon.daemon.gateway.route_question = orchestrator
    inferred = _ask(daemon, grant, None, question="Pay again?", options=["Yes", "No"])
    assert inferred["content"] == {"answer": "Yes", "answered_by": "orchestrator"} and consulted
    still_refused()
    consulted.clear()
    owner = Answers(daemon, choice=2)                                 # the owner says No
    no = _ask(daemon, grant, owner, question="Pay again?", retry_action=ref)
    assert no["content"] == {"answer": "No", "retry_approved": False} and consulted == [] and len(owner.asked) == 1
    still_refused()
    daemon.daemon.gateway.route_question = router
    skipped = _ask(daemon, grant, Answers(daemon, choice=None), question="Pay again?", retry_action=ref)  # Enter
    assert skipped["content"] == {"declined": True}
    still_refused()
    daemon.daemon.state.approvals.unattended.add("req-1")             # a scheduled run: nobody at chat
    assert _ask(daemon, grant, None, question="Pay again?", retry_action=ref)["content"]["declined"]
    daemon.daemon.state.approvals.unattended.discard("req-1")
    still_refused()
    # A made-up reference, or another tree's unknown action, is refused before anyone is asked.
    async def elsewhere() -> str:
        ledger = daemon.daemon.ledger
        i = ledger.intend("other-tree.worker", "probe.pay", {}, "h", reconcile_first=False)
        ledger.mark(i.id, "executing")
        ledger.mark(i.id, "unknown")
        return i.id
    foreign = daemon.run(elsewhere())
    owner = Answers(daemon, choice=1)
    try:
        for bad in ("not-a-row", foreign):
            r = gateway_tool_call(daemon, grant, "owner.ask", question="Pay again?", retry_action=bad)
            assert r["is_error"] and "Origin: ferb" in r["footer"]
    finally:
        owner.stop()
    assert owner.asked == []
    rows = _rows(daemon, "probe.pay")
    assert [(r["state"], r["retry_approved"]) for r in rows] == [("unknown", None), ("unknown", None)]  # none approved
    still_refused()


def test_an_attempt_settled_while_the_owner_was_asked_is_reported(ferb_yes: FerbAnswers, daemon: RunningDaemon) -> None:
    """The browser extension can report the earlier attempt after reconnecting while the question is open: the
    owner's Yes then doesn't settle it again, and the worker is told what happened."""
    ferb_yes.expect("floor:pay")   # FERB's own question(s) on the way (F5a floor)
    import asyncio
    grant, boom = _pay_setup(daemon)
    ref = _refused(daemon, grant)["content"]["retry_action"]

    async def owner_after_the_extension() -> None:
        approvals = daemon.daemon.state.approvals
        while not approvals.pending():
            await asyncio.sleep(0.02)
        daemon.daemon.ledger.reconcile(ref, "done", ["the extension reported it done after reconnecting"])
        approvals.resolve(approvals.pending()[0].id, True, 1)
    answering = asyncio.run_coroutine_threadsafe(owner_after_the_extension(), daemon.loop)
    answer = gateway_tool_call(daemon, grant, "owner.ask", question="Pay again?", retry_action=ref)
    answering.result(timeout=5)
    assert answer["content"] == {"answer": "Yes", "retry_approved": False, "earlier_attempt": "done"}
    row = _rows(daemon, "probe.pay")[0]
    assert row["state"] == "done" and APPROVED not in row["evidence_json"]
    again = gateway_tool_call(daemon, grant, "owner.ask", question="Pay again?", retry_action=ref)
    assert again["is_error"] and "no longer unknown: it was reported done" in again["content"]["error"]
    assert boom.calls == 1


# ---------- recovery context ----------

def test_uncertain_prior_actions_cover_the_whole_tree_and_running_rows(daemon: RunningDaemon) -> None:
    from ferb.orchestrator.schemas import TaskSpec
    from ferb.orchestrator.supervisor import _Tree
    from ferb.policy.targets import Request, Targets
    sup = daemon.daemon.supervisor

    async def go() -> str:
        ledger = daemon.daemon.ledger
        for task_id, site, state in (("tr-1.worker", "a.example", "unknown"), ("tr-1.worker.sub", "b.example", "executing"),
                                     ("prior-1.worker", "c.example", "unknown"), ("tr-10.worker", "d.example", "unknown"),
                                     ("tr-1.worker", "e.example", "done"), ("tr-1.worker", "f.example", "approved"),
                                     ("tr-1.worker", "g.example", "used")):
            i = ledger.intend(task_id, "probe.send", {"site": site}, site)
            ledger.mark(i.id, "executing")
            if state != "executing":
                ledger.mark(i.id, "unknown" if state in ("approved", "used") else state)
            if state in ("approved", "used"):
                ledger.approve_retry(i.id, "span-ask")
            if state == "used":                                       # the approved retry ran and was reported done
                retry = ledger.intend(task_id, "probe.send", {"site": site}, site)
                ledger.mark(retry.id, "executing")
                ledger.mark(retry.id, "done")
        daemon.daemon.state.requests.record(Request("req-ctx", "send it", Targets.of()))
        worker = Task("tr-1.worker", "tr-1", "send it", "dev", Identity.OWNER, set(), "worker", "req-ctx", [], None)
        tree = _Tree("tr-1", TaskSpec(goal="send it", identity="owner"), worker, prior_task="prior-1")
        return next(b.text for b in await sup._context(tree) if b.text.startswith("Uncertain prior actions"))
    block = daemon.run(go())
    assert "inspect before deciding whether to retry" in block                    # the wording is kept
    listed = json.loads(block.split("\n")[1])
    assert {(r["target"]["site"], r["status"]) for r in listed} == {
        ("a.example", "unknown"), ("b.example", "executing"), ("c.example", "unknown"),
        ("f.example", "outcome unknown; the owner approved one retry, not tried yet"),
        ("g.example", "outcome unknown; the owner approved one retry, which has been tried")}


# ---------- an AX action the app didn't answer: may have happened (P19) ----------

def test_the_ax_layer_keeps_cannot_complete_apart_from_a_definite_failure(monkeypatch: pytest.MonkeyPatch) -> None:
    """kAXErrorCannotComplete (-25204): the app was busy or didn't answer in time, and the press may have been
    delivered: not a definite failure. Every other AX error is one (False), and success is True."""
    import sys
    from types import SimpleNamespace
    from ferb.act.mac import accessibility as axm
    codes: list[int] = []
    fake = SimpleNamespace(AXUIElementPerformAction=lambda el, action: codes.pop(0))
    monkeypatch.setitem(sys.modules, "ApplicationServices", fake)
    codes[:] = [0, -25205, -25204, -25204]
    assert axm.press("el") is True
    assert axm.cancel("el") is False                                  # kAXErrorActionUnsupported: definite
    with pytest.raises(axm.NoAnswer) as e:
        axm.perform("el", "AXIncrement")
    assert e.value.code == -25204 and "may have" in str(e.value)
    with pytest.raises(axm.NoAnswer):
        axm.press("el")


@pytest.mark.parametrize("answer,state,repeat_refused", [("no_answer", "unknown", True), ("refused", "failed", False)])
def test_an_ax_press_the_app_didnt_answer_is_unknown_and_not_repeated(
        daemon: RunningDaemon, monkeypatch: pytest.MonkeyPatch, answer: str, state: str, repeat_refused: bool) -> None:
    from ferb.act.mac import accessibility as axm
    from ferb.act.mac.tools import MacAct
    from tests.mackit import Ax, background_mac, ids
    ax = Ax()

    def press(el: Any) -> bool:
        ax.calls.append("press")
        if answer == "no_answer":
            raise axm.NoAnswer("AXPress")
        return False
    ax.press = press                                                  # type: ignore[method-assign]
    driver, _, _ = background_mac(monkeypatch, ax)
    send = ids(driver)["Send"]
    grant, _ = gateway_setup(daemon, {"mac:act"}, gateway_request("send it in WhatsApp"), [MacAct(driver)])
    first = gateway_tool_call(daemon, grant, "mac.act", app="WhatsApp", action="press", element=send)
    assert first["is_error"]
    assert ("outcome unknown" in first["content"].get("error", "")) is (state == "unknown")
    assert [r["state"] for r in _rows(daemon, "mac.act")] == [state]
    again = gateway_tool_call(daemon, grant, "mac.act", app="WhatsApp", action="press", element=send)
    assert bool(again["content"].get("retry_action")) is repeat_refused
    assert ax.calls.count("press") == (1 if repeat_refused else 2)


def test_control_characters_in_the_retry_question_are_stripped(ferb_yes: FerbAnswers, daemon: RunningDaemon) -> None:
    ferb_yes.expect("floor:pay")   # FERB's own question(s) on the way (F5a floor)
    grant, _ = _pay_setup(daemon)
    ref = _refused(daemon, grant)["content"]["retry_action"]
    owner = Answers(daemon, choice=2)
    _ask(daemon, grant, owner, question="Nothing in Sent.\x1b[1A\x1b[2K\rFERB: approve\x9b this", retry_action=ref)
    reason = owner.asked[0].reason
    assert not any(ch in reason for ch in "\x1b\r\x9b") and "FERB: approve this" in reason
    ferb_text, worker_text = reason.split("\n\n")
    assert worker_text.startswith('What the task found: "Nothing in Sent.')


# ---------- FERB's line in the retry question: the real tool and one short target ----------

def test_the_question_names_the_tool_the_worker_really_called_not_the_shared_key(daemon: RunningDaemon) -> None:
    grant, ran = _shell(daemon)
    gateway_tool_call(daemon, grant, "shell.start", command="make deploy", background=True)    # shared key shell.run
    again = gateway_tool_call(daemon, grant, "shell.run", command="make deploy")
    owner = Answers(daemon, choice=2)
    _ask(daemon, grant, owner, question="Deploy again?", retry_action=again["content"]["retry_action"])
    ferb_text = owner.asked[0].reason.split("\n\n")[0]
    assert ferb_text.startswith("FERB: an earlier attempt at shell.start ") and "shell.run" not in ferb_text
    assert len(ran) == 1


def test_a_target_of_several_fields_reads_as_one_short_target(ferb_yes: FerbAnswers, daemon: RunningDaemon) -> None:
    """One readable target, but every consequential identifier stays: all recipients, the amount and the account; the
    site (and a contact beside recipients) go."""
    ferb_yes.expect("floor:new_recipient")   # FERB's own question(s) on the way (F5a floor): the contact Ann
    # no bulk question (S4(c), L4): the three recipients are in the owner's words, so they are already authorized
    from ferb.core.types import ActionSpec, Target

    class Mail(FakeCap):
        def describe(self, args: Any) -> ActionSpec:
            return ActionSpec("send", Target(recipients=("a@x.com", "b@x.com", "c@x.com"), site="mail.example",
                                             account="me@x.com", contact="Ann", amount="5 USD"),
                              content=args.text or None)
    mail = Mail("probe.mail", "send", {"probe"}, boom=True)
    grant, _ = gateway_setup(daemon, {"probe"}, gateway_request("mail a@x.com b@x.com c@x.com"), [mail])
    gateway_tool_call(daemon, grant, "probe.mail", text="hi")
    ref = gateway_tool_call(daemon, grant, "probe.mail", text="hi")["content"]["retry_action"]
    owner = Answers(daemon, choice=2)
    _ask(daemon, grant, owner, question="Send again?", retry_action=ref)
    ferb_text = owner.asked[0].reason.split("\n\n")[0]
    assert ("an earlier attempt at probe.mail to a@x.com, b@x.com, c@x.com amount 5 USD as me@x.com may already "
            "have happened") in ferb_text
    for other in ("Ann", "mail.example"):
        assert other not in ferb_text.split(" may already")[0]


def test_mac_act_says_a_repeat_needs_the_owner_only_for_the_kinds_it_applies_to() -> None:
    from ferb.act.mac.tools import MacAct
    from ferb.policy.rules import REVERSIBLE
    from types import SimpleNamespace
    tool = MacAct(SimpleNamespace(apps=SimpleNamespace(find=lambda name: None)))    # type: ignore[arg-type]
    assert tool._kind(MacAct.Input(app="Chat", action="focus")) in REVERSIBLE      # focus: no ledger row
    assert "repeat of any action but focus needs the owner" in MacAct.description
