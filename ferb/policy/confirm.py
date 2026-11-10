"""Owns the code floor (D23) and the sink rule (S4): when a consequential call needs the owner, decided from facts
(the kind the capability derives, its targets' provenance, what the task tree has read, the known destinations).
Must not judge intent or content: a declared purpose may add a need, never remove one (W2); `why` is never read, and
`effect` only as the kind the gateway declares from it for a generic input (ferb/policy/rules.py `declare`). "Already
authorized" (D33): the target is in the owner's own words for this request or in an answer the owner gave (S3
`owner_words` / `owner_answer`), or a destination FERB already knows (SQ6); payments and account-security changes ask
anyway (SQ1).

- The floor: a payment and an account-security change always ask, and their Yes covers one run (a payment in a task's
  browser tab is one question per payment instead: ferb/policy/payments.py, GATE-204); a permanent delete of
  a path the owner didn't name, and a send to a recipient the owner didn't name, ask once per task tree. A send whose
  recipient FERB can't see (a click declared to send or call, with no contact or recipients: a post) is a send to
  someone the owner didn't name, asked once per tree for its place and the account it declared it sends as, unless
  the owner named that site (and that account, when one is declared) in this request's words or answers. An input
  declared to delete is asked about once per page (or place) and declared item.
- The sink rule (a): a send-class call to a place with no recipient (a post, a submit, a page input declared to send)
  asks when the tree has read the owner's own data and outside content and the owner hasn't named that place in this
  request or approved it before (`approved`: a site only mentioned in an earlier request doesn't count, GATE-205).
  With recipients, the floor's new-recipient question already covers it.
- The sink rule (b): a recipient or an amount that came only from outside content (never from the owner's words or
  answers) asks, unless the recipient is a destination FERB already knows.
- The sink rule (c): irreversible bulk work (a send to two or more recipients, a permanent delete of a folder) asks
  once, shown concretely: the count, the total size and a sample. Recipients and paths in the owner's words are
  already authorized and don't count. The sample's size (SHOWN) is display only.
- The certificate floor (L39; owner, 2026-10-25): a shell command, or an AppleScript's `do shell script`, that turns off
  TLS certificate checks (policy/tls.py reads its words and environment) asks once per task tree for that exact text,
  showing it.
FERB's question (`line`) comes first and is labelled, in the owner's terms (P27): the concrete action, then the call."""

import base64
import binascii
import json
import re
import unicodedata
import weakref
from dataclasses import dataclass, field, replace
from typing import Any, Callable, Iterable
from urllib.parse import quote, quote_plus, unquote, unquote_plus

import idna

from ferb.core import origin
from ferb.core.types import Target
from ferb.policy import provenance, tls, urls
from ferb.policy.destinations import destination_of
from ferb.policy.rules import Kind

ALWAYS = {Kind.PAY: "floor:pay", Kind.ACCOUNT_SECURITY: "floor:account_security"}
UNREAD_AMOUNT = "an amount FERB couldn't read"   # a payment whose amount code couldn't read (GATE-204)
# What an account-security change is, as FERB's question names it (the worker prompt's and the effect help's words)
ACCOUNT_SECURITY = "a password, two-factor or recovery setting, or deleting the account"
SEND_CLASS = frozenset({Kind.SEND, Kind.REPLY, Kind.POST, Kind.MAILBOX_SEND, Kind.SUBMIT, Kind.CALL})
OWNER_SOURCES = frozenset({"owner_words", "owner_answer"})
SINK_DESTINATION = "sink:destination"     # the shared rule/key for a host: one Yes/No covers it, shell and browser
BULK = "sink:irreversible_bulk"
TLS_OFF = "floor:tls_off"                 # L39: a shell command that turns off certificate checks (policy/tls.py)
TLS_QUESTION = ("Run this with certificate checks turned off? `{command}` — this skips the check that {site}")
TERMINAL_INPUT = "floor:terminal_input"   # D31, rule 10: any input into a terminal, asked every time (policy/terminal.py)
FOREGROUND = "floor:foreground"           # rule 17: input that brings an app to the front, once per task per app
FOREGROUND_TOOLS = frozenset({"mac.act", "mac.click", "mac.pointer"})
# A browser's own pages (its settings, extensions, flags): input there asks every time, naming the page (live
# 2026-10-26: a worker switched off the owner's extension on chrome://extensions under a once-per-task screen Yes).
# A where-boundary by the page's address, as the browser reports it, never by what the input is meant to do.
BROWSER_PAGE = "floor:browser_page"
BROWSER_SCHEMES = ("chrome://", "brave://", "edge://", "arc://", "vivaldi://", "opera://", "chrome-extension://",
                   "extension://", "moz-extension://", "safari-web-extension://", "about:")
NOT_BROWSER_PAGES = ("about:blank", "about:srcdoc")     # empty documents, not a browser's own pages


PAIR = "floor:pair_profile"     # a task's browser_pair: the owner's Yes pairs that profile (owner 2026-10-27)


def pair_need(name: str, browser: str, folder: str, email: str = "") -> "Need":
    """One Yes per profile in the task: pairing gives FERB that profile's signed-in accounts. `email`: the account the
    profile is signed in to its browser with, shown after its name (HAND-424); the key stays (browser, folder)."""
    return Need(PAIR, key(PAIR, browser, folder), {"name": name or folder, "browser": browser.capitalize(),
                                                   "email": email})


# "Allow all for a short time" (owner 2026-10-27): the routine questions a window may answer Yes to. Never payments,
# account-security changes, permanent deletes, terminal input, certificate checks, or the sink questions that guard
# against outside content steering a send (S4): those always ask.
ROUTINE = frozenset({FOREGROUND, BROWSER_PAGE, "floor:new_recipient", PAIR})
ALLOW_WINDOW_S = 1800.0


def read_lift_key(rule: str) -> str:
    """The one question key for reading a protected folder in a task tree (GATE-200): every read of it there,
    by FERB's file tools or a shell read escape, is covered by one Yes."""
    return "lift:read:" + rule


def routine(n: "Need") -> bool:
    """Whether a window of "don't ask" may answer this need: the routine rules, and an escape that only reads."""
    return n.rule in ROUTINE or (n.rule.startswith("escape:") and n.values.get("lift") == "read")


def browser_page(address: str | None) -> bool:
    """Whether an address is one of a browser's own pages: its settings, extensions and other internal pages
    (scheme only; what is typed or reported, trimmed). An empty document (about:blank) isn't one."""
    text = str(address or "").strip().casefold()
    return text.startswith(BROWSER_SCHEMES) and not text.startswith(NOT_BROWSER_PAGES)


def browser_page_need(app: str, page: str, verb: str = "") -> "Need":
    """Every input on that browser page asks (always): one Yes covers one input. `verb`: what the input does, as its
    card says it ("Click", "Type"; display only, never part of the key)."""
    return Need(BROWSER_PAGE, key(BROWSER_PAGE, app.casefold(), page.strip()),
                {"app": app, "page": page.strip()} | ({"verb": verb} if verb else {}), always=True)


TYPING_ACTIONS = frozenset({"type", "set_value", "edit", "key", "select"})   # a Mac input that enters text or keys


def input_verb(tool: str, args: dict[str, Any]) -> str:
    """What a Mac input does, as a card says it (GATE-566): "Type" for text or keys, "Run a script" for a script,
    else "Click". From the call's own arguments (structure), display only."""
    if tool == "mac.applescript":
        return "Run a script"
    return "Type" if tool == "mac.act" and str(args.get("action") or "") in TYPING_ACTIONS else "Click"
SHOWN = 3                                 # display only: items one question names (outside sources, recipients,
                                          # paths); the rest are counted. It decides nothing.
QUOTED_CHARS = 200                        # display only: a page or item quoted in FERB's question is clipped to
                                          # this, like the task's reason; it decides nothing
FROM_TASK = "(from the task) "            # a task's text that reads like FERB's own (D33)
_BUNDLE_LIKE = re.compile(r"[a-z0-9-]+(?:\.[a-z0-9-]+){2,}", re.I)   # an app's id (com.apple.MobileSMS): internal
Known = Callable[[str, str], bool]


@dataclass(frozen=True)
class Need:
    rule: str
    key: str                                  # what one answer covers in this task tree (`key`)
    values: dict[str, str] = field(default_factory=dict)          # what FERB's question names
    always: bool = False                      # SQ1: the Yes covers one run of the call, never a later one
    destinations: tuple[tuple[str, str], ...] = ()                # (kind, value) a Yes makes known (SQ6)
    fresh: bool = False                       # asked every time: an earlier Yes never covers it (a No still refuses);
                                              # what covers it is decided before (terminal input, a tab's payment)
    spent_on_ask: bool = False                # an always-ask Yes spent when given, never released (an escape's or a
                                              # protected folder's change, terminal input)
    reaches: Target | None = None             # the target this need was decided for (the call's, or a fact's: the
                                              # host a form posts to): what a standing scope must cover (D30)


def foreground_need(tool: str, args: dict[str, Any]) -> "Need | None":
    """Rule 17 (owner 2026-10-26: "didnt i build ferb to do tasks in the background"): a Mac input delivered in the
    foreground brings its app to the front and can move the owner's Space, so it needs the owner's Yes, once per task
    for that app. The worker's own allow_foreground flag never stands in for it. Background mechanisms ask nothing."""
    # In a web browser, any Mac-tool input or menu choice drives the owner's own browser screen, which the extension
    # does in hidden tabs (live 2026-10-26: the worker switched Chrome profiles through its Profiles menu, moving the
    # owner's Space). Listing menus (no path) reads only. A where-boundary on the target app, never on the step.
    # A script for a browser (mac.applescript) acts in those same windows (its tabs, their pages), so it is the same
    # question, under the same key: one Yes covers the browser for the task, whichever Mac tool acts in it.
    # Which apps are browsers: the one list (ferb/act/mac/browsers.py: known bundle ids, installed http handlers,
    # names without invisible marks, `pid:N` by its running app).
    from ferb.act.mac.browsers import is_browser_app
    acts = tool in FOREGROUND_TOOLS or tool == "mac.applescript" or (
        tool == "mac.menu" and bool(str(args.get("path") or "").strip()))
    in_browser = acts and is_browser_app(str(args.get("app") or ""))
    if not in_browser and (tool not in FOREGROUND_TOOLS
                           or not (args.get("allow_foreground") or args.get("mechanism") == "foreground")):
        return None
    app = str(args.get("app") or "an app")
    return Need(FOREGROUND, key(FOREGROUND, app_key(app)),
                {"app": app} | ({"script": "yes", "script_text": str(args.get("script") or "")}
                                if tool == "mac.applescript" else {}))


def app_key(app: str) -> str:
    """One app however a Mac tool's `app` argument spells it (step 12e, fix 2; live 2026-10-31: "Terminal", then
    "pid:9040", asked twice): the running app's bundle id, casefolded, when the name, `pid:N` or bundle id names exactly
    one running app (`apps.find`, the drivers' own lookup); a name no running app has, the installed app's bundle id
    macOS finds by that name (GATE-560: Safari quit between two inputs); else the argument without case or invisible
    marks ("\u200eWhatsApp" is "WhatsApp")."""
    from ferb.act.mac import apps
    from ferb.act.mac.browsers import clean
    try:
        found = apps.candidates(app.strip())
    except Exception:                                    # the running-app list can't be read: the spelling decides
        return clean(app)
    if len(found) == 1:
        return found[0].bundle_id.casefold()
    if found:                                            # several running: the spelling decides
        return clean(app)
    try:                                                 # GATE-560: not running (it quit, or isn't open yet): the
        installed = apps.installed_bundle(app.strip())   # installed app's bundle id, as when it runs
    except Exception:
        installed = None
    return installed.casefold() if installed else clean(app)


_BUNDLE_ID = re.compile(r"(?:[A-Za-z0-9-]+\.){2,}[A-Za-z0-9-]+|pid:\d+")


def app_label(app: str) -> str:
    """An app as the owner knows it (live 2026-11-07: a card said "com.google.Chrome"): a bundle id or `pid:N` that
    names exactly one running app becomes that app's name; anything else, a site among them, stays as written.
    Display only: keys use `app_key`."""
    text = app.strip()
    if not _BUNDLE_ID.fullmatch(text):
        return app
    from ferb.act.mac import apps
    try:
        found = apps.find(text)
    except Exception:                                    # the running-app list can't be read: as written
        found = None
    return found.name if found is not None and found.name else app


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


def _account(v: dict[str, str]) -> str:
    """A profile's signed-in account after its name in a question (HAND-424), "" when it has none."""
    return f" ({v['email']})" if v.get("email") else ""


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


def _host(where: str) -> str:
    """A site as the owner reads it: its host without "www."; an app's name stays as it is."""
    w = (where or "").strip()
    return w[4:] if w.casefold().startswith("www.") else w


def shown_amount(amount: float | None, currency: str | None = None) -> str:
    """An amount as the owner reads it, with the currency the page showed: "₹248.75", "INR 248.75", "248.75"."""
    if amount is None:
        return ""
    number = str(int(amount)) if amount == int(amount) else f"{amount:.2f}"
    unit = " ".join((currency or "").split())[:8]
    return (unit + number if len(unit) == 1 else f"{unit} {number}") if unit else number


def shown_origin(label: str) -> str:
    """One origin label in the owner's terms, without FERB's internal ids: "the web page on example.com", "a message,
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
    """A task's question as the owner sees it (D33, GATE-225): one paragraph, so no line of it can pass for FERB's own
    check (a newline or a homoglyph doesn't help), marked when it starts like FERB's own words. The app draws the
    question's "from the task" tag from the approval's structural `from_task` flag, never from this text."""
    return from_task(" ".join(text.split()))


def from_task_lines(text: str) -> str:
    """A planner's chat reply or clarify: each line that starts with "FERB" is marked as the task's."""
    return "\n".join(from_task(line) for line in text.split("\n"))


def shown_origins(labels: Iterable[str]) -> str:
    """The outside sources in the owner's terms, the first SHOWN named and the rest counted: those that name what
    was read (a page's host) before kinds only (what reached FERB's history from outside, T-1007)."""
    items = list(dict.fromkeys(shown_origin(o) for o in sorted(set(labels), key=lambda o: (o.count(":") < 2, o))))
    if len(items) <= SHOWN:
        return "; ".join(items)
    return "; ".join(items[:SHOWN]) + f"; and {len(items) - SHOWN} more"


def shown_place(label: str) -> str:
    """One origin label as the place a card says FERB saw something (GATE-566): "a www.google.com page", "a message",
    "an email"; the other kinds as `shown_origin` says them."""
    head, _, rest = label.partition(":")
    kind, _, detail = rest.partition(":")
    if head == "external" and kind == "web" and detail not in ("excluded", "trail"):
        return f"a {detail} page" if detail else "a web page"
    if head == "external" and kind == "message":
        return "a message"
    return shown_origin(label)


def shown_places(labels: Iterable[str]) -> str:
    """The places a card names, the first SHOWN said and the rest counted ("a www.google.com page and a message")."""
    items = list(dict.fromkeys(shown_place(o) for o in sorted(set(labels), key=lambda o: (o.count(":") < 2, o))))
    if len(items) > SHOWN:
        items = items[:SHOWN] + [f"{len(items) - SHOWN} more places"]
    return ", ".join(items[:-1]) + (" and " if len(items) > 1 else "") + items[-1] if items else ""


def value_kind(value: str) -> str:
    """What a value is by its shape (GATE-509), for a card's words: "email address", "phone number", else "text"."""
    text = value.strip().strip('"')
    if _EMAIL.fullmatch(text):
        return "email address"
    if _PHONE.fullmatch(text) and len([c for c in text if c.isdigit()]) >= 7:
        return "phone number"
    return "text"


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


# One group of a grouped value (a phone's, an IBAN's or a card's digit groups): letters and digits holding a digit, a
# leading + kept. A grouped value is two or more groups joined by single spaces on one line.
_GROUP = r"\+?[^\W_]*\d[^\W_]*"
# not glued to a word, a path or a time/decimal (13:27, 4.5): `07 Nov 2026 13:27` holds no grouped value
_RUN = re.compile(rf"(?<![\w+\-./@%:,])({_GROUP}(?: {_GROUP})+)(?![\w\-/@%]|[.:,]\d)")
_GROUPED_DIGITS = re.compile(r"\d+(?:[\s.,\-]+\d+)+")   # digit groups split only by grouping separators


def _spellings(value: str, digits: bool = False) -> set[str]:
    """Other ways the same whole value is written (GATE-517): its letters and digits with every separator and case
    dropped (`+44 7700 900123`, `+44-7700-900123`, `447700900123`; `4,312.75`, `4312.75`, `431275`; `DE00 1234 …`)
    and, for an international phone number, its national number with and without a leading 0. With `digits` (a value
    of the owner's data), also its digits alone where grouping separators split them (`ORD-48213-7731` →
    `482137731`; never across letters: `s6e35` has none). Only forms holding a digit. Shape only (rule 4)."""
    out: set[str] = set()
    skeleton = "".join(c for c in value if c.isalnum())
    if len(skeleton) >= provenance.MIN_LEN and any(c.isdigit() for c in skeleton):
        out.add(skeleton)
    if digits:
        out |= {j for m in _GROUPED_DIGITS.finditer(value)
                if len(j := "".join(c for c in m.group(0) if c.isdigit())) >= provenance.MIN_LEN}
    if value.lstrip().startswith("+") and skeleton.isdigit() and len(skeleton) >= provenance.DESTINATION_PHONE_DIGITS:
        # the national number is the whole phone (GATE-518), with or without its 0: +447700900456 → 07700900456,
        # 7700900456. The country code's length isn't known (1-3 digits), so each is tried
        out |= {form for k in (1, 2, 3) for form in ("0" + skeleton[k:], skeleton[k:])}
    return out


_YEAR = re.compile(r"(?:19|20)\d\d")


def is_year(value: str) -> bool:
    """A plain year, 1900-2099, alone: never the owner's private value (owner, 2026-11-07: "Years don't count").
    Only a year: every other number (a code, a PIN, card digits, a phone) still counts (GATE-523)."""
    return bool(_YEAR.fullmatch(value))


# A full calendar date as numbers: 2026-11-07, 2026/11/07, 07-11-2026, 11/07/2026, 07.11.2026 (GATE-535)
_DATE = re.compile(r"(?:(19|20)\d\d[-/.](\d{1,2})[-/.](\d{1,2})|(\d{1,2})[-/.](\d{1,2})[-/.](19|20)\d\d)")


def is_date(value: str) -> bool:
    """A full calendar date alone (owner D2, 2026-11-08, GATE-535): like a year, never the owner's private value.
    By shape only: year 1900-2099 and a month and day that can be one."""
    m = _DATE.fullmatch(value)
    if m is None:
        return False
    a, b = (int(m[2]), int(m[3])) if m[1] else (int(m[4]), int(m[5]))
    first_month = 1 <= a <= 12 and 1 <= b <= 31                     # year first: month, day
    return first_month if m[1] else (1 <= a <= 31 and 1 <= b <= 12) or (1 <= a <= 12 and 1 <= b <= 31)


def _value_forms(text: str, coarse: Iterable[str] | None = None) -> frozenset[str]:
    """Every form a single-token value of `text` (lowercased) can take in an address or a form: its tokens (coarse:
    whole emails, hosts, paths; fine: values embedded in JSON/.env/query punctuation, N2) and their `_spellings`. A
    token that appears only as one group of a grouped value (`_RUN`) is a part of it, not a value of its own (owner,
    2026-11-07, on a link carrying only part of the owner's data: "dont ask"): the last digits of `+44 7700 900123` are
    not the number. Grouped values are matched by `_in_runs`. `coarse`: the text's coarse tokens when already made."""
    toks = {t for t in set(provenance.tokens(text) if coarse is None else coarse) | _fine_tokens(text)
            if not is_date(t)}                                   # a full date is never a value (GATE-535)
    spans = [m.span(1) for m in _RUN.finditer(text) if _one_value(m.group(1))]
    if spans:
        runs = [text[a:b] for a, b in spans]
        rest = "".join(text[b:a] for (_, b), (a, _) in zip([(0, 0), *spans], [*spans, (len(text), 0)]))
        members = {t for run in runs for t in set(provenance.tokens(run)) | _fine_tokens(run)}
        toks -= members - (set(provenance.tokens(rest)) | _fine_tokens(rest))
    forms = set(toks)
    for value in toks:
        if len(value) <= 64 and "/" not in value:               # not a URL or a path: those keep their own tokens
            forms |= _spellings(value, digits=True)
    return frozenset(forms)


# Each read text's forms, made once and kept while the text is (the gateway keeps a task's prepared texts)
_FORMS: "weakref.WeakKeyDictionary[provenance.Text, frozenset[str]]" = weakref.WeakKeyDictionary()
_SEQUENCES: "weakref.WeakKeyDictionary[provenance.Text, tuple[str, ...]]" = weakref.WeakKeyDictionary()


def _toks(t: provenance.Text) -> frozenset[str]:
    """A read text's value forms for the URL and form checks (`_value_forms`), made once per text."""
    found = _FORMS.get(t)
    if found is None:
        found = _FORMS[t] = _value_forms(t.folded, t.tokens)
    return found


def _sequence(t: provenance.Text) -> tuple[tuple[str, ...], frozenset[str]]:
    """A read text's fine tokens in order (every length, for `_phrase`), and its value forms."""
    seq = _SEQUENCES.get(t)
    if seq is None:
        seq = _SEQUENCES[t] = tuple(x for x in _FINE.split(t.folded) if x)
    return seq, _toks(t)


def _phrase(piece: str, t: provenance.Text, known_to_site: set[str] = set()) -> bool:
    """Whether a piece of several words is written in `t` word for word, holding a whole value of `t` (a token of at
    least MIN_LEN that is a value and not only a group of a grouped value) that the destination doesn't already have
    (`known_to_site`): an address placed whole in one query value (`22 Birch Road Apt 5B, Riverton 99021`). A value
    inside other words of a piece is not matched alone (GATE-516)."""
    words = [x for x in _FINE.split(piece.lower()) if x]
    seq, forms = _sequence(t)
    if len(words) < 2 or not any(len(w) >= provenance.MIN_LEN and _is_value(w) and w in forms and
                                 w not in known_to_site and not is_year(w) for w in words):
        return False
    n = len(words)
    return any(seq[i:i + n] == tuple(words) for i, x in enumerate(seq) if x == words[0])


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
# an address written without its scheme (`www.macworld.com/article/666646/…`, as FERB's own notes record a visit): a
# host of dotted labels, then a path
_HOST_PATH_IN_TEXT = re.compile(r"(?<![\w@./:-])(?:[a-z0-9-]+\.)+[a-z]{2,}/[^\s\"'<>\[\]{}|\\^`()]*")


def _host_had_it(seen: str, dest: str) -> bool:
    """Whether a value in a URL of host `seen` was already known to `dest`: the same host (a leading "www." aside), or
    a host under it (`help.instagram.com` under `instagram.com`). Never a sibling: on shared-suffix platforms
    (`*.vercel.app`, `*.github.io`, `*.netlify.app`…) each owner has their own subdomain, so a registrable-domain
    match would hand one owner's token to another's (codebase review, note on 207b2df). Hosts are normalized
    (ferb/policy/urls.py: IDNA, lowercase)."""
    seen, dest = seen.rstrip("."), dest.rstrip(".")
    bare = seen.removeprefix("www.")
    return dest in (seen, bare, "www." + bare) or dest.endswith("." + seen)


def page_scope(url: str) -> str:
    """Where on a site a value was seen (owner, 2026-11-07, on shared sites: "Same page path only"): the host and the
    page's path through its first segment that holds a value (an id: a document's, a profile's, a post's), else the
    whole path; always starting with "/" after the host. `docs.google.com/document/d/1aB3x/edit` gives
    `docs.google.com/document/d/1ab3x`, `www.scaler.com/academy/mentee-dashboard` the whole path. "" when the address
    can't be read (GATE-528)."""
    parsed = urls.parse(url if "://" in url else "https://" + url)
    if parsed is None or not parsed.host:
        return ""
    kept: list[str] = []
    for seg in (unquote_plus(x).casefold() for x in parsed.path.split("/") if x):
        kept.append(seg)
        if _is_value(seg):
            break
    return parsed.host.rstrip(".") + "/" + "/".join(kept)


def _in_scope(scope: str, dest: str, dest_path: str) -> bool:
    """Whether an address (its host `dest`, its path) is on the page a value was seen on (`page_scope`): a host that
    one covers (`_host_had_it`) and a path that starts, segment by segment, with that page's (GATE-528)."""
    host, sep, path = scope.partition("/")
    if not sep or not _host_had_it(host, dest):
        return False                                        # no page known (an older record): never in scope
    want = [x for x in path.split("/") if x]
    have = [unquote_plus(x).casefold() for x in dest_path.split("/") if x]
    return have[:len(want)] == want


def _read_from(labels: set[str], dest: str) -> bool:
    """Whether a record was read from `dest`'s own site (GATE-506): its outside labels are all web pages of hosts `dest`
    covers (`_host_had_it`), at least one, and nothing else but the private label (the owner's signed-in page there).
    Context FERB assembled (its own, model-written or mixed labels) never is."""
    web = [o.split(":", 2)[2] for o in labels if o.startswith(origin.EXTERNAL + ":web:") and o.count(":") >= 2]
    return bool(web) and all(o == origin.PRIVATE or o.startswith(origin.EXTERNAL + ":web:") for o in labels) and \
        all(_host_had_it(h.lower().rstrip("."), dest) for h in web)


def _same_site_only(private: list[tuple[int, provenance.Text]], dest: str,
                    origins: dict[int, set[str]] | None = None,
                    sites: dict[int, frozenset[str]] | None = None, dest_path: str = "",
                    pages: dict[int, str] | None = None) -> set[str]:
    """The tokens of the owner's own data whose every occurrence is inside a URL whose host `dest` covers
    (`_host_had_it`), or in a record read from `dest`'s own site (`_read_from`, GATE-506: the owner's signed-in page
    there, live 2026-10-18: a class id read from www.scaler.com's own page, sent back to www.scaler.com): a value that
    host already had, such as a reel's id in the owner's tab of that reel. A token also found anywhere else in private
    text (another host's URL or page, a file, plain text) isn't one. Since GATE-528 (owner: "Same page path only")
    the destination must also be on the page the value was seen on (`page_scope`: the link's, the page record's
    `pages`, or FERB's conversation's stored `sites`), not just its host: on a shared site a stranger's page is the
    same host. Shape and provenance only (rule 6)."""
    inside: set[str] = set()
    elsewhere: set[str] = set()
    for order, t in private:
        if origins is not None and _read_from(origins.get(order, set()), dest):
            page = (pages or {}).get(order, "")
            if page and _in_scope(page_scope(page), dest, dest_path):
                inside |= _toks(t)
                continue                                  # another page of that site: its links decide, as below
        read_on = (sites or {}).get(id(t))               # values FERB's conversation stored as read on these pages
        if read_on:
            (inside if all(_in_scope(scope, dest, dest_path) for scope in read_on) else elsewhere).update(_toks(t))
            continue
        links, rest = _url_parts(t)
        for scope, forms in links:
            if _in_scope(scope, dest, dest_path):
                inside |= forms
            else:
                elsewhere |= forms
        elsewhere |= rest
    return inside - elsewhere


@dataclass(frozen=True)
class _Run:
    """One grouped value: its groups' letters and digits (lowercased, `+` dropped), whether it starts with `+`, the
    groups joined, and where each group starts in the joined form (index by position)."""
    groups: tuple[str, ...]
    plus: bool
    joined: str
    starts: dict[int, int]


def _one_value(run: str) -> bool:
    """Whether digit groups joined by single spaces look like one grouped value, as a phone's, an IBAN's or a card's do
    (GATE-527): every group but a leading +country code holds 2 to 6 letters and digits, and the longest is at most
    twice the shortest. A complete value standing next to another number (`account 12345678 3 days`, `OTP 482193 5`,
    `07700900123 07700900456`) is not one: each stays a value of its own. Shape only (rule 4)."""
    groups = ["".join(c for c in g if c.isalnum()) for g in run.split(" ")]
    if run.startswith("+"):
        groups = groups[1:]
    sizes = [len(g) for g in groups]
    return len(sizes) >= 2 and min(sizes) >= 2 and max(sizes) <= 6 and max(sizes) <= 2 * min(sizes)


def _grouped(text: str) -> tuple[_Run, ...]:
    """The grouped values of a lowercased text (`_RUN` runs that `_one_value` takes for one value)."""
    found: list[_Run] = []
    for m in _RUN.finditer(text):
        if not _one_value(m.group(1)):
            continue
        groups = tuple("".join(c for c in g if c.isalnum()) for g in m.group(1).split(" "))
        starts, at = {}, 0
        for i, g in enumerate(groups):
            starts[at] = i
            at += len(g)
        starts[at] = len(groups)
        found.append(_Run(groups, m.group(1).startswith("+"), "".join(groups), starts))
    return tuple(found)


_RUNS: "weakref.WeakKeyDictionary[provenance.Text, tuple[_Run, ...]]" = weakref.WeakKeyDictionary()


def _runs(t: provenance.Text) -> tuple[_Run, ...]:
    """A read text's grouped values; made once."""
    found = _RUNS.get(t)
    if found is None:
        found = _RUNS[t] = _grouped(t.folded)
    return found


def _spans(run: _Run, form: str, first: int = 0, exact: bool = False) -> bool:
    """Whether `form` is two or more contiguous groups of `run` joined, starting at group `first` or later (exactly
    there with `exact`). Linear in the run: a page's table of numbers can't make it slow."""
    at = run.joined.find(form)
    while at != -1:
        start, end = run.starts.get(at), run.starts.get(at + len(form))
        if start is not None and end is not None and end - start >= 2 and (start == first if exact else start >= first):
            return True
        at = run.joined.find(form, at + 1)
    return False


def _in_runs(runs: Iterable[_Run], groups: Iterable[tuple[str, ...]], forms: Iterable[str]) -> bool:
    """Whether a candidate is a whole grouped value of these runs (coordinator ruling, GATE-518): two or more
    contiguous groups of one run, sent as groups (`groups`) or joined (`forms`: a candidate's letters and digits),
    or, for a run that starts with `+`, the groups after its country code with a leading 0 (the national form). One
    group alone is a part (GATE-516)."""
    groups = [g for g in groups if len(g) >= 2 and len("".join(g)) >= provenance.MIN_LEN]
    forms = [f for f in forms if len(f) >= provenance.MIN_LEN]
    for run in runs:
        for want in groups:
            n = len(want)
            if any(run.groups[i:i + n] == want for i in range(len(run.groups) - n + 1)):
                return True
        for form in forms:
            if _spans(run, form) or (run.plus and form.startswith("0") and _spans(run, form[1:], 1, exact=True)):
                return True
    return False


def _candidate(piece: str) -> tuple[list[tuple[str, ...]], set[str]]:
    """A URL piece or typed text as `_in_runs` takes it: its grouped values' groups, and its letters and digits
    (whole, and each grouped value's) when they hold a digit."""
    low = piece.lower()
    runs = _grouped(low)
    forms = {"".join(c for c in low if c.isalnum())} | {r.joined for r in runs}
    return [r.groups for r in runs], {f for f in forms if any(c.isdigit() for c in f)}


def whole_values(text: str) -> list[str]:
    """The owner-data-shaped whole values a text holds, lowercased, in order (for stored provenance, GATE-519): each
    token that is a value (`_is_value`, at least MIN_LEN, not only a group of a grouped value: `kartikey/stealth-launch`,
    `bluefalcon!` included, GATE-529) and each grouped value as written. Plain words are never values."""
    low = text.lower()
    forms = _value_forms(low)
    groups = {g for run in _grouped(low) for g in run.groups}
    seen: dict[str, None] = {}
    for tok in [*provenance.tokens(low), *(x.rstrip(_TRAIL) for x in _FINE.split(low) if x)]:
        if len(tok) >= provenance.MIN_LEN and _is_value(tok) and tok in forms and tok.lstrip("+") not in groups:
            seen.setdefault(tok, None)
    for m in _RUN.finditer(low):
        seen.setdefault(m.group(1), None)
    return list(seen)


def spellings(value: str) -> set[str]:
    """`value` and its other spellings as the sink matches them (`_spellings`, with the owner's digit forms)."""
    return {value} | _spellings(value, digits=True)


def with_forms(t: provenance.Text) -> provenance.Text:
    """`t` with its value forms and URL parts made now (the gateway calls it off the event loop, DMN-13)."""
    _toks(t)
    _url_parts(t)
    _runs(t)
    return t


_UrlParts = tuple[tuple[tuple[str, frozenset[str]], ...], frozenset[str]]
_URL_PARTS: "weakref.WeakKeyDictionary[provenance.Text, _UrlParts]" = weakref.WeakKeyDictionary()


def _url_parts(t: provenance.Text) -> tuple[tuple[tuple[str, frozenset[str]], ...], frozenset[str]]:
    """A read text's URLs, each with the page it names (`page_scope`) and its value forms, and the value forms of the
    rest of it; made once."""
    found = _URL_PARTS.get(t)
    if found is None:
        links: list[tuple[str, frozenset[str]]] = []
        rest, cut = [], 0
        matches = [(m, m.group(0)) for m in _URL_IN_TEXT.finditer(t.folded)]
        matches += [(m, "https://" + m.group(0)) for m in _HOST_PATH_IN_TEXT.finditer(t.folded)
                    if not any(a.start() <= m.start() < a.end() for a, _ in matches)]
        for m, address in sorted(matches, key=lambda item: item[0].start()):
            parsed = urls.parse(address.rstrip(_TRAIL))
            if parsed is not None and parsed.host and m.start() >= cut:
                links.append((page_scope(address.rstrip(_TRAIL)), _value_forms(m.group(0))))
                rest.append(t.folded[cut:m.start()])
                cut = m.end()
        rest.append(t.folded[cut:])
        found = _URL_PARTS[t] = (tuple(links), _value_forms(" ".join(rest)))
    return found


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
    fragment whole and split (by &, = and its route's /). Parsed as the browser will (ferb/policy/urls.py); pieces are `unquote_plus`-decoded."""
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
        for kv in pair.split("="):
            pieces += [kv] + (kv.split("/") if "/" in kv else [])   # a page's own route: #inbox/VALUE
    return pieces + [again for piece in pieces for again in _decoded_again(piece)]


def _decoded_again(piece: str) -> list[str]:
    """A piece percent-encoded more than once (`%2540` → `%40` → `@`), decoded again, at most twice more (GATE-516)."""
    out: list[str] = []
    while "%" in piece and len(out) < 2 and (nxt := unquote(piece)) != piece:
        out.append(nxt)
        piece = nxt
    return out


_WORD_JOINERS = frozenset(".-_'\u2019")                   # inside words, names and file names: index.html, about-us


def _is_value(form: str) -> bool:
    """Whether a URL piece's form is a value rather than an ordinary word (L4): it holds a digit or a character other
    than a letter and the joiners inside words, names and file names — an email's @, a path's /, a number, a code. A
    word ("search", "playlist", "about-us", "index.html", "entry_id") is navigation and wrapper vocabulary, never the
    owner's private value. Shape only, never a meaning (rule 6)."""
    return any(not (c.isalpha() or c in _WORD_JOINERS or unicodedata.category(c).startswith("M")) for c in form)


def _variants(piece: str) -> tuple[set[str], set[str]]:
    """A URL piece's forms to match: (its values as written: itself and its trailing-punctuation-stripped form,
    lowercased, when they are values, not words, with their spellings (`_spellings`); its
    base64/hex decodings (N2), words included, since hiding a word in an encoding is how data is smuggled, scenario
    #2)."""
    shown: set[str] = set()
    decoded: set[str] = set()
    for base in (piece.strip(), piece.strip().rstrip(_TRAIL)):
        if len(base) >= provenance.MIN_LEN:
            low = base.lower()
            if _is_value(low):
                shown.add(low)
                shown |= _spellings(low)                      # its other spellings (GATE-517)
            decoded |= _decodings(base)
    return shown, decoded


def _own_name(host: str, private: list[tuple[int, provenance.Text]], outside: list[tuple[int, provenance.Text]],
              origins: dict[int, set[str]] | None) -> set[str]:
    """GATE-570 (owner), GATE-571: the labels of the destination's own host name (cs50 of cs50.harvard.edu) and their
    spellings, which tell that site nothing: only when the task read that host's name before (a page of that host, or
    the host written in what it read), so a host made up around a value (VALUE.attacker.com, N3) never counts."""
    host = host.rstrip(".").lower()
    bare = host.removeprefix("www.")
    if not bare or "." not in bare:
        return set()
    named = re.compile(r"(?<![\w.-])(?:www\.)?" + re.escape(bare) + r"(?![\w-]|\.[\w-])")
    pages = {o.split(":", 2)[2].lower().rstrip(".").removeprefix("www.")
             for labels in (origins or {}).values() for o in labels
             if o.startswith(origin.EXTERNAL + ":web:") and o.count(":") >= 2}
    if bare not in pages and not any(named.search(t.folded) for _, t in [*private, *outside]):
        return set()
    forms: set[str] = set()
    for label in host.split("."):
        if label and label != "www":
            forms |= {label} | _spellings(label)
            if label.startswith("xn--"):
                try:
                    forms.add(idna.ulabel(label))
                except idna.IDNAError:
                    pass
    return forms


@dataclass(frozen=True)
class PrivateValue:
    """A URL piece carrying the owner's data (GATE-507): the piece as it leaves, the token of the owner's data it
    matched (the piece itself, or a value found inside or decoded from it), and the record that token was first read
    from (span order), so the question can say which value leaves and where FERB read it."""
    piece: str
    matched: str
    first_read: int


def url_carries_private(url: str, private: list[tuple[int, provenance.Text]],
                        outside: list[tuple[int, provenance.Text]], owner: provenance.Text,
                        origins: dict[int, set[str]] | None = None,
                        sites: dict[int, frozenset[str]] | None = None,
                        pages: dict[int, str] | None = None) -> list[str]:
    """`url_private_values`, as the pieces that would leave."""
    return [v.piece for v in url_private_values(url, private, outside, owner, origins, sites, pages)]


def url_private_values(url: str, private: list[tuple[int, provenance.Text]],
                       outside: list[tuple[int, provenance.Text]], owner: provenance.Text,
                       origins: dict[int, set[str]] | None = None,
                       sites: dict[int, frozenset[str]] | None = None,
                       pages: dict[int, str] | None = None) -> list[PrivateValue]:
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
    reel's id from the owner's own tab of that reel), and neither does one whose every private occurrence was read
    from that site's own pages (`origins`: each record's labels by span order; GATE-506). Shape and provenance only,
    never a meaning (rule 6)."""
    if not private:
        return []
    dest = urls.parse(url)
    known_to_site = _same_site_only(private, dest.host, origins, sites, dest.path, pages) \
        | _own_name(dest.host, private, outside, origins) if dest is not None and dest.host else set()
    hits = _private_hits(_url_pieces(url), private, outside, owner, known_to_site)
    if not hits:
        return []
    earliest = min(h.first_read for h in hits)
    seen_url = {url.casefold(), url.replace("\\", "/").casefold()}
    if any(order < earliest and any(u in t.folded for u in seen_url) for order, t in outside):
        return []                                             # a followed link, read before the private value
    if any(_link_spans(url, t.folded) or _broken_link_spans(url, t.folded) for _, t in private):
        # GATE-536 (owner D3, extending GATE-521): a link as a record of the owner's FERB read has it (a signed-in page,
        # a mail, a file), wherever the value came first; an outside-only page's links are as before
        return []
    return hits


def _link_spans(url: str, folded: str) -> list[tuple[int, int]]:
    """Where `url` appears in a read text, as written or inside another address (a redirect wrapper's `?q=` holds it
    as written or percent-encoded): (start, end) of each occurrence."""
    spans: list[tuple[int, int]] = []
    for form in dict.fromkeys(f.casefold() for f in (url, url.replace("\\", "/"), quote(url, safe=""),
                                                     quote(url, safe=":/"), quote_plus(url))):
        at = folded.find(form)
        while at != -1:
            spans.append((at, at + len(form)))
            at = folded.find(form, at + 1)
    return spans


def _broken_link_spans(url: str, folded: str) -> list[tuple[int, int]]:
    """Where `url` appears in a read text with whitespace inside its own extent (a long link the page wrapped), never
    joining text before or after it."""
    chars = url.casefold()
    if not chars:
        return []
    pattern = re.compile(r"\s*".join(re.escape(c) for c in chars))
    return [m.span() for m in pattern.finditer(folded) if any(c.isspace() for c in m.group(0))]


_EMAIL = re.compile(r"[\w.+-]+@[\w-]+(?:\.[\w-]+)+")
_PHONE = re.compile(r"\+?\d[\d ().-]{5,}\d")


def contacts_in(text: str) -> set[str]:
    """The email addresses (lowercased) and phone numbers (digits, at least 7) written in `text`, by shape."""
    found = {m.group(0).lower() for m in _EMAIL.finditer(text)}
    found |= {d for m in _PHONE.finditer(text) if len(d := "".join(c for c in m.group(0) if c.isdigit())) >= 7}
    return found


def own_contact(value: str, own: Iterable[str]) -> bool:
    """Whether a typed value is one of the owner's own email addresses or phone numbers (GATE-538): the same address,
    or the same digits (a number with or without its country code: the last 10 digits when both have 10 or more)."""
    mine = set(own)
    if value.strip().lower() in mine:
        return True
    digits = "".join(c for c in value if c.isdigit())
    return len(digits) >= 7 and not any(c.isalpha() for c in value) and any(
        d == digits or (len(d) >= 10 and len(digits) >= 10 and d[-10:] == digits[-10:]) for d in mine if d.isdigit())


def text_carries_private(text: str, private: list[tuple[int, provenance.Text]],
                         outside: list[tuple[int, provenance.Text]], owner: provenance.Text) -> list[str]:
    """The sink rule for a form (coordinator ruling, step 6b): the values typed into it that are a token of the
    owner's own data (a private tool result), not the owner's typed words and not already public before they were
    read privately, by the same rules as `url_carries_private` (values only, never ordinary words, unless hidden in
    base64/hex). Shape and provenance only, never a meaning (rule 6)."""
    return [h.piece for h in text_private_values(text, private, outside, owner)]


def text_private_values(text: str, private: list[tuple[int, provenance.Text]],
                        outside: list[tuple[int, provenance.Text]], owner: provenance.Text) -> list[PrivateValue]:
    """`text_carries_private` with each value's first private read (span order), so the form sink's question can say
    where FERB read it (GATE-557, as GATE-507 for an address)."""
    if not private or not text.strip():
        return []
    pieces = [text.strip()] + [t for t in _FINE.split(text) if t]
    return _private_hits(pieces, private, outside, owner, set())


def _private_hits(pieces: list[str], private: list[tuple[int, provenance.Text]],
                  outside: list[tuple[int, provenance.Text]], owner: provenance.Text,
                  known_to_site: set[str]) -> list[PrivateValue]:
    """Each piece that carries a value of the owner's own data, with the value it matched and the order that value was
    first read privately."""
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

    hits: list[PrivateValue] = []
    for piece in pieces:
        value = piece.strip()
        # the owner-word exclusion is per matched form, so a typed word inside a decoded blob can't excuse the rest
        shown, decoded = _variants(piece)
        matched = {m for m in (shown | decoded) & set(first_private)
                   if m not in owner_toks and m not in known_to_site and not laundered(m) and not is_year(m)}
        if not matched and value:                              # a grouped value, whole or two or more of its groups
            groups: list[tuple[str, ...]] = []
            forms: set[str] = set()
            for text in (value, *decoded):                     # as written, and decoded from base64 or hex (N2)
                more_groups, more_forms = _candidate(text)
                groups, forms = groups + more_groups, forms | more_forms
            orders = [order for order, t in private if _in_runs(_runs(t), groups, forms)]
            if orders and not _in_runs(_runs(owner), groups, forms) and not any(
                    o < min(orders) and _in_runs(_runs(t), groups, forms) for o, t in outside):
                first_private[value.lower()] = min(orders)
                matched = {value.lower()}
        if not matched and value:                              # several words, written so in the owner's data
            orders = [order for order, t in private if _phrase(value, t, known_to_site)]
            if orders and not _phrase(value, owner) and not any(o < min(orders) and _phrase(value, t)
                                                                  for o, t in outside):
                first_private[value.lower()] = min(orders)
                matched = {value.lower()}
        if matched and value and value not in [h.piece for h in hits]:
            first = min(matched, key=lambda m: (first_private[m], -len(m), m))   # the earliest read, longest form
            hits.append(PrivateValue(value, first, first_private[first]))
    # two neighbouring groups of a grouped value, each in its own piece (?a=7700&b=900123): a whole value leaves
    # (in the value's order: a date path /2026/11/ doesn't carry "11 2026")
    seq = [w for piece in pieces for w in _FINE.split(piece.lower()) if w] if not hits else []
    first_at = {w: i for i, w in reversed(list(enumerate(seq)))}
    last_at = {w: i for i, w in enumerate(seq)}
    for order, t in sorted(private, key=lambda item: item[0]):
        for run in _runs(t):
            for pair in zip(run.groups, run.groups[1:]):
                key = " ".join(pair)
                if len(pair[0] + pair[1]) >= provenance.MIN_LEN and pair[0] in first_at and pair[1] in last_at and \
                        first_at[pair[0]] < last_at[pair[1]] and key not in [h.matched for h in hits] and \
                        not _in_runs(_runs(owner), [pair], ()) and \
                        not any(o < order and _in_runs(_runs(x), [pair], ()) for o, x in outside):
                    hits.append(PrivateValue(key, key, order))
                    break                                      # one hit names the value
    return hits


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
           place: str | None = None, content: str | None = None, approved: Known | None = None,
           own: Callable[[], Iterable[str]] = lambda: ()) -> list[Need]:
    """The needs of one call, in the order the owner reads them. `kind` is the physical kind; `declared` every kind a
    declared purpose or effect adds (SEND / CALL / PAY / ACCOUNT_SECURITY / DELETE_PERMANENT; one or several), which
    only widens what is checked. `arg_sources` is provenance.arg_sources of the target; `known(kind, value)` says
    whether a destination is known for normal use, `approved(kind, value)` whether the owner approved it (the sink rule's
    test, GATE-205; `known` when not given); `outside_read` is the outside content the tree read (origin labels), named in the
    sink question; `place` is where the call happens as the owner knows it (an app's name), else the target's site;
    `content` is what a send carries, sized in the bulk question. `own()` gives the owner's own email addresses and
    phone numbers (GATE-538), read only when S4(b) would ask: one of them is never a value found only outside
    (GATE-556, the one definition `own_contact` the form sink uses too). A permanent delete of a folder (S4(c)) needs the
    folder counted: the gateway adds it (`bulk_delete`)."""
    kinds = {kind} | ({declared} if isinstance(declared, str) else set(declared or ()))
    approved = approved or known
    who = _who(target)
    to_site = site_is_destination(kinds, (k for k, _ in who))      # inside `if sends` below: exactly `not who`
    where = place or target.site or ""
    needs: list[Need] = []
    for k in (Kind.PAY, Kind.ACCOUNT_SECURITY):
        if k in kinds:
            rule = ALWAYS[k]
            shown = {"amount": shown_amount(target.amount, target.currency) or
                     (UNREAD_AMOUNT if k == Kind.PAY else ""), "site": _host(where),
                     "account": target.account or "",
                     "recipients": ", ".join(v for _, v in who)}
            # one run per Yes (SQ1). A payment in a task's browser tab is asked once per payment instead, from the
            # page's amount and merchant (ferb/policy/payments.py, GATE-204): the gateway replaces this need
            needs.append(Need(rule, key(rule, target.site, target.account, target.amount,
                                        sorted(v for _, v in who)), shown, always=True))
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
                    and not approved("host", target.site)):    # S4 (a) for a post on a page, as for a send-class call
                needs.append(Need("sink:new_destination", key("sink:new_destination", target.site),
                                  {"destination": target.site, "read": shown_origins(outside_read)},
                                  destinations=(("host", target.site),)))
        elif (to_site and read_private and read_external
              and not _owner(arg_sources.get("site", [])) and not approved("host", target.site or "")):
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
    off = (tls.certificate_checks_off(content) if kind == Kind.RUN_SHELL else
           tls.applescript_checks_off(content) if kind == Kind.APPLESCRIPT else None) if content else None
    if off is not None:
        # L39: the command's own words (an AppleScript's `do shell script` too) turn certificate checks off; one answer
        # covers this exact command or script in the tree
        needs.append(Need(TLS_OFF, key(TLS_OFF, content), {"command": " ".join((content or "").split()),
                                                          "hosts": " or ".join(off.hosts), "how": ", ".join(off.how)}))
    # A payment's amount is shown and asked in its own question (one Yes per payment): a merchant's price always comes
    # from its page, so a separate "came from outside content" question only repeated it (live 2026-10-27)
    values = who + ([("amount", str(target.amount))] if target.amount is not None and Kind.PAY not in kinds else [])
    mine: set[str] | None = None
    for k, value in values:                     # S4 (b): a value that came only from outside content
        outside = [s for s in arg_sources.get(k, []) if s != "unknown"]
        if not outside or _owner(outside) or origin.PRIVATE in outside:   # GATE-508: also in the owner's own data
            continue
        if k != "amount":                       # GATE-538, GATE-556: the owner's own address or number
            mine = set(own()) if mine is None else mine
            if own_contact(value, mine):
                continue
        dest = None if k == "amount" else destination_of(value)
        if dest is not None and known(*dest):
            continue
        what = "amount" if k == "amount" else ("recipient" if sends else "typed")
        needs.append(Need("sink:outside_value", key("sink:outside_value", k, value),
                          {"what": what, "value": shown_amount(target.amount) if k == "amount" else value,
                           "origin": shown_origins(outside), "places": shown_places(outside),
                           "site": _host(where)},
                          destinations=(dest,) if dest is not None else ()))
    return [replace(n, reaches=target) for n in needs]


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
                 "covers": "floor:delete_permanent"}, reaches=Target(path=path))


def _terminal(v: dict[str, str]) -> str:
    """A terminal input as what FERB would do, from the input's facts (D31): the exact text, keys or clipboard text,
    where, and whether the Yes also covers its Return."""
    where = (f"the terminal {v['label']}" if v.get("label") else "the terminal") + f" in {v['app']}" + \
        (f" (window {v['window']})" if v.get("window") else "")
    if v.get("runs"):
        if v.get("selection"):
            facts = [f"the editor's selection is {v['selection']}"
                     + (f" ({v['selection_size']}, as FERB read it)" if v.get("selection_size") else "")]
        else:
            facts = [f"the selection or current line in {v['file'] or 'the active editor'}, which FERB can't read"]
        if v.get("file") and v.get("selection"):
            facts.append(f"the active file is {v['file']}")
        kind = "menu" if v.get("menu") else "control"
        where = v.get("runs_where") or v["app"]
        opens = (", and a terminal it opens may run its profile's startup command"
                 if where.endswith("terminal") else "")
        pressed = v.get('menu') or v.get('control')
        pressed = f"{v['keys']} ({pressed})" if v.get("keys") else pressed      # a key that presses it (round 3, L1)
        return (f"press {pressed} in {v['app']} (this {kind} runs code in {where}{opens}"
                + "; " + ", ".join(facts) + ")")
    if v.get("script"):
        said = (f"run the AppleScript {v['script']} in {v['app']} (it runs with that app's own access, outside "
                f"FERB's sandbox: {v['surface']})")
        named = "; ".join(f"it names {files}" for files in (v.get("files"), v.get("named")) if files)
        if named:                                    # each file it names, said plainly (round 4, N7)
            said += f"; {named}" + ("; FERB can't see any other file it opens" if v.get("opens") else "")
        elif v.get("opens"):                         # a file named only indirectly (round 3, O1)
            said += "; FERB can't see which file this opens, if it opens one"
        return said
    if v.get("files"):
        return f"open {v['files']} (in {v['app']})"
    if v.get("menu"):
        return f"press {v['menu']} in {where} (a terminal app's menu command can type into its terminal)"
    if v.get("secret"):
        return f"fill your secret {v['secret']} (FERB never shows it) into {where}"
    if v.get("drag"):
        dragged = v["text"] or "something FERB can't read"
        return f"drag {dragged} onto {where} (dropped text goes into the terminal)"
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


def ai_amount(v: dict[str, str]) -> str:
    """GATE-578: when code read no amount on the page, the amount the AI stated, plainly; "" otherwise (an amount code
    read always stands alone)."""
    read = v.get("amount", "")
    if not v.get("declared") or (read and not read.startswith(UNREAD_AMOUNT)):
        return ""
    return v["declared"]


def unseen_line(v: dict[str, str]) -> str:
    """GATE-578: code's check of the AI's amount failed (the page shows no such money amount): the card's one line."""
    amount = ai_amount(v)
    return f"FERB couldn't find {amount} on this page." if amount and v.get("unseen") else ""


def _pays(verb: str, v: dict[str, str]) -> str:
    """A payment as the owner reads it (GATE-204): the amount and merchant code read on the page (never the
    worker's number; "an amount FERB couldn't read" when none, or GATE-578's AI's amount, plainly), where, from which
    account; on a bank page that shows another payment than the one approved, both."""
    to = v.get("merchant") or v.get("recipients") or ""
    stated = ai_amount(v)
    text = (f"{verb} {stated or v.get('amount') or UNREAD_AMOUNT}" + (f" to {to}" if to else "")
            + (f" on {v['site']}" if v.get("site") else "") + (f" from {v['account']}" if v.get("account") else ""))
    if unseen := unseen_line(v):
        text += f" ({unseen[:-1]})"
    if v.get("approved"):
        bank = f" on {v['bank']}" if v.get("bank") else ""
        text += f" (the bank page{bank} shows this, but you approved {v['approved']})"
    return text


def _tls(v: dict[str, str]) -> str:
    """FERB's certificate question (L39, D33: code-written): the exact command and the site whose check it skips."""
    site = f"the site is really {v['hosts']}" if v.get("hosts") else "each site it reaches is really that site"
    return TLS_QUESTION.format(command=v["command"], site=site)


def _part(n: Need, alongside: frozenset[str] = frozenset()) -> str:
    """One need in the owner's terms; `alongside` holds the rules of the other needs in the same question."""
    v = n.values
    if n.rule == TLS_OFF:
        return _tls(v)
    if n.rule == FOREGROUND and v.get("script"):
        return (f"this would run a script in {v['app']}, your own browser, which can act in any of its windows and "
                f"tabs; one Yes covers {v['app']} for the rest of this task, and a No keeps FERB to its own tabs")
    if n.rule == FOREGROUND:
        return (f"this would bring {v['app']} to the front and use your screen; one Yes covers {v['app']} for the rest "
                "of this task, and a No keeps FERB in the background")
    if n.rule == BROWSER_PAGE:
        return (f"this would act on {v['app']}'s own page {v['page']} (its settings or extensions); FERB asks before "
                "every input there, and this Yes covers this one input only")
    if n.rule == PAIR:
        return (f"this pairs your {v['browser']} profile “{v['name']}”{_account(v)} with FERB, so FERB can open tabs "
                "and work in that profile's signed-in accounts; its pairing page opens there and closes itself")
    if n.rule == TERMINAL_INPUT:
        return (f"this would {_terminal(v)}; FERB counts it a terminal because it is {v['surface']}. FERB asks "
                "before every input into a terminal, and this Yes covers this one input only")
    if n.rule == "floor:pay":
        return _pays("this pays", v)
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
    if n.rule == "sink:new_destination" and v.get("form"):
        return (f"this sends the page's answers to {v['destination']}{_clicking(v)}, a place you haven't named: "
                f"{v.get('sent') or 'your own data'}" + (f"; this task read {v['read']}" if v.get("read") else ""))
    if n.rule == "sink:new_destination":
        return (f"this sends to {v['destination']}, a place you haven't named, after this task read your own data and "
                f"outside content ({v['read']})")
    if n.rule == "sink:private_url":
        read = f" ({v['read']})" if v.get("read") else ""
        return (f"this would open {v['destination']}, a place you haven't named, and send it {_carried(v)} in the "
                f"address, after this task read outside content{read}; saying yes lets FERB reach "
                f"{v['destination']} from the browser, now and in later tasks")
    if n.rule == "sink:outside_value" and v["what"] == "typed":
        return (f"this types {v['value']}, which FERB found only in outside content ({v['origin']}), not in your words, "
                "your answers or your own files")
    if n.rule == "sink:outside_value":
        return (f"FERB found the {v['what']} {v['value']} only in outside content ({v['origin']}), not in your words, "
                "your answers or your own files")
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


def shown_sent(sent: list[tuple[str, str]], total: int) -> str:
    """GATE-557: the values a form would send, each with where this task read it (`value` from `where`), the rest
    counted. Display only."""
    parts = [f"{shown_quoted(value)} (from {where})" for value, where in sent]
    more = total - len(sent)
    return "; ".join(parts) + (f"; and {more} more" if more > 0 else "")


def _clicking(v: dict[str, str]) -> str:
    """What the click is, as the page labels the control (GATE-557: page words, quoted), else nothing."""
    return f" by clicking {shown_quoted(v['control'])}" if v.get("control") else ""


def _carried(v: dict[str, str]) -> str:
    """Which of the owner's values a URL would send, and why FERB counts it as theirs (GATE-507): the value, the piece
    of the address it rides in when that differs, and where this task read it."""
    value, matched = v.get("value", ""), v.get("matched") or v.get("value", "")
    same = matched.casefold() == value.casefold()
    inside = "" if same else f" (inside {shown_quoted(value)})"
    where = v.get("source") or "your own data"
    return f"{shown_quoted(value if same else matched)}{inside}, which this task read from {where}"


_LIFTED = {   # what an escape lifts, in the owner's terms (D31): the protection list's kinds (protection.rules)
    # GATE-200: a read covers the task (file tools and shell alike); a change one call or one command
    "read": "read {path} for this task (normally off-limits: a private store FERB stays out of)",
    "write": "change {path} once (normally off-limits: FERB never changes it on its own)",
    "admin": ("run one command as administrator (sudo), with your Mac password (normally off-limits: FERB's shell "
              "never uses admin rights)"),
    "service": ("let one command use macOS's certificate service, trustd (normally off: it reaches certificate "
                "addresses outside FERB's proxy)"),
}


def _short(n: Need, alongside: frozenset[str]) -> str | None:
    """One need as what FERB wants to do, a verb phrase from the need's facts only (never the step or the task's
    reason); None when another need of the same question already says it."""
    v = n.values
    if n.rule == TLS_OFF:
        return _tls(v)
    if n.rule == FOREGROUND and v.get("script"):
        return f"run scripts in {v['app']}, your own browser, for this task"
    if n.rule == FOREGROUND:
        return f"use your screen in {v['app']} (it comes to the front) for this task"
    if n.rule == BROWSER_PAGE:
        return f"change something on {v['app']}'s own page {v['page']} (its settings or extensions)"
    if n.rule == PAIR:
        return f"pair your {v['browser']} profile “{v['name']}”{_account(v)} with FERB, so it can work in that profile"
    if n.rule == TERMINAL_INPUT:
        return _terminal(v)
    if n.rule == "floor:pay":
        return _pays("pay", v)
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
    if n.rule == "sink:new_destination" and v.get("form"):
        return f"send {v.get('sent') or 'your own data'} to {v['destination']}{_clicking(v)}"
    if n.rule == "sink:new_destination":
        return (f"send to {v['destination']}, a place you haven't named, after reading "
                f"{v['read'] or 'outside content'}")
    if n.rule == "sink:private_url":
        return f"open {v['destination']}, sending it {_carried(v)}"
    if n.rule == "sink:outside_value" and v["what"] == "typed":
        return f"type {v['value']}, which FERB found only in {v['origin']}"
    if n.rule == "sink:outside_value":
        return f"use the {v['what']} {v['value']}, which FERB found only in {v['origin']}"
    if n.rule == BULK and v.get("what") == "delete":
        return f"permanently delete {v['path']}: {v['files']} in {v['folders']}, {v['bytes']}"
    if n.rule == BULK and "floor:new_recipient" in alongside:      # the new recipients are named beside it
        return None
    if n.rule == BULK:
        return f"{'call' if v.get('what') == 'call' else 'send to'} {v['count']} recipients at once: {v['sample']}"
    if n.rule.startswith("escape:") and v.get("lift") in _LIFTED:
        return _LIFTED[v["lift"]].format(path=v.get("path", ""))
    return v.get("text", n.rule)                # a rule another part of FERB asks about in its own words


def summary(needs: list[Need], typed: str = "") -> str:
    """FERB's question, the one line its card shows (P27; owner 2026-10-27: "just ask simply what you want with a small
    reason below"; GATE-225: one direct question, no Details): what FERB wants to do, from the needs' facts, carrying
    what decides what will happen (the app and how long the Yes lasts, the exact command, amount and merchant, path and
    count, recipients and the text last typed where it sends (`typed`, quoted and clipped by the caller), the profile,
    a value and its source). The task's reason goes on its own smaller line (`reason_line`); the full question with the
    step (`line`) stays the approval's reason and the owner.ask span's record, never on the card. Display only: it
    decides nothing."""
    said = {n.values.get("covers") for n in needs if n.rule == BULK}
    rules = frozenset(n.rule for n in needs)
    if rules == {TLS_OFF}:                      # L39: the question as the owner reads it, already a question
        return "; ".join(_tls(n.values) for n in needs) + "."
    parts = [p for n in needs if n.rule not in said and (p := _short(n, rules))]
    return "FERB wants to " + "; ".join(parts) + "." + (f' Text last typed there: "{typed}".' if typed else "")


@dataclass(frozen=True)
class Card:
    """FERB's question as its card shows it (GATE-513, refining GATE-225): the question in plain words, why FERB asks
    (one line, "" when the question says it), and the exact thing the owner approves (the command, address, value,
    path or amount; "" when there is none), shown apart in a code box. GATE-566 (owner): the reviewed kinds' words, and
    their buttons' action words (`yes`, `no`: what the app shows for Yes and No; "" keeps "Yes"/"No"); `aside` is the
    card's own sentence when it is folded under another card, `clause` its fact folded into a terminal command's
    sentence (`tail` marks a card that takes them: the detail, the clauses, then `tail`). Display only: it decides
    nothing; the answer is still Yes or No."""
    question: str
    detail: str = ""
    exact: str = ""
    yes: str = ""
    no: str = ""
    aside: str = ""
    clause: str = ""
    tail: str = ""


def _bare(text: str) -> str:
    """A value for the code box: a JSON-quoted value without its outer quotes when nothing inside needs them (no space
    at either end), so invisible characters stay as their visible escapes (terminal.visible)."""
    if len(text) >= 2 and text[0] == text[-1] == '"' and text[1:-1] == text[1:-1].strip() and text[1:-1]:
        return text[1:-1]
    return text


def _place_of(source: str) -> str:
    """Where a value was read, as the owner knows it: the site after "on" in the gateway's `source`, else the source."""
    return source.rsplit(" on ", 1)[1] if " on " in source else source


def _unquoted(text: str) -> str:
    """A JSON-quoted value from the needs' facts (terminal.visible, `shown_quoted`) as plain text for a code box, each
    line apart (the gateway's card strips control characters line by line); as written when it isn't quoted."""
    try:
        value = json.loads(text) if text[:1] == '"' else text
    except ValueError:
        return text
    return value if isinstance(value, str) else text


def _word(text: str) -> str:
    """A value inside a card's sentence: one line, clipped, in curly quotes (never a JSON escape)."""
    one = " ".join(_unquoted(text).split())
    return "“" + (one if len(one) <= QUOTED_CHARS else one[:QUOTED_CHARS - 1] + "…") + "”"


def _in_terminal(v: dict[str, str]) -> str:
    """The terminal a card names: the app's terminal ("VS Code's terminal"); the window and label stay in the record."""
    return f"{v.get('app') or 'the app'}'s terminal"


def _which(v: dict[str, str]) -> str:
    """Which terminal of the app it is (its label, its window), a sentence after the card's own words: the app may
    hold several terminals, the owner's own among them."""
    parts = [f"terminal {v['label']}" if v.get("label") else "", f"window {v['window']}" if v.get("window") else ""]
    said = ", ".join(p for p in parts if p)
    return f" It's {v.get('app') or 'the app'}'s {said}." if said else ""


def _terminal_card(v: dict[str, str]) -> Card | None:
    """GATE-566: every terminal input as one question naming the app, the exact input in the box, and that a Yes
    covers this one input (D31: asked every time). A code editor's run control, an AppleScript, a file that runs when
    opened, a terminal app's menu, a dragged text and a saved secret have their own words."""
    app = v.get("app") or "the app"
    where, which = _in_terminal(v), _which(v)
    if v.get("runs"):                                  # a code editor's run control or menu item
        pressed = v.get("menu") or v.get("control") or "this"
        pressed = f"{v['keys']} ({pressed})" if v.get("keys") else pressed
        if v.get("selection"):
            what, unread = "the selected text", ""
        else:
            what = f"the selection or current line in {v['file']}" if v.get("file") else \
                "the selection or current line"
            unread = "; FERB can't read what it runs"
        return Card(f"Press {pressed} in {app}? It runs code", f"It runs {what} in {v.get('runs_where') or app} with "
                    f"your full access{unread}. Yes covers only this.", _unquoted(v.get("selection", "")),
                    "Do it", "Don't")
    if v.get("script"):                                # an AppleScript to a terminal-like app (Finder, System Events)
        named = "; ".join(f for f in (v.get("names", ""),) if f)
        files = (f" Files it names: {named}" + ("; FERB can't see others it may open" if v.get("opens") else "")
                 + ".") if named else (" FERB can't see which file this opens, if it opens one." if v.get("opens")
                                       else "")
        return Card(f"Let {app} run this script?", f"{app} acts with its own access, outside FERB's safety limits."
                    f"{files} Yes covers only this script.", _unquoted(v["script"]), "Run script", "Don't run")
    if v.get("files"):                                 # a file that runs when it is opened
        path = _unquoted(v.get("open_path", "")) or "this file"
        name = path.rstrip("/").rsplit("/", 1)[-1] or path
        return Card(f"Open {name}? It runs when opened", f"Opening it in {app} runs what's inside with your access. "
                    "Its contents are below. Yes covers these exact contents.",
                    "\n".join(p for p in (path, _unquoted(v.get("open_text", ""))) if p), "Open it", "Don't open")
    if v.get("menu"):
        return Card(f"Press {_unquoted(v['menu'])} in {app}? It can type into its terminal",
                    "A terminal app's menu command can type into its terminal, which runs with your full access. Yes "
                    "covers only this.", yes="Do it", no="Don't")
    if v.get("secret"):
        return Card(f"Fill your secret {_word(v['secret'])} into {where}?", "FERB never shows it. Text in a terminal "
                    "can run with your full access. Yes covers only this." + which, yes="Fill it in", no="Don't")
    if v.get("drag"):
        return Card(f"Drag this onto {where}?", "Dropped text goes into the terminal, where it can run with your full "
                    "access. Yes covers only this." + which, _unquoted(v.get("text", "")), "Drag it", "Don't")
    if v.get("paste"):
        then = " and run it" if v.get("enter") else ""
        if v.get("concealed"):
            return Card(f"Paste a secret item into {where}{then}?", "An app marked it secret, so FERB doesn't show it "
                        "here. Pasted text can run with your full access. Yes covers only this paste." + which,
                        yes="Paste it", no="Don't paste")
        return Card(f"Paste this into {where}{then}?", "Pasted text can run with your full access. Yes covers only this "
                    "paste." + which, _bare(v.get("text", "")), "Paste it", "Don't paste")
    if v.get("text"):
        if v.get("enter"):
            return Card(f"Run this command in {app}?", "It runs with full access to your Mac and files",
                        _bare(v["text"]), "Run it", "Don't run", tail=". Yes covers only this command." + which)
        return Card(f"Type this into {where}?", "FERB won't press Return. If Return gets pressed, it runs with your full "
                    "access. Yes covers only this typing." + which, _bare(v["text"]), "Type it", "Don't type")
    if v.get("keys") in ("return", "enter"):
        return Card(f"Press Return in {where}?", "That runs whatever is on its command line now. FERB didn't type it and "
                    "can't show it to you." + which, yes="Press Return", no="Don't")
    if v.get("keys"):
        return Card(f"Press {v['keys']} in {where}?", "Yes covers only this key press." + which,
                    yes=f"Press {v['keys']}", no="Don't")
    return None


def _source_words(source: str) -> str:
    """Where this task read a value of the owner's, in the owner's terms (GATE-566): the gateway's plain words for it
    (`source_plain`), else "your own data"."""
    return source or "your own data"


def _page_name(page: str) -> str:
    """A browser's own page by its address (GATE-566): the address's first label, capitalized (chrome://extensions →
    Extensions, chrome://settings/passwords → Settings); an extension's page is "extension"."""
    scheme, _, rest = page.strip().partition(":")
    if scheme.casefold().endswith("extension"):
        return "extension"
    first = rest.lstrip("/").split("/", 1)[0].split("?", 1)[0].split("#", 1)[0]
    return first[:1].upper() + first[1:] if first else "settings"


def _card(n: Need, alongside: frozenset[str]) -> Card | None:
    """One need's card, from its facts only; None when another need of the same question already says it."""
    v = n.values
    if n.rule == BULK and "floor:new_recipient" in alongside:      # the new recipients are named beside it
        return None
    if n.rule == "sink:private_url":
        dest = v.get("destination", "")
        value = v.get("matched") or v.get("value", "")
        piece = v.get("value", "")
        inside = f" (inside {_word(piece)})" if piece and piece.casefold() != value.casefold() else ""
        return Card(f"Open {dest}? The link includes {_word(value)}",
                    f"FERB treats {_word(value)}{inside} as yours because it read it in "
                    f"{_source_words(v.get('source_plain', ''))}. Opening this link shows it to {dest}. Yes also lets "
                    f"FERB open {dest} in later tasks.", v.get("url") or dest, "Open it", "Don't open")
    if n.rule == "sink:new_destination" and v.get("form"):
        # GATE-557 (owner GATE-558): the values and where each was read, the site, what the click does; one Yes per
        # site per task (no destination is stored, so the card says this task)
        dest = v.get("destination", "")
        values = [x for x in v.get("values", "").split("\n") if x] or [_unquoted(v.get("value", ""))]
        sources = v.get("sources_plain") or "your own data"
        which = "the values below" if len(values) > 1 else "the value below"
        click = f"Clicking {_word(v['control'])}" if v.get("control") else "This click"
        found = f"; it found it {v['found_via']}" if v.get("found_via") else ""
        more = f" (and {v['more']} more)" if v.get("more") else ""
        return Card(f"Send your details to {dest}?",
                    f"{click} submits the form to {dest}, including {which}{more} from {sources}. You haven't named "
                    f"this site to FERB{found}. Yes covers {dest} for this task.", "\n".join(values), "Send",
                    "Don't send")
    if n.rule == "sink:new_destination":
        dest = v.get("destination", "")
        return Card(f"Send your details to {dest}?", f"This sends to {dest}, and this task has read your own data and "
                    f"outside content. You haven't named this site to FERB. Yes also lets FERB send to {dest} in later "
                    "tasks.", dest, "Send", "Don't send")
    if n.rule == "sink:outside_value":
        what = v.get("what", "value")
        value = v.get("value", "")
        places = v.get("places") or v.get("origin") or "something FERB read"
        later = bool(n.destinations)
        if what == "typed":
            kind = value_kind(value)
            on = f" on {v['site']}" if v.get("site") else ""
            return Card(f"Type this {kind}{on}?", f"You didn't give FERB this {kind}; it only saw it on {places}. A page "
                        "can plant an address to get your data, so check it's right."
                        + (" Yes also lets FERB use it again later." if later else ""), value, "Type it",
                        "Don't type it")
        if what == "recipient":
            said = (f"You never gave FERB this address; it came from {places}. A page can plant an address to get "
                    "your data.")
            title = f"Send to {value}?" if len(value) <= 40 else "Send to this address?"
            return Card(title, said + (" Yes also lets FERB send to it later." if later else ""), value, "Send",
                        "Don't send", aside=said)
        return Card(f"Use {value}, from {places}?", f"You didn't give FERB this amount; it was on {places}. Check it's "
                    "the amount you want.", value, "Use it", "Don't")
    if n.rule == "floor:new_recipient":
        calls = v.get("verb") == "calls"
        verb, act = ("Start a call", "starts a call") if calls else ("Send", "sends")
        if not v.get("recipients"):
            place = v.get("place", "")
            account = f" as {v['account']}" if v.get("account") else ""
            return Card(f"{verb}{' in ' + place if place else ''} without seeing who gets it?",
                        f"This button {act}{account}, but FERB can't tell to whom. Say {'Call' if calls else 'Send'} "
                        f"only if that's what you asked for. Yes covers {place or 'it'} for this task.",
                        yes="Call" if calls else "Send", no="Don't call" if calls else "Don't send")
        people = v["recipients"]
        them = "them" if "," in people else "this person"
        return Card(f"{'Call' if calls else 'Send to'} {people} for the first time?",
                    f"You haven't asked FERB to contact {them} before; check it's who you meant. Yes also lets FERB "
                    f"{'call' if calls else 'send to'} {them} in later tasks.",
                    f"{'Call' if calls else 'To'}: {people}" + (f" · in {v['place']}" if v.get("place") else "")
                    + (f" · from {v['account']}" if v.get("account") else ""),
                    "Call" if calls else "Send", "Don't call" if calls else "Don't send")
    if n.rule == "floor:pay":
        amount = v.get("amount") or UNREAD_AMOUNT
        to = v.get("merchant") or v.get("recipients") or ""
        detail = "Money leaves your account. Yes covers only this payment."
        if v.get("approved"):
            detail = (f"This is different from what you approved ({v['approved']}): the bank page"
                      f"{' on ' + v['bank'] if v.get('bank') else ''} shows {amount}. " + detail)
        stated = ai_amount(v)                         # GATE-578: none read, the AI's stated amount, plainly
        shown = stated or amount
        exact = " · ".join(p for p in (f"{shown} → {to}" if to else shown, v.get("site", ""),
                                       f"from {v['account']}" if v.get("account") else "") if p)
        if stated:                                    # code's check failed: one plain line under the question
            unseen = unseen_line(v)
            return Card(f"Pay {stated}{' to ' + to if to else ''}?", f"{unseen} {detail}" if unseen else detail,
                        exact, "Pay" if unseen else f"Pay {stated}", "Don't pay")
        if not v.get("amount") or amount.startswith(UNREAD_AMOUNT):     # GATE-204: no amount read
            return Card(f"Pay{' ' + to if to else ''}? FERB couldn't read the amount", detail, exact, "Pay", "Don't pay")
        return Card(f"Pay {amount}{' to ' + to if to else ''}?", detail, exact, f"Pay {amount}", "Don't pay")
    if n.rule == "floor:account_security":
        account = v.get("account") or "your account"
        return Card("Change sign-in security" + (f" on {v['site']}" if v.get("site") else "") + "?",
                    f"This changes how you sign in to {account} (password, two-factor or recovery) or deletes the "
                    "account; a mistake can lock you out. Yes covers this one change.",
                    " · ".join(p for p in (v.get("account", ""), v.get("site", "")) if p), "Change it", "Don't change")
    if n.rule == "floor:delete_permanent":             # not reviewed (GATE-566): today's words; its fact folds in
        detail = f"It's on {v['place']}, and it can't be undone." if v.get("place") else \
            "It skips the Trash and can't be undone."
        return Card("Delete this for good?", detail, v.get("path", ""),
                    clause=f"it permanently deletes {v.get('path') or 'something'}")
    if n.rule == BULK and v.get("what") == "delete":
        path = v.get("path", "")
        name = path.rstrip("/").rsplit("/", 1)[-1] or path or "this folder"
        return Card(f"Delete {name} and all {v.get('files', 'the files')} in it?",
                    f"{v.get('files', '')} in {v.get('folders', '')}, {v.get('bytes', '')}"
                    + (f", e.g. {v['sample']}" if v.get("sample") else "")
                    + ". They skip the Trash, so you can't get them back.", path, "Delete all", "Keep them")
    if n.rule == BULK:
        calls = v.get("what") == "call"
        count = v.get("count", "several")
        return Card(f"{'Call' if calls else 'Send to'} {count} people at once?",
                    "You didn't name them" + ("." if calls else f", and once {count} people have it you can't take it "
                                              "back."), v.get("sample", ""),
                    f"{'Call' if calls else 'Send to'} all {count}", "Don't call" if calls else "Don't send")
    if n.rule == TERMINAL_INPUT:
        return _terminal_card(v)
    if n.rule.startswith("escape:") and v.get("lift") == "read":
        path = v.get("path", "this folder")
        return Card(f"Let FERB read {path}?", "It's on your private list, so FERB normally stays out. Yes lets it read "
                    "there (not change anything) until this task ends.", v.get("path", ""), "Allow reading",
                    "Keep out")
    if n.rule.startswith("escape:") and v.get("lift") == "write":
        return Card(f"Let FERB change {v.get('path', 'this')} once?", "It's on your do-not-change list. Yes covers only "
                    "this one change.", "\n".join(p for p in (v.get("path", ""), v.get("command", "")) if p),
                    "Change it once", "Don't")
    if n.rule.startswith("escape:") and v.get("lift") == "admin":
        return Card("Run this command as administrator?", "Admin commands can change anything on your Mac. Next, macOS "
                    "needs your Mac password. Yes covers only this command.", v.get("command", ""), "Run as admin",
                    "Don't")
    if n.rule.startswith("escape:") and v.get("lift") == "service":
        return Card("Let this command check certificates directly?", "macOS will contact certificate servers outside "
                    "FERB's network filter for it. Yes covers only this command.", v.get("command", ""), "Allow once",
                    "Don't")
    if n.rule == FOREGROUND and v.get("script"):
        app = v.get("app", "your browser")
        return Card(f"Let FERB run a script in {app}?",
                    f"A script can read and act in every {app} window and tab, including sites you're signed in to. "
                    f"Yes covers scripts in {app} until this task ends.", v.get("script_text", ""), "Allow scripts",
                    "Don't")
    if n.rule == FOREGROUND:                           # not reviewed (GATE-566): today's words; its fact folds in
        app = v.get("app", "the app")
        return Card(f"Let FERB use your screen in {app} for this task?",
                    f"{app} will come to the front while FERB works in it. One Yes covers {app} until this task ends.",
                    clause=f"{app} comes to the front while FERB types")
    if n.rule == BROWSER_PAGE:
        app, page = v.get("app", "the browser"), v.get("page", "")
        return Card(f"{v.get('verb') or 'Click'} on {app}'s {_page_name(page)} page?",
                    f"It's {app}'s own settings area, where one change affects all your browsing (extensions, "
                    "passwords, privacy). Yes covers this one action.", page, "Allow", "Don't")
    if n.rule == PAIR:
        browser, name, account = v.get("browser", ""), v.get("name", ""), v.get("email", "")
        return Card(f"Let FERB work in your {browser} “{name}” profile?",
                    "FERB could then open tabs there and act as you on sites you're signed in to"
                    + (f" ({account})" if account else "") + ". A pairing page opens and closes itself.",
                    " · ".join(p for p in (browser, name, account) if p), "Pair it", "Don't pair")
    if n.rule == TLS_OFF:
        whom = v["hosts"] if v.get("hosts") else "each site it reaches"
        return Card("Run this without checking the site is real?",
                    f"It skips the check that it's really talking to {whom}, so someone on your network could swap "
                    "what it downloads. Yes covers only this command.", v.get("command", ""), "Run it", "Don't run")
    return None


def card(needs: list[Need], typed: str = "") -> Card:
    """FERB's question card (GATE-513), built from the needs' facts like `summary`: the first need's question and its
    buttons' words, the others' sentences after its reason (a terminal command takes their facts into its own
    sentence, GATE-566), and every exact part, one per line. A need with no card of its own keeps today's sentence
    (`_short`, else its own words), so every question has words; `typed` (the text last typed where it sends, quoted
    and clipped by the caller) closes the reason."""
    rules = frozenset(n.rule for n in needs)
    said = {n.values.get("covers") for n in needs if n.rule == BULK}
    cards: list[Card] = []
    # typing into a terminal leads: its exact command is what the Yes runs (rule 6), whatever else it also covers
    for n in sorted(needs, key=lambda need: need.rule != TERMINAL_INPUT):
        if n.rule in said:
            continue
        # every card names an app by its name, never its bundle id (the values here are display only)
        named = {k: app_label(v) for k, v in n.values.items() if k in ("app", "place") and isinstance(v, str)}
        if any(named[k] != n.values[k] for k in named):
            n = replace(n, values={**n.values, **named})
        made = _card(n, rules)
        if made is None:
            try:
                short = _short(n, rules)
            except (KeyError, TypeError, ValueError):     # a fact its sentence needs is missing: still a question
                short = None if n.rule == BULK and "floor:new_recipient" in rules else "go ahead with this step"
            if short is None:
                continue
            made = Card("FERB wants to " + short + ".")
        cards.append(made)
    if not cards:                               # every need deferred to another: today's line, never an empty card
        return Card(summary(needs, typed))
    # (`card` itself never raises for a missing fact; the gateway still keeps today's line if anything else does)
    first, rest = cards[0], cards[1:]
    lead = first.detail
    if first.tail:                              # a terminal command: the other needs' facts, as clauses of its sentence
        lead += "".join("; " + c.clause for c in rest if c.clause) + first.tail
        rest = [c for c in rest if not c.clause]
    detail = " ".join(p for p in [lead, *(c.aside or c.question + (" " + c.detail if c.detail else "") for c in rest),
                                  f'Text last typed there: "{typed}".' if typed else ""] if p)
    exact = "\n".join(c.exact for c in cards if c.exact)
    return Card(first.question, detail, exact, first.yes, first.no)

def reason_line(why: str) -> str:
    """The task's reason as the small line under a question: its own words, a line that reads like FERB's own check
    marked as the task's (D33); empty when it gave none. The caller has redacted and clipped it."""
    return from_task_lines(" ".join(why.split())) if why.strip() else ""


def line(needs: list[Need], call: str, why: str, typed: str = "") -> str:
    """FERB's own words, first and labelled (P27: second person, the concrete action), then the call, the text last
    typed on that page (`typed`: what the worker typed there, likely what it sends; display only, L3), the reason the
    task gave (quoted)
    and the question. The gateway collapses whitespace, strips control characters, maps the double quotes of the text
    and the reason to single ones and clips what goes in."""
    said = {n.values.get("covers") for n in needs if n.rule == BULK}   # a bulk delete already says the floor's words
    rules = frozenset(n.rule for n in needs)
    if rules == {TLS_OFF}:                      # L39: the exact command is in the question, which asks it
        return "FERB: " + "; ".join(_tls(n.values) for n in needs) + "." + (f' Reason given: "{why}".' if why else "")
    parts = "; ".join(_part(n, rules) for n in needs if n.rule not in said)
    text = "FERB: " + parts + (f". The step: {call}." if call else ".")
    if typed:
        text += f' Text last typed on this page: "{typed}".'
    if why:
        text += f' Reason given: "{why}".'
    return text + " Go ahead?"


FINE, TRAIL = _FINE, _TRAIL          # for stored provenance (carried.derive): the same splitting as the sink
