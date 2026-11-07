"""Own pending question transport and exact answers. Must not treat ambiguity or silence as consent.

A question has no lifetime (spec §8.1, L3): it stays pending while its task waits, however long the owner takes, and
closes when it is answered or when its task ends (`forget_request`, called as the task's request is forgotten). An
answer is `taken` once it reached a waiting call; until then a retry of the same call may wait on the same question
(the gateway, when a worker's call dropped)."""

import secrets
from dataclasses import dataclass
from typing import Any, Literal

from ferb.policy.rules import ActionRequest

# FERB's own consequential checks, by capability (steering.FERB_CHECKS names the same six): a confirmation, a P19
# retry, a delegated conversation's scope, a forget, the downloads cap and which profile FERB browses in (APP-777). Like FERB's check-ins (`options_only`), only
# an option's own words answer them (GATE-510, ORCH-536, ORCH-537), never its number: a "1" typed in a safety check's
# box or in chat is words, not a Yes.
OWN_CHECKS = frozenset({"ferb.confirm", "ferb.retry", "conversation.approve", "memory.forget", "downloads.delete",
                        "browsing.choose"})


@dataclass
class Approval:
    id: str
    capability: str
    reason: str
    request_id: str
    kind: str = "owner_question"
    state: Literal["pending", "granted", "denied", "used", "superseded"] = "pending"
    options: tuple[str, ...] = ()
    choice: str | None = None
    answer: str | None = None
    summary: str = ""                         # FERB's own question in one line, written by code (P27, D33, GATE-225)
    from_task: bool = False                   # a task's (model-written) question: the app tags it (D33, GATE-225)
    why: str = ""                             # the task's reason, shown smaller under the question (2026-10-27)
    detail: str = ""                          # GATE-513: why FERB asks, one plain line under the question
    exact: str = ""                           # GATE-513: the exact thing approved (command, address, value), boxed
    taken: bool = False                       # its answer reached the call that waited for it
    key: str = ""                             # what a retry of the same call matches (the asker's own words)
    task_id: str = ""                         # the task that asked: only its own retry may wait on it (rule 5)
    about: tuple[str, ...] = ()               # P19: the attempts this retry question named, as asked
    part: str = ""                            # the part of a multi-part request FERB's own question is about (8d)
    runtime_switch: dict[str, Any] | None = None   # the offer of another AI service it carries (ORCH-315)
    hidden: tuple[str, ...] = ()              # the kinds of secret FERB hid in the typed answer, by name (task 11)
    # Batch 3b (ORCH-536, GATE-510): how the card answers it, set by whoever asks it, from structure only.
    # `options_only`: FERB's own question about whether the task goes on (a check-in, a paused task's "Resume?", a
    # closed tab's "continue?", a runtime offer): only one of its options answers it, never typed words.
    # `corrects`: FERB's safety check that belongs to a task (a confirmation, a retry, a conversation's scope): its card
    # also takes the owner's words for that task as a correction (`chat.correct`), never as an answer.
    options_only: bool = False
    corrects: bool = False

    def option_for(self, text: str) -> int | None:
        """The option (1-based) a typed answer picks: its exact words in any case, else, for a task's own question, its
        number. FERB's own checks and check-ins take only an option's words (`OWN_CHECKS`)."""
        matches = [i for i, o in enumerate(self.options, 1) if o.strip().casefold() == text.casefold()]
        if len(matches) == 1:
            return matches[0]
        if self.options_only or self.capability in OWN_CHECKS:
            return None
        return int(text) if text.isdecimal() and 1 <= int(text) <= len(self.options) else None


def card_parts(ap: Approval) -> dict[str, Any]:
    """GATE-513: the card's why line and exact part, when it has them (`approval.requested` and `chat.status`)."""
    return ({"detail": ap.detail} if ap.detail else {}) | ({"exact": ap.exact} if ap.exact else {})


def card_flags(ap: Approval) -> dict[str, Any]:
    """How the app's card answers this question (`chat.status` and `approval.requested` carry the same flags)."""
    return ({"options_only": True} if ap.options_only else {}) | ({"corrects": True} if ap.corrects else {})


class Approvals:
    def __init__(self) -> None:
        self._items: dict[str, Approval] = {}
        self.unattended: set[str] = set()

    def question(self, action: ActionRequest, question: str, options: list[str], ident: str | None = None) -> Approval:
        """`ident`: the id of a question asked before a restart and asked again now (it keeps its id)."""
        ap = Approval(ident or "ap-" + secrets.token_hex(6), action.capability, question, action.request_id,
                      options=tuple(options), task_id=action.task_id)
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

    def supersede(self, approval_id: str) -> Approval | None:
        """Close a pending question nobody can use any more: the owner changed the task while it was asked, or its
        waiting call was cancelled (L3). Superseded is not a No and grants nothing. The question, else None when it
        isn't pending."""
        ap = self._items.get(approval_id)
        if ap is None or ap.state != "pending":
            return None
        ap.state = "superseded"
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
