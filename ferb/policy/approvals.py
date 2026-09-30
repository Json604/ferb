"""Approvals and undo windows (section 10.2/10.3).

Approvals (10.3, build step 14a): one approval covers the same target and action type for the rest of the request
(its scope key: request, kind, target — not the content); a different target is a new question. Irreversible actions
(the floor, or flagged by the model / Jev) are the exception: their approval is bound to the exact action (payload
hash) and used once. Undo windows: sends/posts are held (~10 s) before executing; `ferb undo` cancels.
"""

import secrets
from dataclasses import dataclass
from typing import Literal

from ferb.core.interfaces import Clock
from ferb.policy.rules import ActionRequest, irreversible, payload_hash, scope_key

APPROVAL_TTL_S = 600.0                     # an unanswered question expires after 10 minutes
GRANT_TTL_S = 4 * 3600.0                   # a scoped approval lasts as long as a task can
UNATTENDED_TTL_S = 3600.0                  # a scheduled run's question: notify, wait an hour, then skip the step
UNDO_WINDOW_S = 10.0


@dataclass
class Approval:
    id: str
    payload_hash: str
    capability: str
    reason: str
    expires_at: float
    state: Literal["pending", "granted", "denied", "used", "expired"] = "pending"
    scope: str = ""                        # scope key: what a grant covers (empty for irreversible: exact, once)
    request_id: str = ""
    kind: str = ""


class Approvals:
    def __init__(self, clock: Clock, ttl_s: float = APPROVAL_TTL_S) -> None:
        self._clock, self._ttl = clock, ttl_s
        self._items: dict[str, Approval] = {}
        self.unattended: set[str] = set()     # request ids of scheduled runs (the owner isn't at chat)

    def request(self, a: ActionRequest, reason: str) -> Approval:
        ap = Approval("ap-" + secrets.token_hex(6), payload_hash(a), a.capability, reason,
                      self._clock.monotonic() + (UNATTENDED_TTL_S if a.request_id in self.unattended else self._ttl),
                      scope="" if irreversible(a) else scope_key(a),
                      request_id=a.request_id, kind=a.kind)
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
        if granted and ap.scope:
            ap.expires_at = self._clock.monotonic() + GRANT_TTL_S
        return ap

    def consume(self, a: ActionRequest) -> Approval | None:
        """An irreversible action: a granted approval for exactly this action, used once. Anything else: a granted
        approval with the same scope (request, kind, target), which keeps covering later calls."""
        now = self._clock.monotonic()
        exact = irreversible(a)
        h, sk = payload_hash(a), scope_key(a)
        for ap in self._items.values():
            if ap.state != "granted":
                continue
            if now >= ap.expires_at:
                ap.state = "expired"
                continue
            if exact and not ap.scope and ap.payload_hash == h:
                ap.state = "used"
                return ap
            if not exact and ap.scope == sk:
                return ap                                                  # stays granted for this request
        return None

    def pending(self) -> list[Approval]:
        now = self._clock.monotonic()
        return [ap for ap in self._items.values() if ap.state == "pending" and now < ap.expires_at]

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
