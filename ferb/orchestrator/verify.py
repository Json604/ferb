"""Owns literal checks and observed result facts. Must not infer intent, choose a target or claim semantic success from transport facts.

Result observations and mechanical checks.

File existence/content and tab URLs yield literal predicates, never final outcome judgments. Account identity and uncertain action outcomes need
judgment: pass the observed facts to a checker with the task's tools and recorded source reads. Missing observations
are unknown, not contradictions. Unknown or false conditions return to the planner for recovery."""

from dataclasses import dataclass
from pathlib import Path
from typing import Any, Callable

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
    """The deterministic checks. `urls_of` reports every current URL from the task's browser tabs."""

    def __init__(self, ledger: Ledger, urls_of: Callable[[str], list[str]]) -> None:
        self.ledger, self.urls_of = ledger, urls_of

    def code_checkable(self, c: Condition, task_id: str | None = None) -> bool:
        """`ledger_done` is about actions FERB took: when the task took none (it only read — "WhatsApp is
        running"), the condition is a reading one and goes to the verifier instead (owner's report 2026-10-01)."""
        k = c.check
        if k.kind == "ledger_done":
            return task_id is None or bool(self.ledger.for_task(task_id))
        return (k.kind == "file_exists" and bool(k.path)) or (k.kind == "file_contains" and bool(k.path and k.text)) \
            or (k.kind == "tab_url" and bool(k.url_contains))

    async def run(self, i: int, c: Condition, task_id: str, ctx: ToolContext) -> ConditionResult:
        k = c.check
        if k.kind in ("file_exists", "file_contains"):
            try:
                p = resolve(k.path or "", ctx, must_exist=True)
                if k.kind == "file_exists":
                    return ConditionResult(i, c.text, True, "code", f"{p} exists")
                needle = k.text or ""
                found, carry = False, ""
                if Path(p).is_file():
                    with Path(p).open(encoding="utf-8") as source:
                        while chunk := source.read(65536):
                            text = carry + chunk
                            if needle in text:
                                found = True
                                break
                            carry = text[-(len(needle) - 1):] if len(needle) > 1 else ""
            except PathRefused as e:
                return ConditionResult(i, c.text, False, "code", f"{k.path}: {e}")
            except (OSError, UnicodeError) as e:
                # One failed observation route must not prevent the checker from inspecting the artifact
                # another way. Replacement decoding could invent text, so report the unavailable fact.
                return ConditionResult(i, c.text, None, "code",
                                       f"{k.path}: literal {k.kind} observation unavailable: {type(e).__name__}: {e}")
            return ConditionResult(i, c.text, found, "code", f"{p} {'contains' if found else 'does not contain'} "
                                                           f"the expected text")
        if k.kind == "ledger_done":
            rows = self.ledger.for_task(task_id)
            states = sorted({r.intent.state for r in rows})
            return ConditionResult(i, c.text, None, "code",
                                   f"ledger rows: {', '.join(states) or 'none'}; input completion does not prove the goal")

        if k.kind == "tab_url":
            urls = self.urls_of(task_id)
            matches = [url for url in urls if (k.url_contains or "") in url]
            import json
            return ConditionResult(i, c.text, bool(matches), "code", json.dumps({
                "url_contains": k.url_contains, "all_task_urls": urls, "matching_urls": matches}))
        return ConditionResult(i, c.text, None, "code", "Inspect the requested outcome with current source evidence")
