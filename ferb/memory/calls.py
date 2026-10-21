"""Owns running one learning model call (consolidation, distillation) and its record. Must not keep the call's input.

Each call is one `model.learning` span: its role, the call's id (`call`), runtime and model as input; its outcome, the
input's size in characters and estimated tokens, and the runtime's documented limits as output; the error as the
runtime reported it (Codex's own message, redacted by the runtime and again by the span store). No task owns it
(task_id None), so it ages out with the other loose spans; `ferb trace <call>` finds it by `call`, and `ferb stats`
counts the calls and their errors."""
import asyncio
from typing import Any, Callable

from ferb.core.context import ContextBlock, assemble, count_tokens, runtime_limits
from ferb.core.errors import InputTooLarge
from ferb.core.structured import InvalidAnswer, check_final
from ferb.core.types import ModelRequest, Task


async def learning_call(rt: Any, task: Task, system: str, ctx: str, reference: str, schema: dict[str, Any], *,
                        role: str, spans: Any = None,
                        usage: Callable[[str, dict[str, Any]], None] = lambda *args: None) -> str:
    """`ctx` as one protected block within the runtime's documented limits (L28: its window and per-input character
    cap; over the cap it is refused, never sent), run without tools; the answer checked against `schema` (an answer
    outside it, by FERB's check or the runtime's own, raises InvalidAnswer)."""
    span = spans.start("model.learning", task_id=None, input={
        "role": role, "call": task.id, "goal": task.goal, "runtime": getattr(rt, "name", "unknown"),
        "model": getattr(rt, "model", None)}) if spans is not None else None
    facts: dict[str, Any] = {}
    try:
        limits = runtime_limits(rt)
        facts = {"context_budget_tokens": limits.context_budget, "max_input_chars": limits.max_input_chars}
        try:
            out = assemble([ContextBlock(ctx, reference, protected=True)], budget_tokens=limits.context_budget,
                           reserved_tokens=count_tokens(system), max_chars=limits.max_input_chars)
        except InputTooLarge as e:
            facts["input_chars"] = e.chars
            raise
        facts |= {"input_chars": out.chars, "input_tokens_estimate": out.tokens}
        run = await rt.start(task, ModelRequest(system, tuple(out.parts), out.cacheable), "")
        final = ""
        try:
            async for event in rt.events(run):
                if event.kind == "final":
                    final = str(event.data.get("text", ""))
                elif event.kind == "usage":
                    usage(rt.name, dict(event.data))
                elif event.kind == "retrying" and span is not None:  # retried by the runtime itself: the call goes on
                    retry = spans.start("model.retry", task_id=None, parent_id=span.id, input={"runtime": rt.name})
                    spans.finish(retry, error=str(event.data.get("message", "")))
                elif event.kind in ("error", "violation"):
                    raise RuntimeError(str(event.data.get("message") or event.data))
                elif event.kind == "invalid_answer":     # the runtime's own schema check: its reason, not "not JSON"
                    raise InvalidAnswer(str(event.data.get("message", "")))
        finally:
            await rt.cancel(run)
        try:
            answer = check_final(final, schema)
        except ValueError as e:              # the model's answer is wrong, not the call: callers may tell them apart
            raise InvalidAnswer(str(e)) from None
    except asyncio.CancelledError:           # FERB stopping (shutdown), not a failure of the learner
        if span is not None:
            spans.finish(span, output={"outcome": "cancelled", **facts})
        raise
    except BaseException as e:
        if span is not None:
            spans.finish(span, output={"outcome": "failed", **facts}, error=f"{type(e).__name__}: {e}")
        raise
    if span is not None:
        spans.finish(span, output={"outcome": "answered", **facts})
    return answer
