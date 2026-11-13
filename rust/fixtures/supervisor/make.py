"""Fixtures for the supervisor's port (ENG-3): what ferb/orchestrator/supervisor.py composes, recorded from the Python
build. README.md beside this file is the map.

Run from the repo root:  uv run python rust/fixtures/supervisor/make.py

Two parts:

- `units.json`: the module-level functions and the pure methods of supervisor.py, run on invented inputs (the
  owner-facing lines, the planner's history labels, report short forms, the front-window block, the summary call's
  blocks, the chat line's arrival order, the conversation timeline and the planner's other blocks built on a stand-in
  `self` that answers their reads).
- `scenarios.json`: the 28 chat scenarios of rust/fixtures/daemon/make.py run again with the supervisor's
  composition points recorded: every `_system` call (its six inputs and its text), every `_run_once` call's context
  blocks (all fields) with the budget it was assembled in and the parts that resulted, and every call of the planner's
  block builders with the reads it made and the block it returned.

The daemon scenarios are imported from rust/fixtures/daemon/make.py (same stand-ins, same fixed folder `$TMPDIR/ferbdx`
written `/FX`, same ids), so their prompts are the ones in chat_scenarios.json.
"""

import asyncio
import dataclasses
import importlib.util
import json
import sys
from datetime import datetime, timedelta, timezone
from pathlib import Path
from types import SimpleNamespace
from typing import Any

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[2]
_spec = importlib.util.spec_from_file_location("daemon_make", REPO / "rust/fixtures/daemon/make.py")
assert _spec is not None and _spec.loader is not None
dm = importlib.util.module_from_spec(_spec)
sys.modules["daemon_make"] = dm
_spec.loader.exec_module(dm)          # sets FERB_HOME/HOME/TZ=UTC, installs the stand-ins

import ferb.orchestrator.supervisor as sup_mod  # noqa: E402
import ferb.orchestrator.worker as worker_mod  # noqa: E402
from ferb.core import context as core_context  # noqa: E402
from ferb.core.context import ContextBlock  # noqa: E402
from ferb.memory.journal import JournalEntry  # noqa: E402
from ferb.orchestrator.supervisor import Supervisor  # noqa: E402

UTC = timezone.utc
T0 = datetime(2026, 1, 1, 9, 0, tzinfo=UTC)


# ---------------------------------------------------------------------------------------------------------------
# Serialization
# ---------------------------------------------------------------------------------------------------------------

def jsonable(v: Any) -> Any:
    if isinstance(v, (set, frozenset)):
        return sorted(jsonable(x) for x in v)
    if isinstance(v, (list, tuple)):
        return [jsonable(x) for x in v]
    if isinstance(v, dict):
        return {str(k): jsonable(x) for k, x in v.items()}
    if isinstance(v, datetime):
        return v.isoformat()
    if dataclasses.is_dataclass(v) and not isinstance(v, type):
        return jsonable(dataclasses.asdict(v))
    if hasattr(v, "model_dump"):
        return jsonable(v.model_dump())
    return v


def block(b: ContextBlock) -> dict[str, Any]:
    return {"text": b.text, "reference": b.reference, "protected": b.protected, "cache": b.cache,
            "priority": b.priority, "origin": b.origin, "source_refs": list(b.source_refs), "payload": b.payload,
            "history": b.history, "fold": b.fold, "values": jsonable(b.values)}


def sourced(pair: Any) -> dict[str, Any]:
    b, made = pair
    return {"block": block(b), "origins": sorted(made)}


def entry(e: JournalEntry) -> dict[str, Any]:
    return {"id": e.id, "at": e.at.isoformat(), "kind": e.kind, "data": jsonable(e.data), "origin": e.origin}


def mk_entry(d: dict[str, Any]) -> JournalEntry:
    return JournalEntry(d["id"], datetime.fromisoformat(d["at"]), d["kind"], d["data"], d.get("origin", ""))


def hist_entry(e: Any) -> dict[str, Any]:
    return {"at": e.at.isoformat(), "text": e.text, "payload": e.payload, "reference": e.reference, "chat": e.chat,
            "origins": sorted(e.origins), "handle": e.handle, "kind": e.kind, "refs": list(e.refs), "opens": e.opens,
            "values": jsonable(e.values), "result": e.result}


# ---------------------------------------------------------------------------------------------------------------
# Part 1: units
# ---------------------------------------------------------------------------------------------------------------

UNITS: dict[str, list[dict[str, Any]]] = {}


def unit(group: str, args: Any, fn: Any) -> None:
    try:
        out: Any = {"ok": jsonable(fn())}
    except Exception as e:                      # noqa: BLE001  the raise is part of the contract
        out = {"raised": type(e).__name__, "text": str(e)}
    UNITS.setdefault(group, []).append({"args": jsonable(args), **out})


TEXTS = ["", "Done.", "  spaced  out  ", "First paragraph.\n\nSecond paragraph.", "### Heading\n\nBody text here.",
         "#\n\n## Only\n\nReal content", "Stopped, as you asked.\n\nWhat it had found.",
         "I couldn't finish this task.\n\nThe reason is here.", "Stopped, as you asked. This is where things stood:\n\nRow",
         "x" * 400, "word " * 90, "Ünïcödé — “quoted” text…\n\nmore", "line one\nline two\n\npara two\n\npara three",
         "   \n\n  ", "a.\n\nb.", "### A heading line only", "####### seven hashes", "##\tTabbed heading\n\nafter"]


def units() -> None:
    S = sup_mod
    for t in TEXTS:
        unit("short_report", [t, False], lambda t=t: S._short_report(t))
        unit("short_report", [t, True], lambda t=t: S._short_report(t, True))
        unit("heading_only", [t], lambda t=t: S._heading_only(t))
        unit("first_content_line", [t], lambda t=t: S._first_content_line(t))
        unit("said", [t], lambda t=t: S._said(t))
        unit("sentence", [t], lambda t=t: S._sentence(t))
        unit("clipped", [t], lambda t=t: S._clipped(t))
        unit("bare", [t], lambda t=t: S._bare(t))
        unit("since_line", [t], lambda t=t: S._since_line(t))
        unit("without_history_labels", [t], lambda t=t: S.without_history_labels(t))
    for t in ["[c3] (Wed 11 Nov 13:12) FERB (code record): hello", "  [c12]\tFERB (reply, model-written, unchecked): x",
              "(Mon 01 Jan 09:00) (code record) y", "FERB (code record) z", "[c1] [c2] Owner: no label",
              "(reply, model-written, unchecked) w", "(Xyz 01 Jan 09:00) v", "[c] u", "\n [c4] (Sun 31 Dec 23:59)  t"]:
        unit("without_history_labels", [t], lambda t=t: S.without_history_labels(t))
    for text, piece in [("compare the 4 laptops\n\n4", "4"), ("Yes", "Yes"), ("a\nb\n\nb\n\nc", "b"), ("x", ""),
                        ("plan line\n\n  answer  \n\nmore", "answer"), ("a\n\n\n\nb", "zz"), ("p\n \t\nq\nq", "q"),
                        ("only", "only"), ("one\ntwo\nthree", "two"), ("4 apples", "4")]:
        unit("without_whole", [text, piece], lambda a=text, b=piece: S.without_whole(a, b))
    for a in [{"until": "Thu 01 Jan 10:00"}, {"until": "x", "reason": "the build runs"}, {"line": "waiting for X"},
              {"line": "", "until": "y", "reason": ""}]:
        unit("sleeping_line", [a], lambda a=a: S.sleeping_line(a))
    for names in [["one"], ["a  b\tc", "d"], [], ["“q”"]]:
        unit("quoted", [names], lambda n=names: S._quoted(n))
    for refused in [[], [("call mum", "the time is past.")], [("a", "why one"), ("b  c", "why  two.")]]:
        unit("not_set", [refused], lambda r=refused: S._not_set(r))
        unit("not_saved", [refused], lambda r=refused: S._not_saved(r))
        if refused:
            unit("with_reasons", [refused], lambda r=refused: S._with_reasons(r))
    for sites in [[], ["a.example"], ["a.example", "b.example"]]:
        unit("sites_line", [sites], lambda s=sites: S.sites_line(s))
    for lines, chars in [([], 10), (["abc"], 1), (["ab", "cd", "ef"], 5), (["a" * 10, "b"], 20), (["x", "y", "z"], 3)]:
        unit("within", [lines, chars], lambda l=lines, c=chars: S._within(l, c))
    for refs in [[], ["trail:1", "entry:2", "journal:3", "trail:"]]:
        unit("stored_refs", [refs], lambda r=refs: S._stored_refs(r))
    for rec in [{"text": None, "payload": "p"}, {"text": "t"}, {"text": "t", "payload": "q", "segments": [{"a": 1}]},
                {"text": None, "segments": []}]:
        unit("exposure_output", [rec], lambda r=rec: S._exposure_output(r))
    for eid, tid in [("id-1", "t-1"), ("id-2", None), ("id-3", "")]:
        unit("result_reference", [eid, tid], lambda e=eid, t=tid: S.result_reference(e, t))
    for said in [["one"], ["a", "b“c”"]]:
        unit("sent_from", [said], lambda s=said: S.sent_from(s))
    home = str(Path.home())
    for folders in [[home], [home + "/Documents", "/tmp/x"], [home + "x"], []]:
        unit("work_in", [[f.replace(home, "~HOME") for f in folders]], lambda f=folders: S.work_in(f))
    # the day words and reminder lines (UTC here; the port takes the zone)
    for days in [0, 1, 2, 6, 7, 30, -1]:
        at = T0 - timedelta(days=days, hours=3)
        unit("day", [at.isoformat(), T0.isoformat()], lambda a=at: S._day(a, T0))
    for fire, kind, msg, present, now in [
            (T0 + timedelta(hours=2), "reminder", "stretch", False, None),
            (T0 + timedelta(hours=2), "timer", "tea", False, None),
            (T0, "reminder", "back", True, T0),
            (T0 + timedelta(days=1) - timedelta(hours=9), "reminder", "midnight", True, T0),
            (T0 + timedelta(minutes=95), "reminder", "later", True, T0),
            (T0 + timedelta(minutes=95), "reminder", "no now", True, None)]:
        t = SimpleNamespace(fire_at=fire, kind=kind, message=msg)
        unit("reminder_line", [fire.isoformat(), kind, msg, present, now.isoformat() if now else None],
             lambda t=t, p=present, n=now: S._reminder_line(t, p, n))
    for state in ["pending", "cancelled", "delivered"]:
        t = SimpleNamespace(fire_at=T0 + timedelta(hours=5), state=state, message="water  the plants")
        unit("already_set", [(T0 + timedelta(hours=5)).isoformat(), state, "water  the plants"],
             lambda t=t: S._already_set(t))
    from ferb.orchestrator.steering import FERB_CHECKS
    for cap, only in [("owner.ask", False), (sorted(FERB_CHECKS)[0], False), ("plan.ask", True)]:
        ap = SimpleNamespace(capability=cap, options_only=only)
        unit("takes_words", [cap, only], lambda a=ap: S._takes_words(a))
    for summary, reason, options, labels in [("Send it?", "r", (), ()), ("", "Reason only", ("Yes", "No"), ()),
                                              ("Pick", "r", ("Keep", "Edit", "Drop"), ()),
                                              ("Pay?", "r", ("Yes", "No"), ("Pay", "")),
                                              ("Two", "r", ("A", "B"), ("", ""))]:
        ap = SimpleNamespace(summary=summary, reason=reason, options=options, labels=labels)
        unit("plain_choice", [summary, reason, list(options), list(labels)], lambda a=ap: S._plain_choice(a))
    from ferb.orchestrator.schemas import Check, Condition, TaskSpec
    for met in [True, False, None]:
        unit("watch_condition", [met], lambda m=met: S.watch_condition(m).model_dump())
    conds = [Condition(text="file exists", check=Check(kind="file_exists", path="a.md", text=None, url_contains=None, site=None, expected=None), layer="outcome"),
             Condition(text="answered", check=Check(kind="none", path=None, text=None, url_contains=None, site=None, expected=None), layer="artifact")]
    for cs, watch, wm in [([], False, None), (conds, False, None), (conds, True, True), (conds[:1], True, None)]:
        spec = SimpleNamespace(success_conditions=cs)
        unit("checked_conditions", [[c.model_dump() for c in cs], watch, wm],
             lambda s=spec, w=watch, m=wm: [c.model_dump() for c in S.checked_conditions(s, w, m)])
    judged = list(enumerate(conds))
    for i in [0, 1, 2]:
        unit("judged_kind", [[[n, c.model_dump()] for n, c in judged], i], lambda i=i: S.judged_kind(judged, i))
    from ferb.orchestrator.plans import Step
    steps = [Step(id="a", kind="do", goal="Collect the numbers", label=""), Step(id="b", kind="do", goal="Write",
                                                                               label="Write the file")]
    for states in [{}, {"a": {"status": "done"}, "b": {"status": "failed"}},
                   {"a": {"status": "done"}, "b": {"status": "running"}}, {"a": {"status": "blocked"}}]:
        unit("stopped_plan_summary", [[s.model_dump() for s in steps], states],
             lambda st=states: S._stopped_plan_summary(steps, st))
    for kind, data in [("chat.message", {"text": "hi"}), ("chat.notice", {"text": "n", "quotes": [{"text": "q"}, 1]}),
                       ("timer.delivered", {"message": "m", "text": "t"}), ("chat.reply", {}),
                       ("task.answer", {"text": 5})]:
        unit("entry_payload", [kind, data], lambda k=kind, d=data: S._entry_payload(k, d))
    for kind, data in [("chat.message", {"provenance": {"values": {"a": ["b"]}}}),
                       ("chat.reply", {"provenance": {"values": {"a": ["b", 1], "c": "x"}}}),
                       ("chat.reply", {"provenance": {"values": []}}), ("task.answer", {}),
                       ("chat.notice", {"provenance": {"values": {}}})]:
        unit("stored_values", [kind, data], lambda k=kind, d=data: S._stored_values(k, d))
    for e in [{"id": "id-9", "at": "2026-01-01T09:00:00+00:00", "kind": "task.answer",
               "data": {"task_id": "t-1", "status": "done"}, "origin": "inferred,external:web:a.example"},
              {"id": "id-8", "at": "2025-12-30T23:15:00+00:00", "kind": "chat.part",
               "data": {"label": "Part “one”", "task_id": "t-2", "state": "done"}, "origin": ""},
              {"id": "id-7", "at": "2026-01-01T09:00:00+00:00", "kind": "schedule.posted",
               "data": {"name": "Daily", "status": "failed"}, "origin": "ferb"}]:
        unit("result_line", [e], lambda e=e: S.result_line(mk_entry(e)))
    # the front window block: a capture, a missing one, a withheld screenshot
    for case in [
        {"front": None, "image": False, "heading": S.FRONT_WINDOW, "withheld": None, "missing": None, "nth": None},
        {"front": None, "image": False, "heading": "H", "withheld": None, "missing": "timed out", "nth": None},
        {"front": {"facts": {"app": "Finder", "title": "Ünï “x”", "n": 1.5}, "image_ref": "ev-1", "withheld": "",
                   "masking": "fields", "origin": "private"}, "image": True, "heading": S.FRONT_WINDOW,
         "withheld": None, "missing": None, "nth": [2, 3]},
        {"front": {"facts": {"app": "Safari", "url": "https://a.example"}, "image_ref": "ev-2", "withheld": "dark",
                   "masking": "", "origin": "external:web:a.example"}, "image": False, "heading": "H2",
         "withheld": None, "missing": None, "nth": None},
        {"front": {"facts": {"app": "Notes"}, "image_ref": "ev-3", "withheld": "", "masking": "m",
                   "origin": ""}, "image": False, "heading": "H3", "withheld": "it was refused", "missing": None,
         "nth": [1, 1]},
        {"front": {"facts": {}, "image_ref": "ev-4", "withheld": "", "masking": "", "origin": "owner,private"},
         "image": True, "heading": "H4", "withheld": None, "missing": None, "nth": [1, 1]}]:
        front = None
        if case["front"] is not None:
            f = case["front"]
            front = SimpleNamespace(facts=lambda f=f: dict(f["facts"]), image_ref=f["image_ref"], withheld=f["withheld"],
                                    masking=f["masking"], origin=f["origin"])
        image = object() if case["image"] else None
        unit("front_window_block", [case], lambda front=front, image=image, c=case: sourced(S.front_window_block(
            front, image, c["heading"], c["withheld"], c["missing"], tuple(c["nth"]) if c["nth"] else None)))
    # the running summary's blocks
    prev = mk_entry({"id": "id-5", "at": "2026-01-01T08:00:00+00:00", "kind": "chat.summary",
                     "data": {"text": "Earlier: the owner asked for a list."}, "origin": "inferred"})
    copied = [SimpleNamespace(text="[c1] (Thu 01 Jan 09:00) Owner: hi"), SimpleNamespace(text="x </untrusted-data y")]
    for p, c, today in [(None, [], None), (prev, copied, T0), (None, copied[:1], T0 + timedelta(days=3))]:
        unit("summary_blocks", [entry(p) if p else None, [e.text for e in c], today.isoformat() if today else None],
             lambda p=p, c=c, t=today: [block(b) for b in S._summary_blocks(p, c, t)])
    # within_tokens: the planner's cut (tokens as counted by this build; `method` says how)
    for t, limit in [("short", 100), ("word " * 500, 50), ("ü" * 300, 120), ("abc " * 40, 21)]:
        unit("within_tokens", [t, limit], lambda t=t, l=limit: S._within_tokens(t, l))
    since_units()
    questions_units()
    latest_units()
    for prev in [None, {"entry": {"id": "id-000009", "at": "2026-01-01T08:00:00+00:00", "kind": "schedule.posted",
                                  "data": {"text": "Two slots “open”."}, "origin": "inferred,external:web:a.example"},
                        "ran_at": "2025-12-31T23:30:00+00:00", "status": "done"}]:
        unit("previous_run_block", [prev], lambda p=prev: sourced(Supervisor._previous_run_block(
            None if p is None else {**p, "entry": mk_entry(p["entry"])})))
    # the chat line (ORCH-529): arrival orders and what each arrival became
    for script in [["a"], ["a", "b"], ["a", "b", "c"], ["a", "reply", "b"], ["a", "b", "reply", "c", "d", "release"],
                   ["a", "release", "b"], ["a", "b", "release", "c", "reply", "d", "release", "release"]]:
        unit("chat_line", [script], lambda s=script: chat_line(s))


class FakeJournal:
    """The journal reads `_since_block` makes, over invented rows in journal order."""

    def __init__(self, rows: list[JournalEntry]) -> None:
        self.rows = rows

    def of_kind(self, kind: str, field: str = "", equals: str = "", missing: str | None = None) -> list[JournalEntry]:
        return [r for r in self.rows if r.kind == kind and (not field or str(r.data.get(field, missing)) == equals)]

    def kinds_after(self, kinds: tuple[str, ...], entry_id: str) -> list[JournalEntry]:
        at = next(i for i, r in enumerate(self.rows) if r.id == entry_id)
        return [r for r in self.rows[at + 1:] if r.kind in kinds]


def since_units() -> None:
    """`_since_block` and `_since_event` on a stand-in `self` answering their reads from invented rows."""
    T = "2026-01-01T09:00:00+00:00"

    def row(i: int, kind: str, data: dict[str, Any], origin: str = "ferb", at: str = T) -> dict[str, Any]:
        return {"id": f"id-{i:06d}", "at": at, "kind": kind, "data": data, "origin": origin}
    verification = [{"condition": "The page shows a slot\n(second line)", "met": True, "evidence": "seen " * 80},
                    {"condition": "x", "met": None, "evidence": "not seen", "how": "Open the page " * 40},
                    {"condition": "y", "met": False, "evidence": "gone"}, "junk"]
    every = [
        row(1, "chat.message", {"request_id": "r-1", "text": "first"}, "owner", "2025-12-31T22:15:00+00:00"),
        row(2, "schedule.posted", {"task_id": "t-1", "name": "Daily “check”", "status": "done",
                                   "text": "## Heading\n\nAll good today."}, "inferred,external:web:a.example"),
        row(3, "schedule.posted", {"task_id": "t-2", "name": "Permit", "status": "failed",
                                   "text": "I couldn't finish this task.\nIt failed."}, "ferb"),
        row(4, "schedule.posted", {"status": "done_unverified", "text": "x"}),
        row(5, "schedule.skipped", {"schedule_id": "sc-1", "why": "the Mac slept"}),
        row(6, "schedule.skipped", {}),
        row(7, "question.asked", {"approval_id": "ap-1", "question": "Send it?", "exact": "to a@example.com",
                                  "task_id": "t-3.worker"}),
        row(8, "question.answered", {"approval_id": "ap-1", "state": "granted", "kind": "confirm", "choice": "Yes"}),
        row(9, "question.answered", {"approval_id": "ap-2", "state": "denied", "kind": "x"}),
        row(10, "question.asked", {"approval_id": "ap-3", "question": "The code?", "task_id": ""}),
        row(11, "question.answered", {"approval_id": "ap-3", "state": "denied", "kind": "secret", "said": "123"}),
        row(12, "question.asked", {"approval_id": "ap-4", "question": "Which?", "task_id": "t-9.step"}),
        row(13, "question.answered", {"approval_id": "ap-4", "state": "superseded", "kind": "ask"}),
        row(14, "question.answered", {"approval_id": "ap-4", "state": "used", "kind": "ask", "said": "the blue one"}),
        row(15, "task.stopped", {"task_id": "t-4", "by": "owner"}),
        row(16, "task.stopped", {"task_id": "t-5", "by": "code"}),
        row(17, "timer.delivered", {"kind": "timer", "message": "tea"}, "owner"),
        row(18, "timer.delivered", {"kind": "reminder", "message": "call Sam"}, "owner,inferred"),
        row(19, "runtimes.changed", {"runtime": "claude", "roles": ["worker"], "model": ""}),
        row(20, "runtimes.model", {"role": "planner", "model": "m-1", "effort": "high"}),
        row(21, "runtime.switched", {"task_id": "t-6", "runtime": "codex"}),
        row(22, "browsers.paired", {"browser": "Chrome", "folder": "Profile 1"}),
        row(23, "browsing.chosen", {"profile": "Default"}),
        row(24, "schedule.created", {"schedule_id": "sc-1", "rule": "every day at 09:00"}),
        row(25, "schedule.edited", {"schedule_id": "sc-1", "after": {"rule": "every day at 10:00"}}),
        row(26, "schedule.edited", {"schedule_id": "sc-2", "after": None}),
        row(27, "schedule.set_end", {"schedule_id": "sc-2"}),
        row(28, "schedule.pause", {"schedule_id": "sc-unknown"}),
        row(29, "chat.reply", {"request_id": "r-1", "text": "not a since kind"}),
    ]
    cols = {"t-2|outcome_json": json.dumps({"verification": verification}), "t-4|goal": "Book the invented table",
            "t-5|goal": "x"}
    trees = {"t-3.worker": "t-3"}
    origins = {"t-3": ["inferred", "external:web:b.example"], "t-9": ["owner"]}
    labels = {"sc-1": "Daily check", "sc-2": "Permit watch"}
    cases = [
        ("every kind", every, ["r-9"]),
        ("nothing before", every[1:], ["r-9"]),
        ("this turn's own message", every, ["r-1"]),
        ("a message after", every + [row(30, "chat.message", {"request_id": "r-2", "text": "later"}, "owner")], ["r-9"]),
        ("a message in the middle", every[:10] + [row(40, "chat.message", {"request_id": "r-4", "text": "mid"},
                                                       "owner", "2026-01-01T08:30:00+00:00")] + every[10:], ["r-9"]),
        ("read with", [row(1, "chat.message", {"request_id": "r-2", "read_with": "r-3", "text": "x"}, "owner")] + every[1:3],
         ["r-3"]),
    ]
    for what, rows, current in cases:
        entries = [mk_entry(r) for r in rows]
        fake = SimpleNamespace(
            journal=FakeJournal(entries),
            tasks=SimpleNamespace(column=lambda tid, col: cols.get(f"{tid}|{col}"),
                                  tree_of=lambda t: trees.get(t)),
            spans=SimpleNamespace(task_origins=lambda root: set(origins.get(root, []))))
        fake._schedule_label = lambda sid: labels.get(sid, sid)
        fake._since_event = lambda e, fake=fake: Supervisor._since_event(fake, e)
        args = {"what": what, "rows": rows, "current": current, "columns": cols, "trees": trees, "origins": origins,
                "labels": labels}
        unit("since_block", [args], lambda fake=fake, c=current: (
            None if (out := Supervisor._since_block(fake, set(c))) is None else sourced(out)))


def questions_units() -> None:
    """`_questions_block` and `_card_row` on a stand-in `self`: open approvals of every kind."""
    from ferb.daemon.questions import Approval
    from ferb.orchestrator.steering import CONFIRM_ASK
    T = "2026-01-01T09:00:00+00:00"
    asked = [mk_entry({"id": "id-000001", "at": "2025-12-30T08:05:00+00:00", "kind": "question.asked",
                       "data": {"approval_id": "ap-1"}, "origin": "ferb"}),
             mk_entry({"id": "id-000002", "at": T, "kind": "question.asked", "data": {"approval_id": "ap-1"},
                       "origin": "ferb"})]
    approvals = [
        Approval("ap-1", "owner.ask", "Which day suits you?", "r-1", options=("Mon", "Tue"), task_id="t-1.worker"),
        Approval("ap-2", "plan.ask", "Go on?", "r-1", summary="Go on with the plan?"),
        Approval("ap-3", CONFIRM_ASK, "Send the invented note to a@example.com", "r-2", options=("Yes", "No"),
                 summary="Send it?\nmore", task_id="t-2"),
        Approval("ap-4", "check_in", "Keep going?", "r-3", options=("Yes", "No"), options_only=True),
        Approval("ap-5", "owner.ask", "The login code", "r-4", kind="owner_login_code", summary=""),
        Approval("ap-6", "runtime.switch", "x" * 400, "r-5", kind="owner_question", summary=""),
        Approval("ap-7", "ferb.confirm", "  \n  ", "r-6", kind="confirmation", summary="")]
    labels = {"ap-1": "Book a table", "ap-3": "Send the note"}
    for what, pending, conversations in [("none", [], []), ("every kind", approvals, []),
                                         ("with a conversation", approvals[:2],
                                          [{"tree_id": "t-1", "id": "cv-1", "state": "open"}])]:
        fake = SimpleNamespace(journal=FakeJournal(asked),
                               state=SimpleNamespace(approvals=SimpleNamespace(pending=lambda p=pending: list(p))),
                               continuity=SimpleNamespace(task_ids=lambda sid: {"t-1"}),
                               conversations=SimpleNamespace(list=lambda c=conversations: list(c)))
        fake._question_task = lambda a: labels.get(a.id, "")
        fake._routable = Supervisor._routable
        fake._card_row = lambda a, fake=fake: Supervisor._card_row(fake, a)
        rows = [{"id": a.id, "kind": a.kind, "capability": a.capability, "reason": a.reason, "summary": a.summary,
                 "options": list(a.options), "task_label": labels.get(a.id, ""),
                 "asked_at": next((e.at.isoformat() for e in reversed(asked) if e.data["approval_id"] == a.id), None)}
                for a in pending]
        unit("questions_block", [{"what": what, "pending": rows, "conversations": conversations}],
             lambda fake=fake: sourced(Supervisor._questions_block(fake, SimpleNamespace(id="s1"))))


def latest_units() -> None:
    """`_latest_blocks` on a stand-in `self`: a task's answer, a multi-part card (its answer, a row of it), a
    scheduled post, an answer over the token limit."""
    T = "2026-01-01T09:00:00+00:00"

    def row(i: str, kind: str, data: dict[str, Any], origin: str = "inferred") -> dict[str, Any]:
        return {"id": i, "at": T, "kind": kind, "data": data, "origin": origin}
    rows = [
        row("a-1", "task.answer", {"task_id": "t-1", "request_id": "r-1", "status": "done", "text": "Plain “answer”."}),
        row("p-1", "chat.part", {"task_id": "t-2", "request_id": "r-2", "part": "p2", "label": "Second", "state": "done",
                                 "seq": "2026-01-01T09:00:00.000002+00:00", "text": "two"}),
        row("p-2", "chat.part", {"task_id": "t-2", "request_id": "r-2", "part": "p1", "label": "First", "state": "done",
                                 "seq": "2026-01-01T09:00:00.000001+00:00", "text": "one\nline"},
            "inferred,external:web:a.example"),
        row("p-3", "chat.part", {"task_id": "t-2", "request_id": "r-0", "part": "p1", "label": "Old", "state": "done",
                                 "seq": "2025", "text": "old run"}),
        row("p-4", "chat.part", {"task_id": "t-2", "request_id": "r-2", "part": "p3", "label": "Third",
                                 "state": "failed", "seq": "2026-01-01T09:00:00.000003+00:00", "text": "big " * 900}),
        row("a-2", "task.answer", {"task_id": "t-2", "request_id": "r-2", "status": "done", "index": True,
                                   "text": "1. First\n2. Second"}),
        row("s-1", "schedule.posted", {"name": "Daily", "status": "done", "text": "posted", "task_id": "t-3"}, "ferb"),
        row("a-3", "task.answer", {"task_id": "t-4", "status": "done", "text": "x" * 9000}),
        row("a-4", "task.answer", {"task_id": "t-5", "request_id": "r-5", "status": "done", "index": True,
                                   "text": "an index with no rows"})]
    entries = [mk_entry(r) for r in rows]
    order = {"t-2": ["p1", "p2", "p3"]}
    for latest in ["", "missing", "a-1", "p-1", "a-2", "s-1", "a-3", "a-4"]:
        fake = SimpleNamespace(journal=FakeJournal(entries),
                               continuity=SimpleNamespace(part_order=lambda tid: list(order.get(tid, []))))
        fake.journal.by_ids = lambda ids, entries=entries: [e for e in entries if e.id in ids]
        fake._rows_of = lambda tid, req, fake=fake: Supervisor._rows_of(fake, tid, req)
        fake._card_rows = lambda answer, fake=fake: Supervisor._card_rows(fake, answer)
        unit("latest_blocks", [{"latest": latest, "rows": rows, "order": order}],
             lambda fake=fake, latest=latest: [sourced(p) for p in Supervisor._latest_blocks(fake, latest)])


def chat_line(script: list[str]) -> list[Any]:
    """Drive `_ChatLine` with arrivals (a name), "reply" (the holder replied: thinking False) and "release"."""
    loop = asyncio.new_event_loop()
    try:
        line = sup_mod._ChatLine()
        out: list[Any] = []
        arrivals: dict[str, Any] = {}
        for step in script:
            if step == "reply":
                if line.holder is not None:
                    line.holder.thinking = False
                out.append(["reply"])
            elif step == "release":
                held = line.holder
                if held is not None:
                    line.release(held)
                out.append(["release", held.lead.request_id if held else None])
            else:
                a = sup_mod._Arrival(step, step, [], None, loop.create_future())
                arrivals[step] = a
                line.arrive(a)
                out.append(["arrive", step])
            state = {name: (["lead", a.role.result()[1].lead.request_id] if a.role.done() and a.role.result()[0] == "lead"
                            else list(a.role.result()) if a.role.done() else None) for name, a in arrivals.items()}
            holder = line.holder
            out.append({"roles": state, "holder": None if holder is None else {
                "lead": holder.lead.request_id, "arrivals": [x.request_id for x in holder.arrivals],
                "restarts": holder.restarts, "restart": holder.restart, "thinking": holder.thinking},
                "queued": [x.request_id for x in line.queued]})
        return out
    finally:
        loop.close()


# ---------------------------------------------------------------------------------------------------------------
# Part 2: the daemon scenarios, with the composition points recorded
# ---------------------------------------------------------------------------------------------------------------

CURRENT: dict[str, Any] = {"name": "", "records": None}
SENT: list[list[str]] = []                  # the parts each model call was sent (checked against the daemon's fixture)
CATALOGS: dict[str, Any] = {}               # a catalog by the SHA-256 of its JSON: each is stored once


def catalog(value: Any) -> dict[str, str]:
    value = jsonable(value)
    key = dm.sha(json.dumps(value, sort_keys=True))
    CATALOGS[key] = value
    return {"catalog": key}


def record(kind: str, value: dict[str, Any]) -> None:
    recs = CURRENT["records"]
    if recs is not None:
        recs.append({"kind": kind, **value})


def patch_methods() -> None:
    S = Supervisor
    plain_system = S._system

    def _system(self: Any, name: str | None = None, *, live: bool = True, roles: tuple[str, ...] = ()) -> str:
        out = plain_system(self, name, live=live, roles=roles)
        record("system", {"name": name, "live": live, "roles": list(roles), "inputs": {
            "facts": self.facts(), "live_facts": self.live_facts() if live else "",
            "capabilities": self.availability().text, "identity_context": self.identity_context(),
            "skills_catalog": catalog(self.skills_catalog()), "abilities_catalog": catalog(self.abilities_catalog())},
            "system": out})
        return out
    S._system = _system

    plain_newly = S._with_newly_always_on

    def _with_newly_always_on(self: Any, ctx: list[ContextBlock]) -> list[ContextBlock]:
        out = plain_newly(self, ctx)
        record("newly_always_on", {"pending": self.identity_pending(), "blocks": len(ctx), "out": len(out)})
        return out
    S._with_newly_always_on = _with_newly_always_on

    plain_assemble = worker_mod.assemble

    def assemble(blocks: list[ContextBlock], **k: Any) -> Any:
        out = plain_assemble(blocks, **k)
        # the same blocks as written here (temporary paths as /FX), assembled again: the token estimate in the budget
        # line counts the real path's bytes, so a port fed the /FX texts compares with these parts
        fx = plain_assemble([dataclasses.replace(b, text=dm.fx(b.text),
                                                 payload=dm.fx(b.payload) if b.payload is not None else None)
                             for b in blocks], **k)
        record("assemble", {"blocks": [block(b) for b in blocks], "budget_tokens": k.get("budget_tokens"),
                            "reserved_tokens": k.get("reserved_tokens"), "max_chars": k.get("max_chars"),
                            "method": core_context.token_method(), "parts": list(fx.parts),
                            "cacheable": out.cacheable, "shown": list(out.shown)})
        SENT.append(list(out.parts))
        return out
    worker_mod.assemble = assemble

    plain_timeline = S._timeline

    def _timeline(self: Any, current: Any, conversation: str | None, summary: Any = None) -> Any:
        from ferb.orchestrator.supervisor import CONVERSATION_KINDS, SELF_POSTS
        rows = self.journal.entries_of(*CONVERSATION_KINDS, *SELF_POSTS)
        outcomes = {str(e.data.get("task_id")): self.tasks.column(str(e.data.get("task_id")), "outcome_json")
                    for e in rows if e.kind == "task.answer"}
        out = plain_timeline(self, current, conversation, summary)
        record("timeline", {
            "current": sorted(current) if not isinstance(current, str) else current, "conversation": conversation,
            "summary": entry(summary) if summary is not None else None, "rows": [entry(e) for e in rows],
            "task_ids": sorted(self.continuity.task_ids(conversation)) if conversation is not None else None,
            "covered": sorted(self._covered(summary)), "outcomes": outcomes,
            "entries": [hist_entry(e) for e in out[0]], "ages": {k: v.isoformat() for k, v in out[1].items()}})
        return out
    S._timeline = _timeline

    plain_conv = S._conversation_blocks

    def _conversation_blocks(self: Any, current: Any, conversation: str, *, since_last_notice: bool = False) -> Any:
        summary = self._summary(conversation)
        out = plain_conv(self, current, conversation, since_last_notice=since_last_notice)
        record("conversation_blocks", {"conversation": conversation, "since_last_notice": since_last_notice,
                                       "summary": entry(summary) if summary is not None else None,
                                       "blocks": [sourced(p) for p in out[0]],
                                       "ages": {k: v.isoformat() for k, v in out[1].items()}, "latest": out[2]})
        return out
    S._conversation_blocks = _conversation_blocks

    plain_questions = S._questions_block

    def _questions_block(self: Any, session: Any) -> Any:
        pending = self.state.approvals.pending()
        mine = self.continuity.task_ids(session.id)
        out = plain_questions(self, session)
        record("questions_block", {
            "session": session.id, "pending": [dict(jsonable(a), task_label=self._question_task(a),
                                                   root=self._question_root(a), card_row=self._card_row(a))
                                               for a in pending],
            "conversations": jsonable(self.conversations.list()), "mine": sorted(mine), "out": sourced(out)})
        return out
    S._questions_block = _questions_block

    plain_tasks = S._task_state

    def _task_state(self: Any, session: Any) -> Any:
        from ferb.orchestrator.plan_store import EARLIER
        from ferb.orchestrator.continuity import SCHEDULE_SESSION
        mine = self.continuity.task_ids(session.id)
        open_cards = self.state.approvals.pending()
        trees = []
        for key, tree in list(self._tree_of.items()):
            if key != tree.id:
                continue
            last = tree.reports[-1] if tree.reports else None
            steps = ({sid: st for sid, st in self.plans.steps(key).items() if not sid.startswith(EARLIER)}
                     if tree.spec.plan else {})
            pause = self._pauses.get(key)
            trees.append({
                "task_id": key, "mine": key in mine, "schedule": bool(tree.schedule_id), "goal": tree.spec.goal,
                "stop": bool(tree.stop), "reports": len(tree.reports),
                "last_report": jsonable(last) if last is not None else None,
                "steps": [[sid, str(st["status"])] for sid, st in steps.items()],
                "loops": jsonable(self._loop_facts(key)),
                "paused": key in self._pauses or self._waiting.get(key, 0) > 0,
                "question": self._questions.get(key) or getattr(pause, "question", ""),
                "cards": [self._card_row(ap) for ap in open_cards if self._question_root(ap) == key],
                "last_step": jsonable(self._last_steps.get(key)) if key in self._tree_of else None,
                "asleep": jsonable(self._sleeping.get(key)), "preview": jsonable(self.preview_status(key, "")),
                "plan": bool(tree.spec.plan),
                "goals": {st.id: st.goal or st.question or st.kind for st in tree.spec.plan.steps}
                if tree.spec.plan else {},
                "runtime": tree.runtime or tree.spec.worker_type or "default",
                "standing": [sc.model_dump() for sc in self.standing.scopes(key)],
                "folders": list(tree.spec.folders), "origins": sorted(self._task_line_origins(key))})
        candidates = [] if session.id.startswith(SCHEDULE_SESSION) else [
            dict(jsonable(r), origins=sorted(self._task_line_origins(r["task_id"])))
            for r in self.continuity.candidates(session.id)
            if r["task_id"] not in self._tree_of and r["status"] == "needs_user"]
        out = plain_tasks(self, session)
        record("task_state", {"session": session.id, "trees": trees, "candidates": candidates,
                              "out": sourced(out) if out is not None else None})
        return out
    S._task_state = _task_state

    plain_since = S._since_block

    def _since_block(self: Any, current: Any) -> Any:
        out = plain_since(self, current)
        record("since_block", {"out": sourced(out) if out is not None else None})
        return out
    S._since_block = _since_block

    plain_latest = S._latest_blocks

    def _latest_blocks(self: Any, latest: str) -> Any:
        found = self.journal.by_ids([latest]) if latest else []
        rows = []
        order: list[str] = []
        if found:
            e = found[0]
            rows = [entry(r) for r in self.journal.of_kind("chat.part", "task_id", str(e.data.get("task_id")))]
            order = list(self.continuity.part_order(str(e.data.get("task_id")))) if e.kind == "task.answer" else []
        out = plain_latest(self, latest)
        record("latest_blocks", {"latest": latest, "found": [entry(e) for e in found], "part_rows": rows,
                                 "part_order": order, "out": [sourced(p) for p in out]})
        return out
    S._latest_blocks = _latest_blocks

    # ---- the worker's and checker's context (supervisor.py:6852 `_context`) and what it reads ----
    import contextvars
    current_ctx: contextvars.ContextVar[Any] = contextvars.ContextVar("current_ctx", default=None)

    def sub(kind: str, value: dict[str, Any]) -> None:
        rec = current_ctx.get()
        if rec is not None:
            rec["calls"].append({"kind": kind, **value})
        else:
            record(kind, value)

    plain_context = S._context

    async def _context(self: Any, tree: Any, *, learned: bool = False) -> Any:
        spec = tree.spec
        request = self.state.requests.get(tree.worker.request_id)
        root = self._tree_of.get(tree.id)
        reads = {
            "tree_id": tree.id, "worker_id": tree.worker.id, "learned": learned,
            "words": request.words, "payload": request.payload,
            "activity": jsonable(self.relevant_context(request.words)),
            "schedule_id": root.schedule_id if root is not None else None,
            "parent_goal": tree.parent_goal, "goal": spec.goal, "worker_type": spec.worker_type,
            "step_context": tree.step_context, "intake_receipts": list(tree.intake_receipts),
            "disturbance": self.disturbance_context(tree.id),
            "schedule_facts": jsonable(root.schedule_facts) if root is not None else {},
            "reports": jsonable(tree.reports),
            "unresolved": [jsonable([dict(r) for r in self.gateway.ledger.unresolved(t)])
                           for t in [tree.id] + ([tree.prior_task] if tree.prior_task else [])],
            "wait_cut": self._wait_cut(tree.id),
            "conditions": [c.text for c in spec.success_conditions],
            "targets": jsonable(spec.targets), "folders": list(spec.folders), "workspace": self.workspace,
            "posted": ([{k: row[k] for k in ("part", "label", "text") if k in row}
                        for row in root.card.records().values()
                        if not row.get("direct") and row.get("state") in ("done", "updated")]
                       if root is not None and root.card is not None and tree.step_context else []),
            "plan": bool(spec.plan), "plan_steps": jsonable(self.plans.steps(tree.id)) if spec.plan else None,
            "saved_plan": jsonable(self._saved_plan(tree.id)) if not spec.plan else None,
            "owner_questions": jsonable(self.spans.owner_questions(tree.id)),
            "reopened": sorted(self.spans.reopened_answers(tree.id)),
            "confirmations": [jsonable(dict(zip(("id", "rule", "granted", "used", "at"), row))) for row in
                              self.journal.db.conn.execute(
                                  "SELECT id,rule,granted,used,at FROM confirmations WHERE tree_id=?", (tree.id,))],
            "calls": []}
        token = current_ctx.set(reads)
        try:
            out = await plain_context(self, tree, learned=learned)
        finally:
            current_ctx.reset(token)
        reads["out"] = [block(b) for b in out]
        record("context", reads)
        return out
    S._context = _context

    for name in ("_worker_memory", "_result_blocks", "_schedule_messages", "_schedule_block"):
        def wrap_one(name: str = name) -> None:
            plain = getattr(S, name)
            if asyncio.iscoroutinefunction(plain):
                async def hooked(self: Any, *a: Any, **k: Any) -> Any:
                    out = await plain(self, *a, **k)
                    sub(name, {"out": [block(b) for b in out]})
                    return out
            else:
                def hooked(self: Any, *a: Any, **k: Any) -> Any:  # type: ignore[misc]
                    out = plain(self, *a, **k)
                    sub(name, {"out": jsonable(out) if not isinstance(out, (list, ContextBlock)) else
                               [block(b) for b in out] if isinstance(out, list) else block(out)})
                    return out
            setattr(S, name, hooked)
        wrap_one()

    plain_source = S._source_block

    def _source_block(self: Any, tree: Any, label: str, text: str, origin: str,
                      source_refs: tuple[str, ...] = (), *, payload: str | None = None) -> Any:
        out = plain_source(self, tree, label, text, origin, source_refs, payload=payload)
        sub("source_block", {"label": label, "text": text, "origin": origin, "source_refs": list(source_refs),
                             "payload": payload, "reference": out.reference})
        return out
    S._source_block = _source_block

    plain_verifier = S._verifier

    async def _verifier(self: Any, tree: Any, task: Any, judged: Any, span: str, observations: Any = None) -> Any:
        planned = len(tree.spec.success_conditions)
        record("verifier", {
            "tree_id": tree.id, "task_id": task.id, "owner_answer": tree.owner_answer,
            "judged": [[i, c.text, c.layer] for i, c in judged],
            "confirmed": {str(i): r.evidence for i, r in tree.confirmed.items() if i < planned},
            "outcome_status": tree.outcome.status if tree.outcome is not None else None,
            "artifacts": list(tree.outcome.artifacts) if tree.outcome else [],
            "reason": tree.reason, "worker_question": tree.worker_question,
            "part_updates": jsonable(tree.part_updates), "worker_types": list(self.worker_types()),
            "availability": self.availability().text,
            "observations": [{"index": r.index, "predicate_met": r.met, "observation": r.evidence,
                              "predicate": tree.spec.success_conditions[r.index].check.model_dump()}
                             for r in observations or []],
            "calls_since": tree.calls_since,
            "began": jsonable(self.spans.request_began(tree.id, task.request_id, task.id))})
        return await plain_verifier(self, tree, task, judged, span, observations)
    S._verifier = _verifier

    plain_code_facts = S._code_facts

    def _code_facts(self: Any, tree: Any, task: Any) -> Any:
        from ferb.orchestrator.evidence import how_facts
        facts = how_facts(self.journal.db.conn, task.id, since=tree.calls_since)
        out = plain_code_facts(self, tree, task)
        record("code_facts", {"facts": jsonable(facts), "tabs": jsonable(self.task_tabs(task.id)),
                              "out": [block(b) for b in out]})
        return out
    S._code_facts = _code_facts

    plain_recorded = sup_mod.recorded_calls

    def recorded_calls(*a: Any, **k: Any) -> Any:
        out = plain_recorded(*a, **k)
        record("recorded_calls", {"above": k.get("above"), "out": [block(b) for b in out]})
        return out
    sup_mod.recorded_calls = recorded_calls

    plain_changed = S._changed_files

    async def _changed_files(self: Any, tree: Any, task: Any, above: int) -> Any:
        out = await plain_changed(self, tree, task, above)
        record("changed_files", {"above": above, "records": dict(out.records), "out": [block(b) for b in out.blocks]})
        return out
    S._changed_files = _changed_files

    plain_check = S._check

    async def _check(self: Any, tree: Any, vtask: Any, judged: Any, span: str, ctx: Any, plain: Any,
                     effort: Any) -> Any:
        record("check", {"ctx": len(ctx), "plain": len(plain), "effort": effort})
        return await plain_check(self, tree, vtask, judged, span, ctx, plain, effort)
    S._check = _check

    plain_once = worker_mod.WorkerRuns._run_worker_once

    async def _run_worker_once(self: Any, tree: Any, task: Any, prompt: str, ctx: Any, schema: Any, span: str,
                               effort: Any = None, roles: tuple[str, ...] = (), observe_only: bool = False) -> Any:
        owner_active, last_event = self.signals()
        record("run_worker", {
            "task_id": task.id, "role": task.role, "prompt": prompt, "roles": list(roles),
            "blocks": len(ctx), "now": self.clock.now().isoformat(),
            "deadline": task.deadline.isoformat() if task.deadline else None,
            "owner_active": owner_active.isoformat() if owner_active else None,
            "last_event": last_event.isoformat() if last_event else None,
            "browser_context": self.browser_context(task.id) if task.role == "verifier" else None,
            "start_block": block(tree.start_block) if task.role == "worker" and tree.start_block else None,
            "corrections": list(tree.steering.messages)})
        return await plain_once(self, tree, task, prompt, ctx, schema, span, effort, roles, observe_only)
    worker_mod.WorkerRuns._run_worker_once = _run_worker_once

    plain_supervise = S._supervise

    async def _supervise(self: Any, turn: Any, repair: list[str] | None = None, **k: Any) -> Any:
        env = dict(turn.environment)
        owner_active, last_event = self.signals()
        record("supervise", {
            "request_id": turn.request_id, "session": turn.session.id, "preview": turn.preview,
            "environment": jsonable(env), "now": self.clock.now().isoformat(),
            "owner_active": owner_active.isoformat() if owner_active else None,
            "last_event": last_event.isoformat() if last_event else None,
            "live_facts": self.live_facts(owner_state=False) if turn.preview else self.live_facts(last_runs=True),
            "last_run_origins": sorted(self.last_run_origins()), "repair": repair, "last_call": k.get("last_call", ""),
            "entries": [entry(e) for e in turn.entries], "payload": turn.payload,
            "message_origin": sorted(turn.message_origin), "screen": turn.screen is not None,
            "screens": len(turn.screens), "schedule_previous": jsonable(turn.schedule_previous),
            "open": None if turn.open is None else {
                "asked": turn.open.asked, "payload": turn.open.payload,
                "carried": [entry(e) for e in turn.open.carried], "asked_outside": sorted(turn.open.asked_outside)}})
        return await plain_supervise(self, turn, repair, **k)
    S._supervise = _supervise


def scenarios() -> dict[str, Any]:
    patch_methods()
    out: dict[str, Any] = {}
    for fn in dm.CHAT_SCENARIOS:
        CURRENT["records"] = []
        SENT.clear()
        try:
            got = fn()
        except Exception as e:                          # noqa: BLE001
            import traceback
            traceback.print_exc()
            raise SystemExit(f"{fn.__name__}: {type(e).__name__}: {e}")
        sent = [dm.fx(json.dumps([dm.BODIES[p["sha256"]] if isinstance(p, dict) else p for p in c["parts"]]))
                for c in got["model_calls"]]
        if [dm.fx(json.dumps(parts)) for parts in SENT] != sent:
            raise SystemExit(f"{fn.__name__}: the recorded assemblies aren't the parts the model received")
        out[got["name"]] = {"records": CURRENT["records"]}
        CURRENT["records"] = None
        print(f"{fn.__name__}: {len(out[got['name']]['records'])} records", file=sys.stderr)
    return out


BODIES: dict[str, str] = {}


def interned(node: Any) -> Any:
    """Every text of 2,048 characters or more as {"sha256"} of its /FX form (rust/fixtures/daemon's rule); the texts
    go to bodies.json unless rust/fixtures/daemon/bodies.json already holds them."""
    if isinstance(node, str):
        text = dm.fx(node)
        if len(text) >= 2048:
            key = dm.sha(text)
            BODIES[key] = text
            return {"sha256": key}
        return node
    if isinstance(node, list):
        return [interned(x) for x in node]
    if isinstance(node, dict):
        return {k: interned(v) for k, v in node.items()}
    return node


def write(name: str, value: Any) -> int:
    text = dm.fx(json.dumps(value, ensure_ascii=False, indent=1)) + "\n"
    (HERE / name).write_text(text, encoding="utf-8")
    return len(text.encode("utf-8"))


def main() -> None:
    units()
    sizes = {"units.json": write("units.json", {"token_method": core_context.token_method(), "units": UNITS})}
    if "--units" not in sys.argv:
        sizes["scenarios.json"] = write("scenarios.json", {"scenarios": interned(scenarios()),
                                                           "catalogs": interned(CATALOGS)})
        known = json.loads((REPO / "rust/fixtures/daemon/bodies.json").read_text(encoding="utf-8"))
        sizes["bodies.json"] = write("bodies.json", {k: v for k, v in sorted(BODIES.items()) if k not in known})
    for k, v in sizes.items():
        print(f"{k}: {v} bytes")


if __name__ == "__main__":
    try:
        main()
    finally:
        dm.MP.undo()
        import shutil
        shutil.rmtree(dm.ROOT, ignore_errors=True)
