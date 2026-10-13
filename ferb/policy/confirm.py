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

import base64
import binascii
import json
import re
import unicodedata
from dataclasses import dataclass, field
from typing import Callable, Iterable
from urllib.parse import unquote_plus

import idna

from ferb.core.types import Target
from ferb.policy import provenance, urls
from ferb.policy.destinations import destination_of
from ferb.policy.rules import Kind

ALWAYS = {Kind.PAY: "floor:pay", Kind.ACCOUNT_SECURITY: "floor:account_security"}
SEND_CLASS = frozenset({Kind.SEND, Kind.REPLY, Kind.POST, Kind.MAILBOX_SEND, Kind.SUBMIT, Kind.CALL})
OWNER_SOURCES = frozenset({"owner_words", "owner_answer"})
SINK_DESTINATION = "sink:destination"     # the shared rule/key for a host: one Yes/No covers it, shell and browser
SHOWN_ORIGINS = 3                         # outside sources named in one question; the rest are counted
FROM_TASK = "(from the task) "            # a task's text that reads like FERB's own (D33)
TASK_QUESTION = "Question from the task: "
_BUNDLE_LIKE = re.compile(r"[a-z0-9-]+(?:\.[a-z0-9-]+){2,}", re.I)   # an app's id (com.apple.MobileSMS): internal
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
    """One origin label in the owner's terms, without FERB's internal ids: "the web page on kartikey.xyz", "a message,
    wa:123", "an email", "text you pasted", "something FERB remembered"."""
    head, _, rest = label.partition(":")
    if head != "external":
        return label
    kind, _, detail = rest.partition(":")
    if kind == "web":
        if detail == "excluded":
            return "a site on your exclusion list"
        if detail == "trail":
            return "pages you visited"
        return f"the web page on {detail}" if detail else "a web page"
    if kind == "paste":
        return "text you pasted"
    if kind in ("", "unlabeled"):
        return "content FERB couldn't label"
    if kind == "download":
        return "a downloaded file"
    if kind == "memory":
        return "something FERB remembered"
    if kind == "email":
        return "an email"
    if kind == "message":
        return f"a message, {detail}" if detail and not _BUNDLE_LIKE.fullmatch(detail) else "a message"
    return f"content from {kind}"


def reads_as_ferbs(text: str) -> bool:
    """Whether a text starts with "FERB", ignoring case, spaces, punctuation and compatibility forms (fullwidth)."""
    return "".join(c for c in unicodedata.normalize("NFKC", text) if c.isalnum()).casefold().startswith("ferb")


def from_task(text: str) -> str:
    """A task's option (or line) as the owner sees it: only FERB's own checks start with "FERB:" (D33)."""
    return FROM_TASK + text if reads_as_ferbs(text) else text


def task_question(text: str) -> str:
    """A task's question as the owner sees it: always labelled, one paragraph, so no line of it can pass for FERB's
    own check (a newline or a homoglyph doesn't help)."""
    return TASK_QUESTION + " ".join(text.split())


def from_task_lines(text: str) -> str:
    """A planner's chat reply or clarify: each line that starts with "FERB" is marked as the task's."""
    return "\n".join(from_task(line) for line in text.split("\n"))


def shown_origins(labels: Iterable[str]) -> str:
    """The outside sources in the owner's terms, the first SHOWN_ORIGINS named and the rest counted."""
    items = list(dict.fromkeys(shown_origin(o) for o in sorted(set(labels))))
    if len(items) <= SHOWN_ORIGINS:
        return "; ".join(items)
    return "; ".join(items[:SHOWN_ORIGINS]) + f"; and {len(items) - SHOWN_ORIGINS} more"


def destination_key(host: str) -> str:
    """The shared confirmation key for one host (SQ6): a Yes or No to this host covers it for the whole tree, whether
    FERB reached it from a shell command or a browser URL. `host` is already normalized."""
    return key(SINK_DESTINATION, host)


_FINE = re.compile(r'[\s=:"\',;/&?#@<>()\[\]{}|~`!*+]+')   # splits a value out of JSON, .env, query/path/fragment text
_B64 = re.compile(r"[A-Za-z0-9+/_-]{8,}={0,2}")
_HEX = re.compile(r"(?:[0-9a-fA-F]{2}){4,}")
_TRAIL = ".,;:!?)]}>\"'"                                   # trailing punctuation on a URL piece (hunter2xyz. , 998.)
# A small built-in table of two-level public suffixes, so the registrable domain is found (data, never an intent list).
_TWO_LEVEL = frozenset({"co.uk", "org.uk", "gov.uk", "ac.uk", "me.uk", "com.au", "net.au", "org.au", "edu.au",
                        "gov.au", "co.nz", "org.nz", "govt.nz", "co.za", "org.za", "co.in", "net.in", "org.in",
                        "co.jp", "or.jp", "ne.jp", "com.br", "com.cn", "net.cn", "org.cn", "gov.cn", "co.kr",
                        "com.mx", "com.sg", "com.hk", "com.tw", "co.il", "com.tr", "co.id", "com.ua"})


def _fine_tokens(text: str) -> set[str]:
    """A text's fine tokens: split on whitespace and the punctuation that frames a value in JSON, a .env line, a query
    string or a fragment, so an embedded value is a whole token. Lowercased; nothing under MIN_LEN."""
    return {t for t in _FINE.split(text.lower()) if len(t) >= provenance.MIN_LEN}


def _toks(t: provenance.Text) -> set[str]:
    """A read text's tokens for the URL check: the coarse tokens (whole emails, hosts, paths) and the fine tokens
    (values embedded in JSON/.env/query punctuation), combined (N2)."""
    return set(t.tokens) | _fine_tokens(t.folded)


def _decodings(piece: str) -> set[str]:
    """Shape-only decodings of a URL piece that could hide a value: base64 and hex. The decoded text is split the same
    way, so `KEY=value` or a JSON blob yields its values (N2). Never a meaning (rule 6); printable ASCII only."""
    out: set[str] = set()
    for pat, dec_fn in ((_B64, lambda s: base64.b64decode(s + "=" * (-len(s) % 4), altchars=b"-_", validate=False)),
                        (_HEX, bytes.fromhex)):
        if not pat.fullmatch(piece):
            continue
        try:
            dec = dec_fn(piece).decode("ascii")
        except (binascii.Error, ValueError, UnicodeDecodeError):
            continue
        if dec.isprintable():
            out.add(dec.lower())
            out |= _fine_tokens(dec)
    return out


def _value_labels(host: str) -> list[str]:
    """The host labels left of the site's registrable domain — where a value rides in `VALUE.attacker.com` (N3). The
    registrable domain (the label plus its public suffix) is not a value, so `github.com` and `stripe.com` yield
    nothing. A two-level suffix (co.uk, com.au…) is taken from a small built-in table. A punycode label (`xn--`, which
    is how the parser spells `josé-müller77`) is also given decoded, so the value is matched as the owner's data
    spells it (N2)."""
    labels = host.split(".")
    if len(labels) < 3:
        return []
    reg = 3 if ".".join(labels[-2:]) in _TWO_LEVEL else 2
    out: list[str] = []
    for label in labels[:-reg]:
        out.append(label)
        if label.startswith("xn--"):
            try:
                out.append(idna.ulabel(label))
            except idna.IDNAError:
                pass
    return out


def _url_pieces(url: str) -> list[str]:
    """The pieces of a URL a value could ride in: userinfo, the host labels left of the registrable domain, each path
    segment (percent-decoded) with its `;` parameters and file extension stripped, each query name and value, and the
    fragment whole and split. Parsed as the browser will (ferb/policy/urls.py); pieces are `unquote_plus`-decoded."""
    parts = urls.parse(url)
    if parts is None:
        return []
    pieces: list[str] = []
    if parts.username:
        pieces.append(unquote_plus(parts.username))
    if parts.password:
        pieces.append(unquote_plus(parts.password))
    pieces += _value_labels(parts.host)
    for seg in parts.path.split("/"):
        seg = unquote_plus(seg)
        main = seg.split(";", 1)[0]
        pieces += [seg, main]
        if "." in main:
            pieces.append(main.rsplit(".", 1)[0])              # extension stripped: VALUE.png -> VALUE
        for param in seg.split(";")[1:]:
            pieces += [unquote_plus(kv) for kv in param.split("=")]
    for pair in parts.query.split("&"):
        pieces += [unquote_plus(kv) for kv in pair.split("=")]
    frag = unquote_plus(parts.fragment)
    pieces.append(frag)
    for pair in frag.split("&"):                               # #k=VALUE and #VALUE
        pieces += pair.split("=")
    return pieces


def _variants(piece: str) -> set[str]:
    """A URL piece's forms to match: itself and its trailing-punctuation-stripped form, lowercased, plus base64/hex
    decodings (N2)."""
    forms: set[str] = set()
    for base in (piece.strip(), piece.strip().rstrip(_TRAIL)):
        if len(base) >= provenance.MIN_LEN:
            forms.add(base.lower())
            forms |= _decodings(base)
    return forms


def url_carries_private(url: str, private: list[tuple[int, provenance.Text]],
                        outside: list[tuple[int, provenance.Text]], owner: provenance.Text) -> list[str]:
    """The sink rule for navigation (S4, SQ6, carried item 8, Q8, N1): the pieces of `url` that are a token of the
    owner's own data (a private tool result) and are not the owner's typed words and were not already public. `private`
    and `outside` are (span order, prepared text) in read order; a value is public only if it appeared in an
    outside-only result read *before* that value was first read privately (laundering by later echo doesn't count), and
    a record labelled both outside and private counts as private, never making a value public (that split is the
    caller's). A URL seen verbatim in outside-only content read before the private value first appeared is a followed
    link: nothing asked. Shape and provenance only, never a meaning (rule 6)."""
    if not private:
        return []
    first_private: dict[str, int] = {}
    for order, t in private:
        for tok in _toks(t):
            if order < first_private.get(tok, order + 1):
                first_private[tok] = order
    owner_toks = _toks(owner)
    outside_toks = [(order, _toks(t)) for order, t in outside]

    def laundered(tok: str) -> bool:                          # public before it was first read privately
        fp = first_private[tok]
        return any(tok in toks for order, toks in outside_toks if order < fp)

    hits: list[tuple[str, int]] = []
    for piece in _url_pieces(url):
        value = piece.strip()
        # the owner-word exclusion is per matched form, so a typed word inside a decoded blob can't excuse the rest
        matched = {m for m in _variants(piece) & set(first_private) if m not in owner_toks and not laundered(m)}
        if matched and value and value not in [v for v, _ in hits]:
            hits.append((value, min(first_private[m] for m in matched)))
    if not hits:
        return []
    earliest = min(order for _, order in hits)
    seen_url = {url.casefold(), url.replace("\\", "/").casefold()}
    if any(order < earliest and any(u in t.folded for u in seen_url) for order, t in outside):
        return []                                             # a followed link, read before the private value
    return [v for v, _ in hits]


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
    if n.rule == "sink:private_url":
        read = f" ({v['read']})" if v.get("read") else ""
        return (f"this would open {v['destination']}, a place you haven't named, with your own data in the address "
                f"({v['value']}), after this task read outside content{read}; saying yes lets FERB reach "
                f"{v['destination']} from the browser, now and in later tasks")
    if n.rule == "sink:outside_value" and v["what"] == "typed":
        return f"this types {v['value']}, which came from outside content ({v['origin']}), not from you"
    if n.rule == "sink:outside_value":
        return f"the {v['what']} {v['value']} came from outside content ({v['origin']}), not from you"
    return v.get("text", n.rule)                # a rule another part of FERB asks about in its own words


def line(needs: list[Need], call: str, why: str) -> str:
    """FERB's own words, first and labelled (P27: second person, the concrete action), then the call, the reason the
    task gave (quoted) and the question. The gateway collapses whitespace, strips control characters, maps the
    reason's double quotes to single ones and clips what goes in."""
    text = "FERB: " + "; ".join(_part(n) for n in needs) + (f". The step: {call}." if call else ".")
    if why:
        text += f' Reason given: "{why}".'
    return text + " Go ahead?"
