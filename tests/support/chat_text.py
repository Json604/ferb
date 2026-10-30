"""Chat events as plain text lines, for tests that read what a chat turn yielded (the CLI chat renderer this came from
was removed with `ferb chat`; the FERB app renders events itself)."""
from typing import Any

from ferb.sanitize.terminal import printable

# FERB's own Yes/No checks (L3); skipping one is a No (a guess's Keep / Edit / Drop: Drop; an out-of-date item: Keep)
FERB_CHECKS = {"ferb.confirm", "ferb.retry", "conversation.approve", "memory.forget", "memory.confirm",
               "memory.retire"}
SKIPPED = {"memory.confirm": "Drop", "memory.retire": "Keep"}
OUTCOME = {"done": "done ✓", "done_unverified": "done — not confirmed", "failed": "failed ✗"}
HIDDEN = {"owner_login_code", "owner_admin_password", "owner_secret"}


def render(ev: dict[str, Any], verbose: bool = False) -> str | None:
    t = ev.get("type")
    if t in ("reply", "clarify", "error"):
        return f"ferb> {ev['text']}"
    if t == "memory":
        return f"   ✎ {ev['text']}"
    if t == "timer":
        return f"   {ev['text']}" + (f"  [{ev['timer_id']}]" if verbose and ev.get("timer_id") else "")
    if t == "plan":
        if not verbose:
            return f"ferb> {ev['text']}"
        return f"ferb: {ev['text']}  [task {ev['task_id']}, {ev.get('identity', 'owner')}]: {ev.get('goal', '')}"
    if t == "subtask" and verbose:
        if ev["status"] == "started":
            return f"   ▸ {ev['id']} started"
        return f"   {'✓' if ev['status'] == 'done' else '↻' if ev['status'] == 'retrying' else '✗'} {ev['id']} " \
               f"{ev['status']}: {ev.get('summary', '')}"
    if t == "verification" and verbose:
        return "\n".join(f"   {'✓' if r['met'] else '?' if r['met'] is None else '✗'} {r['condition']}  "
                         f"[{r['path']}: {r['evidence']}]" for r in ev["results"])
    if t == "outcome":
        answer = ev.get("answer") or ev.get("summary", "")
        if not verbose:
            return f"ferb> {answer}"
        arts = f"\n      files: {', '.join(ev['artifacts'])}" if ev.get("artifacts") else ""
        return f"ferb: {OUTCOME.get(ev['status'], ev['status'])} — {ev['summary']}{arts}\nferb> {answer}"
    if t == "progress" and ev.get("connection"):
        return f"ferb> {ev['text']}"
    if t == "event" and verbose and ev["topic"] == "tool.called":
        return f"   · {ev['data'].get('tool')}"
    return None


def question(d: dict[str, Any], prompt: bool = True) -> str:
    """An approval in plain words, without terminal control characters (the question and options are model text)."""
    text = printable(_question(d))
    if prompt or d.get("kind") in HIDDEN:
        return text
    return text.rstrip().rsplit("\n", 1)[0] if "\n" in text.rstrip() else text


def _question(d: dict[str, Any]) -> str:
    if d.get("kind") in HIDDEN:
        return f"   ? {d.get('reason', '')} "
    if d.get("kind") == "owner_question" and d.get("options"):
        opts = "\n".join(f"     {i}) {o}" for i, o in enumerate(d["options"], 1))
        skip = SKIPPED.get(str(d.get("capability")), "No") if d.get("capability") in FERB_CHECKS else "skip"
        tail = f"Enter = {skip}" if d.get("capability") in FERB_CHECKS else "Enter to skip"
        return f"   ? {d.get('reason', '')}\n{opts}\n   number or your answer ({tail}): "
    return f"   ? {d.get('reason', '')}\n   your answer (Enter to skip): "
