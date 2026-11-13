"""Fixtures for the supervisor's paths no earlier recording reaches (ENG-3): the running summary's fold, several asks
in one answer and their card, the checklist of a plan of three or more steps, the memory and forget side entries, an
answer routed to an open question in chat, the planner's runtime trouble, and the rest listed in SCENARIOS below.
Recorded from the real Python daemon. README.md beside this file is the map.

Run from the repo root:  uv run python rust/fixtures/supervisor/make_more.py [scenario function names]

It reuses both recorders by import, as make_control.py does: rust/fixtures/daemon/make.py runs each scenario on a real
`ferb.daemon.main.Daemon` with the test suite's fake model (its stand-ins, its guard against launching a browser, its
fixed folder `$TMPDIR/ferbdx` written `/FX`, its ids and counters), and rust/fixtures/supervisor/make.py records the
supervisor's composition points while it runs. Added here:

- a tripwire on every way Python starts a program (`subprocess.Popen`, `asyncio.create_subprocess_exec`,
  `os.system`, `os.posix_spawn`): only FERB's own episode-card reader (`python -I -m ferb.memory.card_reader`, run on
  the scenario's temporary home, as the daemon recorder lets it run) may start; anything else is refused and the run
  fails naming it. Every launch is printed at the end.
- `port` records: each call the supervisor makes to a port another part fills (memory's `remember`, `forget_request`),
  its arguments, what it answered and the record ids it took (`ids`: the id counter before and after: the journal rows
  that part wrote in between are its own), so a Rust replay can answer the same, take the same ids and check it was
  asked the same.
- `seed` steps: journal rows written before the first message (a long earlier conversation), as the daemon's own
  journal writes them; a Rust replay inserts them as it inserts the start's rows (their ids precede the first request).

Output (in this folder): `more_scenarios.json` (make_control.py's shape: scenarios, kept records, inputs, catalogs)
and `more_bodies.json` (texts of 2,048 characters or more no other bodies file holds).
"""

import asyncio
import importlib.util
import json
import os
import subprocess
import sys
from pathlib import Path
from typing import Any

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[2]
_spec = importlib.util.spec_from_file_location("supervisor_make", HERE / "make.py")
assert _spec is not None and _spec.loader is not None
sm = importlib.util.module_from_spec(_spec)
sys.modules["supervisor_make"] = sm
_spec.loader.exec_module(sm)          # imports rust/fixtures/daemon/make.py: the stand-ins, the fixed folder
dm = sm.dm

STALL = dm.STALL
CHAT = dm.CHAT
Model, Scenario, Gate = dm.Model, dm.Scenario, dm.Gate
planner, plan_task, control, worker, checker = dm.planner, dm.plan_task, dm.control, dm.worker, dm.checker
plan, step, side = dm.plan, dm.step, dm.side
chat_turn, tree_of, pending_approvals, on_loop = dm.chat_turn, dm.tree_of, dm.pending_approvals, dm.on_loop

# ---------------------------------------------------------------------------------------------------------------
# The tripwire: nothing but FERB's own card reader starts
# ---------------------------------------------------------------------------------------------------------------

LAUNCHED: list[str] = []
REFUSED: list[str] = []


def _allowed(argv: Any) -> bool:
    args = [str(a) for a in (argv if isinstance(argv, (list, tuple)) else [argv])]
    return args[:4] == [sys.executable, "-I", "-m", "ferb.memory.card_reader"]


_plain_popen_init = subprocess.Popen.__init__


def _popen_init(self: Any, args: Any, *a: Any, **k: Any) -> None:
    LAUNCHED.append(f"Popen {args!r}")
    if not _allowed(args):
        REFUSED.append(f"Popen {args!r}")
        raise OSError(f"make_more.py tripwire: refused to start {args!r}")
    _plain_popen_init(self, args, *a, **k)


_plain_exec = asyncio.create_subprocess_exec


async def _exec(program: Any, *args: Any, **k: Any) -> Any:
    argv = [program, *args]
    LAUNCHED.append(f"create_subprocess_exec {argv!r}")
    if not _allowed(argv):
        REFUSED.append(f"create_subprocess_exec {argv!r}")
        raise OSError(f"make_more.py tripwire: refused to start {argv!r}")
    return await _plain_exec(program, *args, **k)


async def _shell(cmd: Any, *a: Any, **k: Any) -> Any:
    REFUSED.append(f"create_subprocess_shell {cmd!r}")
    raise OSError(f"make_more.py tripwire: refused to start {cmd!r}")


def _system(cmd: Any) -> int:
    REFUSED.append(f"os.system {cmd!r}")
    raise OSError(f"make_more.py tripwire: refused to start {cmd!r}")


def _spawn(path: Any, argv: Any, *a: Any, **k: Any) -> int:
    REFUSED.append(f"os.posix_spawn {argv!r}")
    raise OSError(f"make_more.py tripwire: refused to start {argv!r}")


class _PartsClock:
    """parts.py `_next_seq` reads the wall clock (`datetime.now(timezone.utc)`): here the scenario's FakeClock, so the
    seqs are the same on every run (a Rust replay injects its clock the same way)."""

    @staticmethod
    def now(tz: Any = None) -> Any:
        from datetime import timezone
        return dm.LIVE[-1].daemon.clock.now().astimezone(tz or timezone.utc)

    def __getattr__(self, name: str) -> Any:
        import datetime as real
        return getattr(real.datetime, name)


class _Uuids:
    """`uuid.uuid4()` as a counter (a loop's notice id, notify.py `_loop_notice`), so two runs write the same bytes."""

    n = 0

    def __call__(self) -> Any:
        import uuid
        _Uuids.n += 1
        return uuid.UUID(int=_Uuids.n)


def install_stubs() -> None:
    import uuid
    import ferb.orchestrator.parts as parts_mod
    dm.stub(parts_mod, "datetime", _PartsClock(), "a card row's seq reads the scenario's FakeClock, not the wall clock")
    dm.stub(uuid, "uuid4", _Uuids(), "a loop notice's id: a counter")


def install_tripwire() -> None:
    dm.MP.setattr(subprocess.Popen, "__init__", _popen_init)
    dm.MP.setattr(asyncio, "create_subprocess_exec", _exec)
    dm.MP.setattr(asyncio, "create_subprocess_shell", _shell)
    dm.MP.setattr(os, "system", _system)
    dm.MP.setattr(os, "posix_spawn", _spawn)
    dm.MP.setattr(os, "posix_spawnp", _spawn)
    # modules that bound the function at import (`from asyncio import create_subprocess_exec` and the like)
    for mod in list(sys.modules.values()):
        name = getattr(mod, "__name__", "") or ""
        if not name.startswith("ferb"):
            continue
        for attr, fake in (("create_subprocess_exec", _exec), ("create_subprocess_shell", _shell)):
            if getattr(mod, attr, None) is _plain_exec or (attr == "create_subprocess_shell"
                                                           and getattr(mod, attr, None) is not None
                                                           and getattr(mod, attr) is not _shell
                                                           and callable(getattr(mod, attr))):
                dm.MP.setattr(mod, attr, fake)


# ---------------------------------------------------------------------------------------------------------------
# Port records: what the supervisor asked another part, and what it answered
# ---------------------------------------------------------------------------------------------------------------

def _plain(v: Any) -> Any:
    return json.loads(json.dumps(v, default=lambda o: o.model_dump() if hasattr(o, "model_dump")
                                 else sorted(o) if isinstance(o, (set, frozenset)) else str(o)))


PORTS = ("remember", "forget_request")


def patch_ports(d: Any) -> None:
    """Wrap the supervisor's port callables of a freshly built daemon (each scenario's, each restart's)."""
    sup = d.supervisor
    if sup is None:
        return
    for name in PORTS:
        plain = getattr(sup, name, None)
        if plain is None or getattr(plain, "_recorded", False):
            continue

        def wrapped(*a: Any, _plain_fn: Any = plain, _name: str = name, **k: Any) -> Any:
            ids = sup.ids
            before = ids._n
            out = _plain_fn(*a, **k)
            if asyncio.iscoroutine(out):
                async def later() -> Any:
                    got = await out
                    sm.record("port", {"port": _name, "args": _plain(list(a)), "kwargs": _plain(k),
                                       "answer": _plain(got), "ids": [before, ids._n]})
                    return got
                return later()
            sm.record("port", {"port": _name, "args": _plain(list(a)), "kwargs": _plain(k), "answer": _plain(out),
                               "ids": [before, ids._n]})
            return out
        wrapped._recorded = True  # type: ignore[attr-defined]
        setattr(sup, name, wrapped)


dm.PREPARE.append(lambda d: None)        # (the supervisor is built at the daemon's start; wrapped after `_boot`)
_plain_boot = dm.Scenario._boot


def _boot(self: Any, ids: str) -> Any:
    h = _plain_boot(self, ids)
    on_loop(h, lambda: patch_ports(h.daemon))
    return h


dm.Scenario._boot = _boot


def seed(scn: Any, rows: list[tuple[str, dict[str, Any], str]]) -> None:
    """Journal rows written before the first message, through the daemon's own journal (its ids and clock)."""
    def write() -> list[str]:
        j = scn.host.daemon.journal
        return [j.append(kind, data, origin=origin).id for kind, data, origin in rows]
    ids = on_loop(scn.host, write)
    scn.steps.append({"do": "seed", "rows": len(rows), "ids": ids})
    scn.settle()


SUP = "ferb/orchestrator/supervisor.py"

# ---------------------------------------------------------------------------------------------------------------
# 1. The running summary's fold (ORCH-303, ORCH-304, ORCH-326)
# ---------------------------------------------------------------------------------------------------------------


def long_text(topic: str, n: int) -> str:
    """An invented message of about `n` bytes."""
    unit = f"Notes on the invented {topic}: the north bed gets beans, the south bed gets squash, water twice a week. "
    return (unit * (n // len(unit) + 1))[:n].rstrip() + "."


def earlier_chat(big: int) -> list[tuple[str, dict[str, Any], str]]:
    """23 earlier exchanges in chat s1: the first three owner messages long (about 7,000 bytes each), the rest short."""
    rows: list[tuple[str, dict[str, Any], str]] = []
    for i in range(1, 24):
        rid = f"seed-{i:02d}"
        text = long_text(f"garden plan {i}", 7000) if i <= big else f"Invented question number {i}?"
        rows.append(("chat.message", {"session_id": "s1", "request_id": rid, "text": text, "untrusted_parts": 0,
                                      "input_paths": []}, "owner"))
        rows.append(("chat.reply", {"session_id": "s1", "request_id": rid, "text": f"Invented answer number {i}.",
                                    "provenance": {"kinds": ["ferb", "inferred", "owner", "private"], "values": {}}},
                     "ferb,inferred,owner,private"))
    return rows


def summary(text: str) -> str:
    return json.dumps({"summary": text})


def mm_summary_fold() -> dict[str, Any]:
    """`_keep_recent` after a reply: beyond the last 20 exchanges lie three ~7,000-byte messages (more than
    FOLD_CHUNK_TOKENS), so the fold runs in the background: `_summarize` makes two `_fold` calls (each at most one chunk,
    `_foldable`), each written as a chat.summary row covering its entries. The next turn's planner reads the summary in
    its conversation header and only the entries after what it covers (`_summary`, `_covered`)."""
    m = Model([planner("reply", "The beans go in the north bed."),
               planner("reply", "Squash goes in the south bed.")],
              {"summary": [summary("The owner planned an invented garden: beans north, squash south."),
                           summary("The owner planned an invented garden: beans north, squash south, watering twice "
                                   "a week; then asked short invented questions.")]})
    scn = Scenario("summary_fold", "A long chat: after the reply, the conversation beyond the last 20 exchanges folds "
                   "into FERB's running summary in two calls; the next planner turn reads the summary.",
                   CHAT + [f"{SUP}:4186 _keep_recent", f"{SUP}:4207 _summarize", f"{SUP}:4223 _fold",
                           f"{SUP}:4270 _foldable", f"{SUP}:4161 _summary", f"{SUP}:4166 _covered"], m)
    seed(scn, earlier_chat(3))
    chat_turn(scn, "where do the beans go?")
    scn.wait_model("summary", 2)
    scn.settle()
    chat_turn(scn, "and the squash?")
    return scn.finish()


# ---------------------------------------------------------------------------------------------------------------
# 2. Several asks in one answer, their card, the checklist (ORCH-319, ORCH-543, ORCH-626, APP-894/895)
# ---------------------------------------------------------------------------------------------------------------

def part(id: str, quote: str, label: str, *, direct: str | None = None, answer_step: str | None = None,
         conditions: list[int] | None = None) -> dict[str, Any]:
    return {"id": id, "quote": quote, "label": label, "direct": direct, "answer_step": answer_step,
            "conditions": conditions or []}


def mm_several_asks() -> dict[str, Any]:
    """Two asks in one message: one answered at once (`direct`), one by the task's single step. The card: the direct
    part said in the first reply (`say_direct`), the step's part posted once when its check accepts (`_post_part`,
    `_part_event`), the saved answer an index with "N of M done" (`_final_card`, `card_state`)."""
    spec = plan_task("Count the invented files in the workspace", name="File count",
                     parts=[part("p1", "what's 2 plus 2", "Sum", direct="4"),
                            part("p2", "count my files", "File count", answer_step="worker", conditions=[0])])
    m = Model([planner("task", "2 plus 2 is 4. Counting your files now.", spec)],
              {"worker": worker("done", "There are 3 files."), "worker.verify": checker(True)})
    scn = Scenario("several_asks", "One message with two asks: a direct part and a task's part, one card.",
                   CHAT + [f"{SUP}:4465 run_tree", f"{SUP}:4713 _card_payload", f"{SUP}:4767 _part_event",
                           f"{SUP}:5374 _post_part", f"{SUP}:7495 _final_card", f"{SUP}:4851 card_state"], m)
    chat_turn(scn, "what's 2 plus 2, and count my files")
    app = scn.conn("app")
    scn.call(app, "chat.status", {})
    scn.close(app)
    return scn.finish()


def mm_plan_parts_checklist() -> dict[str, Any]:
    """A plan of three steps (the checklist, ORCH-626), two of them answering parts: the first step's check fails,
    so the step after it can never run and its part is settled at once as couldn't-do (`_never_ran`, `_close_rows`,
    `_couldnt_do`)."""
    spec = plan_task("Find the invented recipe, list its ingredients, then price them", name="Recipe costs",
                     conds=[dm.condition("The ingredients are listed"), dm.condition("Each ingredient is priced")],
                     plan=plan(step("find", "Find the invented recipe", label="Find recipe"),
                               step("list", "List the recipe's ingredients", label="List ingredients",
                                    depends_on=["find"], conditions=["The ingredients are listed"]),
                               step("price", "Price each ingredient", label="Price them", depends_on=["list"],
                                    conditions=["Each ingredient is priced"])),
                     parts=[part("p1", "list the ingredients", "Ingredients", answer_step="list", conditions=[0]),
                            part("p2", "price them", "Prices", answer_step="price", conditions=[1])])
    m = Model([planner("task", "Finding the recipe first.", spec)],
              {"step.find": worker("done", "Found the invented soup recipe.", part_updates=[]),
               "step.find.verify": checker(True),
               "step.list": worker("not_done", "", reason="The recipe page lists no ingredients.", part_updates=[]),
               "step.list.verify": checker(None, next="fail", reason="The page has no ingredient list.")})
    scn = Scenario("plan_parts_checklist", "A three-step plan with a checklist and two parts: the middle step fails, "
                   "the last never runs, and both parts settle.",
                   CHAT + [f"{SUP}:4736 _checklist", f"{SUP}:5430 _close_rows", f"{SUP}:5453 _never_ran",
                           f"{SUP}:5415 _couldnt_do"], m)
    chat_turn(scn, "find the soup recipe, list the ingredients and price them")
    return scn.finish()


# ---------------------------------------------------------------------------------------------------------------
# 7. Memory and forget side entries
# ---------------------------------------------------------------------------------------------------------------

def remember(content: str, quote: str, *, kind: str = "preference", subject: str = "owner",
             key: str = "") -> dict[str, Any]:
    return {"kind": kind, "subject": subject, "key": key or content.lower().replace(" ", "_")[:30],
            "content": content, "quote": quote, "login_origin": None, "until": None, "replaces": [],
            "review_in_days": 0}


def memory_items(scn: Any) -> list[str]:
    def read() -> list[str]:
        rows = scn.host.daemon.db.conn.execute("SELECT id FROM memory_items ORDER BY rowid").fetchall()
        return [r[0] for r in rows]
    return on_loop(scn.host, read)


def mm_remember_then_forget() -> dict[str, Any]:
    """A remember side entry in a reply turn (quote-checked against the owner's words, `_quote_source`,
    `_remember_about`; the memory port saves it and code says what was saved), then a forget of that item by id: a
    forget asks the owner first (I2), and their Yes in chat answers it."""
    m = Model([planner("reply", "Noted.", side_entries=side(remember=[remember(
        "Prefers tea over coffee", "I prefer tea over coffee", key="drink")]))])
    scn = Scenario("remember_then_forget", "A remember side entry saved with code's receipt, then a forget of it.",
                   CHAT + [f"{SUP}:3344 _side_entries", f"{SUP}:3321 _quote_source", f"{SUP}:3330 _remember_about"],
                   m)
    chat_turn(scn, "remember that I prefer tea over coffee")
    items = memory_items(scn)
    item = items[-1] if items else "missing"
    m.script.sup.append(planner("reply", "I'll forget that.", side_entries=side(
        forget=[{"item_id": item, "quote": "forget that I prefer tea"}])))
    chat_turn(scn, "forget that I prefer tea")
    return scn.finish()


# ---------------------------------------------------------------------------------------------------------------
# 5. An answer routed to an open question in chat (`_answer`)
# ---------------------------------------------------------------------------------------------------------------

def mm_answer_in_chat() -> dict[str, Any]:
    """A plan step asks the owner in place; the owner answers in chat and the planner routes it to the question by
    id (`answer_question`): the answer recorded is the owner's own words (`_answer`), the step carries on."""
    spec = plan_task("Sort the invented downloads then report", name="Sort downloads",
                     plan=plan(step("sort", "Sort the downloads"),
                               step("report", "Report what was sorted", depends_on=["sort"])))
    m = Model([planner("task", "On it.", spec)],
              {"step.sort": [worker("not_done", "", reason="Which folder should I sort?",
                                    question="Which folder should I sort?"),
                             worker("done", "Sorted the Downloads folder.")],
               "step.sort.verify": [checker(None, next="ask", question="Which folder should I sort?"),
                                    checker(True)],
               "step.report": worker("done", "Reported."), "step.report.verify": checker(True)})
    scn = Scenario("answer_in_chat", "A step's question answered in chat: the planner routes the owner's words to "
                   "it by id.", CHAT + [f"{SUP}:3155 _answer", f"{SUP}:3731 _question_task",
                                        f"{SUP}:3738 _question_root"], m)
    turn = scn.stream(scn.conn("chat"), "chat.send", {"session_id": "s1", "text": "sort my downloads and tell me"})
    scn.wait("an approval is pending", lambda: bool(pending_approvals(scn)))
    scn.settle()
    ap = pending_approvals(scn)[0].id
    m.script.sup.append(planner("reply", "Got it.", side_entries=side(answer_question={
        "approval_id": ap, "answer": "the Downloads folder"}), answers_question=True))
    chat_turn(scn, "the Downloads folder")
    scn.end(turn)
    return scn.finish()


# ---------------------------------------------------------------------------------------------------------------
# 2b, 3. The checklist of a single ask, ask and wait steps
# ---------------------------------------------------------------------------------------------------------------

def mm_checklist_single() -> dict[str, Any]:
    """APP-894, ORCH-626: a single ask whose plan has three steps carries its checklist (steps and their states) in
    the plan event and its end (`_checklist`, `_plan_lines`, `_step_payload`)."""
    spec = plan_task("Collect, sort and count the invented receipts", name="Receipts",
                     plan=plan(step("collect", "Collect the receipts", label="Collect"),
                               step("sort", "Sort them by month", label="Sort", depends_on=["collect"]),
                               step("count", "Count them", label="Count", depends_on=["sort"])))
    m = Model([planner("task", "On it.", spec)],
              {"step.collect": worker("done", "Collected 4 receipts."), "step.collect.verify": checker(True),
               "step.sort": worker("done", "Sorted into 2 months."), "step.sort.verify": checker(True),
               "step.count": worker("done", "There are 4 receipts."), "step.count.verify": checker(True)})
    scn = Scenario("checklist_single", "A single ask with a three-step plan: its checklist in the plan event and the "
                   "outcome.", CHAT + [f"{SUP}:4736 _checklist", f"{SUP}:4724 _plan_lines"], m)
    chat_turn(scn, "collect, sort and count my receipts")
    return scn.finish()


def mm_ask_step() -> dict[str, Any]:
    """ORCH-322: a plan's ask step (`_plan_question`, `_ask_card`): the question waits in place with its options; the
    owner's pick (chat.approve) is the step's answer and the step after it reads it."""
    spec = plan_task("Paint the invented fence in the colour the owner picks", name="Fence colour",
                     plan=plan(step("pick", "Ask which colour", kind="ask", question="Which colour for the fence?",
                                    options=["Red", "Blue"]),
                               step("paint", "Paint the fence", depends_on=["pick"])))
    m = Model([planner("task", "I'll ask you first.", spec)],
              {"step.paint": worker("done", "Painted it blue."), "step.paint.verify": checker(True)})
    scn = Scenario("ask_step", "A plan's ask step waits for the owner's pick; the next step reads the answer.",
                   CHAT + [f"{SUP}:5039 _plan_question", f"{SUP}:5061 _ask_card"], m)
    turn = scn.stream(scn.conn("chat"), "chat.send", {"session_id": "s1", "text": "paint the fence, ask me the colour"})
    scn.wait("an approval is pending", lambda: bool(pending_approvals(scn)))
    scn.settle()
    app = scn.conn("app")
    ap = scn.call(app, "chat.status", {})["result"]["approvals"][0]["approval_id"]
    scn.call(app, "chat.approve", {"approval_id": ap, "granted": True, "choice": 2})
    scn.end(turn)
    scn.close(app)
    return scn.finish()


def mm_wait_step() -> dict[str, Any]:
    """A plan's wait step whose time has come (plan_executor `_wait`): waiting, then done, and the step after it
    runs."""
    s1 = step("hold", "Wait until nine", kind="wait")
    s1["until"] = "2026-01-01T09:00:00+00:00"
    spec = plan_task("Wait until nine then check the invented mailbox", name="Mailbox check",
                     plan=plan(s1, step("check", "Check the mailbox", depends_on=["hold"])))
    m = Model([planner("task", "Waiting until nine.", spec)],
              {"step.check": worker("done", "The mailbox has 2 letters."), "step.check.verify": checker(True)})
    scn = Scenario("wait_step", "A plan's wait step for a time already come, then its next step.",
                   CHAT + ["ferb/orchestrator/plan_executor.py:47 PlanExecutor._wait"], m)
    chat_turn(scn, "at nine check my mailbox")
    return scn.finish()


# ---------------------------------------------------------------------------------------------------------------
# 5, 6. A worker's question routed by the orchestrator; the checker's runtime down
# ---------------------------------------------------------------------------------------------------------------

def mm_route_question() -> dict[str, Any]:
    """A worker asks the owner through owner.ask: the gateway hands the question to the supervisor's router
    (`route_question`, a model call `worker.question` with the task's context), which says to ask the owner; the
    owner's pick comes back as the tool's answer."""
    m = Model([planner("task", "On it.", plan_task("Order the invented poster in the owner's size",
                                                   name="Poster order"))],
              {"worker.verify": checker(True)})
    scn = Scenario("route_question", "A worker's owner.ask goes through the supervisor's question router, then to "
                   "the owner.", CHAT + [f"{SUP}:7054 route_question"], m)
    m.script.workers["worker"] = dm.acting(scn, "worker", [("owner.ask", {"question": "Which size?",
                                                                          "options": ["Small", "Large"]})],
                                           worker("done", "Ordered the large poster."))
    turn = scn.stream(scn.conn("chat"), "chat.send", {"session_id": "s1", "text": "order the poster"})
    scn.wait("an approval is pending", lambda: bool(pending_approvals(scn)))
    scn.settle()
    app = scn.conn("app")
    ap = scn.call(app, "chat.status", {})["result"]["approvals"][0]["approval_id"]
    scn.call(app, "chat.approve", {"approval_id": ap, "granted": True, "choice": 2})
    scn.end(turn)
    scn.close(app)
    return scn.finish()


def usage_limit() -> Any:
    from ferb.core.types import RuntimeEvent
    return RuntimeEvent("error", {"message": "usage limit reached (invented)", "account": "usage_limit"})


def mm_checker_runtime_down() -> dict[str, Any]:
    """ORCH-315: the checker's runtime reports its usage limit (`_check`'s `_RuntimeDown`): code says so once and
    offers the other signed-in runtimes in place (`_service_down`, `runtime_offer`); the owner's pick switches the
    task's runtime and the check runs again."""
    m = Model([planner("task", "On it.", plan_task("Count the invented files", name="File count"))],
              {"worker": worker("done", "There are 3 files."), "worker.verify": [usage_limit(), checker(True)]})
    scn = Scenario("checker_runtime_down", "The checker's runtime hits its usage limit; FERB offers another runtime "
                   "and the owner's pick carries on.", CHAT + [f"{SUP}:6688 _service_down",
                                                              f"{SUP}:6727 runtime_offer"], m)
    turn = scn.stream(scn.conn("chat"), "chat.send", {"session_id": "s1", "text": "count my files"})
    scn.wait("an approval is pending", lambda: bool(pending_approvals(scn)))
    scn.settle()
    app = scn.conn("app")
    ap = scn.call(app, "chat.status", {})["result"]["approvals"][0]["approval_id"]
    scn.call(app, "chat.approve", {"approval_id": ap, "granted": True, "choice": 1})
    scn.end(turn)
    scn.close(app)
    return scn.finish()


# ---------------------------------------------------------------------------------------------------------------
# 8. The planner's last call, an unbacked part, a correction typed on a pending check's card
# ---------------------------------------------------------------------------------------------------------------

def empty_answer() -> Any:
    from ferb.core.types import RuntimeEvent
    return RuntimeEvent("error", {"message": "no answer twice (invented)", "empty": True})


def mm_last_call_empty() -> dict[str, Any]:
    """ORCH-558: the planner's call ends without an answer (EmptyAnswer): one last call with no lookup tools, told
    that nothing was looked up (`_supervise`'s last call, `_lookup_tools`); its answer is the turn's."""
    m = Model([empty_answer(), planner("reply", "Hello again.")])
    scn = Scenario("last_call_empty", "A planner call without an answer gets one last call with no tools.",
                   CHAT + [f"{SUP}:3475 _supervise (last call)", f"{SUP}:3685 _lookup_tools"], m)
    chat_turn(scn, "hello")
    return scn.finish()


def mm_unbacked_part() -> dict[str, Any]:
    """T-1066: a part the planner says this turn's entries answer (direct "") with no entry quoting its words goes
    back for repair (`_unbacked_parts`); the repaired answer runs."""
    bad = plan_task("Count the invented files", name="File count",
                    parts=[part("p1", "remind me tomorrow", "Reminder", direct=""),
                           part("p2", "count my files", "File count", answer_step="worker", conditions=[0])])
    good = plan_task("Count the invented files", name="File count")
    m = Model([planner("task", "On it.", bad), planner("task", "Counting your files.", good)],
              {"worker": worker("done", "There are 3 files."), "worker.verify": checker(True)})
    scn = Scenario("unbacked_part", "A part answered by no entry of the turn is sent back for repair.",
                   CHAT + [f"{SUP}:1485 _unbacked_parts"], m)
    chat_turn(scn, "remind me tomorrow and count my files")
    return scn.finish()


def mm_correct_pending_check() -> dict[str, Any]:
    """GATE-510, ORCH-538: the owner types in a pending confirmation's box (chat.correct): never a Yes or No; the
    words reach the task as their correction, the check is set aside, the attempt starts again from it."""
    m = Model([planner("task", "Adding the alias.", plan_task("Add the alias ll to ~/.zshrc", name="Shell alias",
                                                               paths=["~/.zshrc"]))],
              {"worker.verify": checker(True)})
    scn = Scenario("correct_pending_check", "A correction typed on a pending confirmation's card sets it aside and "
                   "steers the task.", CHAT + ["ferb/daemon/areas/chat.py:201 _chat_correct",
                                               f"{SUP}:6745 correct_from_question"], m)
    m.script.workers["worker"] = [dm.acting(scn, "worker", [("fs.write", {"path": "~/.zshrc",
                                                                          "content": "alias ll='ls -l'\n",
                                                                          "overwrite": True, "effect": "edit",
                                                                          "why": "add the alias"})],
                                            worker("not_done", "", reason="The change was set aside.")),
                                  worker("done", "Left ~/.zshrc as it is, as you asked.")]
    app = scn.conn("app")
    turn = scn.stream(scn.conn("chat"), "chat.send", {"session_id": "s1", "text": "add an alias ll to my zshrc"})
    scn.wait("an approval is pending", lambda: bool(pending_approvals(scn)))
    scn.settle()
    ap = scn.call(app, "chat.status", {})["result"]["approvals"][0]["approval_id"]
    scn.call(app, "chat.correct", {"approval_id": ap, "text": "actually leave the file as it is"})
    scn.end(turn)
    scn.close(app)
    return scn.finish()


def mm_earlier_result() -> dict[str, Any]:
    """ORCH-619: the planner names an earlier result of this chat by id (`earlier_results`): checked when planned
    (`_result_problems`, `_earlier`, `_worker_room`) and given whole to the worker and checker (`_result_blocks`)."""
    m = Model([planner("task", "On it.", plan_task("Count the invented files", name="File count"))],
              {"worker": [worker("done", "There are 3 files: a.txt, b.txt, c.txt."),
                          worker("done", "The longest name is a.txt (all are 5 characters).")],
               "worker.verify": [checker(True), checker(True)]})
    scn = Scenario("earlier_result", "A task that uses an earlier result of the chat by id.",
                   CHAT + [f"{SUP}:3947 _result_problems", f"{SUP}:3925 _result_blocks", f"{SUP}:3852 _earlier"], m)
    chat_turn(scn, "count my files")

    def answer_id() -> str:
        row = scn.host.daemon.db.conn.execute(
            "SELECT id FROM journal WHERE kind = 'task.answer' ORDER BY seq DESC LIMIT 1").fetchone()
        return str(row[0])
    rid = on_loop(scn.host, answer_id)
    spec = plan_task("Find the longest file name among the counted files", name="Longest name")
    spec["earlier_results"] = [rid]
    m.script.sup.append(planner("task", "Looking at them.", spec))
    chat_turn(scn, "which of them has the longest name?")
    return scn.finish()


# ---------------------------------------------------------------------------------------------------------------
# 9. Loops (L31, ORCH-505, ORCH-322)
# ---------------------------------------------------------------------------------------------------------------

def loop_round(status: str, answer: str, action: str, **k: Any) -> str:
    return worker(status, answer, loop_action=action, probe=None, **k)


def loop_status(scn: Any, tree: str, step_id: str) -> str:
    return str(on_loop(scn.host, lambda: scn.host.daemon.supervisor._loop_status.get((tree, step_id), {})).get(
        "state", ""))


def mm_loop_steer_finish() -> dict[str, Any]:
    """A loop step (`_loop_step` on LoopRunner): its first round carries on and the loop watches (no probe: it waits
    for a backup check, a correction or an event). The owner's correction in chat (control continue) wakes it
    (`wake_loops`): the planner's task state shows the running loop (`_loop_facts`); the second round reports the end
    condition met, a separate check confirms it (`_loop_end_reached`, `_judged`) and the loop, then the task, ends
    done. Its plan line says how the loop ends (`loop_line`)."""
    _Uuids.n = 0
    spec = plan_task("Keep the invented radio playing until the song ends", name="Radio",
                     plan=plan(step("keep", "Keep the invented radio playing",
                                    repeat={"end_when": "the invented song ends", "event": None, "event_match": []})))
    m = Model([planner("task", "Keeping it playing.", spec)],
              {"loop.keep": [loop_round("done", "Playing.", "continue"),
                             loop_round("done", "The song ended.", "finished")],
               "loop.keep.verify": [checker(True), checker(True), checker(True)]})
    scn = Scenario("loop_steer_finish", "A loop round carries on; the owner's correction wakes the loop; the next "
                   "round reports the end, a check confirms it, the loop ends done.",
                   CHAT + [f"{SUP}:5735 _loop_step", f"{SUP}:5677 wake_loops", f"{SUP}:2224 _loop_facts",
                           f"{SUP}:5956 _loop_end_reached", f"{SUP}:5992 _judged"], m)
    turn = scn.stream(scn.conn("chat"), "chat.send", {"session_id": "s1", "text": "keep the radio playing"})
    scn.wait_model("loop.keep")
    tree = tree_of(scn, "loop.keep")
    scn.wait("the loop watches", lambda: loop_status(scn, tree, "keep") == "watching")
    scn.settle()
    m.script.sup.append(planner("reply", "Checking it again now.", control=control(tree, "continue")))
    chat_turn(scn, "is it still playing?")
    scn.end(turn)
    return scn.finish()


def mm_loop_pause_stop() -> dict[str, Any]:
    """A loop round that needs the owner (its check asks): the loop pauses (`_loop_step`'s wait_owner), says so in a
    notice (`on_loop_notice`), and asks the round's own question with Stop (`_pause_question`, LOOP_ASK). chat.status
    shows the paused loop (`loop_statuses`); the owner's Stop ends the loop and its task (`stop_tree`)."""
    _Uuids.n = 0
    spec = plan_task("Keep the invented radio playing", name="Radio",
                     plan=plan(step("keep", "Keep the invented radio playing", repeat={"end_when": "", "event": None, "event_match": []})))
    m = Model([planner("task", "Keeping it playing.", spec)],
              {"loop.keep": loop_round("not_done", "", "blocked", reason="The radio tab closed.",
                                       question="The radio tab closed. Open it again?"),
               "loop.keep.verify": checker(None, next="ask", question="The radio tab closed. Open it again?")})
    scn = Scenario("loop_pause_stop", "A loop round needs the owner: the loop pauses, asks with Stop, and the owner's "
                   "Stop ends it.", CHAT + [f"{SUP}:2205 _pause_question", f"{SUP}:5682 loop_statuses"], m)
    turn = scn.stream(scn.conn("chat"), "chat.send", {"session_id": "s1", "text": "keep the radio playing"})
    scn.wait("an approval is pending", lambda: bool(pending_approvals(scn)))
    scn.settle()
    app = scn.conn("app")
    ap = scn.call(app, "chat.status", {})["result"]["approvals"][0]["approval_id"]
    scn.call(app, "chat.approve", {"approval_id": ap, "granted": True, "choice": 1})
    scn.end(turn)
    scn.close(app)
    return scn.finish()


# ---------------------------------------------------------------------------------------------------------------
# 10. A paused task mentioned the next day, a tab the owner closed, findings on a reason
# ---------------------------------------------------------------------------------------------------------------

def mm_mention_paused() -> dict[str, Any]:
    """ORCH-312: a task the owner paused yesterday is mentioned once when they're back (`mention_paused`,
    `_mention_pause`): Resume / Stop on the pause's one wait; their Stop stops it."""
    m = Model([planner("task", "On it.", plan_task("Tidy the invented notes", name="Tidy notes"))],
              {"worker": STALL})
    scn = Scenario("mention_paused", "A task paused yesterday is mentioned once when the owner is back; Stop "
                   "stops it.", CHAT + [f"{SUP}:1840 mention_paused", f"{SUP}:1863 _mention_pause"], m)
    turn, tree = dm.stalled_task(scn, "tidy my notes")
    m.script.sup.append(planner("reply", "Paused.", control=control(tree, "pause")))
    chat_turn(scn, "hold on")
    scn.wait("the task is paused", lambda: dm.row_status(scn, tree) == "paused")
    scn.settle()
    scn.advance(90000)
    on_loop(scn.host, lambda: asyncio.ensure_future(scn.host.daemon.supervisor.mention_paused()))
    scn.steps.append({"do": "mention_paused"})
    scn.wait("an approval is pending", lambda: bool(pending_approvals(scn)))
    scn.settle()
    app = scn.conn("app")
    ap = scn.call(app, "chat.status", {})["result"]["approvals"][0]["approval_id"]
    scn.call(app, "chat.approve", {"approval_id": ap, "granted": True, "choice": 2})
    scn.end(turn)
    scn.close(app)
    return scn.finish()


def mm_owner_closed_tab() -> dict[str, Any]:
    """HAND-201: the owner closed a tab FERB held for a task (`browser.owner_stopped`, `notify_plan_event` ->
    `owner_stopped_part`): the task pauses and FERB asks whether to continue; their Stop stops it."""
    m = Model([planner("task", "On it.", plan_task("Read the invented recipe page", name="Recipe"))],
              {"worker": STALL})
    scn = Scenario("owner_closed_tab", "The owner closes a tab the task held: it pauses and asks; Stop stops it.",
                   CHAT + [f"{SUP}:2072 owner_stopped_part", f"{SUP}:2307 notify_plan_event"], m)
    turn, tree = dm.stalled_task(scn, "read the recipe page")
    data = {"task_id": tree, "tab": "t1", "part": "tab", "borrowed": False}
    on_loop(scn.host, lambda: scn.host.daemon.supervisor.notify_plan_event("browser.owner_stopped", data))
    scn.steps.append({"do": "plan_event", "topic": "browser.owner_stopped", "data": data})
    scn.wait("an approval is pending", lambda: bool(pending_approvals(scn)))
    scn.settle()
    app = scn.conn("app")
    ap = scn.call(app, "chat.status", {})["result"]["approvals"][0]["approval_id"]
    scn.call(app, "chat.approve", {"approval_id": ap, "granted": True, "choice": 2})
    scn.end(turn)
    scn.close(app)
    return scn.finish()


def mm_reason_findings() -> dict[str, Any]:
    """ORCH-562: the worker reports not done with a reason; the checker fails it with a finding quoting a sentence of
    that reason: code marks or removes the quoted sentence of the reason (`_worker`'s findings on a reason)."""
    reason = "The invented shop's page didn't load. The shop closed for good last year."
    m = Model([planner("task", "On it.", plan_task("Find the invented shop's opening hours", name="Shop hours"))],
              {"worker": worker("not_done", "", reason=reason),
               "worker.verify": checker(None, next="fail", reason=reason, findings=[
                   ("The shop closed for good last year.", "unsupported", "Nothing read says it closed.")]),
               "worker.fix": json.dumps({"answer": ""}), "worker.recheck": json.dumps({"findings": []})})
    scn = Scenario("reason_findings", "A not-done report's reason with a finding: code settles the quoted sentence.",
                   CHAT + [f"{SUP}:6135 _worker (ORCH-562)"], m)
    chat_turn(scn, "find the shop's opening hours")
    return scn.finish()


def mm_runtime_switch_tap() -> dict[str, Any]:
    """ORCH-315: the worker's runtime reports its usage limit; FERB offers the other signed-in runtimes in place
    (`_service_down`, `runtime_offer`); the app's one tap answers it (`answer_runtime_switch`, the daemon's
    `runtimes.switch_task`) and the task carries on with that runtime."""
    m = Model([planner("task", "On it.", plan_task("Count the invented files", name="File count"))],
              {"worker": [usage_limit(), worker("done", "There are 3 files.")], "worker.verify": checker(True)})
    scn = Scenario("runtime_switch_tap", "The worker's runtime hits its usage limit; the app's one tap switches the "
                   "task's runtime and it carries on.", CHAT + [f"{SUP}:6727 runtime_offer",
                                                              f"{SUP}:6733 answer_runtime_switch"], m)
    turn = scn.stream(scn.conn("chat"), "chat.send", {"session_id": "s1", "text": "count my files"})
    scn.wait("an approval is pending", lambda: bool(pending_approvals(scn)))
    scn.settle()
    tree = tree_of(scn, "worker")
    sup = scn.host.daemon.supervisor
    offer = on_loop(scn.host, lambda: sup.runtime_offer(tree))
    runtime = str((offer or {}).get("choices", ["none"])[0])
    answered = on_loop(scn.host, lambda: sup.answer_runtime_switch(tree, runtime))
    scn.steps.append({"do": "runtime_switch", "task_id": tree, "runtime": runtime, "offer": offer,
                      "answered": answered})
    scn.end(turn)
    return scn.finish()


def mm_event_wait() -> dict[str, Any]:
    """A plan's wait step for an event (plan_executor `_wait` on `wakes_on`): the event reaches the supervisor
    (`notify_plan_event`: stored for the plan that waits on it, which wakes), the step ends and the next runs."""
    s1 = step("hold", "Wait for the invented download to finish", kind="wait")
    s1["event"] = "browser.download_finished"
    spec = plan_task("Wait for the invented download then open it", name="Download",
                     plan=plan(s1, step("open", "Open the downloaded file", depends_on=["hold"])))
    m = Model([planner("task", "Waiting for the download.", spec)],
              {"step.open": worker("done", "Opened it."), "step.open.verify": checker(True)})
    scn = Scenario("event_wait", "A plan's wait step for an event; the event wakes it and the next step runs.",
                   CHAT + [f"{SUP}:2307 notify_plan_event", "ferb/orchestrator/plan_executor.py:47 _wait"], m)
    turn = scn.stream(scn.conn("chat"), "chat.send", {"session_id": "s1", "text": "open the download when it's done"})
    scn.wait("the plan waits", lambda: bool(on_loop(scn.host, lambda: list(
        scn.host.daemon.supervisor._plan_runs))))
    scn.settle()
    data = {"download_id": "d1", "file": "invented.pdf"}
    on_loop(scn.host, lambda: scn.host.daemon.supervisor.notify_plan_event("browser.download_finished", data))
    scn.steps.append({"do": "plan_event", "topic": "browser.download_finished", "data": data})
    scn.end(turn)
    return scn.finish()


# ---------------------------------------------------------------------------------------------------------------
# Output, as make_control.py writes it
# ---------------------------------------------------------------------------------------------------------------

# What the Rust replay reads of the composition records: each planner call's environment and live facts, each role's
# instruction inputs, each context's activity search and desktop observations (make_control.py's KEPT), and the ports.
KEPT = {"supervise": ("environment", "live_facts"), "system": ("live", "inputs"),
        "context": ("activity", "disturbance"), "port": ("port", "args", "kwargs", "answer", "ids")}


def kept_records(records: dict[str, list[dict[str, Any]]]) -> tuple[dict[str, Any], dict[str, Any]]:
    inputs: dict[str, Any] = {}
    out: dict[str, Any] = {}
    for name, recs in records.items():
        kept = []
        for r in recs:
            keys = KEPT.get(r["kind"])
            if keys is None:
                continue
            row = {"kind": r["kind"], **{k: r.get(k) for k in keys}}
            if r["kind"] == "context":          # what the memory port gave the worker (`_worker_memory`'s blocks)
                row["memory"] = next((c["out"] for c in r.get("calls", []) if c["kind"] == "_worker_memory"), [])
            if r["kind"] == "system":
                key = dm.sha(json.dumps(row["inputs"], sort_keys=True))
                inputs[key] = row["inputs"]
                row["inputs"] = {"inputs": key}
            kept.append(row)
        out[name] = kept
    return out, inputs


def write(name: str, value: Any) -> int:
    text = dm.fx(json.dumps(value, ensure_ascii=False, separators=(",", ":"))) + "\n"
    (HERE / name).write_text(text, encoding="utf-8")
    return len(text.encode("utf-8"))


SCENARIOS = [mm_summary_fold, mm_several_asks, mm_plan_parts_checklist, mm_remember_then_forget, mm_answer_in_chat,
             mm_checklist_single, mm_ask_step, mm_wait_step, mm_route_question, mm_checker_runtime_down,
             mm_last_call_empty, mm_unbacked_part, mm_correct_pending_check, mm_earlier_result,
             mm_loop_steer_finish, mm_loop_pause_stop, mm_mention_paused, mm_owner_closed_tab, mm_reason_findings,
             mm_runtime_switch_tap, mm_event_wait]


def main() -> None:
    install_stubs()
    install_tripwire()
    sm.patch_methods()
    only = set(sys.argv[1:])
    out: list[dict[str, Any]] = []
    records: dict[str, Any] = {}
    failed: list[str] = []
    for fn in SCENARIOS:
        if only and fn.__name__ not in only:
            continue
        sm.CURRENT["records"] = []
        sm.SENT.clear()
        try:
            got = fn()
        except Exception as e:                          # noqa: BLE001
            import traceback
            traceback.print_exc()
            failed.append(f"{fn.__name__}: {type(e).__name__}: {e}")
            for h in list(dm.LIVE):
                try:
                    h.stop()
                except Exception:                       # noqa: BLE001
                    pass
                dm.LIVE.remove(h)
            continue
        sent = [dm.fx(json.dumps([dm.BODIES[p["sha256"]] if isinstance(p, dict) else p for p in c["parts"]]))
                for c in got.get("model_calls", [])]
        if [dm.fx(json.dumps(parts)) for parts in sm.SENT] != sent:
            failed.append(f"{fn.__name__}: the recorded assemblies aren't the parts the model received")
        out.append(got)
        recs = sm.CURRENT["records"]
        records[got["name"]] = recs
        sm.CURRENT["records"] = None
        print(f"{fn.__name__}: {len(got.get('model_calls', []))} model calls, {len(recs)} records", file=sys.stderr)
    print("programs started: " + ("; ".join(LAUNCHED) or "none"), file=sys.stderr)
    if REFUSED:
        failed.append("the tripwire refused: " + "; ".join(REFUSED))
    if dm.PROBLEMS:
        failed.append("problems:\n" + "\n".join(dm.PROBLEMS))
    if failed:
        raise SystemExit("\n".join(failed))
    kept, inputs = kept_records(records)
    value = {"scenarios": out, "records": sm.interned(kept), "inputs": sm.interned(inputs),
             "catalogs": sm.interned(sm.CATALOGS)}
    if only:
        old = json.loads((HERE / "more_scenarios.json").read_text(encoding="utf-8")) \
            if (HERE / "more_scenarios.json").exists() else {"scenarios": [], "records": {}, "inputs": {},
                                                               "catalogs": {}}
        names = {s["name"] for s in out}
        value = {"scenarios": [s for s in old["scenarios"] if s["name"] not in names] + out,
                 "records": {**old["records"], **value["records"]}, "inputs": {**old["inputs"], **value["inputs"]},
                 "catalogs": {**old["catalogs"], **value["catalogs"]}}
    size = write("more_scenarios.json", value)
    known: set[str] = set()
    for rel in ("rust/fixtures/daemon/bodies.json", "rust/fixtures/supervisor/bodies.json",
                "rust/fixtures/supervisor/control_bodies.json"):
        known |= set(json.loads((REPO / rel).read_text(encoding="utf-8")))
    bodies = {k: v for k, v in sorted({**dm.BODIES, **sm.BODIES}.items()) if k not in known}
    if only and (HERE / "more_bodies.json").exists():
        bodies = {**json.loads((HERE / "more_bodies.json").read_text(encoding="utf-8")), **bodies}
        bodies = dict(sorted(bodies.items()))
    size_b = write("more_bodies.json", bodies)
    print(f"more_scenarios.json: {size} bytes; more_bodies.json: {size_b} bytes")


if __name__ == "__main__":
    try:
        main()
    finally:
        dm.MP.undo()
        import shutil
        shutil.rmtree(dm.ROOT, ignore_errors=True)
