"""Owns the code floor (D23) and the sink rule (S4): when a consequential call needs the owner, decided from facts
(the kind the capability derives, its targets' provenance, what the task tree has read, the known destinations).
Must not judge intent or content: a declared purpose may add a need, never remove one (W2); `why` is never read, and
`effect` only as the kind the gateway declares from it for a generic input (ferb/policy/rules.py `declare`). "Already
authorized" (D33): the target is in the owner's own words for this request or in an answer the owner gave (S3
`owner_words` / `owner_answer`), or a destination FERB already knows (SQ6); payments and account-security changes ask
anyway (SQ1).

- The floor: a payment and an account-security change always ask, and their Yes covers one run; a permanent delete of
  a path the owner didn't name, and a send to a recipient the owner didn't name, ask once per task tree. A send whose
  recipient FERB can't see (a click declared to send or call, with no contact or recipients: a post) is a send to
  someone the owner didn't name, asked once per tree for its place and the account it declared it sends as, unless
  the owner named that site (and that account, when one is declared) in this request's words or answers. An input
  declared to delete is asked about once per page (or place) and declared item.
- The sink rule (a): a send-class call to a place with no recipient (a post, a submit, a page input declared to send)
  asks when the tree has read the owner's own data and outside content and the owner hasn't named that place. With
  recipients, the floor's new-recipient question already covers it.
- The sink rule (b): a recipient or an amount that came only from outside content (never from the owner's words or
  answers) asks, unless the recipient is a destination FERB already knows.
- The sink rule (c): irreversible bulk work (a send to two or more recipients, a permanent delete of a folder) asks
  once, shown concretely: the count, the total size and a sample. Recipients and paths in the owner's words are
  already authorized and don't count. The sample's size (SHOWN) is display only.
FERB's question (`line`) comes first and is labelled, in the owner's terms (P27): the concrete action, then the call."""

import base64
import binascii
import json
import re
import unicodedata
from dataclasses import dataclass, field
from typing import Any, Callable, Iterable
from urllib.parse import unquote_plus

import idna

from ferb.core.types import Target
from ferb.policy import provenance, urls
from ferb.policy.destinations import destination_of
from ferb.policy.rules import Kind

ALWAYS = {Kind.PAY: "floor:pay", Kind.ACCOUNT_SECURITY: "floor:account_security"}
# What an account-security change is, as FERB's question names it (the worker prompt's and the effect help's words)
ACCOUNT_SECURITY = "a password, two-factor or recovery setting, or deleting the account"
SEND_CLASS = frozenset({Kind.SEND, Kind.REPLY, Kind.POST, Kind.MAILBOX_SEND, Kind.SUBMIT, Kind.CALL})
OWNER_SOURCES = frozenset({"owner_words", "owner_answer"})
SINK_DESTINATION = "sink:destination"     # the shared rule/key for a host: one Yes/No covers it, shell and browser
BULK = "sink:irreversible_bulk"
TERMINAL_INPUT = "floor:terminal_input"   # D31, rule 10: any input into a terminal, asked every time (policy/terminal.py)
SHOWN = 3                                 # display only: items one question names (outside sources, recipients,
                                          # paths); the rest are counted. It decides nothing.
QUOTED_CHARS = 200                        # display only: a page or item quoted in FERB's question is clipped to
                                          # this, like the task's reason; it decides nothing
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


def site_is_destination(kinds: Iterable[str], keys: Iterable[str]) -> bool:
    """Whether a call's site is where it sends, not only where it happens: a send-class call (its kind, or a kind a
    declared purpose adds) whose target has no recipient or contact (`keys`: its arg_sources keys, as `_who` names
    them). A post, a submit, a page input declared to send: `decide` asks about that place (the floor for a recipient
    FERB can't see, the sink rule (a)), and the learning card reader judges where the site came from (L29, SQ7)."""
    return bool(SEND_CLASS & set(kinds)) and not any(k == "contact" or k.startswith("recipient:") for k in keys)


def _owner(sources: list[str]) -> bool:
    return bool(OWNER_SOURCES & set(sources))


def owner_named(sources: list[str]) -> bool:
    """Whether an argument appears in the owner's words or answers (provenance.arg_sources): already authorized."""
    return _owner(sources)


def shown_quoted(text: str) -> str:
    """A page or item from the task inside FERB's labelled sentence (D33): one line, clipped, quoted as data."""
    one = " ".join(text.split())
    return json.dumps(one if len(one) <= QUOTED_CHARS else one[:QUOTED_CHARS - 1] + "…", ensure_ascii=False)


def sample(items: Iterable[str]) -> str:
    """Up to SHOWN items named, each quoted (a name from a disk or a page is data inside FERB's labelled sentence,
    D33), and the rest counted ('"a", "b", "c" and 2 more'). Display only."""
    listed = [json.dumps(i, ensure_ascii=False) for i in items]
    if len(listed) <= SHOWN:
        return ", ".join(listed)
    return ", ".join(listed[:SHOWN]) + f" and {len(listed) - SHOWN} more"


def _counted(n: int, thing: str) -> str:
    return f"{n} {thing}" + ("" if n == 1 else "s")


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
    """The outside sources in the owner's terms, the first SHOWN named and the rest counted."""
    items = list(dict.fromkeys(shown_origin(o) for o in sorted(set(labels))))
    if len(items) <= SHOWN:
        return "; ".join(items)
    return "; ".join(items[:SHOWN]) + f"; and {len(items) - SHOWN} more"


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


_URL_IN_TEXT = re.compile(r"[a-z][a-z0-9+.-]*://[^\s\"'<>\[\]{}|\\^`]+")   # a URL's extent inside read text (shape)


def _host_had_it(seen: str, dest: str) -> bool:
    """Whether a value in a URL of host `seen` was already known to `dest`: the same host (a leading "www." aside), or
    a host under it (`help.instagram.com` under `instagram.com`). Never a sibling: on shared-suffix platforms
    (`*.vercel.app`, `*.github.io`, `*.netlify.app`…) each owner has their own subdomain, so a registrable-domain
    match would hand one owner's token to another's (codebase review, note on 207b2df). Hosts are normalized
    (ferb/policy/urls.py: IDNA, lowercase)."""
    seen, dest = seen.rstrip("."), dest.rstrip(".")
    bare = seen.removeprefix("www.")
    return dest in (seen, bare, "www." + bare) or dest.endswith("." + seen)


def _same_site_only(private: list[tuple[int, provenance.Text]], dest: str) -> set[str]:
    """The tokens of the owner's own data whose every occurrence is inside a URL whose host `dest` covers
    (`_host_had_it`): a value that host already had in its own address, such as a reel's id in the owner's tab of that
    reel. A token also found anywhere else in private text (another host's URL, plain text) isn't one. Shape only
    (rule 6)."""
    inside: set[str] = set()
    elsewhere: set[str] = set()
    for _, t in private:
        rest, cut = [], 0
        for m in _URL_IN_TEXT.finditer(t.folded):
            parsed = urls.parse(m.group(0).rstrip(_TRAIL))
            if parsed is not None and parsed.host and _host_had_it(parsed.host, dest):
                inside |= set(provenance.tokens(m.group(0))) | _fine_tokens(m.group(0))
                rest.append(t.folded[cut:m.start()])
                cut = m.end()
        rest.append(t.folded[cut:])
        elsewhere |= set(provenance.tokens(" ".join(rest))) | _fine_tokens(" ".join(rest))
    return inside - elsewhere


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


_WORD_JOINERS = frozenset(".-_'\u2019")                   # inside words, names and file names: index.html, about-us


def _is_value(form: str) -> bool:
    """Whether a URL piece's form is a value rather than an ordinary word (L4): it holds a digit or a character other
    than a letter and the joiners inside words, names and file names — an email's @, a path's /, a number, a code. A
    word ("search", "playlist", "about-us", "index.html", "entry_id") is navigation and wrapper vocabulary, never the
    owner's private value. Shape only, never a meaning (rule 6)."""
    return any(not (c.isalpha() or c in _WORD_JOINERS or unicodedata.category(c).startswith("M")) for c in form)


def _variants(piece: str) -> tuple[set[str], set[str]]:
    """A URL piece's forms to match: (its values as written: itself and its trailing-punctuation-stripped form,
    lowercased, when they are values, not words; its base64/hex decodings (N2), words included, since hiding a word
    in an encoding is how data is smuggled, scenario #2)."""
    shown: set[str] = set()
    decoded: set[str] = set()
    for base in (piece.strip(), piece.strip().rstrip(_TRAIL)):
        if len(base) >= provenance.MIN_LEN:
            if _is_value(base.lower()):
                shown.add(base.lower())
            decoded |= _decodings(base)
    return shown, decoded


def url_carries_private(url: str, private: list[tuple[int, provenance.Text]],
                        outside: list[tuple[int, provenance.Text]], owner: provenance.Text) -> list[str]:
    """The sink rule for navigation (S4, SQ6, carried item 8, Q8, N1, L4): the pieces of `url` that are a token of the
    owner's own data (a private tool result) and are not the owner's typed words and were not already public. Only a
    value counts, never an ordinary word (`_is_value`), unless it is hidden in base64/hex: the words of FERB's context
    wrapper and of the pages the owner reads ("search", "history", "about-us") are not the owner's data. `private`
    and `outside` are (span order, prepared text) in read order; a value is public only if it appeared in an
    outside-only result read *before* that value was first read privately (laundering by later echo doesn't count), and
    a record labelled both outside and private counts as private, never making a value public (that split is the
    caller's). A URL seen verbatim in outside-only content read before the private value first appeared is a followed
    link: nothing asked. A value whose every private occurrence is inside a URL of the destination's own host (or a host
    it sits under) discloses nothing new to that host, so it doesn't count (L4, owner live use 2026-10-14: a
    reel's id from the owner's own tab of that reel). Shape and provenance only, never a meaning (rule 6)."""
    if not private:
        return []
    dest = urls.parse(url)
    known_to_site = _same_site_only(private, dest.host) if dest is not None and dest.host else set()
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
        shown, decoded = _variants(piece)
        matched = {m for m in (shown | decoded) & set(first_private)
                   if m not in owner_toks and m not in known_to_site and not laundered(m)}
        if matched and value and value not in [v for v, _ in hits]:
            hits.append((value, min(first_private[m] for m in matched)))
    if not hits:
        return []
    earliest = min(order for _, order in hits)
    seen_url = {url.casefold(), url.replace("\\", "/").casefold()}
    if any(order < earliest and any(u in t.folded for u in seen_url) for order, t in outside):
        return []                                             # a followed link, read before the private value
    return [v for v, _ in hits]


_ASCII_LOWER = str.maketrans("ABCDEFGHIJKLMNOPQRSTUVWXYZ", "abcdefghijklmnopqrstuvwxyz")


def account_key(value: str | None) -> str:
    """An account or recipient as declarations and scopes are compared: spaces collapsed, ASCII letters lowercased
    (no Unicode case folding: straße is not strasse), one leading "@" dropped."""
    return " ".join(str(value or "").split()).translate(_ASCII_LOWER).removeprefix("@")


def site_key(value: str | None) -> str:
    """A site as the browser names it: parsed as a URL host (IDNA, lowercase, ferb/policy/urls.py), so a Unicode name
    is the punycode host a tab shows; a trailing dot and one leading "www." dropped. Never another host's subdomain."""
    text = str(value or "").strip()
    parsed = urls.parse("https://" + text) if text else None
    host = (parsed.host if parsed is not None and parsed.host else text.lower()).rstrip(".")
    return host[4:] if host.startswith("www.") else host


def decide(kind: str, declared: str | Iterable[str] | None, target: Target, arg_sources: dict[str, list[str]], *,
           read_private: bool, read_external: bool, known: Known, outside_read: Iterable[str] = (),
           place: str | None = None, content: str | None = None) -> list[Need]:
    """The needs of one call, in the order the owner reads them. `kind` is the physical kind; `declared` every kind a
    declared purpose or effect adds (SEND / CALL / PAY / ACCOUNT_SECURITY / DELETE_PERMANENT; one or several), which
    only widens what is checked. `arg_sources` is provenance.arg_sources of the target; `known(kind, value)` says
    whether a destination is known; `outside_read` is the outside content the tree read (origin labels), named in the
    sink question; `place` is where the call happens as the owner knows it (an app's name), else the target's site;
    `content` is what a send carries, sized in the bulk question. A permanent delete of a folder (S4(c)) needs the
    folder counted: the gateway adds it (`bulk_delete`)."""
    kinds = {kind} | ({declared} if isinstance(declared, str) else set(declared or ()))
    who = _who(target)
    to_site = site_is_destination(kinds, (k for k, _ in who))      # inside `if sends` below: exactly `not who`
    where = place or target.site or ""
    needs: list[Need] = []
    for k in (Kind.PAY, Kind.ACCOUNT_SECURITY):
        if k in kinds:
            rule = ALWAYS[k]
            needs.append(Need(rule, key(rule, target.site, target.account, target.amount, sorted(v for _, v in who)),
                              {"amount": shown_amount(target.amount), "site": where, "account": target.account or "",
                               "recipients": ", ".join(v for _, v in who)}, always=True))
    if Kind.DELETE_PERMANENT in kinds and not _owner(arg_sources.get("path", [])):
        if target.path:
            needs.append(Need("floor:delete_permanent", key("floor:delete_permanent", target.path),
                              {"path": target.path}))
        else:   # an input declared to destroy: the page its tab shows (a fact), else its place, and the declared item;
            # a declared one that names no item is a one-run Yes (SQ1): it never covers another unnamed delete there
            on = target.page or where
            one_run = kind != Kind.DELETE_PERMANENT and not target.item
            needs.append(Need("floor:delete_permanent", key("floor:delete_permanent", None, on or None, target.item),
                              {"path": shown_quoted(target.item) if target.item else "an item it doesn't name",
                               "place": (shown_quoted(on) if target.page else on)}, always=one_run))
    sends = kinds & SEND_CLASS
    calls = Kind.CALL in kinds and not sends - {Kind.CALL}
    if sends:
        new = [v for k, v in who if not _owner(arg_sources.get(k, [])) and not known(*destination_of(v))]
        if new:
            needs.append(Need("floor:new_recipient", key("floor:new_recipient", sorted(new)),
                              {"recipients": ", ".join(new), "verb": "calls" if calls else "sends to"},
                              destinations=tuple(destination_of(v) for v in new)))
        elif to_site and (kind not in SEND_CLASS or not target.site):
            # a send FERB can't see the recipient of: a click declared to send or call names no contact (its site is
            # an app or a page's host, never a destination to record), or a send-class call names neither recipient
            # nor place. One Yes per tree covers that place, and the account the click declared it sends as. A site
            # the owner named in this request's words or answers, from an account they named if one is declared, is
            # already authorized (D33): a form they asked to submit, a post to the site they named. A site FERB merely
            # knows is not: it stays known for good (SQ6), long after the request that named it.
            site_named = bool(target.site) and _owner(arg_sources.get("site", []))
            if not (site_named and (not target.account or _owner(arg_sources.get("account", [])))):
                account = (account_key(target.account),) if target.account else ()
                needs.append(Need("floor:new_recipient", key("floor:new_recipient", None, site_key(target.site),
                                                             *account),
                                  {"recipients": "", "verb": "calls" if calls else "sends to", "place": where,
                                   "account": target.account or ""}))
            if (target.page and target.site and read_private and read_external and not site_named
                    and not known("host", target.site)):    # S4 (a) for a post on a page, as for a send-class call
                needs.append(Need("sink:new_destination", key("sink:new_destination", target.site),
                                  {"destination": target.site, "read": shown_origins(outside_read)},
                                  destinations=(("host", target.site),)))
        elif (to_site and read_private and read_external
              and not _owner(arg_sources.get("site", [])) and not known("host", target.site or "")):
            needs.append(Need("sink:new_destination", key("sink:new_destination", target.site),
                              {"destination": target.site or "", "read": shown_origins(outside_read)},
                              destinations=(("host", target.site or ""),)))
        # S4 (c): a send to two or more recipients the owner didn't name, one question with count and size; the
        # new-recipient part beside it names every new one (L3: a Yes makes each known, SQ6), else a sample
        unnamed = sorted({v for k, v in who if k.startswith("recipient:") and not _owner(arg_sources.get(k, []))})
        if len(unnamed) > 1:
            needs.append(Need(BULK, key(BULK, unnamed, content),
                              {"what": "call" if calls else "send", "count": str(len(unnamed)),
                               "bytes": _counted(len((content or "").encode()), "byte"), "sample": sample(unnamed)}))
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


def bulk_delete(path: str, facts: dict[str, Any]) -> Need:
    """S4 (c): a permanent delete of a folder the owner didn't name (fs.usage `facts`, counted with details), one
    question: the folder, how many files and folders it holds, their total size and a sample of what is inside. One
    answer covers exactly the folder as counted."""
    listed = [str(p) for p in facts.get("paths", [])]
    root = listed[0] if listed and listed[0].endswith("/") else ""
    inside = [p[len(root):] if root and p.startswith(root) else p for p in listed[1:]]
    counted = str(facts.get("path") or path)
    return Need(BULK, key(BULK, facts),
                {"what": "delete", "path": path, "counted": "" if counted == path else counted,
                 "files": _counted(int(facts["files"]), "file"),
                 "folders": _counted(int(facts["directories"]), "folder"),
                 "bytes": _counted(int(facts["bytes"]), "byte"), "sample": sample(inside),
                 "covers": "floor:delete_permanent"})


def _terminal(v: dict[str, str]) -> str:
    """A terminal input as what FERB would do, from the input's facts (D31): the exact text, keys or clipboard text,
    where, and whether the Yes also covers its Return."""
    where = f"the terminal in {v['app']}"
    if v.get("paste"):
        what = ("the clipboard's item, which an app marked secret (FERB doesn't show it)" if v.get("concealed") else
                f"the clipboard's text {v['text']}")
        said = f"paste {what} into {where}"
    elif v.get("text"):
        said = f"type {v['text']} into {where}"
    elif v.get("keys") in ("return", "enter"):
        return f"press Return in {where} (it runs what is on its command line, which FERB didn't type there)"
    else:
        return f"press {v['keys']} in {where}"
    return said + (", then press Return there" if v.get("enter") else "") + " (it runs as you, outside FERB's sandbox)"


def _part(n: Need, alongside: frozenset[str] = frozenset()) -> str:
    """One need in the owner's terms; `alongside` holds the rules of the other needs in the same question."""
    v = n.values
    if n.rule == TERMINAL_INPUT:
        return (f"this would {_terminal(v)}; FERB counts it a terminal because it is {v['surface']}. FERB asks "
                "before every input into a terminal, and this Yes covers this one input only")
    if n.rule == "floor:pay":
        return (f"this pays {v['amount'] or 'an amount'}" + (f" to {v['recipients']}" if v["recipients"] else "")
                + (f" on {v['site']}" if v["site"] else "") + (f" from {v['account']}" if v["account"] else ""))
    if n.rule == "floor:account_security":
        return (f"this changes account security ({ACCOUNT_SECURITY})" +
                (f" on {v['site']}" if v["site"] else "") + (f" for {v['account']}" if v["account"] else ""))
    if n.rule == "floor:delete_permanent" and v.get("place"):
        return f"this permanently deletes {v['path']} on {v['place']} (it can't be undone)"
    if n.rule == "floor:delete_permanent":
        return f"this permanently deletes {v['path']} (it skips the Trash)"
    if n.rule == "floor:new_recipient" and not v["recipients"]:
        where = (f" in {v['place']}" if v.get("place") else "") + (f" as {v['account']}" if v.get("account") else "")
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
    if n.rule == BULK and v.get("what") == "delete":
        counted = f"that is {v['counted']}; " if v.get("counted") else ""
        return (f"this permanently deletes {v['path']} ({counted}it skips the Trash): {v['files']} in {v['folders']}, "
                f"{v['bytes']}" + (f", for example {v['sample']}" if v["sample"] else ""))
    if n.rule == BULK and "floor:new_recipient" in alongside:      # the new recipients are named beside it, each one
        return (f"that is {v['count']} recipients at once" if v.get("what") == "call" else
                f"that is {v['bytes']} to {v['count']} recipients at once")
    if n.rule == BULK and v.get("what") == "call":
        return f"this calls {v['count']} recipients you didn't name in this request at once: {v['sample']}"
    if n.rule == BULK and v.get("what") == "send":
        return (f"this sends {v['bytes']} to {v['count']} recipients you didn't name in this request at once: "
                f"{v['sample']}")
    return v.get("text", n.rule)                # a rule another part of FERB asks about in its own words


_LIFTED = {   # what an escape lifts, in the owner's terms (D31): the protection list's kinds (protection.rules)
    "read": "read {path} once, with one command (normally off-limits: a private store FERB's shell never reads)",
    "write": "change {path} once, with one command (normally off-limits: FERB's shell never writes there)",
    "admin": ("run one command as administrator (sudo), with your Mac password (normally off-limits: FERB's shell "
              "never uses admin rights)"),
    "service": ("let one command use macOS's certificate service, trustd (normally off: it reaches certificate "
                "addresses outside FERB's proxy)"),
}


def _short(n: Need, alongside: frozenset[str]) -> str | None:
    """One need as what FERB wants to do, a verb phrase from the need's facts only (never the step or the task's
    reason); None when another need of the same question already says it."""
    v = n.values
    if n.rule == TERMINAL_INPUT:
        return _terminal(v)
    if n.rule == "floor:pay":
        return (f"pay {v['amount'] or 'an amount'}" + (f" to {v['recipients']}" if v["recipients"] else "")
                + (f" on {v['site']}" if v["site"] else "") + (f" from {v['account']}" if v["account"] else ""))
    if n.rule == "floor:account_security":
        return (f"change account security ({ACCOUNT_SECURITY})" + (f" on {v['site']}" if v["site"] else "")
                + (f" for {v['account']}" if v["account"] else ""))
    if n.rule == "floor:delete_permanent" and v.get("place"):
        return f"permanently delete {v['path']} on {v['place']}"
    if n.rule == "floor:delete_permanent":
        return f"permanently delete {v['path']}, skipping the Trash"
    if n.rule == "floor:new_recipient" and not v["recipients"]:
        where = (f" in {v['place']}" if v.get("place") else "") + (f" as {v['account']}" if v.get("account") else "")
        if v["verb"] == "calls":
            return f"start a call{where}, with someone the step doesn't name"
        return f"send something{where}, to someone the step doesn't name"
    if n.rule == "floor:new_recipient":
        return f"{'call' if v['verb'] == 'calls' else 'send to'} {v['recipients']}, whom you didn't name"
    if n.rule == "sink:new_destination":
        return (f"send to {v['destination']}, a place you haven't named, after reading "
                f"{v['read'] or 'outside content'}")
    if n.rule == "sink:private_url":
        return f"open {v['destination']} with your own data in the address ({v['value']})"
    if n.rule == "sink:outside_value" and v["what"] == "typed":
        return f"type {v['value']}, which came from {v['origin']}, not from you"
    if n.rule == "sink:outside_value":
        return f"use the {v['what']} {v['value']}, which came from {v['origin']}, not from you"
    if n.rule == BULK and v.get("what") == "delete":
        return f"permanently delete {v['path']}: {v['files']} in {v['folders']}, {v['bytes']}"
    if n.rule == BULK and "floor:new_recipient" in alongside:      # the new recipients are named beside it
        return None
    if n.rule == BULK:
        return f"{'call' if v.get('what') == 'call' else 'send to'} {v['count']} recipients at once: {v['sample']}"
    if n.rule.startswith("escape:") and v.get("lift") in _LIFTED:
        return _LIFTED[v["lift"]].format(path=v.get("path", ""))
    return v.get("text", n.rule)                # a rule another part of FERB asks about in its own words


def summary(needs: list[Need]) -> str:
    """FERB's question in one line (P27; owner 2026-10-24: "we should have the short version and in details see if
    you want the raw things"): what FERB wants to do, from the needs' facts, never the step or the task's reason. The
    full question (`line`) stays the approval's reason, shown under Details. Display only: it decides nothing."""
    said = {n.values.get("covers") for n in needs if n.rule == BULK}
    rules = frozenset(n.rule for n in needs)
    parts = [p for n in needs if n.rule not in said and (p := _short(n, rules))]
    return "FERB wants to " + "; ".join(parts) + "."


def line(needs: list[Need], call: str, why: str, typed: str = "") -> str:
    """FERB's own words, first and labelled (P27: second person, the concrete action), then the call, the text last
    typed on that page (`typed`: what the worker typed there, likely what it sends; display only, L3), the reason the
    task gave (quoted)
    and the question. The gateway collapses whitespace, strips control characters, maps the double quotes of the text
    and the reason to single ones and clips what goes in."""
    said = {n.values.get("covers") for n in needs if n.rule == BULK}   # a bulk delete already says the floor's words
    rules = frozenset(n.rule for n in needs)
    parts = "; ".join(_part(n, rules) for n in needs if n.rule not in said)
    text = "FERB: " + parts + (f". The step: {call}." if call else ".")
    if typed:
        text += f' Text last typed on this page: "{typed}".'
    if why:
        text += f' Reason given: "{why}".'
    return text + " Go ahead?"
