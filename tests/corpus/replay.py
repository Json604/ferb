"""Replays corpus scenarios through FERB's real gateway (a real Daemon, its real Gateway.call, fake tools).

Each scenario gets a fresh daemon in its own temp home, one task tree, and its reads in order: tool results come from
fake read tools whose `read_sources` FERB labels itself (origin.from_read_source, the labelling at the source); context
FERB carries between tasks (its conversation, memory, a schedule's earlier run, its own facts) is written as the
`context.exposure` records the supervisor writes today. Then the outbound action is called through the gateway. The
request is marked unattended, so a question FERB asks is recorded (an owner.ask span marked `ferb_question`, with the
rules it confirms) and declined at once: nothing waits, nothing is answered Yes, nothing becomes known.

What is approximate (README, "Approximations"): `_carried_labels` and `_schedule_request_origins` mirror how the
supervisor labels derived records (since GATE-519: the kinds of everything the writer saw, never its hosts and
senders) and how a schedule's run starts; they are the only coupling to that storage. The per-entry `segments` a
history read now also stores are not read by the gateway yet (phase 3 updates this harness to write them). Everything
else is behaviour seen through `Gateway.call`."""
from __future__ import annotations

import json
import shutil
import tempfile
from collections import Counter, defaultdict
from dataclasses import dataclass, field
from datetime import timedelta
from pathlib import Path
from typing import Any
from urllib.parse import urlsplit

from pydantic import BaseModel

from ferb.act.browser.tools import FERB_BROWSER, BrowserNavigate, BrowserOpen
from ferb.clock.clock import FakeClock
from ferb.core import origin
from ferb.core.ids import SequentialIds
from ferb.core.types import ActionFacts, ActionSpec, Target, Task
from ferb.orchestrator.tree import TaskRecord
from ferb.policy.rules import Kind
from ferb.policy.targets import Request, Targets
from tests.conftest import FakeCap, RunningDaemon
from tests.corpus.model import Read, Scenario
from tests.support import egress
from tests.support.browser_sink import FakeBrowser

TASK, REQUEST, TAB = "task-1", "req-1", "t1"
SCOPES = {FERB_BROWSER.read, "probe"}
HARNESS_ERRORS = ("is not available to this", "invalid arguments", "task is not running", "outside this worker")


# ---------- today's storage of carried context (the approximate part) ----------

def _web(host: str) -> str:
    return origin.external("web", host)


def _host(url: str) -> str:
    return (urlsplit(url).hostname or url) if "://" in url else url


def _carried_labels(r: Read) -> set[str]:
    """How the supervisor labels a record carried from earlier work: the kinds of everything its writer saw
    (GATE-519: supervisor._context_exposure joins the conversation's kinds; replies and summaries keep the turn's)."""
    outside = origin.kinds(_web(h) for h in r.carried_from)
    if r.kind == "history" or (r.kind == "ferb_fact" and r.via != "facts"):
        return {origin.OWNER, origin.FERB, origin.INFERRED, origin.PRIVATE} | outside
    if r.kind == "ferb_fact":
        return {origin.FERB}
    if r.kind == "memory":                       # a fact FERB observed itself is private (learning.memory_origin);
        return {origin.PRIVATE} if not outside else {origin.OWNER, origin.INFERRED, origin.PRIVATE} | outside
    if r.kind == "schedule_run":                 # Schedule facts: FERB's block with the earlier answer's labels
        return {origin.FERB, origin.INFERRED, origin.PRIVATE} | outside
    if r.kind == "front_window":
        return {_web(_host(r.where))}
    if r.kind == "outside":                      # a planner lookup of outside content (making turn only)
        return {_read_label(r.where)}
    raise ValueError(r.kind)


# FERB's own clock as its context shows it (labelled `ferb`): what a summary or reply is written beside
CLOCK = "Now: Saturday 2026-11-07 10:42. Today is Sat 07 Nov 2026."


def _segments(r: Read) -> list[dict[str, Any]]:
    """The per-entry segments the supervisor stores with a conversation read (GATE-519): an owner's message whole
    (kinds owner); an entry FERB wrote (a reply, a summary) with the owner values its own text holds, as
    carried.derive finds them beside FERB's clock (the earlier task's own reads aren't in this scenario, so a value
    it repeats is found nowhere and carried, fail-closed, as the owner's)."""
    from ferb.policy import carried
    outside = origin.kinds(_web(h) for h in r.carried_from)
    entries = [r.text] if r.kind == "ferb_fact" else r.text.split("\n\n")
    out: list[dict[str, Any]] = []
    for entry in entries:
        if entry.startswith("Owner:"):
            out.append({"ref": "entry:owner", "kinds": [origin.OWNER], "len": len(entry), "whole": True})
            continue
        made = {origin.INFERRED, origin.PRIVATE} | outside
        earlier = [({origin.PRIVATE}, r.where)] if r.kind == "history" and r.where else []
        stored = carried.derive(entry, [({origin.FERB}, CLOCK), *earlier], made)
        out.append({"ref": "entry:ferb", "kinds": sorted(made), "len": len(entry), "values": stored["values"]})
    return out


def _read_label(where: str) -> str:
    if where.startswith(("email:", "message:")):
        kind, _, ident = where.partition(":")
        return origin.external(kind, ident)
    return _web(_host(where))


def _schedule_request_origins(make: list[Read]) -> set[str]:
    """supervisor._sink_origins_for_new_schedule: `inferred`, the owner's message, and the outside kinds of the making
    turn's context, FERB's own history and memory included when outside content reached them (T-1007, GATE-525)."""
    found = {origin.INFERRED, origin.OWNER}
    for r in make:
        if r.kind in ("owner_words", "owner_paste"):
            continue
        found |= {origin.kind(o) for o in _carried_labels(r) if origin.is_external(o)}
    return found


# ---------- fake tools ----------

class _Form(FakeCap):
    """Typing into a page, then a click that submits its form, as the extension reports it (GATE-202)."""
    generic_input = True

    class Input(BaseModel):
        tab: str = TAB
        host: str = "www.example.com"
        action: str | None = None
        text: str = ""

    def __init__(self) -> None:
        super().__init__("probe.form", Kind.CLICK, {"probe"})

    def describe(self, args: Any) -> ActionSpec:
        typed = Kind.FILL_FORM if args.text else Kind.CLICK
        return ActionSpec(typed, Target(site=args.host), content=args.text or f"submit:{args.host}",
                          facts=ActionFacts(page_url=f"https://{args.host}/form", form_action_host=args.action,
                                            submits_form=None if args.text else True))


def _read_sources(r: Read) -> tuple[tuple[str, str], ...]:
    w = r.where
    if r.kind == "private":
        kind, _, ident = w.partition(":")
        return ((kind if kind in ("file", "app") else "file", ident or w),)
    if r.kind == "signed_in":
        if w.startswith("mail:"):
            return (("owner_account", "mail"), ("email", w.partition(":")[2]))
        return (("owner_account", "chrome"), ("web", w))
    if r.kind == "outside":
        if w.startswith(("email:", "message:")):
            kind, _, ident = w.partition(":")
            return ((kind, ident),)
        return (("web", w),)
    raise ValueError(r.kind)


# ---------- the replay ----------

@dataclass
class Step:
    name: str
    outcome: str                     # asked | blocked | ran
    rules: list[str] = field(default_factory=list)
    error: str = ""


@dataclass
class Result:
    id: str
    family: str
    domain: str
    expected: str
    got: str                         # ask | no_ask
    verdict: str                     # correct | false_question | miss | masked
    rules: list[str]
    steps: list[Step]
    known_today: str = ""
    note: str = ""

    def row(self) -> dict[str, Any]:
        return {"id": self.id, "family": self.family, "domain": self.domain, "expected": self.expected,
                "got": self.got, "verdict": self.verdict, "rules": self.rules, "known_today": self.known_today,
                "note": self.note, "steps": [s.__dict__ for s in self.steps]}


def start_daemon(home: Path) -> RunningDaemon:
    """As tests/conftest.py's `daemon` fixture builds it: fake clock and ids, no watchers, no owner input tap."""
    d = RunningDaemon(home, clock=FakeClock(), ids=SequentialIds("id"), watch_owner_input=False)
    d.daemon._permissions = lambda: {"accessibility": True, "screen": True}   # type: ignore[method-assign]
    egress.install(getattr(d.daemon, "proxy", None))
    return d


class _Run:
    def __init__(self, d: RunningDaemon, s: Scenario) -> None:
        self.d, self.s = d, s
        self.fb = FakeBrowser()
        self.form = _Form()
        self.send = FakeCap("probe.send", Kind.SEND, {"probe"})
        self.mail = FakeCap("probe.mail_send", Kind.MAILBOX_SEND, {"probe"})
        self.post = FakeCap("probe.post", Kind.POST, {"probe"})
        self.readers: list[FakeCap] = []
        self.steps: list[Step] = []
        self.grant: Any = None

    # the tree

    def setup(self, words: str, request_origins: set[str]) -> None:
        caps: list[Any] = [BrowserOpen(self.fb, FERB_BROWSER), BrowserNavigate(self.fb, FERB_BROWSER),   # type: ignore[arg-type]
                           self.form, self.send, self.mail, self.post]
        for i, r in enumerate(x for x in self.s.reads if x.phase == "run"):
            if r.kind in ("private", "signed_in", "outside"):
                # a page read carries its address, as the browser tools' results do (GATE-528's page scope)
                page = {"url": r.where} if r.where.startswith(("http://", "https://")) else {}
                cap = FakeCap(f"probe.read{i}", "read", {"probe"}, reads=_read_sources(r),
                              content={"text": r.text} | page)
                caps.append(cap)
                self.readers.append(cap)
        d = self.d.daemon
        for c in caps:
            d.registry.capabilities[c.name] = c
        task = Task(TASK, None, "g", "dev", FERB_BROWSER.identity, SCOPES, "worker", REQUEST, [],
                    d.clock.now() + timedelta(minutes=10))

        async def go() -> Any:
            root = d.spans.start("task", task_id=task.id, input={"request_id": REQUEST,
                                                               "request_origins": sorted(request_origins)})
            d.tasks.add(TaskRecord(task, d.clock.now(), root.id))
            d.state.requests.record(Request(REQUEST, words, Targets.of(), payload=words))
            d.state.approvals.unattended.add(REQUEST)      # a question is recorded and declined at once
            for h in self.s.approved_hosts:
                d.gateway.destinations.record("host", h, "confirmed")
            token = d.tokens.issue(task.id, "w1", SCOPES, FERB_BROWSER.identity, ttl_s=600)
            return d.tokens.validate(token)
        self.grant = self.d.run(go())

    def expose(self, text: str, labels: set[str], reference: str, payload_only: bool = False,
               segments: list[dict[str, Any]] | None = None) -> None:
        async def go() -> None:
            spans = self.d.daemon.spans
            span = spans.start("context.exposure", task_id=TASK, input={"source_refs": [], "reference": reference})
            output = {"payload": text} if payload_only else {"text": text, "payload": text}
            spans.finish(span, output=output | ({"segments": segments} if segments else {}),
                         origin=origin.join(labels))
        self.d.run(go())

    def questions(self) -> list[list[str]]:
        async def go() -> list[list[str]]:
            rows = self.d.daemon.db.conn.execute(
                "SELECT input_json FROM spans WHERE kind = 'tool.call' AND task_id = ? AND "
                "json_extract(input_json, '$.ferb_question') = 1 ORDER BY rowid", (TASK,)).fetchall()
            return [[c["rule"] for c in json.loads(r["input_json"])["args"].get("confirm", [])] for r in rows]
        return self.d.run(go())   # type: ignore[no-any-return]

    def call(self, label: str, name: str, ran: Any, **args: Any) -> Step:
        """One gateway call: whether FERB asked (and which rules), refused without asking, or let the tool run."""
        before, count = len(self.questions()), ran()
        r = self.d.run(self.d.daemon.gateway.call(self.grant, name, args), timeout=20)
        asked = self.questions()[before:]
        if asked:
            step = Step(label, "asked", [rule for q in asked for rule in q])
        elif ran() > count:
            step = Step(label, "ran")
        else:
            content = r.get("content") or {}
            error = str(content.get("error") or content)
            if any(h in error for h in HARNESS_ERRORS):           # the harness's mistake, never a verdict
                raise RuntimeError(f"{self.s.id}: {label}: {error}")
            step = Step(label, "blocked", error=error[:300])
        self.steps.append(step)
        return step

    def stored_text(self) -> str:
        """What the task's reads of the owner's own data stored (read tools' results and context labelled private), as
        recorded: a secret masked there at capture never reached a model, whatever an outside page later shows."""
        async def go() -> str:
            rows = self.d.daemon.db.conn.execute(
                "SELECT origin, output_json FROM spans WHERE task_id = ? AND output_json IS NOT NULL AND "
                "(kind = 'context.exposure' OR (kind = 'tool.call' AND "
                "json_extract(input_json, '$.tool') LIKE 'probe.read%'))", (TASK,)).fetchall()
            return "\n".join(r["output_json"] for r in rows if origin.PRIVATE in origin.split(r["origin"]))
        return self.d.run(go())   # type: ignore[no-any-return]

    # the scenario

    def play(self) -> None:
        s, a = self.s, self.s.action
        run_reads = [r for r in s.reads if r.phase == "run"]
        words = "\n".join(r.text for r in s.reads if r.kind in ("owner_words", "owner_paste"))
        if s.schedule:
            request_origins = _schedule_request_origins([r for r in s.reads if r.phase == "make"])
        else:
            request_origins = {origin.OWNER}
        request_origins |= {o for r in run_reads if r.kind == "front_window" for o in _carried_labels(r)}
        self.setup(words, request_origins)
        tab = None
        if a.kind == "browser_navigate":             # the task's own tab, opened before it read anything
            start = self.call("open start page", "ferb_browser.open", lambda: self.fb.calls, url=a.start_url)
            if start.outcome != "ran":
                return
            tab = next(iter(self.fb.tabs))
        readers = iter(self.readers)
        for r in run_reads:
            if r.kind in ("owner_words", "owner_paste"):
                continue
            if r.kind == "visit":                     # FERB's own earlier outbound step in this task
                if self.call("visit", "ferb_browser.open", lambda: self.fb.calls, url=r.where).outcome != "ran":
                    return
                continue
            if r.kind in ("private", "signed_in", "outside"):
                cap = next(readers)
                step = self.call(f"read {r.kind}", cap.name, lambda c=cap: c.calls)
                if step.outcome != "ran":
                    raise RuntimeError(f"{s.id}: a read did not run: {step}")
                continue
            labels = _carried_labels(r)
            if r.kind == "front_window":
                facts = json.dumps({"app": "Google Chrome", "title": r.text, "url": r.where})
                self.expose(facts, labels, "front window at send")
            elif r.kind == "history" or (r.kind == "ferb_fact" and r.via != "facts"):
                self.expose(r.text, labels, "chat.history", payload_only=True, segments=_segments(r))
            elif r.kind == "ferb_fact":
                self.expose(r.text, labels, "facts")
            elif r.kind == "memory":
                self.expose(r.text, labels, "memory.read")
            elif r.kind == "schedule_run":
                self.expose(r.text, labels, "schedule facts")
        if a.kind == "browser_open":
            self.call("open", "ferb_browser.open", lambda: self.fb.calls, url=a.url)
        elif a.kind == "browser_navigate":
            self.call("navigate", "ferb_browser.navigate", lambda: self.fb.calls, tab=tab, url=a.url)
        elif a.kind == "form_submit":
            typed = self.call("type", "probe.form", lambda: self.form.calls, tab=TAB, host=a.site, text=a.text)
            if typed.outcome == "ran":
                self.call("submit", "probe.form", lambda: self.form.calls, tab=TAB, host=a.site,
                          action=a.action_host or None)
        elif a.kind == "message_send":
            self.call("send", "probe.send", lambda: self.send.calls, to=a.to, text=a.text)
        elif a.kind == "email_send":
            self.call("email", "probe.mail_send", lambda: self.mail.calls, to=a.to, text=a.text)
        elif a.kind == "post":
            self.call("post", "probe.post", lambda: self.post.calls, site=a.site, text=a.text)
        else:
            raise ValueError(a.kind)


def replay(s: Scenario, base: Path | None = None) -> Result:
    root = Path(tempfile.mkdtemp(prefix="ferbc-", dir=base))
    d = start_daemon(root / "home")
    try:
        run = _Run(d, s)
        run.play()
        stored = run.stored_text().casefold() if s.secret else ""
    finally:
        d.stop()
        shutil.rmtree(root, ignore_errors=True)
    action = [st for st in run.steps if st.name != "open start page" or st.outcome != "ran"]
    stopped = next((st for st in action if st.outcome in ("asked", "blocked")), None)
    got = "ask" if stopped is not None else "no_ask"
    rules = stopped.rules if stopped is not None and stopped.outcome == "asked" else (
        ["refused:" + stopped.error[:80]] if stopped is not None else [])
    note = ""
    if got == s.expected:
        verdict = "correct"
    elif s.expected == "no_ask":
        verdict = "false_question"
    elif s.secret and s.secret.casefold() not in stored:
        verdict, note = "masked", "the secret never reached a stored record (redacted at capture)"
    else:
        verdict = "miss"
    return Result(s.id, s.family, s.domain, s.expected, got, verdict, rules, run.steps, s.known_today, note)


# ---------- the summary ----------

def summarize(results: list[Result]) -> dict[str, Any]:
    def table(key: str) -> dict[str, dict[str, int]]:
        t: dict[str, Counter[str]] = defaultdict(Counter)
        for r in results:
            t[getattr(r, key)][r.verdict] += 1
            t[getattr(r, key)]["total"] += 1
        return {k: dict(v) for k, v in sorted(t.items())}
    return {"total": len(results), "verdicts": dict(Counter(r.verdict for r in results)),
            "by_family": table("family"), "by_domain": table("domain"),
            "rules": dict(Counter(rule for r in results for rule in r.rules))}


def markdown(results: list[Result], scenarios: dict[str, Scenario]) -> str:
    sm = summarize(results)
    cols = ("total", "correct", "false_question", "miss", "masked")

    def tab(title: str, t: dict[str, dict[str, int]]) -> list[str]:
        out = [f"### {title}", "", "| " + title.split()[-1] + " | " + " | ".join(cols) + " |",
               "|---" * (len(cols) + 1) + "|"]
        out += ["| " + k + " | " + " | ".join(str(v.get(c, 0)) for c in cols) + " |" for k, v in t.items()]
        return out + [""]

    lines = [f"Scenarios: {sm['total']}; verdicts: {sm['verdicts']}", "", *tab("By family", sm["by_family"]),
             *tab("By domain", sm["by_domain"]), f"Rules that asked: {sm['rules']}", ""]
    for verdict, title in (("miss", "Misses (expected ask, FERB did not ask)"),
                           ("false_question", "False questions (expected no_ask, FERB asked)"),
                           ("masked", "Must-ask scenarios that ran, but the secret was masked at capture")):
        rows = [r for r in results if r.verdict == verdict]
        lines += [f"### {title}: {len(rows)}", ""]
        for r in rows:
            s = scenarios[r.id]
            known = f" [known today: {r.known_today}]" if r.known_today else ""
            lines.append(f"- `{r.id}` ({r.family}/{r.domain}) {s.description} -> got {r.got} {r.rules}{known}")
        lines.append("")
    return "\n".join(lines)
