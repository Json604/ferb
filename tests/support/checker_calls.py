"""A recorded worker task for the checker's starting evidence (fix wave 6b): probe tools whose result is the text they
are given, a scripted checker that rebuilds every recorded call from its context and task.evidence (counting the
task.evidence calls it needs), and a narrowed window for the checker's next call."""

import json
import re
from typing import Any, Callable

from pydantic import BaseModel

from ferb.core.context import ContextBlock, overhead_chars
from ferb.core.types import ActionSpec, Identity, RiskLevel, ToolContext, ToolResult
from tests.conftest import tool

# A recorded call shown in the checker's context: its header line, then its text in FERB's data wrapper
SHOWN = re.compile(r'^Recorded call \d+ of \d+[^\n]*?task\.evidence span_id=([^\s,:]+)(?:[^\n]*?offset=(\d+))?:\n'
                   r'<untrusted-data id="([0-9a-f]+)">\n(.*?)\n</untrusted-data id="\3">', re.S | re.M)
LEFT_OUT = re.compile(r'\[Context omitted; fetch full text: task\.evidence span_id=([^\s,:]+) \(recorded call')


class Echo:
    """A probe tool whose result is the text it is given, plus `pad` dots; `observation` as the test declares it."""
    identities = {Identity.OWNER, Identity.FERB}
    risk, reversible = RiskLevel.LOW, True

    class Input(BaseModel):
        text: str = ""
        pad: int = 0

    def __init__(self, name: str, kind: str, observation: bool, reads: tuple[tuple[str, str], ...] = ()) -> None:
        self.name = self.description = name
        self.kind, self.observation, self.reads, self.scopes = kind, observation, reads, set()

    def describe(self, args: Any) -> ActionSpec:
        return ActionSpec(self.kind)

    async def run(self, args: Any, ctx: ToolContext) -> ToolResult:
        return ToolResult({"text": args.text + "." * args.pad}, read_sources=self.reads)


class Saves(Echo):
    """A probe action whose result is short whatever it is given: how many characters it saved."""

    async def run(self, args: Any, ctx: ToolContext) -> ToolResult:
        return ToolResult({"saved": len(args.text) + args.pad})


class Pads(Echo):
    """A probe read whose result puts the text it is given after `pad` letters and before 300 more, so the result is
    always the longer side of its record and a test can place the text at a chosen offset of the record."""

    async def run(self, args: Any, ctx: ToolContext) -> ToolResult:
        return ToolResult({"text": "a" * args.pad + args.text + "b" * 300})


def latest_call(daemon: Any) -> str:
    """The span id of the latest recorded tool call."""
    async def go() -> str:
        return str(daemon.daemon.db.conn.execute(
            "SELECT id FROM spans WHERE kind = 'tool.call' ORDER BY rowid DESC LIMIT 1").fetchone()[0])
    return daemon.run(go())                                     # type: ignore[no-any-return]


def add(daemon: Any, *caps: Echo) -> None:
    for cap in caps:
        daemon.daemon.registry.capabilities[cap.name] = cap


def verdict(met: bool | None = True, evidence: str = "Seen in the recorded calls.") -> str:
    return json.dumps({"results": [{"index": 0, "met": met, "evidence": evidence,
                                    "how": "Look at it yourself." if met is None else None}]})


def checker_context(script: Any) -> str:
    """The latest checker context the scripted runtime was given."""
    return next(c for _, key, c, _ in reversed(script.contexts) if key.endswith(".verify"))


def bundle(context: str) -> tuple[dict[str, tuple[str, int | None]], list[str]]:
    """The recorded calls the checker's context holds: span_id -> (text shown, the offset its rest starts at or None),
    and the span ids listed as left out."""
    shown = {m.group(1): (m.group(4).replace("<\\/untrusted-data", "</untrusted-data"),
                          int(m.group(2)) if m.group(2) else None) for m in SHOWN.finditer(context)}
    return shown, LEFT_OUT.findall(context)


def rebuild(sock: Any, token: str, context: str, *, use_context: bool = True) -> tuple[dict[str, str], int]:
    """Every recorded call's complete record text, as a checker that must see all of them gets it, and how many
    task.evidence calls that took. use_context=False is the checker before fix wave 6b: nothing in its context, so it
    lists the calls page by page and reads each one."""
    calls = 0

    def evidence(**args: Any) -> dict[str, Any]:
        nonlocal calls
        calls += 1
        result = tool(sock, token, "task.evidence", **args)
        assert not result["is_error"], result
        return result["content"]                                   # type: ignore[no-any-return]
    shown: dict[str, tuple[str, int | None]] = {}
    if use_context:
        shown, left_out = bundle(context)
        ids = list(shown) + left_out
    else:
        ids, after = [], 0
        while after is not None:
            page = evidence(kind="tool.call", after=after)
            ids += [r["id"] for r in page["records"]]
            after = page["next_after"]
    texts = {}
    for span_id in ids:
        text, offset = shown.get(span_id, ("", 0))
        while offset is not None:
            page = evidence(span_id=span_id, offset=offset)
            text, offset = text + page["text"], page["next_offset"]
        texts[span_id] = text
    return texts, calls


def narrow_checker(monkeypatch: Any, keep: Callable[[ContextBlock], bool]) -> None:
    """The checker's calls get a per-input character cap that holds its protected context, a marker for every block
    left out, and exactly the recorded calls `keep` selects, with far less than one more call's room to spare. The
    choice of what fits stays `assemble`'s own."""
    import ferb.orchestrator.worker as worker
    real = worker.assemble

    def fitted(blocks: list[ContextBlock], *, budget_tokens: int, reserved_tokens: int = 0,
               max_chars: int | None = None) -> Any:
        active = [b for b in blocks if b.text]
        if any(b.text.startswith("Recorded call ") for b in active):
            def marker(b: ContextBlock) -> int:
                return len(f"[Context omitted; fetch full text: {b.reference}]")
            base = overhead_chars(budget_tokens, reserved_tokens) + 64 + sum(
                len(b.text) + 2 if b.protected else marker(b) + 2 for b in active)
            max_chars = base + sum(len(b.text) - marker(b) for b in active if not b.protected and keep(b))
        return real(blocks, budget_tokens=budget_tokens, reserved_tokens=reserved_tokens, max_chars=max_chars)
    monkeypatch.setattr(worker, "assemble", fitted)


# A fold note `assemble` writes for the lowest-priority recorded calls whose markers couldn't all fit (fix round 1, I1)
FOLDED = re.compile(r"\[(\d+) more not shown or listed one by one; fetch full text: task\.evidence kind=tool\.call, "
                    r"the (observations|actions) among these recorded calls — from task\.evidence span_id=([^\s]+) "
                    r"\(recorded call (\d+) of \d+: [^)]*\) to task\.evidence span_id=([^\s]+) "
                    r"\(recorded call (\d+) of \d+: [^)]*\)\]")


def recorded_task(daemon: Any, task_id: str, calls: int) -> tuple[list[str], list[str]]:
    """A worker run of `calls` short recorded calls (about 340 characters each, as in review 1's measurement), every
    third an action: their span ids and roles, in recorded order. Recorded with FERB's production ids (32 hex
    characters), not the test daemon's short sequential ones, so each marker is its real size."""
    from ferb.core.ids import RandomIds
    from ferb.store.spans import SpanStore

    async def go() -> tuple[list[str], list[str]]:
        d = daemon.daemon
        spans = SpanStore(d.db, d.clock, RandomIds(), d.redact)
        run = spans.start("worker", task_id=task_id, input={"goal": "label these emails"})
        ids, roles = [], []
        for i in range(calls):
            acts = i % 3 == 0
            span = spans.start("tool.call", task_id=task_id, parent_id=run.id,
                               input={"tool": "probe.act" if acts else "probe.read",
                                      "args": {"text": f"label message {i}"}, "kind": "navigate" if acts else "read"})
            spans.finish(span, output={"text": f"message {i}: " + "subject and preview line " * 8},
                         origin="external:email")
            ids.append(span.id)
            roles.append("action" if acts else "observation")
        spans.finish(run, output={})
        return ids, roles
    return daemon.run(go(), timeout=300)                        # type: ignore[no-any-return]


def reachable(context: str, ids: list[str], roles: list[str]) -> tuple[set[str], set[str], set[str]]:
    """How each recorded call can be reached from an assembled checker context: shown, listed by its own span_id, or
    folded into a note's range (every call of the note's role between its first and last that isn't shown or listed),
    each exactly once."""
    shown, listed = set(bundle(context)[0]), set(LEFT_OUT.findall(context))
    assert not shown & listed
    folded: set[str] = set()
    for count, role, first, a, last, b in FOLDED.findall(context):
        a_, b_ = int(a) - 1, int(b) - 1
        assert ids[a_] == first and ids[b_] == last and roles[a_] == roles[b_] == role[:-1]
        inside = {ids[i] for i in range(a_, b_ + 1) if roles[i] == role[:-1]} - shown - listed
        assert len(inside) == int(count) and not inside & folded
        folded |= inside
    assert shown | listed | folded == set(ids)
    return shown, listed, folded
