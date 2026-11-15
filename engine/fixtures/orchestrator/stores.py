"""The store scenarios of make.py: each runs the Python modules on a migrated store in its own temporary FERB home.

A scenario is {"name", "seed": {table: dump}, "ids_from", "clock_from", "steps": [...], "tables": {table: dump}}: the rows set up
before its steps (written by the Python build: journal rows, spans, tasks...), the next sequential id when the steps
start (and the seconds the clock moved during the setup), each step's call with what it returned or raised, and the tables the steps wrote, at the end. A Rust test
inserts the seed rows, runs the same steps and compares every return and every row. A dump is {"columns", "rows"}
in rowid order, `rowid` first. Time is a FakeClock at 2026-01-01T09:00:00Z that moves only at an `advance` step.
"""

import asyncio
import json
import sqlite3
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Callable

from ferb.clock.clock import FakeClock
from ferb.core.ids import SequentialIds
from ferb.core.types import Identity, Task, ToolContext
from ferb.memory.journal import Journal
from ferb.orchestrator import (attention, cards, continuity, conversation_tools, conversations, context_layers,
                               episodes, evidence, ledger, loops, parts, plan_executor, plan_store, skills, test_runs,
                               tree, verify)
from ferb.orchestrator.plans import Part, Plan, Step
from ferb.orchestrator.probes import Probe
from ferb.orchestrator.schemas import Condition, TaskSpec
from ferb.sanitize.redact import Redactor
from ferb.store.db import Database
from ferb.store.spans import SpanStore

START = datetime(2026, 1, 1, 9, 0, tzinfo=timezone.utc)


class Scenario:
    def __init__(self, root: Path, name: str, fx: Callable[[Any], Any], call: Callable[..., dict[str, Any]],
                 seed_tables: tuple[str, ...], tables: tuple[str, ...]) -> None:
        self.name, self.fx, self.call = name, fx, call
        self.home = root / name
        self.db = Database(self.home / "ferb.db")
        self.db.migrate()
        self.clock = FakeClock(START)
        self.ids = SequentialIds("id")
        self.redactor = Redactor()
        self.journal = Journal(self.db, self.clock, self.ids, self.redactor)
        self.spans = SpanStore(self.db, self.clock, self.ids, self.redactor)
        self.seed_tables, self.tables = seed_tables, tables
        self.steps: list[dict[str, Any]] = []
        self.seed: dict[str, Any] = {}
        self.ids_from = 0
        self.clock_from = 0

    def seeded(self) -> None:
        """The setup is done: its rows are the seed; the steps start with the next id."""
        self.seed = {t: self.dump(t) for t in self.seed_tables}
        self.ids_from = self.ids._n
        self.clock_from = int((self.clock.now() - START).total_seconds())

    def dump(self, table: str) -> dict[str, Any]:
        cur = self.db.conn.execute(f"SELECT rowid AS rowid_, * FROM {table} ORDER BY rowid")
        cols = [d[0] for d in cur.description]
        return {"columns": cols, "rows": [list(r) for r in cur.fetchall()]}

    def step(self, op: str, args: list[Any], fn: Callable[..., Any], *a: Any, **k: Any) -> Any:
        got = self.call(fn, *a, **k)
        self.steps.append(self.fx({"op": op, "args": args, **got}))
        return got.get("out")

    def note(self, op: str, args: list[Any], out: Any = None) -> None:
        self.steps.append(self.fx({"op": op, "args": args, "out": out}))

    def advance(self, seconds: float) -> None:
        self.clock.advance(seconds)
        self.steps.append({"op": "advance", "args": [seconds], "out": None})

    def result(self) -> dict[str, Any]:
        return self.fx({"name": self.name, "seed": self.seed, "ids_from": self.ids_from,
                        "clock_from": self.clock_from, "steps": self.steps,
                        "tables": {t: self.dump(t) for t in self.tables}})


def next_id(ids: SequentialIds) -> int:
    return int(ids.new_id().rsplit("-", 1)[1])


def redact(value: Any) -> Any:
    return Redactor()(value)


# ---------------------------------------------------------------------------------------------------------------

def plan_store_scenario(sc: Scenario) -> None:
    sc.seeded()
    store = plan_store.PlanStore(sc.db, sc.redactor)
    plan = {"steps": [{"id": "a", "kind": "do", "goal": "Find it"}, {"id": "b", "kind": "do", "depends_on": ["a"]},
                      {"id": "w", "kind": "wait", "event": "song.ended", "event_match": [{"field": "tab", "value": "t1"}]}],
            "standing_sends": [{"capability": "browser.act", "identity": "owner", "site": "x.com", "content": None}]}
    p = Plan.model_validate(plan)
    sc.step("create", ["t1", plan, {"words": "find it é", "card": "4111 1111 1111 1111"}, "owner"], store.create, "t1", p,
            {"words": "find it é", "card": "4111 1111 1111 1111"}, "owner")
    sc.step("create", ["t1", plan, {"words": "again"}, None], store.create, "t1", p, {"words": "again"}, None)
    sc.step("steps", ["t1"], store.steps, "t1")
    sc.step("begin", ["t1", "a", "running"], store.begin, "t1", "a")
    sc.step("finish", ["t1", "a", {"status": "done", "summary": "found", "n": 1.5}], store.finish, "t1", "a",
            {"status": "done", "summary": "found", "n": 1.5})
    sc.step("begin", ["t1", "a", "running"], store.begin, "t1", "a")
    sc.step("finish", ["t1", "b", {"summary": "no status"}], store.finish, "t1", "b", {"summary": "no status"})
    sc.step("finish", ["t1", "w", {"status": "needs_user"}], store.finish, "t1", "w", {"status": "needs_user"})
    sc.step("paused", ["t1"], store.paused, "t1")
    sc.step("begin", ["t1", "w", "paused"], store.begin, "t1", "w", "paused")
    sc.step("paused", ["t1"], store.paused, "t1")
    sc.step("retry_unfinished", ["t1"], store.retry_unfinished, "t1")
    sc.step("set_status", ["t1", "suspended"], store.set_status, "t1", "suspended")
    sc.step("pending", [], store.pending)
    sc.step("reopen", ["t1"], store.reopen, "t1")
    sc.step("load", ["t1"], lambda: {**{k: v for k, v in store.load("t1").items() if k != "plan"},
                                       "plan": store.load("t1")["plan"].model_dump()})
    sc.step("load", ["nope"], store.load, "nope")
    for name, payload in [("song.ended", {"tab": "t2"}), ("song.ended", {"tab": "t1", "n": 1}),
                          ("song.ended", {"tab": True}), ("other", {"tab": "t1"})]:
        sc.step("event", [name, payload], store.event, name, payload)
    m = [{"field": "tab", "value": "t1"}]
    sc.step("event_after", ["song.ended", 0, m], store.event_after, "song.ended", 0, p.steps[2].event_match)
    sc.step("event_after", ["song.ended", 2, []], store.event_after, "song.ended", 2, [])
    sc.step("event_seq_after", ["song.ended", 0, [{"field": "tab", "value": "True"}]], store.event_seq_after,
            "song.ended", 0, [type(p.steps[2].event_match[0])(field="tab", value="True")])
    sc.step("event_cursor", [], store.event_cursor)
    new = {"steps": [{"id": "a", "kind": "do", "goal": "Find it"}, {"id": "c", "kind": "do", "depends_on": ["a"]}]}
    sc.step("replace", ["t1", new], store.replace, "t1", Plan.model_validate(new))
    sc.step("earlier", ["t1"], store.earlier, "t1")
    sc.step("update_plan", ["t1", new], store.update_plan, "t1", Plan.model_validate(new))
    sc.step("create_single", ["t2", "Do one thing", {"w": 1}, "owner", False], store.create_single, "t2",
            "Do one thing", {"w": 1}, "owner", restored=False)
    sc.step("finish", ["t2", "worker", {"status": "done"}], store.finish, "t2", "worker", {"status": "done"})
    sc.step("create", ["t2", new, {}, None], store.create, "t2", Plan.model_validate(new), {}, None)
    sc.step("finish", ["t2", "a", {"status": "done", "x": 1}], store.finish, "t2", "a", {"status": "done", "x": 1})
    sc.step("finish", ["t2", "c", {"status": "failed"}], store.finish, "t2", "c", {"status": "failed"})
    sc.step("create_single", ["t2", "Again", {}, None, False], store.create_single, "t2", "Again", {}, None,
            restored=False)
    sc.step("create_single", ["t2", "Restored", {}, None, True], store.create_single, "t2", "Restored", {}, None,
            restored=True)
    sc.step("steps", ["t2"], store.steps, "t2")
    sc.step("revisions", [], lambda: dict(store.revisions))


def ledger_scenario(sc: Scenario) -> None:
    sc.seeded()
    led = ledger.Ledger(sc.db, sc.clock, sc.ids)

    def intent(*a: Any, **k: Any) -> Any:
        i = led.intend(*a, **k)
        return [i.id, i.task_id, i.capability, i.target, i.payload_hash, i.state]

    def row(r: Any) -> Any:
        return [r.intent.id, r.intent.state, r.reconciled, r.evidence, r.retry_approved, r.retried_by]

    def refusal(*a: Any) -> Any:
        e = led.refusal(*a)
        return None if e is None else [str(e), e.intent_id, e.state]
    target = {"to": ["b@x.com"], "a": 1, "z": "é"}
    first = sc.step("intend", ["t1.worker", "mail.send", target, "h1", True], intent, "t1.worker", "mail.send", target, "h1")
    sc.step("intend", ["t1.other", "mail.send", target, "h1", True], intent, "t1.other", "mail.send", target, "h1")
    sc.step("refusal", ["t1.x", "mail.send", "h1"], refusal, "t1.x", "mail.send", "h1")
    sc.step("mark", [first[0], "executing", ["started"]], led.mark, first[0], "executing", ["started"])
    sc.step("mark", [first[0], "intended", []], led.mark, first[0], "intended")
    sc.step("mark_unknown_after_restart", [], led.mark_unknown_after_restart)
    sc.step("mark", [first[0], "done", []], led.mark, first[0], "done")
    sc.step("refusal", ["t1.worker", "mail.send", "h1"], refusal, "t1.worker", "mail.send", "h1")
    sc.step("approved_retry", ["t1.worker", "mail.send", "h1"], led.approved_retry, "t1.worker", "mail.send", "h1")
    sc.step("approve_retry", [first[0], "span-9"], led.approve_retry, first[0], "span-9")
    sc.step("approved_retry", ["t1.worker", "mail.send", "h1"], led.approved_retry, "t1.worker", "mail.send", "h1")
    second = sc.step("intend", ["t1.worker", "mail.send", target, "h1", True], intent, "t1.worker", "mail.send", target, "h1")
    sc.step("approve_retry", [first[0], "span-10"], led.approve_retry, first[0], "span-10")
    sc.step("intend", ["t1.worker", "mail.send", target, "h1", False], intent, "t1.worker", "mail.send", target, "h1",
            reconcile_first=False)
    sc.step("mark", [second[0], "executing", []], led.mark, second[0], "executing")
    sc.advance(5)
    sc.step("mark", [second[0], "failed", ["timeout"]], led.mark, second[0], "failed", ["timeout"])
    sc.step("reconcile", [first[0], "done", []], led.reconcile, first[0], "done", [])
    sc.step("reconcile", [second[0], "done", ["x"]], led.reconcile, second[0], "done", ["x"])
    sc.step("reconcile", [first[0], "done", ["span-11", "sent folder"]], led.reconcile, first[0], "done",
            ["span-11", "sent folder"])
    sc.step("attempts", [first[0]], lambda: [row(r) for r in led.attempts(first[0])])
    sc.step("get", ["missing"], led.get, "missing")
    sc.step("unresolved", ["t1"], led.unresolved, "t1")
    sc.step("intend", ["t2", "pay", {}, "h2", True], intent, "t2", "pay", {}, "h2")
    sc.step("unresolved", ["t2"], led.unresolved, "t2")
    sc.step("get", [first[0]], lambda: row(led.get(first[0])))


def tree_scenario(sc: Scenario) -> None:
    sc.seeded()
    t = tree.TaskTree()
    t.attach(sc.db, sc.clock.now)

    def add(tid: str, parent: str | None, request: str, goal: str, tree_id: str | None, status: str) -> None:
        task = Task(id=tid, parent_id=parent, goal=goal, device_id="mac", identity=Identity.OWNER,
                    scopes=frozenset({"fs:write", "browser:read", "é:x"}), role="worker", request_id=request,
                    success_conditions=[], deadline=None)
        t.add(tree.TaskRecord(task, sc.clock.now(), "span-1", tree_id), status)
    for args in [("t1", None, "r1", "Root é", None, "running"), ("t1.worker", "t1", "r1", "Work", "t1", "planned"),
                 ("t2", None, "r2", "Other", None, "paused"), ("t10", None, "r2", "Ten", None, "done"),
                 ("t1.worker", "t1", "r1", "Work again", "t1", "running")]:
        sc.step("add", list(args), add, *args)
        sc.advance(1)
    sc.step("tree_of", ["t1.worker"], t.tree_of, "t1.worker")
    t.remove("t1.worker")
    sc.note("remove", ["t1.worker"])
    sc.step("tree_of", ["t1.worker"], t.tree_of, "t1.worker")
    sc.step("tree_of", ["zzz"], t.tree_of, "zzz")
    sc.step("set_status", ["t1", "done", {"summary": "ok", "n": 1}, ""], t.set_status, "t1", "done", {"summary": "ok", "n": 1})
    sc.step("set_status", ["t2", "failed", None, "2026-02-02T00:00:00+00:00"], t.set_status, "t2", "failed", None,
            "2026-02-02T00:00:00+00:00")
    sc.step("set_identity", ["t1", "ferb"], t.set_identity, "t1", "ferb")
    sc.step("rows", ["t1"], t.rows, "t1")
    sc.step("column", ["t1", "goal"], t.column, "t1", "goal")
    sc.step("column", ["t1", "identity"], t.column, "t1", "identity")
    sc.step("column", ["nope", "goal"], t.column, "nope", "goal")
    sc.step("latest_root", [], t.latest_root)
    sc.step("recorded", ["t2"], t.recorded, "t2")
    sc.step("roots_with_prefix", ["t1"], t.roots_with_prefix, "t1")
    sc.step("trees_of_request", ["r2"], lambda: sorted(t.trees_of_request("r2")))
    sc.advance(60)
    sc.step("interrupt_running", [], t.interrupt_running)


def continuity_scenario(sc: Scenario) -> None:
    # setup: tasks, journal rows of two chats, a scheduled run, an older-build row for the upgrade
    for kind, data in [("chat.message", {"session_id": "owner", "request_id": "r1", "text": "find my file"}),
                       ("chat.reply", {"session_id": "owner", "request_id": "r1", "text": "Done."}),
                       ("chat.message", {"request_id": "r0", "text": "an old message", "untrusted_parts": 0}),
                       ("chat.notice", {"id": "n1", "text": "a notice"}),
                       ("chat.notice", {"id": "n2", "text": "to chat-b", "session_id": "chat-b"}),
                       ("chat.message", {"session_id": "chat-b", "request_id": "r2", "text": "other chat"}),
                       ("task.answer", {"task_id": "t1", "text": "the answer"}),
                       ("schedule.posted", {"text": "a schedule ran"}),
                       ("chat.part", {"task_id": "t1", "part": "p1", "text": "part"}),
                       ("chat.message", {"session_id": "schedule:s1", "request_id": "r3", "text": "run"}),
                       ("chat.clarify", {"session_id": "owner", "text": "Which of your files?"}),
                       ("chat.message", {"session_id": "owner", "request_id": "r9",
                                         "text": "yes <untrusted-data id=\"a1\">pasted</untrusted-data id=\"a1\"> go"})]:
        sc.journal.append(kind, data)
        sc.clock.advance(1)
    ttree = tree.TaskTree()
    ttree.attach(sc.db, sc.clock.now)
    for tid, request in [("t1", "r1"), ("t2", "r2"), ("t3", "r3"), ("t9", "r9")]:
        task = Task(id=tid, parent_id=None, goal="goal " + tid, device_id="mac", identity=Identity.OWNER,
                    scopes=frozenset(), role="worker", request_id=request, success_conditions=[], deadline=None)
        ttree.add(tree.TaskRecord(task, sc.clock.now(), "s", None))
    s9 = sc.spans.start("task", input={"identity": "owner", "goal": "Old goal", "targets": {"sites": ["x.com"]}},
                        task_id="t9")
    sc.spans.finish(s9, output={"summary": "x"})
    sc.db.conn.execute("INSERT INTO conversations(id) VALUES ('owner')")
    sc.db.conn.execute("INSERT INTO task_context (task_id,conversation_id,spec_json,owner_words,prior_task,"
                       "request_payload) VALUES ('t2','owner','{}','yes\nWhich of my files?',NULL,'')")
    sc.seeded()
    c = continuity.Continuity(sc.db, sc.redactor, lambda task: {"external:web:example.com", "private"})
    continuity.secrets = type("S", (), {"token_hex": staticmethod(lambda n: "c00000000001")})
    spec = TaskSpec.model_validate({"goal": "Find my file", "identity": "owner", "name": "Find file",
                                    "success_conditions": [{"text": "found", "check": None}],
                                    "parts": [{"id": "p1", "quote": "q", "label": "L", "direct": "x"}]})
    sc.step("ensure", ["bad id!"], c.ensure, "bad id!")
    sc.step("ensure", ["chat-b"], c.ensure, "chat-b")
    sc.step("attach", [True, "x"], c.attach, True, "x")
    sc.step("attach", [False, "nope"], c.attach, False, "nope")
    sc.step("attach", [True, None], c.attach, True)
    sc.step("current", [], c.current)
    sc.step("attach", [False, "chat-b"], c.attach, False, "chat-b")
    sc.step("attach", [False, None], c.attach)
    sc.step("pending", ["owner"], c.pending, "owner")
    sc.step("save_pending", ["owner", "my password is hunter2", "asked?", "payload"], c.save_pending, "owner",
            "my password is hunter2", "asked?", "payload")
    sc.step("pending", ["owner"], c.pending, "owner")
    sc.step("pending_payload", ["owner"], c.pending_payload, "owner")
    sc.step("pending_payload", ["none"], c.pending_payload, "none")
    sc.step("save", ["t1", "owner", spec.model_dump(), "find my file", None, "find my file\npayload"], c.save, "t1",
            "owner", spec, "find my file", None, "find my file\npayload")
    sc.step("save", ["t3", "schedule:s1", spec.model_dump(), "run", "t1", ""], c.save, "t3", "schedule:s1", spec,
            "run", "t1", "")
    sc.step("load", ["t1", "owner"], lambda: (lambda r: None if r is None else
                                               [r[0].model_dump(), r[1].request_id, r[1].words, r[1].payload, r[2]])(
        c.load("t1", "owner")))
    sc.step("load", ["t2", "owner"], lambda: (lambda r: None if r is None else
                                               [r[0].model_dump(), r[1].request_id, r[1].words, r[1].payload, r[2]])(
        c.load("t2", "owner")))
    sc.step("load", ["t1", "chat-b"], c.load, "t1", "chat-b")
    sc.step("prior_of", ["t3"], c.prior_of, "t3")
    sc.step("prior_of", ["t9"], c.prior_of, "t9")
    sc.step("candidates", ["owner"], c.candidates, "owner")
    sc.step("conversation_of", ["t1"], c.conversation_of, "t1")
    c.lend("look-1", "owner")
    sc.note("lend", ["look-1", "owner"])
    c.lend("look-2", "schedule:s1")
    sc.note("lend", ["look-2", "schedule:s1"])
    for who in ("t1", "look-1", "look-2", "nobody"):
        sc.step("reaches_schedules", [who], c.reaches_schedules, who)
        sc.step("readable_tasks", [who], lambda w=who: sorted(c.readable_tasks(w)))
        sc.step("entries", [who, None, None, 0, 20], c.entries, who)
        sc.step("tasks", [who], c.tasks, who)
    sc.step("entries", ["look-1", "2026-01-01T09:00:03+00:00", "2026-01-01T09:00:08+00:00", 1, 2], c.entries,
            "look-1", since="2026-01-01T09:00:03+00:00", until="2026-01-01T09:00:08+00:00", after=1, limit=2)
    for e in ("id-000001", "id-000005", "id-000007", "nope"):
        sc.step("entry", ["look-1", e], c.entry, "look-1", e)
        sc.step("result", ["owner", e, ["task.answer", "chat.reply"]], c.result, "owner", e, ("task.answer", "chat.reply"))
        sc.step("result", ["schedule:s1", e, ["task.answer"]], c.result, "schedule:s1", e, ("task.answer",))
    c.unlend("look-1")
    sc.note("unlend", ["look-1"])
    sc.step("conversation_of", ["look-1"], c.conversation_of, "look-1")
    sc.step("part_order", ["t1"], c.part_order, "t1")
    sc.step("part_order", ["zz"], c.part_order, "zz")
    for args in [("owner", "task.answer", "", "r1", "journal:id-000007", False), ("owner", "task.answer", "", "t3", "", True),
                 ("owner", "chat.notice", "", "", "", False), ("preview:x", "chat.notice", "", "", "", False),
                 ("owner", "chat.message", "schedule:s1", "", "", True), ("owner", "chat.message", "chat-b", "", "", False)]:
        sc.step("belongs", list(args), c.belongs, *args[:4], doc=args[4], schedules=args[5])
    for kind, data, conv in [("chat.notice", {}, "owner"), ("chat.notice", {}, "schedule:x"),
                             ("chat.message", {}, "owner"), ("chat.message", {"session_id": "b"}, "b")]:
        sc.step("in_conversation", [kind, data, conv], continuity.in_conversation, kind, data, conv)
    sc.step("upgrade_legacy", [], continuity.upgrade_legacy, sc.db, sc.redactor)


def conversations_scenario(sc: Scenario) -> None:
    published: list[Any] = []
    sc.seeded()
    store = conversations.ConversationStore(sc.db, sc.clock, sc.redactor, lambda t, d: published.append([t, d]))
    from ferb.orchestrator.conversations import ConversationScope
    raw = {"identity": "owner", "account": "me@example.com", "site": "web.whatsapp.com", "app": "net.whatsapp.WhatsApp",
           "thread": "Book club", "recipients": ["Ana", "Ben"], "topic": "the date", "handover_when": "a date is agreed",
           "window_id": "w7"}
    scope = ConversationScope.model_validate(raw)
    sc.step("approval_text", [raw], scope.approval_text)
    sc.step("card", [raw], scope.card)
    sc.step("open", ["c1", "t1", raw], store.open, "c1", "t1", scope)
    sc.step("open", ["c1", "t1", raw], store.open, "c1", "t1", scope)
    sc.step("open", ["c1", "t9", raw], store.open, "c1", "t9", scope)
    sc.step("open", ["c2", "t1", raw], store.open, "c2", "t1", scope)
    sc.step("get", ["nope"], store.get, "nope")
    sc.step("begin_turn", ["c1"], store.begin_turn, "c1")
    sc.step("approve", ["c1", "", "Yes"], store.approve, "c1", "", "Yes")
    sc.step("approve", ["c1", "ap-1", "Yes"], store.approve, "c1", "ap-1", "Yes")
    sc.step("approve", ["c1", "ap-1", "Yes"], store.approve, "c1", "ap-1", "Yes")
    sc.step("bind", ["c1", "t9.worker"], store.bind, "c1", "t9.worker")
    sc.step("bind", ["c1", "t1.converse"], store.bind, "c1", "t1.converse")

    def guard(task_id: str, observation: bool, identity: str, site: str | None, account: str | None,
              recipients: list[str], contact: str | None, kind: str, declared: str | None) -> Any:
        grant = type("G", (), {"task_id": task_id, "identity": Identity(identity)})()
        cap = type("C", (), {"observation": observation})()
        target = type("T", (), {"site": site, "account": account, "recipients": recipients, "contact": contact})()
        spec = type("S", (), {"target": target, "kind": kind, "declared": declared, "effect_kind": None})()
        return store.guard(grant, "x", cap, spec)
    for args in [("t1.converse", True, "owner", None, None, [], None, "read", None),
                 ("t1.converse", False, "ferb", "web.whatsapp.com", None, [], None, "send", None),
                 ("t1.converse", False, "owner", "evil.com", None, [], None, "send", None),
                 ("t1.converse", False, "owner", "WEB.WhatsApp.com", "other@x", [], None, "send", None),
                 ("t1.converse", False, "owner", "net.whatsapp.whatsapp", None, ["Ben", "Ana"], None, "send", None),
                 ("t1.converse", False, "owner", "web.whatsapp.com", None, ["Zed"], None, "fill_form", None),
                 ("t1.converse", False, "owner", "web.whatsapp.com", None, ["Zed"], None, "fill_form", "send"),
                 ("t1.converse", False, "owner", "web.whatsapp.com", None, [], "Zed", "click", None),
                 ("t1.converse", False, "owner", "web.whatsapp.com", "me@example.com", ["Ana", "Ben"], "Ana", "send", None),
                 ("t1.worker", False, "owner", "web.whatsapp.com", None, [], None, "send", None),
                 ("t5.worker", False, "owner", "x", None, [], None, "send", None)]:
        sc.step("guard", list(args), guard, *args)
    events = [{"id": "e1", "source": "notification", "data": {"bundle_id": "net.whatsapp.WhatsApp", "conversation": "Book club",
                                                             "text": "Friday?", "account": "me@example.com"}},
              {"id": "e2", "source": "notification", "data": {"bundle_id": "NET.WHATSAPP.WHATSAPP", "conversation": "Other"}},
              {"id": "e3", "source": "conversation_message", "data": {"bundle_id": "net.whatsapp.WhatsApp",
                                                                      "conversation": "Book club", "account": "x@y"}},
              {"id": "", "source": "notification", "data": {}},
              {"id": "e4", "source": "conversation_message", "data": {"bundle_id": "net.whatsapp.WhatsApp",
                                                                      "conversation": "Book club", "text": "pw: hunter22"}},
              {"id": "e1", "source": "notification", "data": {"bundle_id": "net.whatsapp.WhatsApp", "conversation": "Book club"}}]
    for e in events:
        sc.step("observe", [e], store.observe, e)
    sc.step("events", ["c1", 0], store.events, "c1")
    sc.step("pending", ["c1"], store.pending, "c1")
    n = sc.step("begin_turn", ["c1"], store.begin_turn, "c1")
    sc.step("finish_turn", ["c1", n, {"status": "done", "conversation_action": "handover", "evidence": []}],
            store.finish_turn, "c1", n, {"status": "done", "conversation_action": "handover", "evidence": []})
    sc.step("finish_turn", ["c1", n, {"status": "done", "conversation_action": "wait", "summary": "waiting"}],
            store.finish_turn, "c1", n, {"status": "done", "conversation_action": "wait", "summary": "waiting"})
    sc.step("pending", ["c1"], store.pending, "c1")
    sc.advance(10)
    n = sc.step("begin_turn", ["c1"], store.begin_turn, "c1")
    sc.step("finish_turn", ["c1", n, {"status": "not_done", "conversation_action": "wait"}], store.finish_turn, "c1", n,
            {"status": "not_done", "conversation_action": "wait"})
    sc.step("owner_change", ["c1", "resume", "go on"], store.owner_change, "c1", "resume", "go on")
    sc.step("owner_change", ["c1", "bogus", "x"], store.owner_change, "c1", "bogus", "x")
    sc.step("owner_change", ["c1", "pause", "  "], store.owner_change, "c1", "pause", "  ")
    typed = {"id": "e5", "source": "typed_text", "actor": "owner", "data": {"bundle_id": "net.whatsapp.WhatsApp",
                                                                            "window_id": "w7"}}
    sc.step("observe", [typed], store.observe, typed)
    sc.step("get", ["c1"], store.get, "c1")
    n2 = sc.step("owner_change", ["c1", "resume", "carry on"], store.owner_change, "c1", "resume", "carry on")
    n = sc.step("begin_turn", ["c1"], store.begin_turn, "c1")
    sc.step("finish_turn", ["c1", n, {"status": "done", "conversation_action": "handover", "summary": "agreed Friday",
                                      "evidence": ["trail:e1"]}],
            store.finish_turn, "c1", n, {"status": "done", "conversation_action": "handover", "summary": "agreed Friday",
                                         "evidence": ["trail:e1"]})
    sc.step("digest", ["c1"], store.digest, "c1")
    sc.step("list", [None], store.list)
    sc.step("list", ["t9"], store.list, "t9")
    # a forget: the copied evidence goes and the conversation pauses
    sc.db.conn.execute("INSERT INTO attention_notices (id, source_refs_json, text, origin, created_at, delivered_at, "
                       "quotes_json, considered_refs_json, cards_json) VALUES ('n1', '[\"trail:e1\"]', 'Ana asked', "
                       "'external:observation', 'x', NULL, '[]', '[]', '[]')")
    sc.note("sql", ["INSERT INTO attention_notices (id, source_refs_json, text, origin, created_at, delivered_at, "
                    "quotes_json, considered_refs_json, cards_json) VALUES ('n1', '[\"trail:e1\"]', 'Ana asked', "
                    "'external:observation', 'x', NULL, '[]', '[]', '[]')"])
    sc.step("forget_sources", [["trail:e1", "trail:e3"]], store.forget_sources, {"trail:e1", "trail:e3"})
    sc.step("finish_turn", ["c1", n, {"status": "done", "conversation_action": "finished", "evidence": ["x"]}],
            store.finish_turn, "c1", n, {"status": "done", "conversation_action": "finished", "evidence": ["x"]})
    sc.step("conversation_read", ["t1.worker", None], lambda: _answer(conversation_tools.ConversationRead(
        store, lambda t: None).run(type("A", (), {"conversation_id": None, "offset": 0, "max_chars": 1000})(),
                                   type("X", (), {"task_id": "t1.worker"})())))
    sc.step("conversation_read", ["t1.worker", "c1", 10, 1000], lambda: _answer(conversation_tools.ConversationRead(
        store, lambda t: "t1").run(type("A", (), {"conversation_id": "c1", "offset": 10, "max_chars": 1000})(),
                                   type("X", (), {"task_id": "t1.worker"})())))
    sc.step("conversation_read", ["t1.worker", "nope"], lambda: _answer(conversation_tools.ConversationRead(
        store, lambda t: None).run(type("A", (), {"conversation_id": "nope", "offset": 0, "max_chars": 1000})(),
                                   type("X", (), {"task_id": "t1.worker"})())))
    sc.step("published", [], lambda: published)


def _answer(coro: Any) -> Any:
    r = asyncio.run(coro) if asyncio.iscoroutine(coro) else coro
    return {"content": r.content, "is_error": r.is_error, "read_sources": [list(x) for x in r.read_sources]}


def runner_scenario(sc: Scenario) -> None:
    sc.seeded()
    store = conversations.ConversationStore(sc.db, sc.clock, sc.redactor, lambda t, d: None)
    from ferb.orchestrator.conversations import ConversationScope
    raw = {"identity": "ferb", "account": "bot@x", "site": "chat.example", "app": "Chat", "thread": "T1",
           "recipients": ["Ana"], "topic": "hello", "handover_when": "she answers"}
    scope = ConversationScope.model_validate(raw)
    emitted: list[Any] = []
    turns = [{"status": "done", "conversation_action": "wait", "summary": "asked"},
             {"status": "done", "conversation_action": "handover", "summary": "she said hi", "evidence": ["trail:e9"]}]

    async def approve(text: str, options: list[str], card: Any) -> Any:
        emitted.append(["approve", text, options, card])
        return {"approval_id": "ap-7", "answer": "Yes"}

    async def turn(ident: str, number: int, events: Any, digest: Any) -> Any:
        emitted.append(["turn", ident, number, events, digest])
        if number == 1:
            sc.db.conn.execute("INSERT INTO conversation_events(conversation_id,source_id,origin,data_json,at) "
                               "VALUES('c9','trail:e9','external:conversation','{\"text\": \"hi\"}','x')")
        return turns[number - 1]

    async def emit(event: Any) -> None:
        emitted.append(["emit", event])
    runner = conversations.ConversationRunner(store, approve, turn, emit, lambda: False)
    sc.step("run", ["c9", "t4", raw], lambda: asyncio.run(runner.run("c9", "t4", scope)))
    sc.step("emitted", [], lambda: emitted)


def parts_scenario(sc: Scenario) -> None:
    sc.seeded()
    emitted: list[Any] = []
    clock = {"t": datetime(2026, 1, 1, 9, 0, tzinfo=timezone.utc)}
    parts.datetime = type("D", (), {"now": staticmethod(lambda tz=None: clock["t"])})
    ps = [Part(id="p1", quote="what's 2+2", label="Sum", direct="4"),
          Part(id="p2", quote="find the file", label="File", answer_step="find", conditions=[0]),
          Part(id="p3", quote="email Ana", label="Email", answer_step="mail", conditions=[1]),
          Part(id="p4", quote="pay it", label="Pay", answer_step="pay", conditions=[2])]
    card = parts.Card(sc.journal, "t1", "r1", ps, emitted.append, lambda: "inferred")

    def state() -> Any:
        return {"rows": card.rows(), "tally": list(card.tally()), "stopped": card.stopped(), "line": card.line(),
                "ended": card.ended(), "unposted": [p.id for p in card.unposted()], "here": sorted(card.here)}
    sc.step("state", [], state)
    sc.step("say_direct", [], card.say_direct)
    sc.step("say_direct", [], card.say_direct)
    sc.step("part_of", ["mail"], lambda: card.part_of("mail").id)
    sc.step("show", ["p2", "asks", {"question": {"text": "Which file?\nmore"}, "options": [], "empty": ""}],
            card.show, ps[1], "asks", question={"text": "Which file?\nmore"}, options=[], empty="")
    sc.step("state", [], state)
    sc.step("settle", ["p2", "done", "Found it at ~/a.txt\nDetails", "", True, "t1.find"], card.settle, ps[1], "done",
            "Found it at ~/a.txt\nDetails", verified=True, step_task="t1.find")
    sc.step("show", ["p2", "working", {}], card.show, ps[1], "working")
    sc.step("settle", ["p2", "done", "Again", "", True, ""], card.settle, ps[1], "done", "Again", verified=True)
    sc.step("settle", ["p3", "failed", "Couldn't do: no address", "", False, ""], card.settle, ps[2], "failed",
            "Couldn't do: no address")
    sc.step("settle", ["p3", "done", "Sent", "Couldn't confirm: x", False, ""], card.settle, ps[2], "done", "Sent",
            note="Couldn't confirm: x")
    sc.step("settle", ["p4", "stopped", "You said no", "", False, ""], card.settle, ps[3], "stopped", "You said no")
    sc.step("update", ["p2", "Changed answer", "the file moved", "t1.x"], card.update, ps[1], "Changed answer",
            "the file moved", "t1.x")
    sc.step("update", ["p2", "Changed answer", "same", ""], card.update, ps[1], "Changed answer", "same")
    sc.step("state", [], state)
    sc.step("composed", [["Sites contacted: 2"], ["/a.txt", "/b.txt"], 300], parts.composed, card, ["Sites contacted: 2"],
            ["/a.txt", "/b.txt"], 300)
    sc.step("composed", [[], [], 20], parts.composed, card, [], [], 20)
    # a follow-up turn of the same task: earlier answers stay, changed ones update
    card2 = parts.Card(sc.journal, "t1", "r2", ps, emitted.append, lambda: "inferred")
    card = card2
    sc.step("state", [], state)
    sc.step("settle", ["p3", "failed", "Still no", "", False, ""], card.settle, ps[2], "failed", "Still no")
    sc.step("settle", ["p4", "stopped", "No again", "", False, ""], card.settle, ps[3], "stopped", "No again")
    sc.step("settle", ["p2", "done", "Changed answer", "", True, ""], card.settle, ps[1], "done", "Changed answer")
    sc.step("settle", ["p2", "done", "New answer", "", False, ""], card.settle, ps[1], "done", "New answer")
    sc.step("state", [], state)
    sc.step("done_line", [3, 4, 1], parts.done_line, 3, 4, 1)
    sc.step("emitted", [], lambda: emitted)


def loops_scenario(sc: Scenario) -> None:
    sc.seeded()
    log: list[Any] = []
    t = {"now": 1767258000.0, "steers": 0}
    store = loops.LoopStore(sc.db, sc.clock)
    step = Step(id="watch", kind="do", goal="Keep the video playing, forever", repeat={"end_when": "the song ends"})
    probe = Probe(tool="media.state", args_json='{"tab": "t1"}', field="playing", op="equals", value_json="false")
    rounds = [
        {"status": "done", "summary": "started it", "loop_action": "continue", "probe": probe, "steers_seen": 0},
        {"status": "done", "summary": "restarted", "loop_action": "continue", "probe": probe},
        {"status": "done", "summary": "restarted again", "loop_action": "continue", "probe": probe},
        {"status": "done", "summary": "restarted 3", "loop_action": "continue", "probe": None, "unchecked": True},
        {"status": "blocked", "summary": "the tab closed", "loop_action": "blocked", "question": "Open it again?"},
        {"status": "done", "summary": "the song ended", "loop_action": "finished", "finished_verified": True,
         "evidence": ["e1"]},
    ]
    reads = [(True, None, {"external:web"}), (False, None, set()), (True, None, set()), (False, None, set()),
             (False, None, set()), (False, "tab gone", set()), (True, None, set()), (False, None, set()),
             (False, None, set()), (loops.MISSING, None, set())] + [(True, None, set())] * 40

    async def round_(reason: dict[str, Any]) -> dict[str, Any]:
        shown = {**reason, "observed": "<MISSING>"} if reason.get("observed") is loops.MISSING else reason
        log.append(["round", shown])
        t["now"] += 30
        return rounds.pop(0)

    async def read(p: Probe) -> Any:
        value, error, origins = reads.pop(0)
        log.append(["read", t["now"]])
        return value, error, origins

    def record(kind: str, p: Probe, value: Any, error: Any, origins: set[str], n: int) -> None:
        log.append(["record", kind, "<MISSING>" if value is loops.MISSING else value, error, sorted(origins), n])

    async def notify(text: str, model_written: bool = False) -> None:
        log.append(["notify", text, model_written])

    async def wait_owner(result: Any) -> Any:
        log.append(["wait_owner", None if result is None else result.get("summary")])
        return {"why": "your message", "owner": True, "answer": "yes, reopen it", "question": "Open it again?"}

    async def sleep(seconds: float) -> None:
        log.append(["sleep", round(seconds, 6)])
        t["now"] += max(seconds, 0.001)

    runner = loops.LoopRunner(
        store=store, task_id="t1", step=step, round=round_, arm=lambda p: None, probe=read, record=record,
        event=lambda cursor: None, steers=lambda: t["steers"], notify=notify,
        status=lambda s: log.append(["status", s]), stopped=lambda: not rounds, now=lambda: t["now"],
        wait_owner=wait_owner, sleep=sleep, event_cursor=4)
    sc.step("run", [], lambda: asyncio.run(runner.run()))
    sc.step("log", [], lambda: log)
    sc.step("texts", ["t1", "watch"], store.texts, "t1", "watch")
    sc.step("digest", ["t1", "watch"], store.digest, "t1", "watch")
    sc.step("save", ["t1", "watch", {"bogus": 1}], store.save, "t1", "watch", bogus=1)


def executor_scenario(sc: Scenario) -> None:
    store = plan_store.PlanStore(sc.db, sc.redactor)
    plan = {"steps": [{"id": "a", "kind": "do", "goal": "A"}, {"id": "b", "kind": "do", "goal": "B", "depends_on": ["a"]},
                      {"id": "w", "kind": "wait", "goal": "Wait", "until": "2026-01-01T08:00:00Z"},
                      {"id": "e", "kind": "wait", "goal": "Event", "event": "ping", "depends_on": ["w"]},
                      {"id": "c", "kind": "do", "goal": "C", "depends_on": ["b", "e"]},
                      {"id": "f", "kind": "do", "goal": "F"}, {"id": "g", "kind": "do", "goal": "G", "depends_on": ["f"]},
                      {"id": "h", "kind": "do", "goal": "H", "depends_on": ["g"]}]}
    store.create("t1", Plan.model_validate(plan), {}, None)
    store.event("ping", {"n": 1})
    store.finish("t1", "e", {"status": "pending", "event_cursor": 0})     # its cursor from an earlier run: it sees ping
    sc.seeded()
    told: list[Any] = []
    results = {"a": {"status": "done", "summary": "a!"}, "b": {"status": "done_unverified", "summary": "b"},
               "c": {"status": "done", "summary": "c"}, "f": {"status": "failed", "summary": "no"}}

    async def execute(step: Step, inputs: list[str], prior: dict[str, Any]) -> dict[str, Any]:
        told.append(["execute", step.id, inputs, prior])
        if step.id == "c":
            raise KeyError("boom")
        return results[step.id]
    ex = plan_executor.PlanExecutor(store, execute, lambda: False, now=sc.clock.now,
                                    on_step=lambda task, step, status, facts: told.append([task, step, status, facts]))
    sc.step("run", ["t1"], lambda: asyncio.run(ex.run("t1")))
    sc.step("told", [], lambda: told)


def attention_scenario(sc: Scenario) -> None:
    s1 = sc.spans.start("attention.consider", input={"source_refs": ["trail:e1"], "context": "ctx Ana asked"})
    sc.spans.finish(s1, output={"text": "Ana asked about Friday", "quotes": [{"text": "Friday?"}]})
    s2 = sc.spans.start("model.turn", input={"chat": "earlier \"text\": \"Ana asked about Friday\" and Friday?"})
    sc.spans.finish(s2, output={"text": "ok Ana asked about Friday"})
    sc.seeded()
    notes = attention.AttentionNotices(sc.db, sc.clock, sc.redactor)
    sc.step("add", ["n1", ["trail:e1"], "Ana asked about Friday", [{"text": "Friday?"}, {"text": " "}, "x"], ["trail:e2"]],
            notes.add, "n1", ["trail:e1"], "Ana asked about Friday", [{"text": "Friday?", "origin": "owner"}, {"text": " "}, "x"],
            ["trail:e2"], cards=[{"type": "link", "host": "x.com"}])
    sc.step("add", ["n1", [], "dup", None, None], notes.add, "n1", [], "dup")
    sc.step("add", ["n2", ["trail:e3"], "Second notice", None, None], notes.add, "n2", ["trail:e3"], "Second notice")
    sc.step("unseen", [], notes.unseen)
    sc.step("acknowledge", [["n2", "zz"]], notes.acknowledge, ["n2", "zz"])
    sc.step("acknowledge", [["n2"]], notes.acknowledge, ["n2"])
    sc.step("list", [False], notes.list, pending=False)
    sc.step("told", [], notes.told)
    sc.step("notice_sources", ["n1"], attention.notice_sources, sc.db, "n1")
    entry = sc.journal.append("chat.notice", {"id": "n1", "text": "Ana asked about Friday", "quotes": [{"text": "Friday?"}]})
    e = sc.spans.start("context.exposure", input={"source_refs": [f"entry:{entry.id}"]})
    sc.note("journal+span", [])
    sc.step("expire_derived", [["trail:e1"], "Source evidence was forgotten.", "2026-01-02T00:00:00+00:00"],
            attention.expire_derived, sc.db, {"trail:e1"}, "Source evidence was forgotten.", "2026-01-02T00:00:00+00:00")
    sc.step("expire_derived", [["trail:e1"], "Source evidence was forgotten.", "later"],
            attention.expire_derived, sc.db, {"trail:e1"}, "Source evidence was forgotten.", "later")
    sc.step("forms", ["Ana “x”", "gone"], attention._forms, "Ana “x”", "gone")
    sc.step("notice_payload", [{"text": "t", "quotes": [{"text": "q"}, "x", {}]}], attention.notice_payload,
            {"text": "t", "quotes": [{"text": "q"}, "x", {}]})
    _ = e


def _has(db: Any, table: str, column: str) -> bool:
    return any(r[1] == column for r in db.conn.execute(f"PRAGMA table_info({table})"))


def cards_scenario(sc: Scenario) -> None:
    calls = [
        {"tool": "browser.act", "kind": "pay", "target": {"site": "www.Shop.com"}, "facts": {"page_amount": "$5",
         "page_merchant": "Shop", "page_url": "https://www.shop.com/checkout", "tab_chain": "c1"},
         "confirmations": ["cf1"], "card_entry": "Visa (personal)"},
        {"tool": "browser.act", "kind": "click", "declared": "pay", "target": {"site": "shop.com"},
         "facts": {"page_amount": "$6", "tab_chain": "c1"}, "payment": {"covered_by": "cf2"}},
        {"tool": "browser.act", "kind": "pay", "target": {"site": "other.com"}, "boundary_refused": True, "declined": True,
         "confirmations": ["cf3"], "facts": {"tab_chain": "c9"}},
        {"tool": "browser.act", "kind": "pay", "target": {"site": "other.com"}, "confirmations": ["cf3"],
         "facts": {"tab_chain": "c9"}},
        {"tool": "mail.send", "kind": "mailbox_send", "target": {"recipients": ["Ana@X.com", ""], "account": "me"}},
        {"tool": "mac.act", "kind": "mac_input", "declared": ["send"], "fact_kinds": ["submit"],
         "target": {"site": "net.whatsapp", "contact": "Ben"}},
        {"tool": "browser.act", "kind": "submit", "target": {"site": "forms.example"}, "facts": {}},
        {"tool": "browser.act", "kind": "fill_form", "declared": "send", "target": {"site": "forms.example",
         "recipients": ["x"]}, "confirmations": ["cf1"]},
        {"tool": "fs.trash", "kind": "trash", "target": {"path": "~/old.txt"}},
        {"tool": "fs.delete", "kind": "delete_permanent", "target": {"path": "/FXTMP/gone.txt"}, "boundary_refused": True,
         "declined": True},
        {"tool": "owner.ask", "kind": "send", "ferb_question": True, "target": {"recipients": ["q"]}},
        {"tool": "browser.act", "kind": "send", "target": {"site": "x.com"}, "observed": 1},
    ]
    home = str(Path.home())
    for i, c in enumerate(calls):
        c = json.loads(json.dumps(c).replace("/FXTMP", home))
        s = sc.spans.start("tool.call", input=c, task_id="t1.worker" if i % 2 else "t1")
        err = "boom" if i == 6 else None
        sc.spans.finish(s, output={"ok": True, "after": {"changed": i == 6}, "observed_change": i == 7}, error=err)
        sc.clock.advance(1)
    s = sc.spans.start("tool.call", input={"tool": "x", "kind": "pay"}, task_id="t1")    # never finished
    for cid, rule, granted in [("cf1", "floor:pay", 1), ("cf2", "floor:pay", 1), ("cf3", "floor:pay", 0),
                               ("cf4", "other", 1)]:
        sc.db.conn.execute("INSERT INTO confirmations (id, tree_id, rule, key, granted, at) VALUES (?, 't1', ?, 'k', ?, 'x')",
                           (cid, rule, granted))
    cols = [r[1] for r in sc.db.conn.execute("PRAGMA table_info(downloads)")]
    for did, state, path in [("d1", "taken", home + "/Downloads/a.pdf"), ("d2", "refused", None), ("d3", "taken", "/x/b")]:
        row = {"id": did, "task_id": "t1.worker", "profile": "p", "identity": "owner", "state": state, "path": path,
               "origin": "external:web:x.com"}
        for col in cols:
            if col == "at":
                row["at"] = "2026-01-01T09:00:0" + did[1] + "+00:00"
        names = [k for k in row]
        sc.db.conn.execute(f"INSERT INTO downloads ({','.join(names)}) VALUES ({','.join('?' * len(names))})",
                           [row[k] for k in names])
    (Path(home) / "exists.txt").write_text("x")
    sc.seeded()
    sc.step("payments", ["t1"], cards.payments, sc.spans, "t1")
    sc.step("sends", ["t1"], cards.sends, sc.spans, "t1")
    sc.step("withdrawn", ["t1"], lambda: sorted(list(x) for x in cards.withdrawn(sc.spans, "t1")))
    sc.step("paid", ["t1"], cards.paid, sc.spans, "t1")
    for kinds in (["pay"], ["send", "reply", "post", "mailbox_send", "call"], ["submit"], ["trash", "delete_permanent"]):
        sc.step("ran", ["t1", kinds], cards.ran, sc.spans, "t1", *kinds)
        sc.step("tried", ["t1", kinds], cards.tried, sc.spans, "t1", *kinds)
    sc.step("downloaded", ["t1"], cards.downloaded, sc.spans, "t1")
    for c in calls[:3]:
        sc.step("approved", [c], cards.approved, sc.spans, c)
        sc.step("purchase", [c], lambda c=c: list(cards.purchase(c)))
    sc.step("files", [["~/exists.txt", "", "~/missing.txt", home + "/exists.txt"]], cards.files,
            ["~/exists.txt", "", "~/missing.txt", home + "/exists.txt"])
    sc.step("event", ["timer", "tm1", "1-minute timer", "09:01", "Set.", None, None, None, "09:00"], cards.event,
            "timer", "tm1", "1-minute timer", "09:01", "Set.", started="09:00")
    sc.step("event", ["schedule", "s1", "Daily", "9am", "line", "daily", "until met", "2026-02-01", None], cards.event,
            "schedule", "s1", "Daily", "9am", "line", "daily", "until met", "2026-02-01")
    sc.step("note", ["m1", "likes tea", "saved", "Saved."], cards.note, "m1", "likes tea", "saved", "Saved.")
    sc.step("links", [["a.com", "b.org"], "Sites contacted: 2"], cards.links, ["a.com", "b.org"], "Sites contacted: 2")
    # the literal checks over the same records
    checks = verify.Checks(lambda task: ["https://shop.com/thanks?id=1", "https://x.com/"], sc.spans)
    ctx = ToolContext(task_id="t1", worker_id="w", request_id="r", identity=Identity.OWNER, scopes=frozenset(),
                      device_id="mac", span_id=None, workspace=str(Path(home) / "ws"), owner_roots=(), untrusted_roots=())
    (Path(home) / "ws").mkdir(exist_ok=True)
    (Path(home) / "ws" / "note.txt").write_text("hello wörld " * 10000)
    (Path(home) / "ws" / "bad.txt").write_bytes(b"abc\xff hello")
    conditions = [
        {"kind": "file_exists", "path": "note.txt"}, {"kind": "file_exists", "path": "nope.txt"},
        {"kind": "file_contains", "path": "note.txt", "text": "wörld hello"},
        {"kind": "file_contains", "path": "note.txt", "text": "absent"},
        {"kind": "file_contains", "path": "bad.txt", "text": "abc"}, {"kind": "file_contains", "path": "bad.txt", "text": "hello"},
        {"kind": "file_contains", "path": str(Path(home) / "ws"), "text": "x"},
        {"kind": "tab_url", "url_contains": "thanks"}, {"kind": "tab_url", "url_contains": "nothing"},
        {"kind": "sent", "expected": "ana@x.com"}, {"kind": "sent", "expected": "@ben", "site": "NET.whatsapp"},
        {"kind": "sent", "expected": "zed", "site": "x.com"}, {"kind": "paid", "site": "shop.com"}, {"kind": "paid"},
        {"kind": "paid", "site": "other.com"}, {"kind": "submitted", "site": "forms.example"},
        {"kind": "submitted", "site": "x.com"}, {"kind": "trashed", "path": "~/old.txt"},
        {"kind": "deleted", "path": "~/gone.txt"}, {"kind": "downloaded", "path": "~/Downloads/a.pdf"},
        {"kind": "downloaded", "path": "~/exists.txt"}, {"kind": "downloaded", "path": "~/never.pdf"},
        {"kind": "after_change", "tool": None}, {"kind": "after_value", "tool": "browser.act", "text": "x"},
        {"kind": "after_change", "tool": "nothing"}, {"kind": "none"}, {"kind": "account"},
    ]
    for i, k in enumerate(conditions):
        cond = Condition.model_validate({"text": f"condition {i}", "check": {"path": None, "text": None,
                                                                              "url_contains": None, "site": None,
                                                                              "expected": None, **k}})
        sc.step("code_checkable", [cond.model_dump()], checks.code_checkable, cond, "t1")
        sc.step("check", [cond.model_dump()], lambda cond=cond, i=i: asyncio.run(checks.run(i, cond, "t1", ctx)).as_dict())
        sc.step("not_done_said", [cond.model_dump()], verify.not_done_said, cond)
    s3 = sc.spans.start("tool.call", input={"tool": "mac.act", "kind": "click"}, task_id="t1")
    sc.spans.finish(s3, output={"after": {"changed": True}, "ok": False}, error="lost")
    sc.note("span", [])
    cond = Condition.model_validate({"text": "changed", "check": {"kind": "after_change", "path": None, "text": None,
                                                                 "url_contains": None, "site": None, "expected": "TRUE "}})
    sc.step("check", [cond.model_dump()], lambda: asyncio.run(checks.run(0, cond, "t1", ctx)).as_dict())
    _ = s


def evidence_scenario(sc: Scenario) -> None:
    for i, (kind, inp, out, origin, err) in enumerate([
        ("task", {"request": "find it <untrusted-data id=\"z\">x</untrusted-data id=\"z\">", "goal": "g"}, {"summary": "s"}, None, None),
        ("tool.call", {"tool": "fs.read", "kind": "read", "args": {"path": "/a"}}, {"text": "secret-value-123 and more " * 600},
         "private", None),
        ("worker", {"goal": "w"}, {"answer": "a"}, None, None),
        ("tool.call", {"tool": "browser.act", "kind": "send", "target": {"account": "me", "site": "x.com"},
                       "args": {"allow_foreground": True}, "facts": {"typed": 3}}, {"screen": {"front": "x"}},
         "external:web:x.com", None),
        ("tool.call", {"tool": "memory.read", "args": {}}, {"items": []}, None, None),
        ("context.source", {"source_refs": ["memory:m1"], "text": "ctx"}, {"t": 1}, "owner", None),
        ("tool.call", {"tool": "shell.run", "boundary_refused": True}, None, None, "refused"),
        ("worker", {"goal": "w2"}, {"answer": "b"}, "inferred", None),
        ("tool.call", {"tool": "fs.write", "kind": "write_file", "args": {"path": "/b"}, "content": "x" * 50},
         {"ok": True}, "ferb", None),
        ("disturbance.change", {"x": 1}, {"changes": {"tab_sound": {"tab": "t9"}},
                                          "actions": [{"task_id": "t1.worker", "tool": "browser.act"}, "x"]}, None, None),
    ]):
        task = "t1" if kind in ("task", "disturbance.change") else "t1.worker"
        s = sc.spans.start(kind, input=inp, task_id=task)
        sc.spans.finish(s, output=out, error=err, origin=origin)
        sc.clock.advance(1)
    secrets_ = ["secret-value-123", "é\"q"]
    sc.seeded()
    te = evidence.TaskEvidence(sc.spans, parent=lambda t: None, root=lambda t: "t1", store=None,
                               secrets=lambda: secrets_, readable=lambda t: {"t1"})
    A = evidence.TaskEvidence.Input

    def run(task: str, **args: Any) -> Any:
        return _answer(te.run(A(**args), type("X", (), {"task_id": task})()))
    for args in [{}, {"kind": "tool.call"}, {"kind": "nothing"}, {"query": "fs.read"}, {"limit": 2}, {"after": 2, "limit": 2},
                 {"span_id": "id-000004", "max_chars": 1000}, {"span_id": "id-000004", "offset": 5, "max_chars": 1000},
                 {"span_id": "nope"}, {"span_ids": ["id-000004", "id-000002", "id-000004", "zz"], "max_chars": 3000},
                 {"span_id": "x", "span_ids": ["y"]}, {"task_id": "t2"}, {"task_id": "t1", "kind": "worker"},
                 {"kind": "evidence"}, {"span_id": "id-000002"}, {"span_id": "id-000016"}]:
        sc.step("task_evidence", ["t1.worker", args], run, "t1.worker", **args)
    for text, offset, size in [("abcdefsecret-value-123xyz", 0, 10), ("abcdefsecret-value-123xyz", 8, 4),
                               ("secret-value-123secret-value-123", 3, 20), ("é\\\"q and é\"q", 0, 3)]:
        sc.step("page_bounds", [text, offset, size, secrets_], evidence.page_bounds, text, offset, size, secrets_)
    row = sc.spans.tool_call("t1.worker", "id-000004", root_id="t1")
    sc.step("record_text", [row], evidence.record_text, row)
    for task, since in [("t1.worker", 0), ("t1.worker", 3), ("t1", 0)]:
        sc.step("how_facts", [task, since], evidence.how_facts, sc.db.conn, task, since)
    for earlier, since in [(1, 0), (0, 5)]:
        sc.step("recorded_calls", ["t1.worker", "t1", earlier, since],
                lambda e=earlier, s=since: [[b.text, b.reference, b.protected, b.priority, b.fold] for b in
                                            evidence.recorded_calls(sc.spans, "t1.worker", "t1",
                                                                    lambda n: n.startswith(("fs.read", "memory")),
                                                                    sc.redactor, lambda: secrets_, above=3,
                                                                    earlier_runs=e, since=s)])
    for o in ("owner_stated", "observed", "inferred", "third_party", "x"):
        sc.step("memory_origin", [o], evidence.memory_origin, o)
    for b, end in [("2026-01-02", False), ("2026-01-02", True), (" 2026-01-02T10:00 ", False), ("", True), (None, True)]:
        sc.step("bound", [b, end], evidence.ChatHistory._bound, b, end)


def misc_scenario(sc: Scenario) -> None:
    sc.journal.append("chat.message", {"session_id": "owner", "request_id": "r1", "text": "hi", "test_run": {"run": 1}})
    sc.journal.append("chat.message", {"session_id": "chat-x", "request_id": "r2", "text": "hi"})
    for tid, req in [("t1", "r1"), ("t2", "r2")]:
        sc.db.conn.execute("INSERT INTO tasks (id, tree_id, request_id, goal, identity, role, scopes_json, status, "
                           "created_at, updated_at) VALUES (?, ?, ?, 'g', 'owner', 'worker', '[]', 'done', 'x', 'x')",
                           (tid, tid, req))
    sc.seeded()
    tr = test_runs.TestRuns(sc.db)
    for op, args in [("session", ["owner"]), ("session", ["chat-x"]), ("session", [""]), ("request", ["r1"]),
                     ("request", [None]), ("task", ["t1"]), ("task", ["t1.worker"]), ("task", ["t2.x"]),
                     ("task", ["t9"]), ("add", ["chat-x", "r2"]), ("task", ["t2.x"]), ("session", ["chat-x"])]:
        sc.step(op, args, getattr(tr, op), *args)
    ep = {"task_id": "t1", "request_id": "r1", "request_text": "do it", "goal": "g", "status": "done", "verified": True,
          "started_at": "a", "ended_at": "b", "seconds": 1.5, "answer": "ans", "conditions": [{"text": "c", "met": None}],
          "steps": [{"at": "x", "text": "é"}], "problems": [], "tools": {"fs.read": {"calls": 1}},
          "origins": ["owner", "external:web:x.com"], "tool_origins": [], "usage": {"tokens": 3}, "desktop": {},
          "made_by": "code:episode/1", "built_at": "now", "how": "read the file"}
    sc.step("write", [ep], episodes.write, sc.db, ep)
    sc.step("get", ["t1"], episodes.get, sc.db, "t1")
    ep2 = {**ep, "task_id": "t2", "verified": None, "how": "", "origins": ["ferb"], "tool_origins": ["private"]}
    sc.step("write", [ep2], episodes.write, sc.db, ep2)
    sc.step("get", ["t2"], episodes.get, sc.db, "t2")
    sc.step("get", ["t9"], episodes.get, sc.db, "t9")
    from ferb.orchestrator.schemas import TargetsSpec
    for t in [TargetsSpec(sites=["https://www.Example.com/a", "news.site.org/x", " ", "a..b"], apps=["Notes"],
                          recipients=["Ana"], paths=["/a/b/report.pdf", "x/"]), TargetsSpec()]:
        sc.step("target_words", [t.model_dump()], context_layers.target_words, t)
    cands = [{"text": "likes tea", "reference": "memory:m1", "origin": "owner,private", "source_refs": ["m1"], "fold": "f"},
             {"text": "B", "reference": "memory:m2", "origin": ""}]
    sc.step("ranked_memory_blocks", [cands], lambda: [[b.text, b.reference, b.priority, b.origin, list(b.source_refs),
                                                       b.payload, b.fold, sorted(o)]
                                                      for b, o in context_layers.ranked_memory_blocks(cands)])
    # skills
    folder = sc.home / "skills"
    (folder / "write-notes").mkdir(parents=True)
    (folder / "write-notes" / "SKILL.md").write_text("---\nname: Write notes\ndescription: \"How I write\"\nname: Again\n"
                                                    "other: x\n---\nThe body\r\nline")
    (folder / "write-notes" / "ref").mkdir()
    (folder / "write-notes" / "ref" / "a.md").write_text("ref text")
    (folder / "plain").mkdir()
    (folder / "plain" / "SKILL.md").write_text("no front matter")
    (folder / "broken").mkdir()
    (folder / "broken" / "SKILL.md").write_bytes(b"\xff")
    (folder / "linked").symlink_to(folder / "write-notes")
    (folder / "a-file.md").write_text("x")
    sk = skills.Skills(folder)
    sc.step("catalog", [], sk.catalog)
    for skill, file in [("write-notes", "SKILL.md"), ("write-notes", "ref/a.md"), ("write-notes", "../plain/SKILL.md"),
                        ("linked", "SKILL.md"), ("", "SKILL.md"), ("a/b", "x"), ("write-notes", "/etc/x"),
                        ("write-notes", "ref//./a.md"), ("broken", "SKILL.md"), ("plain", "SKILL.md")]:
        sc.step("skill_read", [skill, file], sk.read, skill, file)


def decide_scenario(sc: Scenario) -> None:
    from ferb.decide import decider, questions
    sc.seeded()
    posted: list[Any] = []
    replies = [(429, None), (200, {"answers": {"q0": {"noul": 0.2}, "q1": {"choice": "ignore", "confidence": 0.9}}}),
               (500, None), (200, {"answers": {"q0": {"choice": "nope"}}}), (402, None)]

    async def post(url: str, headers: dict[str, str], body: Any, timeout: float) -> Any:
        posted.append([url, headers, body, timeout])
        return replies.pop(0)

    async def key() -> str:
        return "key-123"

    async def sleep(s: float) -> None:
        posted.append(["sleep", s])
    client = decider.JevClient(key, post=post, sleep=sleep)
    mono = {"t": 10.0}

    def monotonic() -> float:
        mono["t"] += 0.25
        return mono["t"]
    decider.time = type("T", (), {"monotonic": staticmethod(monotonic)})
    rng = iter([0.05, 0.5, 0.01] * 10)
    d = decider.ShadowDecider(sc.db, sc.clock, sc.ids, client, sc.redactor, enabled=lambda: {"memory.worth_keeping"},
                              rng=lambda: next(rng))
    A = decider.Ask

    async def run() -> Any:
        out = []
        out.append(await d.ask({"text": "pw: hunter2"}, [A("memory.worth_keeping#1", "yes", "m1"),
                                                           A("trail.relevance", "note", "e1")]))
        out.append(await d.advice({"s": 1}, [A("trail.relevance", "note", "e2"),
                                             A("action.alignment", "aligned", None, {"a": "A", "b": "B"})]))
        out.append(await d.answers({"s": 2}, [A("trail.attention", "no", "e3")]))
        out.append(await d.ask({"s": 3}, [A("trail.relevance", "note", "e4")]))
        await d.idle()
        out.append(await d.answers({"s": 4}, [A("trail.attention", "no", "e5")]))
        d.mark_effect("trail.attention#2", "e3", "considered")
        d.mark_effect("trail.attention", None, "x")
        return out
    sc.step("run", [], lambda: asyncio.run(run()))
    sc.step("posted", [], lambda: posted)
    sc.step("stats", [], d.stats)
    sc.step("flagged_refs", ["trail.relevance", 5], d.flagged_refs, "trail.relevance", 5)
    sc.step("questions", [], lambda: [[q.id, q.api(), sorted(q.stricter), q.shadow_sample, q.effect,
                                       q.shadow_only, q.live] for q in questions.QUESTIONS.values()])
    for qid, answer in [("memory.worth_keeping", {"noul": 0.5}), ("memory.worth_keeping", {"noul": "0.4"}),
                        ("trail.relevance", {"choice": "note", "confidence": 1}), ("trail.relevance", {"choice": "zz"}),
                        ("trail.relevance", {"choice": "note"}), ("memory.worth_keeping", {})]:
        sc.step("value", [qid, answer], questions.QUESTIONS[qid].value, answer)


def steering_scenario(sc: Scenario) -> None:
    """steering.py's own decisions with the supervisor's parts as fakes that record what they were asked: the open
    questions a correction settles, the standing scope it restates, a correction's delivery to live runs."""
    from types import SimpleNamespace as NS
    from ferb.core.errors import Unsupported
    from ferb.orchestrator import steering
    from ferb.orchestrator.plans import SendScope
    sc.seeded()
    log: list[Any] = []

    class Approvals:
        def __init__(self, items: list[Any]) -> None:
            self.items, self.unattended = items, {"r-sched"}

        def pending(self) -> list[Any]:
            return [a for a in self.items if a.state == "pending"]

        def supersede(self, ident: str) -> None:
            log.append(["supersede", ident])
            next(a for a in self.items if a.id == ident).state = "superseded"

        def resolve(self, ident: str, granted: bool, text: str = "") -> None:
            log.append(["resolve", ident, granted, text])
            next(a for a in self.items if a.id == ident).state = "answered"

    def ap(ident: str, request: str, task: str, capability: str, kind: str = "owner_question", only: bool = False) -> Any:
        return NS(id=ident, request_id=request, task_id=task, capability=capability, kind=kind, options_only=only,
                  state="pending")
    items = [ap("q1", "r1", "", "ferb.confirm"), ap("q2", "r9", "t1.worker", "ferb.retry"),
             ap("q3", "r9", "t2.worker", "ferb.confirm"), ap("q4", "r1", "", "owner.ask"),
             ap("q5", "r1", "", "owner.ask", only=True), ap("q6", "r1", "", "memory.forget"),
             ap("q7", "r1", "", "owner.ask", kind="secret"), ap("q8", "r1", "", "conversation.approve", kind="other"),
             ap("q9", "r1", "", "owner.ask")]
    sup = NS(state=NS(approvals=Approvals(items)), tasks=NS(tree_of=lambda t: t.split(".", 1)[0]), journal=sc.journal,
             publish=lambda topic, data: log.append(["publish", topic, data]))
    sc.step("settle_questions", ["r1", "also cc Bob", ["q4", "q5", "q6", "q7", "q8", "zz"], "t1"],
            steering.settle_questions, sup, "r1", "also cc Bob", ["q4", "q5", "q6", "q7", "q8", "zz"], "t1")
    sc.step("settle_questions", ["r1", "  ", ["q9"], None], steering.settle_questions, sup, "r1", "  ", ["q9"], None)
    sc.step("log", [], lambda: list(log))
    log.clear()
    # the standing scope a correction restates
    a = SendScope(capability="browser.act", identity="owner", account="me", site="x.com", recipients=[], content=None)
    b = SendScope(capability="mail.send", identity="owner", recipients=["ana@x.com"], content="hi")
    c = SendScope(capability="mail.send", identity="owner", recipients=["zed@y.com"])

    class Standing:
        def __init__(self, held: list[Any]) -> None:
            self.held = held

        def in_force(self, tree: str) -> list[Any]:
            return list(self.held)

        def scopes(self, tree: str) -> list[Any]:
            return [s for s, _ in self.held]

        def revoke(self, tree: str) -> None:
            log.append(["revoke", tree])

        def grant_each(self, tree: str, pairs: list[Any]) -> None:
            log.append(["grant_each", tree, [[s.model_dump(), a_] for s, a_ in pairs]])

        def grant(self, tree: str, scopes: list[Any], approval: str) -> None:
            log.append(["grant", tree, [s.model_dump() for s in scopes], approval])
    cases = [("unchanged", [], None, False), ([a, b], [(a, "stated:x")], None, False),
             ([a, b, c], [(a, "stated:x")], None, False), ([a, c], [(a, "schedule:s1"), (b, "schedule:s1")], None, True),
             ([b], [(a, "s"), (b, "s")], {"plan": True}, True), ([], [(a, "s")], {"plan": True}, False),
             ([a], [(a, "s")], None, False)]
    for n, (standing, held, plan, unattended) in enumerate(cases):
        plan_obj = Plan.model_validate({"steps": [{"id": "x", "kind": "do"}], "standing_sends": [a.model_dump()]}) \
            if plan else None
        tree_ = NS(id=f"t{n}", spec=NS(plan=plan_obj), unattended_scopes={} if unattended else None,
                   worker=NS(request_id="r-sched" if n == 3 else "r1"))
        sup.gateway = NS(redact=sc.redactor)
        sup.standing = Standing(held)
        sup.owner_typed_scopes = lambda tree, revised: [s for s in revised if "zed@y.com" not in s.recipients]
        sup.plans = NS(update_plan=lambda tid, p: log.append(["update_plan", tid, [s.model_dump() for s in p.standing_sends]]))
        r = sc.call(steering.restate_scope, sup, tree_, standing if standing == "unchanged" else list(standing))
        out = r.get("out")
        sc.steps.append(sc.fx({"op": "restate_scope", "args": [
            standing if standing == "unchanged" else [s.model_dump() for s in standing],
            [[s.model_dump(), a_] for s, a_ in held], bool(plan), unattended, tree_.worker.request_id, tree_.id],
            **({"out": None if out is None else [out.text, [s.model_dump() for s in out.scopes], out.approval_id,
                                                 out.widened]} if "out" in r else r)}))
        sc.step("log", [], lambda: list(log))
        log.clear()
    # a correction delivered to the live runs
    state = steering.Steering(messages=["first", "second"], delivered={"run-2": 1})

    class Runtime:
        def __init__(self, fail: Any) -> None:
            self.fail = fail

        async def continue_(self, run: Any, text: str) -> None:
            log.append(["continue", run.id, text])
            if self.fail is not None:
                raise self.fail
    tree_ = NS(id="t1", steering=state)
    for run_id, fail in [("run-1", None), ("run-1", None), ("run-2", Unsupported("no")), ("run-3", KeyError("x"))]:
        run = NS(id=run_id, task_id="t1.worker")
        sc.step("deliver", [run_id, None if fail is None else type(fail).__name__],
                lambda run=run, fail=fail: asyncio.run(steering.deliver(sup, tree_, Runtime(fail), run)))
    sc.step("log", [], lambda: [list(log), dict(state.delivered)])


def chat_history_scenario(sc: Scenario) -> None:
    for kind, data in [("chat.message", {"session_id": "owner", "request_id": "r1", "text": "find my file " * 60}),
                       ("chat.reply", {"session_id": "owner", "request_id": "r1", "text": "Done."}),
                       ("task.answer", {"task_id": "t1", "text": "the answer"})]:
        sc.journal.append(kind, data)
        sc.clock.advance(3600 * 24)
    sc.db.conn.execute("INSERT INTO tasks (id, tree_id, request_id, goal, identity, role, scopes_json, status, "
                       "created_at, updated_at) VALUES ('t1', 't1', 'r1', 'g', 'owner', 'worker', '[]', 'done', 'x', 'x')")
    sc.db.conn.execute("INSERT INTO conversations(id) VALUES ('owner')")
    sc.db.conn.execute("INSERT INTO task_context (task_id,conversation_id,spec_json,owner_words,prior_task,"
                       "request_payload) VALUES ('t1','owner','{}','w',NULL,'')")
    sc.seeded()
    c = continuity.Continuity(sc.db, sc.redactor, lambda task: {"external:web:x.com"})
    found = ["id-000003", "id-000001", "nope"]

    async def search(root: str, query: str, since: Any, until: Any, limit: int) -> list[str]:
        return found
    tool = evidence.ChatHistory(c, lambda t: t if t != "orphan" else None, search)
    nosearch = evidence.ChatHistory(c, lambda t: t)
    I = evidence.ChatHistory.Input
    for task, args, which in [("t1", {}, tool), ("t1", {"limit": 1}, tool), ("t1", {"include_tasks": True}, tool),
                              ("t1", {"include_tasks": True, "after": 3, "limit": 1}, tool),
                              ("t1", {"since": "2026-01-02", "until": "2026-01-02"}, tool),
                              ("t1", {"query": "file", "limit": 1, "after": 1}, tool), ("t1", {"query": "x"}, nosearch),
                              ("t1", {"entry_id": "id-000002", "max_chars": 1000, "offset": 3}, tool),
                              ("t1", {"entry_id": "t1", "include_tasks": True}, tool), ("t1", {"entry_id": "t1"}, tool),
                              ("t1", {"task_id": "t1"}, tool), ("t1", {"task_id": "zz"}, tool), ("orphan", {}, tool)]:
        sc.step("chat_history", [task, args, which is nosearch], lambda task=task, args=args, which=which: _answer(
            which.run(I(**args), type("X", (), {"task_id": task})())))


SCENARIOS: list[tuple[str, Callable[[Scenario], None], tuple[str, ...], tuple[str, ...]]] = [
    ("plan_store", plan_store_scenario, (), ("task_plans", "plan_steps", "plan_events")),
    ("ledger", ledger_scenario, (), ("action_ledger",)),
    ("tree", tree_scenario, (), ("tasks",)),
    ("continuity", continuity_scenario, ("journal", "tasks", "spans", "conversations", "task_context"),
     ("conversations", "current_conversation", "task_context")),
    ("conversations", conversations_scenario, (),
     ("delegated_conversations", "conversation_events", "conversation_turns", "conversation_workers",
      "conversation_owner_changes", "forgotten_refs", "attention_notices")),
    ("conversation_runner", runner_scenario, (), ("delegated_conversations", "conversation_turns", "conversation_events")),
    ("parts", parts_scenario, (), ("journal",)),
    ("loops", loops_scenario, (), ("loop_state",)),
    ("plan_executor", executor_scenario, ("task_plans", "plan_steps", "plan_events"), ("plan_steps", "task_plans")),
    ("attention", attention_scenario, ("spans",), ("attention_notices", "journal", "spans")),
    ("cards", cards_scenario, ("spans", "confirmations", "downloads"), ("spans",)),
    ("evidence", evidence_scenario, ("spans",), ()),
    ("misc", misc_scenario, ("journal", "tasks"), ("episodes", "episode_how")),
    ("decide", decide_scenario, (), ("decisions",)),
    ("steering", steering_scenario, (), ("journal",)),
    ("chat_history", chat_history_scenario, ("journal", "tasks", "conversations", "task_context"), ()),
]


def make_stores(root: Path, fx: Callable[[Any], Any], call: Callable[..., dict[str, Any]]) -> list[dict[str, Any]]:
    out = []
    for name, fn, seeds, tables in SCENARIOS:
        sc = Scenario(root / "stores", name, fx, call, seeds, tables)
        os_home = Path.home()
        fn(sc)
        out.append(sc.result())
        _ = os_home
        sc.db.conn.close()
    return out
