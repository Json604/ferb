"""The payment question's one-time picture (GATE-578, GATE-579, APP-828): the page a payment control is on, captured
when FERB asks (rule 5: prepared before asking), for the owner's eyes only.

Owner-only by construction: the bytes live in this process's memory only, keyed by the question's id. They never go
into a model call, a tool result, a span, the journal, a log or the database; FERB.app takes them once
(`questions.picture`, FERB.app only), and they go when the question is answered, set aside or closed, when its task
ends, or when the daemon stops (memory: nothing survives a restart). The owner's word (GATE-578) is that this picture
needs no masking, so a task tab is captured through `_TabDriver.owner_view` (HAND-497, extension 0.19.8: unmasked, no
secret-field look) and a picture is missing only when nothing could be captured (CaptureUnavailable). With an older
extension, and for a Mac app window, it is the live preview's capture (the capability's `preview_target`: the tab's
masked `record_frame`, the window through ScreenCaptureKit), used as it is; no text-recognition pass runs on it."""
from __future__ import annotations

import asyncio
import time
from collections.abc import Callable
from contextlib import AbstractAsyncContextManager
from dataclasses import dataclass, field
from typing import Any

from ferb.act.point import PREVIEW_DEADLINE

PICTURE_LIMIT_S = 8.0      # the most the question waits for its picture (the live preview's own limit, PREVIEW_LIMIT_S)
NOT_CAPTURED = "FERB couldn't capture the payment page."

Hold = Callable[[list[str], str], AbstractAsyncContextManager[None]]


@dataclass
class PayPictures:
    """The pictures waiting for FERB.app, by question id, with the task tree each belongs to (memory only)."""
    _held: dict[str, tuple[str, bytes]] = field(default_factory=dict)

    def hold(self, approval_id: str, root: str, image: bytes) -> None:
        self._held[approval_id] = (root, image)

    def has(self, approval_id: str) -> bool:
        return approval_id in self._held

    def take(self, approval_id: str) -> bytes | None:
        """The picture, once: FERB.app's one fetch removes it (the notch and the chat window share the app's copy)."""
        held = self._held.pop(approval_id, None)
        return held[1] if held is not None else None

    def drop(self, approval_id: str) -> None:
        self._held.pop(approval_id, None)

    def forget(self, roots: set[str]) -> None:
        """A task tree ended: its pictures go, whether or not the app took them."""
        for key in [k for k, (root, _) in self._held.items()
                    if any(root == r or root.startswith(r + ".") for r in roots)]:
            del self._held[key]


async def capture(cap: Any, args: Any, owner: str, hold: Hold, keys: list[str], holder: str,
                  limit_s: float = PICTURE_LIMIT_S) -> bytes | None:
    """One picture of the surface the payment call works on, or None when it has none or nothing could be captured
    within `limit_s`: a task tab's owner-only view when its extension has one, else the call's `preview_target`. Taken
    under the call's resource locks (queued like a call that reads, so the worker's next act can't run in the middle
    of it); never raises."""
    driver: Any = getattr(cap, "driver", None)
    tab = getattr(args, "tab", None)
    try:                                       # HAND-497: the driver method itself, never the op by name
        view, supported = driver.owner_view, driver.owner_view_supported
    except AttributeError:
        view = supported = None
    owned = bool(tab) and callable(view) and callable(supported)
    declare = getattr(cap, "preview_target", None)
    try:
        target = declare(args, owner, None) if callable(declare) else None
    except Exception:
        target = None
    if not owned and (target is None or target.resolve is None):
        return None

    async def take() -> bytes:
        PREVIEW_DEADLINE.set(time.monotonic() + limit_s)       # this task only: the capture's own waits fit the limit
        async with hold(list(keys), holder):
            raw: Any = None
            if owned:
                ref = driver.owned_by(str(tab), owner)
                if supported(ref):                             # GATE-579: unmasked, for the owner's eyes only
                    raw = (await view(ref)).jpeg()
            if raw is None:                                    # an older extension, or a Mac app window
                if target is None or target.resolve is None:
                    raise ValueError("no picture")
                _, shoot = await target.resolve()
                raw = await shoot()
        image = raw[0] if isinstance(raw, tuple) else raw
        if not isinstance(image, (bytes, bytearray)) or not image:
            raise ValueError("no picture")
        return bytes(image)

    try:
        return await asyncio.wait_for(take(), timeout=limit_s)
    except asyncio.CancelledError:
        raise
    except Exception:                                          # CaptureUnavailable included: "couldn't capture"
        return None
