"""Own the daemon's side of the intro's "Try" beat (notes/setup-intro.md step 6): `intro.suggest`, one first task that
fits what FERB saw on this Mac (or a solid general task), and `intro.preview`, that task's plan shown as rows without
doing any of it. Must not act, start a task or record chat: the preview is the planner's turn alone (Supervisor.preview)
and the suggestion one tool-less model call; both are FERB.app's only (auth.APP_ONLY)."""

import asyncio
import json
from typing import Any, AsyncIterator

from ferb.core.errors import FerbError
from ferb.core.types import Identity, Task
from ferb.daemon.server import Session
from ferb.orchestrator.worker import couldnt

# the task shown when the scan isn't ready or found nothing clear: general, safe to show, true of any Mac
GENERAL_TASK = "Find three things on my Mac I could tidy up, and show me how you'd do it."
INTRO_SESSION = "intro"                  # the preview's planner turn reads no chat of the owner's: this one has none
SUGGEST_SCHEMA: dict[str, Any] = {"type": "object", "additionalProperties": False, "required": ["request"],
                                  "properties": {"request": {"type": "string"}}}


class Intro:
    """A mixin of the Daemon (ferb/daemon/main.py): its methods use the daemon's state."""

    async def _intro_suggest(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """`intro.suggest {}`: {"request", "fits"}. With what FERB saw on this Mac (decision 16, state "seen"), one
        model call with no tools reads its summary and facts, redacted, and names one task in the owner's voice
        (`fits` true). Without it (not started, scanning, failed, closed), or when the call fails or names nothing,
        the general task (`fits` false): it never fails the app."""
        general = {"request": GENERAL_TASK, "fits": False}
        seen = self._mac_profile_service().status()
        if seen.get("state") != "seen":
            return general
        summary = " ".join(str(self.redact(str(seen.get("summary") or ""))).split())
        facts = [" ".join(str(self.redact(str(f.get("text", "")))).split())
                 for f in seen.get("facts") or [] if isinstance(f, dict)]
        if not summary and not any(facts):
            return general
        try:
            request = await self._suggest(summary, [f for f in facts if f])
        except asyncio.CancelledError:
            raise
        except Exception as exc:
            self._journal().append("intro.suggested", {"fits": False, "why": type(exc).__name__})
            return general
        if not request:
            self._journal().append("intro.suggested", {"fits": False, "why": "nothing clear"})
            return general
        self._journal().append("intro.suggested", {"fits": True})
        return {"request": request, "fits": True}

    async def _suggest(self, summary: str, facts: list[str]) -> str:
        from ferb.core.prompts import load_prompt
        from ferb.memory.calls import learning_call
        capabilities = self.supervisor.availability().text if self.supervisor is not None else ""
        system = load_prompt("orchestrator/prompts/intro.md", {"capabilities": capabilities})
        task = Task(f"intro-suggest-{self.clock.now().isoformat()}", None, "a first task to show the owner",
                    str(self.config.get("device", "id", "dev")), Identity.OWNER, set(), "verifier", "intro-suggest",
                    [], None)
        rt = self._make_runtime("orchestrator", mode="supervisor", tool_names=[], schema=SUGGEST_SCHEMA)
        answer = json.loads(await learning_call(rt, task, system, json.dumps({"summary": summary, "facts": facts},
                                                                            ensure_ascii=False, indent=1),
                                                "what FERB saw on this Mac", SUGGEST_SCHEMA, role="intro_suggest",
                                                spans=self.spans, usage=self._record_learning_usage("intro_suggest")))
        return " ".join(str(self.redact(str(answer.get("request", "")))).split())

    async def _intro_preview(self, session: Session, params: dict[str, Any]) -> AsyncIterator[dict[str, Any]]:
        """`intro.preview {request}` (a stream): the request's plan as the real chat shows it, every event marked
        `preview: true` (`plan`, a `part` per step, then `done`), with nothing done (Supervisor.preview). A planner
        failure ends the stream with its reason, as a chat turn's does."""
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
