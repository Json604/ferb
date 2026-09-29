"""Verification and reconciliation (section 7 steps 8-9).

Verification checks each success condition with fresh reads, after the actors finish. Conditions with a
code-checkable form are checked by code — file exists / contains, the subtask's ledger rows are done, a tab's URL,
the signed-in account — and only conditions that need judgment go to a read-only verifier worker. Each result
records which path it took ("code" or "verifier") and its evidence. A worker's own claim is evidence, never
verification.

Reconciliation settles an `unknown` action (e.g. Send clicked, connection lost) by re-reading the target with a
read-only worker: happened -> done, didn't -> failed, unclear -> stays unknown and the owner decides. Never retried.
"""

import re
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Awaitable, Callable

from ferb.act.fs.files import PathRefused, resolve
from ferb.core.types import ToolContext
from ferb.orchestrator.ledger import Ledger
from ferb.orchestrator.schemas import Condition

REF = re.compile(r"\{([a-z][a-z0-9_-]*)\.([a-z][a-z0-9_-]*)\}")


@dataclass
class ConditionResult:
    index: int
    text: str
    met: bool | None               # None: couldn't be decided
    path: str                      # "code" | "verifier"
    evidence: str

    def as_dict(self) -> dict[str, Any]:
        return {"index": self.index, "condition": self.text, "met": self.met, "path": self.path,
                "evidence": self.evidence}


class Checks:
    """The deterministic checks. `urls_of` / `account_of` come from the browser drivers (a subtask's tabs)."""

    def __init__(self, ledger: Ledger, urls_of: Callable[[str], list[str]],
                 account_of: Callable[[str, str], Awaitable[str | None]]) -> None:
        self.ledger, self.urls_of, self.account_of = ledger, urls_of, account_of

    def code_checkable(self, c: Condition) -> bool:
        k = c.check
        return (k.kind == "file_exists" and bool(k.path)) or (k.kind == "file_contains" and bool(k.path and k.text)) \
            or k.kind == "ledger_done" or (k.kind == "tab_url" and bool(k.url_contains)) \
            or (k.kind == "account" and bool(k.site and k.expected))

    async def run(self, i: int, c: Condition, task_id: str, ctx: ToolContext,
                  fields: dict[str, dict[str, Any]] | None = None) -> ConditionResult:
        k = c.check
        if k.text and REF.search(k.text):                   # {reader.field}: the value a reader actually found
            missing = [m.group(0) for m in REF.finditer(k.text)
                       if not isinstance((fields or {}).get(m.group(1), {}).get(m.group(2)), str)]
            if missing:
                return ConditionResult(i, c.text, False, "code", f"no reader value for {', '.join(missing)}")
            k = k.model_copy(update={"text": REF.sub(lambda m: str((fields or {})[m.group(1)][m.group(2)]), k.text)})
        if k.kind in ("file_exists", "file_contains"):
            try:
                p = resolve(k.path or "", ctx, must_exist=True)
            except PathRefused as e:
                return ConditionResult(i, c.text, False, "code", f"{k.path}: {e}")
            if k.kind == "file_exists":
                return ConditionResult(i, c.text, True, "code", f"{p} exists")
            text = Path(p).read_text(errors="replace")[:2_000_000] if Path(p).is_file() else ""
            found = (k.text or "") in text
            return ConditionResult(i, c.text, found, "code", f"{p} {'contains' if found else 'does not contain'} "
                                                           f"the expected text")
        if k.kind == "ledger_done":
            rows = [r for r in self.ledger.for_task(task_id) if not k.expected or r.intent.capability == k.expected]
            states = sorted({r.intent.state for r in rows})
            met: bool | None = bool(rows) and states == ["done"]
            if "unknown" in states:
                met = None
            return ConditionResult(i, c.text, met, "code", f"ledger rows: {', '.join(states) or 'none'}")
        if k.kind == "tab_url":
            urls = self.urls_of(task_id)
            hit = next((u for u in urls if (k.url_contains or "") in u), None)
            return ConditionResult(i, c.text, hit is not None, "code",
                                   f"tab at {hit}" if hit else f"no tab URL contains {k.url_contains!r}")
        acct = await self.account_of(task_id, k.site or "")
        ok = acct is not None and acct.lower() == (k.expected or "").lower()
        return ConditionResult(i, c.text, ok, "code",
                               f"signed in on {k.site} as {acct}" if acct else f"account on {k.site} unreadable")
