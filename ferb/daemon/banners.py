"""Own FERB's macOS banner categories, the owner's switches for them, and FERB.app as their poster (L34 D). Must not
decide what to tell the owner: callers decide that; this only routes what they hand it.

While FERB.app holds a `notifications.subscribe` stream (macOS lets it notify), each banner goes to it and FERB.app posts
it with its own name and icon (UNUserNotificationCenter); otherwise the daemon posts it with osascript (Script Editor's
name). The owner switches categories off in Settings ([banners] in config.toml); chat, logs and the journal keep
everything either way."""

import asyncio
from dataclasses import dataclass, field
from typing import Any, Callable

from ferb.core.errors import FerbError

SECTION = "banners"
CATEGORIES = ("question", "reminder", "alert", "task_finished", "loop")
# banners one FERB.app connection may leave unread; past that it stopped reading, and osascript posts the rest
QUEUE_SIZE = 100


@dataclass(frozen=True)
class Banner:
    title: str
    text: str
    category: str = ""                                      # one of CATEGORIES; "" for one no switch governs
    target: dict[str, str] = field(default_factory=dict)    # what a click opens: {"kind": "question"|"task"|"reminder", …}
    when_hidden: bool = False                               # FERB.app posts it only while the notch isn't showing it
    ident: str = ""                                         # the notification's id: a repost replaces it; "" for a new one
    card: dict[str, Any] = field(default_factory=dict)      # a reminder's typed card from its row (T-1055); {} for none


def enabled(get: Callable[..., Any], category: str) -> bool:
    """A category posts unless the owner set it to false; a banner with no category always posts."""
    return category not in CATEGORIES or get(SECTION, category, True) is not False


def categories(get: Callable[..., Any]) -> dict[str, bool]:
    return {c: enabled(get, c) for c in CATEGORIES}


def set_categories(config: Any, changes: Any) -> dict[str, bool]:
    """The owner's switches from Settings → Notifications, checked whole before anything is written."""
    if not isinstance(changes, dict) or not changes:
        raise FerbError("set names the categories to switch: " + ", ".join(CATEGORIES))
    unknown = sorted(str(k) for k in changes if k not in CATEGORIES)
    if unknown:
        raise FerbError(f"unknown banner categories {unknown}; they are {', '.join(CATEGORIES)}")
    if not all(isinstance(v, bool) for v in changes.values()):
        raise FerbError("each category is switched with true or false")
    for category, on in changes.items():
        config.set(SECTION, category, on)
    return categories(config.get)


class Posters:
    """FERB.app's open `notifications.subscribe` streams, one queue each."""

    def __init__(self) -> None:
        self._queues: list[asyncio.Queue[dict[str, Any]]] = []

    @property
    def connected(self) -> bool:
        return bool(self._queues)

    def open(self) -> asyncio.Queue[dict[str, Any]]:
        queue: asyncio.Queue[dict[str, Any]] = asyncio.Queue(maxsize=QUEUE_SIZE)
        self._queues.append(queue)
        return queue

    def close(self, queue: asyncio.Queue[dict[str, Any]]) -> None:
        self._queues = [q for q in self._queues if q is not queue]

    def deliver(self, item: dict[str, Any]) -> bool:
        """Hand the banner to every open poster; False when none took it (none connected, or each stopped reading)."""
        took = False
        for queue in self._queues:
            try:
                queue.put_nowait(item)
                took = True
            except asyncio.QueueFull:
                pass
        return took
