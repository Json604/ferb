"""Owns the code floor (D23) and the sink rule (S4): when a consequential call needs the owner, decided from facts
(the kind the capability derives, its targets' provenance, what the task tree has read, the known destinations).
Must not judge intent or content: a declared purpose may add a need, never remove one (W2); `why` and `effect` are
never read. "Already authorized" (D33): the target is in the owner's own words for this request or in an answer the
owner gave (S3 `owner_words` / `owner_answer`), or a destination FERB already knows (SQ6); payments and
account-security changes ask anyway (SQ1).

- The floor: a payment and an account-security change always ask, and their Yes covers one run; a permanent delete of
  a path the owner didn't name, and a send to a recipient the owner didn't name, ask once per task tree.
- The sink rule (a): a send-class call to a place with no recipient (a post, a submit) asks when the tree has read the
  owner's own data and outside content and the owner hasn't named that place. With recipients, the floor's
  new-recipient question already covers it.
- The sink rule (b): a recipient or an amount that came only from outside content (never from the owner's words or
  answers) asks, unless the recipient is a destination FERB already knows.
FERB's question (`line`) comes first and is labelled, in the owner's terms (P27): the concrete action, then the call."""

from dataclasses import dataclass, field
from typing import Callable

from ferb.core.types import Target
from ferb.policy.destinations import destination_of
from ferb.policy.rules import Kind

ALWAYS = {Kind.PAY: "floor:pay", Kind.ACCOUNT_SECURITY: "floor:account_security"}
SEND_CLASS = frozenset({Kind.SEND, Kind.REPLY, Kind.POST, Kind.MAILBOX_SEND, Kind.SUBMIT, Kind.CALL})
OWNER_SOURCES = frozenset({"owner_words", "owner_answer"})
Known = Callable[[str, str], bool]


@dataclass(frozen=True)
class Need:
    rule: str
    key: str                                  # what one answer covers in this task tree
    values: dict[str, str] = field(default_factory=dict)          # what FERB's question names
    always: bool = False                      # SQ1: the Yes covers one run of the call, never a later one
    destinations: tuple[tuple[str, str], ...] = ()                # (kind, value) a Yes makes known (SQ6)


def _who(target: Target) -> list[tuple[str, str]]:
    """(arg_sources key, value) of each recipient and the contact."""
    return [(f"recipient:{i}", r) for i, r in enumerate(target.recipients)] + (
        [("contact", target.contact)] if target.contact else [])


def _owner(sources: list[str]) -> bool:
    return bool(OWNER_SOURCES & set(sources))


def shown_amount(amount: float | None) -> str:
    if amount is None:
        return ""
    return str(int(amount)) if amount == int(amount) else str(amount)


def decide(kind: str, declared: str | None, target: Target, arg_sources: dict[str, list[str]], *,
           read_private: bool, read_external: bool, known: Known, outside_read: str = "") -> list[Need]:
    """The needs of one call, in the order the owner reads them. `kind` is the physical kind; `declared` the
    purpose-derived one (SEND / CALL), which only widens what is checked. `arg_sources` is provenance.arg_sources of
    the target; `known(kind, value)` says whether a destination is known; `outside_read` names the outside content the
    tree read, for the sink question."""
    kinds = {kind} | ({declared} if declared else set())
    who = _who(target)
    needs: list[Need] = []
    for k in (Kind.PAY, Kind.ACCOUNT_SECURITY):
        if k in kinds:
            rule = ALWAYS[k]
            needs.append(Need(rule, "|".join([rule, target.site or "", target.account or "", str(target.amount or ""),
                                              ",".join(sorted(v for _, v in who))]),
                              {"amount": shown_amount(target.amount), "site": target.site or "",
                               "account": target.account or "", "recipients": ", ".join(v for _, v in who)},
                              always=True))
    if Kind.DELETE_PERMANENT in kinds and not _owner(arg_sources.get("path", [])):
        needs.append(Need("floor:delete_permanent", f"floor:delete_permanent|{target.path or ''}",
                          {"path": target.path or "an item it doesn't name"}))
    if kinds & SEND_CLASS:
        new = [v for key, v in who if not _owner(arg_sources.get(key, [])) and not known(*destination_of(v))]
        if new:
            calls = Kind.CALL in kinds and not kinds & (SEND_CLASS - {Kind.CALL})
            needs.append(Need("floor:new_recipient", "floor:new_recipient|" + ",".join(sorted(new)),
                              {"recipients": ", ".join(new), "verb": "calls" if calls else "sends to"},
                              destinations=tuple(destination_of(v) for v in new)))
        elif (not who and target.site and read_private and read_external
              and not _owner(arg_sources.get("site", [])) and not known("host", target.site)):
            needs.append(Need("sink:new_destination", f"sink:new_destination|{target.site}",
                              {"destination": target.site, "read": outside_read},
                              destinations=(("host", target.site),)))
    values = who + ([("amount", str(target.amount))] if target.amount is not None else [])
    for key, value in values:                   # S4 (b): a value that came only from outside content
        outside = [s for s in arg_sources.get(key, []) if s != "unknown"]
        if not outside or _owner(outside):
            continue
        dest = None if key == "amount" else destination_of(value)
        if dest is not None and known(*dest):
            continue
        needs.append(Need("sink:outside_value", f"sink:outside_value|{key}|{value}",
                          {"what": "amount" if key == "amount" else "recipient",
                           "value": shown_amount(target.amount) if key == "amount" else value,
                           "origin": ", ".join(outside)},
                          destinations=(dest,) if dest is not None else ()))
    return needs


def line(needs: list[Need], call: str, why: str) -> str:
    """FERB's own words, first and labelled (P27: second person, the concrete action), then the call and the
    worker's reason. The gateway collapses whitespace and strips control characters from what goes in."""
    parts = []
    for n in needs:
        v = n.values
        if n.rule == "floor:pay":
            parts.append(f"this pays {v['amount'] or 'an amount'}" + (f" to {v['recipients']}" if v["recipients"] else "")
                         + (f" on {v['site']}" if v["site"] else "") + (f" from {v['account']}" if v["account"] else ""))
        elif n.rule == "floor:account_security":
            parts.append("this changes account security (a password, sign-in or recovery setting)" +
                         (f" on {v['site']}" if v["site"] else "") + (f" for {v['account']}" if v["account"] else ""))
        elif n.rule == "floor:delete_permanent":
            parts.append(f"this permanently deletes {v['path']} (not the Trash; FERB keeps no copy)")
        elif n.rule == "floor:new_recipient":
            parts.append(f"this {v['verb']} {v['recipients']}, whom you didn't name in this request")
        elif n.rule == "sink:new_destination":
            parts.append(f"this task has read your own data and outside content ({v['read']}); this step would send to "
                         f"{v['destination']}, a place you haven't named")
        elif n.rule == "sink:outside_value":
            parts.append(f"the {v['what']} {v['value']} comes from outside content ({v['origin']}), not from you")
        else:                                   # a rule another part of FERB asks about with its own words
            parts.append(v.get("text", n.rule))
    text = "FERB: " + "; ".join(parts) + f". The step: {call}."
    if why:
        text += f" What the worker says: {why}"
    return text + " Go ahead?"
