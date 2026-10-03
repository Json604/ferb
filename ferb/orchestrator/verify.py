"""Result observations and mechanical checks (ARCHITECTURE.md section 9).

File existence/content and tab URLs can be checked directly. Account identity and uncertain action outcomes need
judgment: pass the observed facts to a checker with the task's tools and recorded source reads. Missing observations
are unknown, not contradictions. Unknown or false conditions return to the planner for recovery.
"""

from dataclasses import dataclass
from pathlib import Path
from typing import Any, Awaitable, Callable

from ferb.act.fs.files import PathRefused, resolve
from ferb.core.types import ToolContext
from ferb.orchestrator.ledger import Ledger
from ferb.orchestrator.schemas import Condition


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
    """The deterministic checks. `urls_of` / `account_of` come from the browser drivers (a task's tabs)."""

    def __init__(self, ledger: Ledger, urls_of: Callable[[str], list[str]],
                 account_of: Callable[[str, str], Awaitable[str | None]]) -> None:
        self.ledger, self.urls_of, self.account_of = ledger, urls_of, account_of

    def code_checkable(self, c: Condition, task_id: str | None = None) -> bool:
        """`ledger_done` is about actions FERB took: when the task took none (it only read — "WhatsApp is
        running"), the condition is a reading one and goes to the verifier instead (owner's report 2026-10-01)."""
        k = c.check
        if k.kind == "ledger_done":
            return task_id is None or bool(self.ledger.for_task(task_id))
        return (k.kind == "file_exists" and bool(k.path)) or (k.kind == "file_contains" and bool(k.path and k.text)) \
            or (k.kind == "tab_url" and bool(k.url_contains)) or (k.kind == "account" and bool(k.site and k.expected))

    async def run(self, i: int, c: Condition, task_id: str, ctx: ToolContext) -> ConditionResult:
        k = c.check
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
            rows = self.ledger.for_task(task_id)
            states = sorted({r.intent.state for r in rows})
            return ConditionResult(i, c.text, None, "code",
                                   f"ledger rows: {', '.join(states) or 'none'}; input completion does not prove the goal")

        if k.kind == "tab_url":
            urls = self.urls_of(task_id)
            hit = next((u for u in urls if (k.url_contains or "") in u), None)
            return ConditionResult(i, c.text, hit is not None, "code",
                                   f"tab at {hit}" if hit else f"no tab URL contains {k.url_contains!r}")
        acct = await self.account_of(task_id, k.site or "")
        # An alias, a full address and a condition with alternatives need interpretation. An absent extractor
        # or closed tab is missing evidence, not evidence of a wrong account. Give the checker the observation.
        return ConditionResult(i, c.text, None, "code",
                               f"observed account on {k.site}: {acct}; requested: {k.expected}" if acct else
                               f"account on {k.site} unreadable; requested: {k.expected}; inspect page or recorded evidence")
