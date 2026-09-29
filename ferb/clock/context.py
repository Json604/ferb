"""Time header (every model turn) and tool-result footer (section 13)."""

from datetime import datetime


def _span(seconds: float) -> str:
    seconds = max(0, int(seconds))
    if seconds < 60:
        return f"{seconds}s"
    if seconds < 3600:
        return f"{seconds // 60}m"
    return f"{seconds // 3600}h{(seconds % 3600) // 60:02d}m"


def header(now: datetime, task_started: datetime | None = None, deadline: datetime | None = None,
           owner_last_active: datetime | None = None, last_trail_event: datetime | None = None,
           typical: str | None = None) -> str:
    """For the start of every model turn. Unknown values say so (owner activity + trail arrive with build step 10)."""
    parts = [f"now {now.strftime('%A %Y-%m-%d %H:%M:%S')} {now.strftime('%Z') or now.strftime('%z')}"]
    if task_started:
        parts.append(f"task started {task_started.strftime('%H:%M:%S')} (+{_span((now - task_started).total_seconds())})")
    if deadline:
        remaining = (deadline - now).total_seconds()
        parts.append(f"deadline {deadline.strftime('%H:%M:%S')} "
                     f"({'in ' + _span(remaining) if remaining >= 0 else 'passed ' + _span(-remaining) + ' ago'})")
    parts.append("owner last active " + (_span((now - owner_last_active).total_seconds()) + " ago"
                                         if owner_last_active else "unknown"))
    parts.append("last trail event " + (_span((now - last_trail_event).total_seconds()) + " ago"
                                        if last_trail_event else "unknown"))
    if typical:
        parts.append(typical)                                     # measured durations (build step 14)
    return "[" + " | ".join(parts) + "]"


def footer(now: datetime, task_started: datetime | None = None, deadline: datetime | None = None) -> str:
    """Appended to every tool result, e.g. `[now 15:42:10 IST | task +11m | deadline in 18m]`."""
    parts = [f"now {now.strftime('%H:%M:%S')} {now.strftime('%Z') or now.strftime('%z')}"]
    if task_started:
        parts.append(f"task +{_span((now - task_started).total_seconds())}")
    if deadline:
        remaining = (deadline - now).total_seconds()
        parts.append(f"deadline in {_span(remaining)}" if remaining >= 0 else f"deadline passed {_span(-remaining)} ago")
    return "[" + " | ".join(parts) + "]"
