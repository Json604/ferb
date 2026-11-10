"""Owns the one-line summary of a tool result and the step line format (P26). Must not judge or invent: every word
comes from the call's own arguments and result. The same step line feeds `ferb logs` live and `ferb trace --steps`."""

import re
from datetime import datetime
from typing import Any, Callable

ARG_KEYS = ("url", "path", "command", "text", "query", "app", "bundle_id", "action", "element", "profile", "tab", "id")
MAX = 160


def clip(text: Any, n: int) -> str:
    s = " ".join(str(text).split())
    return s if len(s) <= n else s[: n - 1] + "…"


def shorten(text: Any, n: int, fold: bool = True) -> str:
    """A short form that never cuts a word (ORCH-601): whitespace folded (unless `fold` is False: the text's own line
    breaks kept); longer than `n`, it ends at the last sentence end that keeps at least half of it, else at the last
    word that fits, then "…". Only a single word longer than `n` is cut inside. For a line kept short (the planner's
    short forms, a live status line); text the owner reads as FERB's answer is never shortened."""
    s = " ".join(str(text).split()) if fold else str(text)
    if len(s) <= n:
        return s
    head = s[:n - 1]
    ends = [m.start() for m in re.finditer(r"[.!?]\s", head)]
    if ends and ends[-1] + 1 >= n // 2:
        return head[:ends[-1] + 1] + " …"
    spaces = [m.start() for m in re.finditer(r"\s", head)]
    return (head[:spaces[-1]].rstrip(" ,;:(—-\n\t") if spaces and spaces[-1] > 0 else head) + "…"


def plural(n: int, one: str, many: str) -> str:
    return f"{n} {one if n == 1 else many}"


def choices(options: Any) -> str:
    """Options as the owner reads them: "Yes or No", "A, B or C" (steering and the CLI's question line share it)."""
    opts = [str(o) for o in options]
    if len(opts) < 2:
        return "".join(opts)
    return ", ".join(opts[:-1]) + " or " + opts[-1]


# D31 fix round 4: which check decided each click's point, as `MacComputerDriver._own_point` notes it on the call's span
POINT_CHECKS = (("hit_test", "hit test"), ("window_list", "window list"), ("refused: cant_tell", "refused (can't tell)"))


def point_checks(counts: dict[str, int]) -> str:
    """The deciding checks of the clicks FERB checked, counted (`ferb trace`, `ferb stats`): how often the system hit
    test answered, the window list decided, or neither could tell."""
    return "Click point checks: " + ", ".join(f"{label} {counts.get(key, 0)}" for key, label in POINT_CHECKS)


def duration(seconds: float | None) -> str:
    if seconds is None:
        return "?s"
    s = int(round(seconds))
    return f"{s // 60}m{s % 60:02d}s" if s >= 60 else f"{s}s"


def front_pid(app: Any) -> int | None:
    """A front-app reading's pid; None when the front app couldn't be read."""
    pid = app.get("pid") if isinstance(app, dict) else None
    return pid if isinstance(pid, int) else None


class CameForward:
    """What came forward in one task's desktop records, read in order (P26). "Came to the front" is stated only when a
    readable front app differs from the last readable one, so a sample where the front app couldn't be read, and the
    same app read again after it, state nothing; "the Space changed" likewise needs a readable Space count that differs
    from the last readable one. The sentinel and the trace both read records through this one rule."""

    def __init__(self) -> None:
        self.pid: int | None = None
        self.epoch: int | None = None

    def baseline(self, snapshot: dict[str, Any]) -> None:
        self.pid = front_pid(snapshot.get("frontmost_app"))
        epoch = snapshot.get("active_space_epoch")
        self.epoch = epoch if isinstance(epoch, int) else None

    def change(self, changes: dict[str, Any]) -> str:
        """What one disturbance.change brought forward, "" for nothing: "Google Chrome came to the front", "the Space
        changed", or both (the bundle id when the app's name wasn't recorded)."""
        parts = []
        if "frontmost_app" in changes:
            entry = changes.get("frontmost_app") or {}
            before = front_pid(entry.get("before"))
            self.pid = before if before is not None else self.pid
            after = entry.get("after")
            pid = front_pid(after)
            if pid is not None and isinstance(after, dict):
                if self.pid is not None and pid != self.pid:
                    parts.append(f"{after.get('name') or after.get('bundle_id') or 'another app'} came to the front")
                self.pid = pid
        if "active_space_epoch" in changes:
            entry = changes.get("active_space_epoch") or {}
            before, after = entry.get("before"), entry.get("after")
            self.epoch = before if isinstance(before, int) else self.epoch
            if isinstance(after, int):
                if self.epoch is not None and after != self.epoch:
                    parts.append("the Space changed")
                self.epoch = after
        return " and ".join(parts)


def seconds(value: float) -> str:
    """An elapsed time: one decimal under 10 s, so a short gap never reads as 0 s; whole seconds from 10 s."""
    return f"{value:.1f}" if value < 10 else str(int(value))


def no_on_screen_action(what: str, processes: list[dict[str, Any]] | None, task_id: str, *, in_task: bool | None,
                        last_ended_s: float | None) -> str:
    """A desktop change with no FERB on-screen call in flight or just ended (P26): what happened, when the latest
    on-screen FERB call ended (`in_task` False: this task made none; None: a record made before this was kept;
    `last_ended_s` None: none since the daemon started), and the processes FERB started that were running then. This
    task's show their (redacted) command; another task's show only their id, since its command text was never this
    task's to read (MP2). Facts only; who did it is the reader's conclusion (rule 6, controller ruling 2026-10-09): a
    FERB effect can land late."""
    if processes is None:
        running = "not recorded"
    else:
        tree = task_id.split(".", 1)[0]
        running = "; ".join(
            f"{clip(p.get('command', ''), 60)} (started by this task)" if str(p.get("task", "")).split(".", 1)[0] == tree
            else f"{p.get('id', '?')} (started by another FERB task)" for p in processes) or "nothing FERB started"
    if in_task is None or (in_task and last_ended_s is None):
        when = ""
    elif last_ended_s is None:
        when = " (none yet)"
    elif in_task:
        when = f" (the last ended {seconds(last_ended_s)} s earlier)"
    else:
        when = f" (none in this task; the last of any FERB task ended {seconds(last_ended_s)} s earlier)"
    return f"{what} — no FERB on-screen action in progress{when}; running: {running}"


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


def _ask(a: dict[str, Any], c: dict[str, Any]) -> str:
    """A worker's question, or FERB's own (its args name what it confirms, D33)."""
    ferb = " (FERB's question)" if a.get("confirm") else ""
    if c.get("declined") or "answer" not in c:
        return "no answer" + ferb
    who = "the orchestrator answered" if c.get("answered_by") == "orchestrator" else "you answered"
    return f'{who}: "{clip(c["answer"], 100)}"{ferb}'


SUMMARIES: dict[str, Callable[[dict[str, Any], dict[str, Any]], str]] = {
    "fs.list": lambda a, c: _list(c),
    "fs.read": lambda a, c: _read(c),
    "fs.write": lambda a, c: f"wrote {c.get('path', a.get('path', ''))} ({c.get('bytes', c.get('size'))} bytes)",
    "fs.edit": lambda a, c: f"edited {c.get('path', a.get('path', ''))}",
    "fs.trash": lambda a, c: f"moved to Trash: {c.get('path', a.get('path', ''))}",
    "shell.run": lambda a, c: _shell(c),
    "shell.wait": lambda a, c: _shell(c),
    "shell.send": lambda a, c: 'sent: "' + clip(str(a.get("text", "")), 120) + '"',
    "mac.clipboard": lambda a, c: f"wrote {c.get('characters', 0)} characters to clipboard",
    "owner.ask": lambda a, c: _ask(a, c),
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
