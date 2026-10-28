"""Owns literal checks and observed result facts. Must not infer intent, choose a target or claim semantic success from transport facts.

Result observations and mechanical checks.

File existence/content and tab URLs yield literal predicates, never final outcome judgments. Account identity and uncertain action outcomes need
judgment: pass the observed facts to a checker with the task's tools and recorded source reads. Missing observations
are unknown, not contradictions. Unknown or false conditions return to the planner for recovery."""

from dataclasses import dataclass
from pathlib import Path
from typing import Any, Callable

from ferb.act import aftercheck
from ferb.act.fs.files import PathRefused, resolve
from ferb.core.types import ToolContext
from ferb.orchestrator.schemas import Condition
from ferb.store.spans import SpanStore


UNCHECKED = "unchecked"            # no check gave a verdict: the work stopped first, or the checker itself failed (V5)


@dataclass
class ConditionResult:
    index: int
    text: str
    met: bool | None               # None: couldn't be decided
    path: str                      # "code" | "verifier" | UNCHECKED
    evidence: str
    how: str = ""                  # how the owner could check it themselves (checker, unconfirmed only)

    def as_dict(self) -> dict[str, Any]:
        d = {"index": self.index, "condition": self.text, "met": self.met, "path": self.path,
             "evidence": self.evidence}
        return {**d, "how": self.how} if self.how else d


class Checks:
    """The deterministic checks. `urls_of` reports every current URL from the task's browser tabs."""

    def __init__(self, urls_of: Callable[[str], list[str]], spans: SpanStore | None = None) -> None:
        self.urls_of, self.spans = urls_of, spans

    def code_checkable(self, c: Condition, task_id: str | None = None) -> bool:
        """Only literal predicates are code-checked. Any other kind, including a stale `ledger_done` from an older
        plan, is unknown to code and goes to the checker: tool completion is not proof of the outcome (V6)."""
        k = c.check
        if k.kind in ("after_change", "after_value"):     # V2: the task's last acting call read something back
            return self.spans is not None and task_id is not None
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
        if k.kind in ("after_change", "after_value"):
            call = self.spans.latest_acting_call(task_id, k.tool) if self.spans is not None else None
            if call is None:
                return ConditionResult(i, c.text, None, "code", f"no {k.tool or 'acting'} call recorded in this task")
            after = (call["output"] or {}).get("after")     # a call that died has no output: nothing was read back
            if not isinstance(after, dict):
                return ConditionResult(i, c.text, None, "code",
                                       f"the latest {call['tool']} call recorded no after-read"
                                       + (f" ({call['error']})" if call["error"] else ""))
            expect = ({"value_contains": k.text or ""} if k.kind == "after_value"
                      else {"changed": (k.expected or "true").strip().lower() != "false"})
            failed = bool(call["error"]) or (call["output"] or {}).get("ok") is False    # as the tool itself reported
            if failed and expect.get("changed") is True and after.get("changed") is True:
                # The screen changed, but the call itself reported an error: the change read back may not be the one
                # expected (a partial or different effect). Not a confirmation; unknown, for the checker.
                return ConditionResult(i, c.text, None, "code", f"the latest {call['tool']} call changed the screen "
                                       f"but reported an error ({call['error'] or 'ok: false'}), so the change "
                                       "isn't confirmed")
            got = aftercheck.compare(expect, after)
            return ConditionResult(i, c.text, got["met"], "code", got["evidence"])
        if k.kind == "tab_url":
            urls = self.urls_of(task_id)
            matches = [url for url in urls if (k.url_contains or "") in url]
            import json
            return ConditionResult(i, c.text, bool(matches), "code", json.dumps({
                "url_contains": k.url_contains, "all_task_urls": urls, "matching_urls": matches}))
        return ConditionResult(i, c.text, None, "code", "Inspect the requested outcome with current source evidence")
