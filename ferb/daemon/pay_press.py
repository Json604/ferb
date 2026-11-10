"""The page as it is just before a covered payment press (GATE-580), read fresh rather than from the latest snapshot.

A snapshot's facts (`ActionFacts.page_amounts`, the page's address) are as old as the snapshot: a total a script
changes while the owner looks at the question isn't in them. So just before the press, under the call's resource
locks, the tab is read again through two of the browser driver's reads that take no new snapshot (a new one would make
the pressed element's id stale): `shows` (the tab's address now, from the browser's tab list) and `find` (the page's
text lines, all frames, holding the approved amount's last digits; `payments.digits_of`). The money amounts on those
lines (`page_amounts`, the GATE-576 shape) are what `payments.pressed_change` compares, as GATE-217 compares. Cost:
three extension round trips (the tab list and two page searches), only for a press an approval covers on a browser
tab. A tab whose driver has no such reads (a Mac app) gives None: nothing fresher to compare."""
from __future__ import annotations

import asyncio
import json
from collections.abc import Callable
from contextlib import AbstractAsyncContextManager
from typing import Any

from ferb.policy.payments import Pressed, digits_of

PRESS_LIMIT_S = 10.0       # the most a press waits for its fresh read; past it the page counts as unread

Hold = Callable[[list[str], str], AbstractAsyncContextManager[None]]


async def read(cap: Any, args: Any, owner: str, declared: str | None, hold: Hold, keys: list[str],
               holder: str) -> Pressed | None:
    """The tab's address and the money amounts on its lines holding `declared`'s last digits, read now; None when the
    call isn't on a browser tab the driver can read this way. A read that fails gives a Pressed with None fields."""
    from ferb.act.browser.ext_bridge import page_amounts
    driver: Any = getattr(cap, "driver", None)
    tab = getattr(args, "tab", None)
    if not tab or not all(callable(getattr(driver, m, None)) for m in ("owned_by", "shows", "find")):
        return None
    needle = digits_of(declared)

    async def look() -> Pressed:
        async with hold(list(keys), holder):
            ref = driver.owned_by(str(tab), owner)
            now = await driver.shows(ref, needle or " ")
            if not now.get("open"):
                return Pressed(None, None)
            amounts: tuple[str, ...] | None = ()
            if needle:
                found = await driver.find(ref, needle, 0, 1_000_000)
                hits = json.loads(found["text"]) if found.get("next_offset") is None else None
                # HAND-484: the latest snapshot's matching elements come first (as old as that snapshot): only the
                # page's text lines, read now, count
                lines = hits[int(found.get("element_matches") or 0):] if hits is not None else None
                amounts = page_amounts([str(h.get("text") or "") for h in lines]) if lines is not None else None
            return Pressed(str(now.get("url") or "") or None, amounts)

    try:
        return await asyncio.wait_for(look(), timeout=PRESS_LIMIT_S)
    except asyncio.CancelledError:
        raise
    except Exception:
        return Pressed(None, None)
