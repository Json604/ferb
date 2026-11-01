"""Own the daemon's side of the intro's "Try" beat (notes/setup-intro.md step 6): `intro.suggest`, two first tasks that
fit what FERB saw on this Mac (topped up with general tasks), and `intro.preview`, the picked task's plan shown as rows
without doing any of it. Must not act, start a task or record chat: the preview is the planner's turn alone
(Supervisor.preview) and the suggestion one tool-less model call; both are FERB.app's only (auth.APP_ONLY)."""

import asyncio
import json
from typing import Any, AsyncIterator

from ferb.core.errors import FerbError
from ferb.core.types import Identity, Task
from ferb.daemon.server import LONG_WAITS, Session
from ferb.orchestrator.continuity import PREVIEW_SESSION
from ferb.orchestrator.worker import couldnt

# shown when the scan isn't ready or found nothing clear, and to top up a reply with fewer than two: general, safe to
# show, true of any Mac; one on the Mac and its files, one in the browser (owner, 2026-11-01)
GENERAL_TASKS = ("Find three things on my Mac I could tidy up, and show me how you'd do it.",
                 "Compare three well-reviewed electric kettles on the web and tell me which is the best value.")
# the one main app each general task would use, as macOS names it; "" is the owner's default browser (the app resolves
# it), so the intro's orb can point there while the plan shows
GENERAL_APPS = ("Finder", "")
# an app name is a short label the app matches against running apps' names
APP_NAME_MAX = 60
# a suggestion is one short request shown as a choice in the notch, cut at a word boundary
REQUEST_MAX = 280
# the preview's planner session: `chat.send` refuses the prefix, so no owner chat (or anything said in one) is in it
INTRO_SESSION = PREVIEW_SESSION + "intro"
# a technical bound, not a cap of the owner's: the suggestion's model call ends just before the app stops waiting for
# its reply (server.LONG_WAITS), so a stalled model still gives the app the general tasks
SUGGEST_LIMIT_S = LONG_WAITS["intro.suggest"] - 5.0
SUGGEST_SCHEMA: dict[str, Any] = {
    "type": "object", "additionalProperties": False, "required": ["requests"],
    "properties": {"requests": {"type": "array", "items": {
        "type": "object", "additionalProperties": False, "required": ["request", "app"],
        "properties": {"request": {"type": "string"}, "app": {"type": "string"}}}}}}


def _key(request: str) -> str:
    return " ".join(request.casefold().split())


def _within(words: str, limit: int) -> str:
    """`words` (single-spaced) cut to at most `limit` characters at a word boundary, with nothing added; one word
    longer than that is cut at the limit."""
    if len(words) <= limit:
        return words
    head = words[:limit]
    if words[limit] != " " and " " in head:
        head = head.rsplit(" ", 1)[0]
    return head.rstrip()


def two(from_scan: list[tuple[str, str]]) -> tuple[list[tuple[str, str]], int]:
    """The two suggestions as (request, app): the scan's first two distinct requests, topped up from GENERAL_TASKS
    with their GENERAL_APPS (two identical requests count as one); and how many of them came from the scan."""
    chosen: list[tuple[str, str]] = []
    for request, app in from_scan:
        if request and _key(request) not in {_key(c) for c, _ in chosen} and len(chosen) < 2:
            chosen.append((request, app))
    used = len(chosen)
    for request, app in zip(GENERAL_TASKS, GENERAL_APPS):
        if len(chosen) < 2 and _key(request) not in {_key(c) for c, _ in chosen}:
            chosen.append((request, app))
    return chosen, used


class Intro:
    """A mixin of the Daemon (ferb/daemon/main.py): its methods use the daemon's state."""

    async def _intro_suggest(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """`intro.suggest {}`: {"suggestions": [{"request", "app"}, {"request", "app"}], "fits"}. With what FERB saw on
        this Mac (decision 16, state "seen"), one model call with no tools reads its summary and facts, redacted, and
        names two tasks in the owner's voice, of different kinds where it can, each with the one main app it would use
        (as macOS names it; "" for the default browser), so the intro's orb can point there; fewer than two usable are
        topped up from GENERAL_TASKS (with GENERAL_APPS). `fits`: at least one came from the scan. Without a scan (not
        started, scanning, failed, closed), or when the call fails, stalls or names nothing, the two general tasks: it
        never fails the app. Each outcome is journaled (`intro.suggested`, with why; never the summary)."""
        from_scan: list[tuple[str, str]] = []
        why = ""
        seen = self._mac_profile_service().status()
        summary, facts = "", []
        if seen.get("state") != "seen":
            why = f"scan {seen.get('state')}"
        else:
            summary = " ".join(str(self.redact(str(seen.get("summary") or ""))).split())
            facts = [f for f in (" ".join(str(self.redact(str(f.get("text", "")))).split())
                                 for f in seen.get("facts") or [] if isinstance(f, dict)) if f]
            if not summary and not facts:
                why = "nothing seen"
        if not why:
            try:
                async with asyncio.timeout(SUGGEST_LIMIT_S):
                    from_scan = await self._suggest(summary, facts)
            except asyncio.CancelledError:
                raise
            except TimeoutError:
                why = "timed out"
            except Exception as exc:
                why = type(exc).__name__
        chosen, used = two(from_scan)
        if not why and used < 2:
            why = "topped up" if used else "nothing clear"
        self._journal().append("intro.suggested", {"fits": used > 0, "from_scan": used, **({"why": why} if why else {})})
        return {"suggestions": [{"request": request, "app": app} for request, app in chosen], "fits": used > 0}

    async def _suggest(self, summary: str, facts: list[str]) -> list[tuple[str, str]]:
        from ferb.core.prompts import load_prompt
        from ferb.memory.calls import learning_call
        capabilities = self.supervisor.availability().text if self.supervisor is not None else ""
        system = load_prompt("orchestrator/prompts/intro.md", {"capabilities": capabilities})
        task = Task(f"intro-suggest-{self.clock.now().isoformat()}", None, "first tasks to show the owner",
                    str(self.config.get("device", "id", "dev")), Identity.OWNER, set(), "verifier", "intro-suggest",
                    [], None)
        rt = self._make_runtime("orchestrator", mode="supervisor", tool_names=[], schema=SUGGEST_SCHEMA)
        answer = json.loads(await learning_call(rt, task, system, json.dumps({"summary": summary, "facts": facts},
                                                                            ensure_ascii=False, indent=1),
                                                "what FERB saw on this Mac", SUGGEST_SCHEMA, role="intro_suggest",
                                                spans=self.spans, usage=self._record_learning_usage("intro_suggest")))
        def words(value: Any) -> str:
            return " ".join(str(self.redact(str(value or ""))).split())
        found: list[tuple[str, str]] = []
        for item in answer.get("requests") or []:
            if isinstance(item, dict):
                found.append((_within(words(item.get("request")), REQUEST_MAX),
                              words(item.get("app"))[:APP_NAME_MAX]))
        return found

    async def _intro_preview(self, session: Session, params: dict[str, Any]) -> AsyncIterator[dict[str, Any]]:
        """`intro.preview {request}` (a stream; the app previews the suggestion the owner picked): its plan as the real
        chat shows it, every event marked `preview: true` (`plan`, a `part` per step, then `done`), with nothing done
        (Supervisor.preview). A planner failure ends the stream with its reason, as a chat turn's does; the app leaving
        ends it (a stream ends when its caller leaves)."""
        if self._shutting_down:
            raise FerbError("daemon is shutting down; no preview was started")
        if self.supervisor is None:
            raise FerbError("daemon not started")
        request = " ".join(self._redact_text(str(params.get("request") or "")).split())
        if not request:
            raise FerbError("request must be a nonempty task")
        try:
            async for event in self.supervisor.preview(INTRO_SESSION, request):
                yield event
        except (FerbError, asyncio.CancelledError):
            raise
        except Exception as exc:
            raise FerbError(self._redact_text(couldnt("show that plan", exc, self.redact))) from None
