"""The planner's conversation as tests read it (L28): the timeline's entries, oldest first, as their display lines."""
from typing import Any


def conversation_text(supervisor: Any, conversation: str, current_request: str = "") -> str:
    """Call on the daemon's loop. Every entry after FERB's running summary, complete, joined like the prompt's parts."""
    entries, _ = supervisor._timeline(current_request, conversation, supervisor._summary(conversation))
    return "\n\n".join(entry.text for entry in entries)
