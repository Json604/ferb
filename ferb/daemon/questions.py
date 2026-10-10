"""Own pending question transport and exact answers. Must not treat timeout or ambiguity as consent."""

import secrets
from dataclasses import dataclass
from typing import Literal

from ferb.core.interfaces import Clock
from ferb.policy.rules import ActionRequest

APPROVAL_TTL_S = 600.0


@dataclass
class Approval:
    id: str
    capability: str
    reason: str
    expires_at: float
    request_id: str
    kind: str = "owner_question"
    state: Literal["pending", "granted", "denied", "used", "expired"] = "pending"
    options: tuple[str, ...] = ()
    choice: str | None = None
    answer: str | None = None


class Approvals:
    def __init__(self, clock: Clock, ttl_s: float = APPROVAL_TTL_S) -> None:
        self._clock, self._ttl = clock, ttl_s
        self._items: dict[str, Approval] = {}
        self.unattended: set[str] = set()

    def question(self, action: ActionRequest, question: str, options: list[str]) -> Approval:
        ap = Approval("ap-" + secrets.token_hex(6), action.capability, question,
                      self._clock.monotonic() + self._ttl, action.request_id, options=tuple(options))
        self._items[ap.id] = ap
        return ap

    def resolve(self, approval_id: str, granted: bool, choice: int | None = None, text: str | None = None) -> Approval:
        ap = self._items[approval_id]
        if ap.state != "pending":
            raise ValueError(f"question {approval_id} is {ap.state}")
        if self._clock.monotonic() >= ap.expires_at:
            ap.state = "expired"
            raise ValueError(f"question {approval_id} expired")
        if ap.kind in {"owner_admin_password", "owner_secret"}:
            if text and any(c in text for c in "\r\n\x00"):
                raise ValueError("the hidden password must be one line")
            text = text or None
        else:
            text = (text or "").strip() or None
        if text is not None and choice is None and ap.options:
            matches = [i for i, o in enumerate(ap.options, 1) if o.strip().casefold() == text.casefold()]
            choice = matches[0] if len(matches) == 1 else None
            if choice is None and text.isdigit() and 1 <= int(text) <= len(ap.options):
                choice = int(text)
        if granted and choice is not None:
            if not 1 <= choice <= len(ap.options):
                raise ValueError(f"pick one of 1–{len(ap.options)}")
            ap.choice = ap.options[choice - 1]
        if granted:
            ap.answer = text
        ap.state = "granted" if granted else "denied"
        return ap

    def pending(self) -> list[Approval]:
        now = self._clock.monotonic()
        return [ap for ap in self._items.values() if ap.state == "pending" and now < ap.expires_at]

    def get(self, approval_id: str) -> Approval | None:
        return self._items.get(approval_id)

    def forget_request(self, request_id: str) -> None:
        self.unattended.discard(request_id)
        for key in [key for key, ap in self._items.items() if ap.request_id == request_id]:
            self._items.pop(key)
