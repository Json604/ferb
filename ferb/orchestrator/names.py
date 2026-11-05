"""Owns how a task, schedule, reminder or timer is named in a heading (T-1055, owner D32). Must not call a model or
rename a stored row: the name is the planner's, written when the item is made; code only shows it.

Every heading (cards, banners, the Tasks page, Settings → Schedules & Reminders) shows the short name. A row made
before names existed, or one the planner left unnamed, shows its instruction's first clause cut at NAME_CHARS with
"…" (structure only, `loops.short_label`). The full instruction stays where the owner opens the item in the app."""

from ferb.orchestrator.loops import short_label

NAME_CHARS = 40          # the cut of an unnamed row's instruction (D32: "~40 characters")
NAME_LIMIT = 60          # a written name longer than a heading line is cut too (the planner writes 2-5 words)


def shown_name(name: str | None, text: str) -> str:
    """The heading for an item: its short name (whitespace folded, cut at NAME_LIMIT at a word), else its
    instruction's first clause cut at NAME_CHARS."""
    words = " ".join(str(name or "").split())
    if not words:
        return short_label(text, NAME_CHARS)
    if len(words) <= NAME_LIMIT:
        return words
    return words[:NAME_LIMIT - 1].rsplit(" ", 1)[0].rstrip(" .,;:!?(") + "…"
