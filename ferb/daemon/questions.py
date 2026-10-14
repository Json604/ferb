"""Own pending question transport and exact answers. Must not treat ambiguity or silence as consent.

A question has no lifetime (spec §8.1, L3): it stays pending while its task waits, however long the owner takes, and
closes when it is answered or when its task ends (`forget_request`, called as the task's request is forgotten). An
answer is `taken` once it reached a waiting call; until then a retry of the same call may wait on the same question
(the gateway, when a worker's call dropped)."""

import secrets
from dataclasses import dataclass
from typing import Literal

from ferb.policy.rules import ActionRequest


@dataclass
class Approval:
    id: str
    capability: str
    reason: str
    request_id: str
    kind: str = "owner_question"
    state: Literal["pending", "granted", "denied", "used"] = "pending"
    options: tuple[str, ...] = ()
    choice: str | None = None
    answer: str | None = None
    display_text: str = ""
    taken: bool = False                       # its answer reached the call that waited for it
    key: str = ""                             # what a retry of the same call matches (the asker's own words)

    def option_for(self, text: str) -> int | None:
        """The option (1-based) a typed answer picks: its exact words in any case, else its number."""
        matches = [i for i, o in enumerate(self.options, 1) if o.strip().casefold() == text.casefold()]
        if len(matches) == 1:
            return matches[0]
        return int(text) if text.isdecimal() and 1 <= int(text) <= len(self.options) else None


class Approvals:
    def __init__(self) -> None:
        self._items: dict[str, Approval] = {}
        self.unattended: set[str] = set()

    def question(self, action: ActionRequest, question: str, options: list[str]) -> Approval:
        ap = Approval("ap-" + secrets.token_hex(6), action.capability, question, action.request_id,
                      options=tuple(options))
        self._items[ap.id] = ap
        return ap

    def resolve(self, approval_id: str, granted: bool, choice: int | None = None, text: str | None = None) -> Approval:
        ap = self._items[approval_id]
        if ap.state != "pending":
            raise ValueError(f"question {approval_id} is {ap.state}")
        if ap.kind in {"owner_admin_password", "owner_secret"}:
            if text and any(c in text for c in "\r\n\x00"):
                raise ValueError("the hidden password must be one line")
            text = text or None
        else:
            text = (text or "").strip() or None
        if text is not None and choice is None and ap.options:
            choice = ap.option_for(text)
        if granted and choice is not None:
            if not 1 <= choice <= len(ap.options):
                raise ValueError(f"pick one of 1–{len(ap.options)}")
            ap.choice = ap.options[choice - 1]
        if granted:
            ap.answer = text
        ap.state = "granted" if granted else "denied"
        return ap

    def pending(self) -> list[Approval]:
        return [ap for ap in self._items.values() if ap.state == "pending"]

    def get(self, approval_id: str) -> Approval | None:
        return self._items.get(approval_id)

    def of_request(self, request_id: str) -> list[Approval]:
        """Every question of this request still kept (open, or answered and not yet forgotten), oldest first."""
        return [ap for ap in self._items.values() if ap.request_id == request_id]

    def forget_request(self, request_id: str) -> None:
        self.unattended.discard(request_id)
        for key in [key for key, ap in self._items.items() if ap.request_id == request_id]:
            self._items.pop(key)
