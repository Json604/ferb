"""Owns time facts attached to model turns and tool results. Must not infer ambiguous owner intent or turn reminders into unauthorized actions.

Time header (every model turn) and tool-result footer."""

from datetime import datetime


def _span(seconds: float) -> str:
    seconds = max(0, int(seconds))
    if seconds < 60:
        return f"{seconds}s"
    if seconds < 3600:
        return f"{seconds // 60}m"
    return f"{seconds // 3600}h{(seconds % 3600) // 60:02d}m"


def header(now: datetime, task_started: datetime | None = None, deadline: datetime | None = None,
           owner_last_active: datetime | None = None, last_trail_event: datetime | None = None) -> str:
    """For the start of every model turn. Unknown values say so (owner activity or trail may be unavailable)."""
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


# ---------- time awareness ----------

def ago(then: datetime, now: datetime) -> str:
    """A relative age computed now, always with the absolute time: "3 days ago (Sat 26 Sep, 14:02)"."""
    secs = (now - then).total_seconds()
    local = then.astimezone(now.tzinfo) if now.tzinfo else then
    stamp = local.strftime("%a %d %b, %H:%M")
    if secs < 0:
        return f"in {_span(-secs)} ({stamp})"
    if secs < 60:
        rel = "just now"
    elif secs < 3600:
        rel = f"{int(secs // 60)} min ago"
    elif secs < 86400:
        rel = f"{int(secs // 3600)} h ago"
    elif secs < 2 * 86400:
        rel = "yesterday"
    else:
        rel = f"{int(secs // 86400)} days ago"
    return f"{rel} ({stamp})"


def time_of_day(now: datetime) -> str:
    h = now.hour
    return "night" if h < 5 else "morning" if h < 12 else "afternoon" if h < 17 else "evening" if h < 22 else "night"


def time_block(now: datetime, task_started: datetime | None = None, deadline: datetime | None = None,
               owner_last_active: datetime | None = None, last_trail_event: datetime | None = None,
               ages: dict[str, datetime] | None = None,
               since_last_talk: list[str] | None = None) -> str:
    """Everything time-dependent, for the END of a prompt (the stable part before it stays cacheable): the header,
    time of day and idle time as facts, fresh relative ages of the tagged items above, and — after a gap of over an
    hour — what happened since the owner last talked to FERB."""
    lines = [header(now, task_started, deadline, owner_last_active, last_trail_event),
             f"Time of day: {time_of_day(now)}."]
    if owner_last_active is not None and (now - owner_last_active).total_seconds() >= 300:
        lines.append(f"The owner has been away from the Mac for {_span((now - owner_last_active).total_seconds())}.")
    if ages:
        lines.append("Ages of the items above: " + "; ".join(f"[{k}] {ago(v, now)}" for k, v in ages.items()))
    if since_last_talk:
        lines.append("Since you last talked with the owner:\n" + "\n".join(f"- {x}" for x in since_last_talk))
    return "\n".join(lines)
