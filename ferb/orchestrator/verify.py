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
from ferb.orchestrator import cards
from ferb.orchestrator.schemas import ACTION_CHECKS, Condition
from ferb.policy.confirm import account_key, site_key
from ferb.policy.rules import Kind
from ferb.store.spans import SpanStore


UNCHECKED = "unchecked"            # no check gave a verdict: the work stopped first, or the checker itself failed (V5)
# ORCH-613: an action check whose action didn't happen because the owner declined it on its own card (and nothing
# later went ahead with it): not met, and the owner's decision, never FERB failing
DECLINED = "owner"
DECLINED_SAID = "You said no to it on its card, so FERB didn't do it"


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


def not_done_said(c: Condition) -> str:
    """ORCH-609: what a literal action check found false, as FERB tells the owner: one short sentence per kind, built
    from the condition's recorded target only (never its goal text): "No form on <site> was submitted." and the like.
    "" for a kind that isn't an action check."""
    k = c.check
    on = f" on {k.site}" if k.site else ""
    name = Path(k.path).name if k.path else "the file"
    return {"sent": f"Nothing was sent to {k.expected}{on}." if k.expected else f"Nothing was sent{on}.",
            "paid": f"No payment went through{on}.",
            "submitted": f"No form{on} was submitted.",
            "trashed": f"{name} wasn't moved to the Trash.",
            "deleted": f"{name} wasn't deleted.",
            "downloaded": f"{name} wasn't downloaded."}.get(k.kind, "")


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
        if k.kind in ACTION_CHECKS:                       # ORCH-576: an action the task's own records show
            return self.spans is not None and task_id is not None and (
                k.kind == "paid" or bool(k.expected if k.kind == "sent" else k.site if k.kind == "submitted"
                                         else k.path))
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
        if k.kind in ACTION_CHECKS:
            return self._action(i, c, task_id, ctx)
        if k.kind == "tab_url":
            urls = self.urls_of(task_id)
            matches = [url for url in urls if (k.url_contains or "") in url]
            import json
            return ConditionResult(i, c.text, bool(matches), "code", json.dumps({
                "url_contains": k.url_contains, "all_task_urls": urls, "matching_urls": matches}))
        return ConditionResult(i, c.text, None, "code", "Inspect the requested outcome with current source evidence")

    def _action(self, i: int, c: Condition, task_id: str, ctx: ToolContext) -> ConditionResult:
        """ORCH-576: met only when a call of that kind ran without an error with that target in the task's own records
        (its tool calls, as the cards read them; a download: the browser's taken file). A literal predicate: who, where
        or which file, compared as the gateway compares them, never what a report says."""
        assert self.spans is not None
        k = c.check

        def result(met: bool | None, evidence: str) -> ConditionResult:
            return ConditionResult(i, c.text, met, "code", evidence)

        def not_met(evidence: str) -> ConditionResult:
            """Not met; the owner's decision when the latest such call they were asked about is one they declined."""
            return self.declined(i, c, task_id, ctx) or result(False, evidence)
        aimed = self._aimed(c, ctx)
        if k.kind == "sent":
            assert aimed is not None
            if any(aimed[1](call) for call in cards.ran(self.spans, task_id, *aimed[0])):
                return result(True, f"FERB sent it to {k.expected}")
            on = f" on {k.site}" if k.site else ""
            return not_met(f"FERB didn't send it; no send to {k.expected}{on} went through in this task")
        if k.kind == "paid":
            assert aimed is not None
            calls = [x for x in cards.paid(self.spans, task_id) if aimed[1](x)]
            on = f" on {k.site}" if k.site else ""
            if calls:
                return result(True, f"FERB paid{on} after your Yes")
            return not_met(f"FERB didn't pay; no payment you said Yes to went through{on} in this task")
        if k.kind == "submitted":
            assert aimed is not None
            if any(aimed[1](call) for call in cards.ran(self.spans, task_id, *aimed[0])):
                return result(True, f"FERB submitted the form on {k.site}")
            return not_met(f"FERB didn't submit it; no form on {k.site} went through in this task")
        try:
            want_path = str(resolve(k.path or "", ctx, must_exist=False))
        except PathRefused as e:
            return result(False, f"{k.path}: {e}")
        if k.kind == "downloaded":
            if any(_same(p, want_path) for p in cards.downloaded(self.spans, task_id)):
                return result(True, f"FERB downloaded {want_path}")
            if Path(want_path).exists():         # a download FERB has no record of (a command's): for the checker
                return result(None, f"{want_path} is there, but FERB has no record of downloading it in this task")
            return result(False, f"FERB didn't download it; {want_path} isn't there and no download of it went "
                          "through in this task")
        assert aimed is not None
        done = [x for x in cards.ran(self.spans, task_id, *aimed[0]) if aimed[1](x)]
        did = (f"moved {want_path} to the Trash" if k.kind == "trashed" else f"deleted {want_path} permanently")
        if done:
            return result(True, f"FERB {did}")
        return not_met(f"FERB didn't do it; nothing in this task {did}")

    def _aimed(self, c: Condition, ctx: ToolContext | None) -> tuple[tuple[str, ...], Callable[[dict[str, Any]], bool]] | None:
        """An action check's call kinds and whether a recorded call has its target (who, where or which file, compared
        as the gateway compares them); None for a check with no such call (a download, a path FERB refuses)."""
        k = c.check

        def on_site(call: dict[str, Any]) -> bool:
            return not k.site or site_key(k.site) == site_key((call.get("target") or {}).get("site"))
        if k.kind == "sent":
            want = account_key(k.expected)

            def reaches(call: dict[str, Any]) -> bool:
                target = call.get("target") or {}
                reached = [account_key(r) for r in [*(target.get("recipients") or []), target.get("contact")] if r]
                return want in reached and on_site(call)
            return tuple(cards.SEND_KINDS), reaches
        if k.kind == "paid":
            return (Kind.PAY,), on_site
        if k.kind == "submitted":
            return (Kind.SUBMIT,), lambda call: site_key((call.get("target") or {}).get("site")) == site_key(k.site)
        if k.kind not in ("trashed", "deleted") or ctx is None:
            return None
        try:
            want_path = str(resolve(k.path or "", ctx, must_exist=False))
        except PathRefused:
            return None
        return ((Kind.TRASH if k.kind == "trashed" else Kind.DELETE_PERMANENT,),
                lambda call: _same(cards.target_path(call), want_path))

    def declined(self, i: int, c: Condition, task_id: str, ctx: ToolContext | None) -> ConditionResult | None:
        """ORCH-613: an action check's condition the owner decided: of the task's calls of its kind at its target that
        went ahead or that the owner was asked about, the latest is one they declined on its card (the gateway's
        `declined`). Not met, and theirs, whoever found it not met: code (nothing of the kind ran) or the checker (an
        earlier call of the kind ran, such as a checkout's first form, but the one they said no to was the one that
        would have done it). None otherwise. Structure only (rule 4)."""
        if self.spans is None or c.check.kind not in ACTION_CHECKS:
            return None
        aimed = self._aimed(c, ctx)
        if aimed is None:
            return None
        tried = [call for call in cards.tried(self.spans, task_id, *aimed[0]) if aimed[1](call)]
        if tried and tried[-1].get("declined"):
            return ConditionResult(i, c.text, False, DECLINED, DECLINED_SAID)
        return None


def _same(a: str, b: str) -> bool:
    """Two paths naming the same place, as written or resolved (a deleted file can't be resolved: as written)."""
    if not a or not b:
        return False
    try:
        return Path(a).expanduser().resolve(strict=False) == Path(b).expanduser().resolve(strict=False)
    except (OSError, ValueError):
        return a == b
