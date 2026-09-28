"""Approvals and undo windows (section 10.2/10.3).

Approvals: single-use, expire in minutes, bound to the exact action (account, target, payload hash) — any change
voids them. Undo windows: sends/posts are held (~10 s) before executing; `ferb undo` cancels.
"""

import secrets
from dataclasses import dataclass
from typing import Literal

from ferb.core.interfaces import Clock
from ferb.policy.rules import ActionRequest, payload_hash

APPROVAL_TTL_S = 300.0
UNDO_WINDOW_S = 10.0


@dataclass
class Approval:
    id: str
    payload_hash: str
    capability: str
    reason: str
    expires_at: float
    state: Literal["pending", "granted", "denied", "used", "expired"] = "pending"


class Approvals:
    def __init__(self, clock: Clock, ttl_s: float = APPROVAL_TTL_S) -> None:
        self._clock, self._ttl = clock, ttl_s
        self._items: dict[str, Approval] = {}

    def request(self, a: ActionRequest, reason: str) -> Approval:
        ap = Approval("ap-" + secrets.token_hex(6), payload_hash(a), a.capability, reason,
                      self._clock.monotonic() + self._ttl)
        self._items[ap.id] = ap
        return ap

    def resolve(self, approval_id: str, granted: bool) -> Approval:
        ap = self._items[approval_id]
        if ap.state != "pending":
            raise ValueError(f"approval {approval_id} is {ap.state}")
        if self._clock.monotonic() >= ap.expires_at:
            ap.state = "expired"
            raise ValueError(f"approval {approval_id} expired")
        ap.state = "granted" if granted else "denied"
        return ap

    def consume(self, a: ActionRequest) -> Approval | None:
        """A granted, unexpired, unused approval for exactly this action; marks it used (single-use)."""
        h, now = payload_hash(a), self._clock.monotonic()
        for ap in self._items.values():
            if ap.state == "granted" and ap.payload_hash == h:
                if now >= ap.expires_at:
                    ap.state = "expired"
                    continue
                ap.state = "used"
                return ap
        return None

    def get(self, approval_id: str) -> Approval | None:
        return self._items.get(approval_id)


@dataclass
class Hold:
    id: str
    payload_hash: str
    action: ActionRequest
    release_at: float                      # Clock.monotonic()
    state: Literal["held", "released", "undone"] = "held"


class Holds:
    """Undo windows. The daemon releases due holds; `ferb undo` (or `ferb stop`) cancels them."""

    def __init__(self, clock: Clock, window_s: float = UNDO_WINDOW_S) -> None:
        self._clock, self.window_s = clock, window_s
        self._items: dict[str, Hold] = {}

    def hold(self, a: ActionRequest) -> Hold:
        h = Hold("hold-" + secrets.token_hex(6), payload_hash(a), a, self._clock.monotonic() + self.window_s)
        self._items[h.id] = h
        return h

    def undo(self, hold_id: str) -> bool:
        h = self._items.get(hold_id)
        if h is None or h.state != "held":
            return False
        h.state = "undone"
        return True

    def undo_all(self) -> list[Hold]:
        undone = [h for h in self._items.values() if h.state == "held"]
        for h in undone:
            h.state = "undone"
        return undone

    def due(self) -> list[Hold]:
        """Holds whose window has passed; marks them released (each is returned once)."""
        now = self._clock.monotonic()
        out = [h for h in self._items.values() if h.state == "held" and now >= h.release_at]
        for h in out:
            h.state = "released"
        return out

    def pending(self) -> list[Hold]:
        return [h for h in self._items.values() if h.state == "held"]

    def get(self, hold_id: str) -> Hold | None:
        return self._items.get(hold_id)
