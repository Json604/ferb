"""Owns the one-line summary of a tool result and the step line format (P26). Must not judge or invent: every word
comes from the call's own arguments and result. The same step line feeds `ferb logs` live and `ferb trace --steps`."""

from datetime import datetime
from typing import Any, Callable

ARG_KEYS = ("url", "path", "command", "text", "query", "app", "bundle_id", "action", "element", "profile", "tab", "id")
MAX = 160


def clip(text: Any, n: int) -> str:
    s = " ".join(str(text).split())
    return s if len(s) <= n else s[: n - 1] + "…"


def plural(n: int, one: str, many: str) -> str:
    return f"{n} {one if n == 1 else many}"


def duration(seconds: float | None) -> str:
    if seconds is None:
        return "?s"
    s = int(round(seconds))
    return f"{s // 60}m{s % 60:02d}s" if s >= 60 else f"{s}s"


def _nonblank_lines(text: Any) -> int:
    return len([ln for ln in str(text).splitlines() if ln.strip()])


def _read(c: dict[str, Any]) -> str:
    size, text = c.get("size"), str(c.get("text", ""))
    if not c.get("offset") and c.get("next_offset") is None:
        return f"read {size} bytes"
    return f"read {len(text.encode())} of {size} bytes from offset {c.get('offset', 0)}"


def _list(c: dict[str, Any]) -> str:
    n, total = len(c.get("entries", [])), c.get("total")
    return f"{n} of {total} entries" if isinstance(total, int) and total > n else plural(n, "entry", "entries")


def _shell(c: dict[str, Any]) -> str:
    if c.get("running"):
        return f"still running ({c.get('id')})"
    text = f"exit {c.get('exit_code')} · {plural(_nonblank_lines(c.get('output', '')), 'line', 'lines')}"
    return text + (" · output capture failed" if c.get("capture_error") else "")


def _ask(c: dict[str, Any]) -> str:
    if c.get("declined") or "answer" not in c:
        return "no answer"
    who = "the orchestrator answered" if c.get("answered_by") == "orchestrator" else "you answered"
    return f'{who}: "{clip(c["answer"], 100)}"'


SUMMARIES: dict[str, Callable[[dict[str, Any], dict[str, Any]], str]] = {
    "fs.list": lambda a, c: _list(c),
    "fs.read": lambda a, c: _read(c),
    "fs.write": lambda a, c: f"wrote {c.get('path', a.get('path', ''))} ({c.get('bytes', c.get('size'))} bytes)",
    "fs.edit": lambda a, c: f"edited {c.get('path', a.get('path', ''))}",
    "fs.trash": lambda a, c: f"moved to Trash: {c.get('path', a.get('path', ''))}",
    "shell.run": lambda a, c: _shell(c),
    "shell.wait": lambda a, c: _shell(c),
    "owner.ask": lambda a, c: _ask(c),
}


def summarize(tool: str, args: dict[str, Any], content: dict[str, Any], is_error: bool) -> str:
    if is_error and content.get("error"):
        return clip("error: " + str(content["error"]), MAX)
    rule = SUMMARIES.get(tool)
    if rule is not None:
        try:
            return clip(rule(args, content), MAX)
        except (TypeError, ValueError, AttributeError, KeyError):
            pass
    return clip(", ".join(sorted(content)) or "done", MAX)


def step_line(at: str, tool: str, args: dict[str, Any], why: str, summary: str, seconds: float | None,
              error: str | None) -> str:
    shown = "  ".join(f"{k}={clip(args[k], 100)}" for k in ARG_KEYS if args.get(k) not in (None, ""))
    head = datetime.fromisoformat(at).strftime("%H:%M:%S") + f"  {tool}" + (f"  {shown}" if shown else "")
    if why:
        head += f"  (why: {clip(why, 100)})"
    tail = f"✗ {clip(error, 200)}" if error else f"→ {summary}"
    return f"{head}  {tail}  [{seconds:.1f}s]" if seconds is not None else f"{head}  {tail}"
