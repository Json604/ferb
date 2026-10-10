"""Owns the code floor (D23) and the sink rule (S4): when a consequential call needs the owner, decided from facts
(the kind the capability derives, its targets' provenance, what the task tree has read, the known destinations).
Must not judge intent or content: a declared purpose may add a need, never remove one (W2); `why` and `effect` are
never read. "Already authorized" (D33): the target is in the owner's own words for this request or in an answer the
owner gave (S3 `owner_words` / `owner_answer`), or a destination FERB already knows (SQ6); payments and
account-security changes ask anyway (SQ1).

- The floor: a payment and an account-security change always ask, and their Yes covers one run; a permanent delete of
  a path the owner didn't name, and a send to a recipient the owner didn't name, ask once per task tree. A send whose
  recipient FERB can't see (a click declared to send or call, with no contact) is a send to someone the owner didn't
  name.
- The sink rule (a): a send-class call to a place with no recipient (a post, a submit) asks when the tree has read the
  owner's own data and outside content and the owner hasn't named that place. With recipients, the floor's
  new-recipient question already covers it.
- The sink rule (b): a recipient or an amount that came only from outside content (never from the owner's words or
  answers) asks, unless the recipient is a destination FERB already knows.
FERB's question (`line`) comes first and is labelled, in the owner's terms (P27): the concrete action, then the call."""

import json
from dataclasses import dataclass, field
from typing import Callable, Iterable

from ferb.core.types import Target
from ferb.policy.destinations import destination_of
from ferb.policy.rules import Kind

ALWAYS = {Kind.PAY: "floor:pay", Kind.ACCOUNT_SECURITY: "floor:account_security"}
SEND_CLASS = frozenset({Kind.SEND, Kind.REPLY, Kind.POST, Kind.MAILBOX_SEND, Kind.SUBMIT, Kind.CALL})
OWNER_SOURCES = frozenset({"owner_words", "owner_answer"})
SHOWN_ORIGINS = 3                         # outside sources named in one question; the rest are counted
Known = Callable[[str, str], bool]


@dataclass(frozen=True)
class Need:
    rule: str
    key: str                                  # what one answer covers in this task tree (`key`)
    values: dict[str, str] = field(default_factory=dict)          # what FERB's question names
    always: bool = False                      # SQ1: the Yes covers one run of the call, never a later one
    destinations: tuple[tuple[str, str], ...] = ()                # (kind, value) a Yes makes known (SQ6)


def key(*parts: object) -> str:
    """A need's key: the rule and the exact values one answer covers, unambiguous (JSON of the tuple)."""
    return json.dumps(parts, ensure_ascii=False)


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


def shown_origin(label: str) -> str:
    """One origin label in the owner's terms: "the web page on kartikey.xyz", "a message, wa:123", "text you pasted"."""
    head, _, rest = label.partition(":")
    if head != "external":
        return label
    kind, _, detail = rest.partition(":")
    if kind == "web":
        if detail == "excluded":
            return "a site on your exclusion list"
        return f"the web page on {detail}" if detail else "a web page"
    if kind == "paste":
        return "text you pasted"
    if kind in ("", "unlabeled"):
        return "content FERB couldn't label"
    if kind == "download":
        return "a downloaded file"
    article = {"message": "a message", "email": "an email"}.get(kind, kind)
    return f"{article}, {detail}" if detail else article


def shown_origins(labels: Iterable[str]) -> str:
    """The outside sources in the owner's terms, the first SHOWN_ORIGINS named and the rest counted."""
    items = list(dict.fromkeys(shown_origin(o) for o in sorted(set(labels))))
    if len(items) <= SHOWN_ORIGINS:
        return "; ".join(items)
    return "; ".join(items[:SHOWN_ORIGINS]) + f"; and {len(items) - SHOWN_ORIGINS} more"


def decide(kind: str, declared: str | None, target: Target, arg_sources: dict[str, list[str]], *,
           read_private: bool, read_external: bool, known: Known, outside_read: Iterable[str] = (),
           place: str | None = None) -> list[Need]:
    """The needs of one call, in the order the owner reads them. `kind` is the physical kind; `declared` the
    purpose-derived one (SEND / CALL), which only widens what is checked. `arg_sources` is provenance.arg_sources of
    the target; `known(kind, value)` says whether a destination is known; `outside_read` is the outside content the
    tree read (origin labels), named in the sink question; `place` is where the call happens as the owner knows it
    (an app's name), else the target's site."""
    kinds = {kind} | ({declared} if declared else set())
    who = _who(target)
    where = place or target.site or ""
    needs: list[Need] = []
    for k in (Kind.PAY, Kind.ACCOUNT_SECURITY):
        if k in kinds:
            rule = ALWAYS[k]
            needs.append(Need(rule, key(rule, target.site, target.account, target.amount, sorted(v for _, v in who)),
                              {"amount": shown_amount(target.amount), "site": where, "account": target.account or "",
                               "recipients": ", ".join(v for _, v in who)}, always=True))
    if Kind.DELETE_PERMANENT in kinds and not _owner(arg_sources.get("path", [])):
        needs.append(Need("floor:delete_permanent", key("floor:delete_permanent", target.path),
                          {"path": target.path or "an item it doesn't name"}))
    sends = kinds & SEND_CLASS
    calls = Kind.CALL in kinds and not sends - {Kind.CALL}
    if sends:
        new = [v for k, v in who if not _owner(arg_sources.get(k, [])) and not known(*destination_of(v))]
        if new:
            needs.append(Need("floor:new_recipient", key("floor:new_recipient", sorted(new)),
                              {"recipients": ", ".join(new), "verb": "calls" if calls else "sends to"},
                              destinations=tuple(destination_of(v) for v in new)))
        elif not who and (kind not in SEND_CLASS or not target.site):
            # a send FERB can't see the recipient of: a click declared to send or call names no contact (its site is
            # an app, never a destination to record), or a send-class call names neither recipient nor place
            needs.append(Need("floor:new_recipient", key("floor:new_recipient", None, target.site),
                              {"recipients": "", "verb": "calls" if calls else "sends to", "place": where}))
        elif (not who and read_private and read_external
              and not _owner(arg_sources.get("site", [])) and not known("host", target.site or "")):
            needs.append(Need("sink:new_destination", key("sink:new_destination", target.site),
                              {"destination": target.site or "", "read": shown_origins(outside_read)},
                              destinations=(("host", target.site or ""),)))
    values = who + ([("amount", str(target.amount))] if target.amount is not None else [])
    for k, value in values:                     # S4 (b): a value that came only from outside content
        outside = [s for s in arg_sources.get(k, []) if s != "unknown"]
        if not outside or _owner(outside):
            continue
        dest = None if k == "amount" else destination_of(value)
        if dest is not None and known(*dest):
            continue
        what = "amount" if k == "amount" else ("recipient" if sends else "typed")
        needs.append(Need("sink:outside_value", key("sink:outside_value", k, value),
                          {"what": what, "value": shown_amount(target.amount) if k == "amount" else value,
                           "origin": shown_origins(outside)},
                          destinations=(dest,) if dest is not None else ()))
    return needs


def _part(n: Need) -> str:
    v = n.values
    if n.rule == "floor:pay":
        return (f"this pays {v['amount'] or 'an amount'}" + (f" to {v['recipients']}" if v["recipients"] else "")
                + (f" on {v['site']}" if v["site"] else "") + (f" from {v['account']}" if v["account"] else ""))
    if n.rule == "floor:account_security":
        return ("this changes account security (a password, sign-in or recovery setting)" +
                (f" on {v['site']}" if v["site"] else "") + (f" for {v['account']}" if v["account"] else ""))
    if n.rule == "floor:delete_permanent":
        return f"this permanently deletes {v['path']} (it skips the Trash)"
    if n.rule == "floor:new_recipient" and not v["recipients"]:
        where = f" in {v['place']}" if v.get("place") else ""
        if v["verb"] == "calls":
            return f"this starts a call{where} with a contact it doesn't name"
        return f"this sends{where} to a contact it doesn't name"
    if n.rule == "floor:new_recipient":
        return f"this {v['verb']} {v['recipients']}, whom you didn't name in this request"
    if n.rule == "sink:new_destination":
        return (f"this sends to {v['destination']}, a place you haven't named, after this task read your own data and "
                f"outside content ({v['read']})")
    if n.rule == "sink:outside_value" and v["what"] == "typed":
        return f"this types {v['value']}, which came from outside content ({v['origin']}), not from you"
    if n.rule == "sink:outside_value":
        return f"the {v['what']} {v['value']} came from outside content ({v['origin']}), not from you"
    return v.get("text", n.rule)                # a rule another part of FERB asks about in its own words


def line(needs: list[Need], call: str, why: str) -> str:
    """FERB's own words, first and labelled (P27: second person, the concrete action), then the call, the reason the
    task gave (quoted) and the question. The gateway collapses whitespace, strips control characters, maps the
    reason's double quotes to single ones and clips what goes in."""
    text = "FERB: " + "; ".join(_part(n) for n in needs) + f". The step: {call}."
    if why:
        text += f' Reason given: "{why}".'
    return text + " Go ahead?"
