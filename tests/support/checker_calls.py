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
