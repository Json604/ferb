"""Owns the text of `ferb trace` (P26): the story (default) and the steps view. Must not add anything the records
don't say. Every number is exact except the "≈ $ API-equivalent" estimate, which says it is one (RQ4)."""

from datetime import datetime
from typing import Any

from ferb.trace import lines
from ferb.trace.compose import walk

# V4: "done" is recorded only when every condition was verified, "done_unverified" when the work finished but it stands
# unconfirmed; its label is "not confirmed" (V5: "unverified" is said only beside a condition a check ran on). A "done"
# without the verified flag is an older record that never said it was checked: shown as recorded.
STATUS = {"done": "done", "done_unverified": "done (not confirmed)", "failed": "failed ✗", "needs_user": "waiting for you",
          "cancelled": "stopped", "interrupted": "interrupted by a restart", "running": "running",
          "unknown": "outcome unknown"}


def _hms(at: str | None) -> str:
    return datetime.fromisoformat(at).strftime("%H:%M:%S") if at else "--:--:--"


def _k(n: int) -> str:
    return f"{n / 1000:.0f}k" if n >= 10000 else (f"{n / 1000:.1f}k" if n >= 1000 else str(n))


def _status(ep: dict[str, Any]) -> str:
    if ep.get("status") == "done" and ep.get("verified"):
        return "done ✓ verified"
    return STATUS.get(str(ep.get("status")), str(ep.get("status")))


def _cost(u: dict[str, Any]) -> str:
    if not u.get("usage_records"):
        return "Tokens: no usage recorded" + (f" for {u['model_calls']} model calls" if u.get("model_calls") else "")
    inp, cached = u["input_tokens"], u["cached_input_tokens"]
    line = f"Tokens: {_k(inp)} in ({round(100 * cached / inp) if inp else 0}% cached) · {_k(u['output_tokens'])} out"
    if u.get("cost_usd") is not None:
        note = "estimate; your subscription covers it" if u.get("billing") == ["subscription"] else "estimate"
        part = " for the priced calls" if u.get("unpriced_models") else ""
        line += f" · ≈ ${u['cost_usd']:.2f} API-equivalent{part} ({note})"
    if u.get("unpriced_models"):
        line += " · cost unknown for " + ", ".join(u["unpriced_models"])
    if u["usage_records"] < u.get("model_calls", 0):
        line += f" · usage recorded for {u['usage_records']} of {u['model_calls']} model calls"
    return line


def story(ep: dict[str, Any], verdict: dict[str, Any] | None = None) -> list[str]:
    u = ep.get("usage") or {}
    out = [f'#{ep["task_id"][:8]} · "{lines.clip(ep.get("request_text", ""), 80)}" · {_status(ep)} · '
           f'{lines.duration(ep.get("seconds"))} · {lines.plural(u.get("model_calls", 0), "model call", "model calls")}']
    for i, s in enumerate(ep.get("steps") or [], 1):
        mark = f"{s['mark']} " if s.get("mark") else ""
        out.append(f"{i}. {_hms(s['at'])} {s['role']} ({lines.duration(s.get('seconds'))}): {mark}{s['text']}")
    if ep.get("answer"):
        out.append("Answer: " + " ".join(str(ep["answer"]).split()))
    if ep.get("problems"):
        out.append(" · ".join(ep["problems"]))
    out.append(_cost(u))
    d = ep.get("desktop") or {}
    near = d.get("near_ferb") or []
    out.append(f"Desktop: {d.get('changes', 0)} changes during the task, "
               + (f"{len(near)} near a FERB on-screen action" if near else "none near a FERB on-screen action"))
    out += [f"  · {_hms(n['at'])} {', '.join(n['changed'])} near {', '.join(n['tools'])}" for n in near]
    if verdict:
        when = datetime.fromisoformat(verdict["at"]).strftime("%d %b %H:%M")
        out.append(f"Your verdict: {verdict['verdict']} ({when})" + (f" — {verdict['note']}" if verdict.get("note") else ""))
    return out


def steps(trace: dict[str, Any]) -> list[str]:
    calls = sorted((n for n in walk(trace.get("spans") or []) if n["kind"] == "tool.call"), key=lambda n: n["started_at"])
    out = []
    for n in calls:
        inp = n.get("input") or {}
        secs = ((datetime.fromisoformat(n["ended_at"]) - datetime.fromisoformat(n["started_at"])).total_seconds()
                if n.get("ended_at") else None)
        out.append(lines.step_line(n["started_at"], str(inp.get("tool", "?")), inp.get("args") or {},
                                   inp.get("why", ""), str(n.get("summary") or ""), secs, n.get("error")))
    return out
