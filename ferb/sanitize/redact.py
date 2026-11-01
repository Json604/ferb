"""Own secret redaction before model, trace, log and database boundaries (rule 14: secrets, OTPs and card details
included, never reach a model provider, a trace, a log or the database). Must not use secret content to decide
actions; a gate asks only for a specific hit (`Redactor.hit`).

Three sources of spans, all found on the text as given and masked once: known values (known.py), formats a value shows
by its own shape (formats.py) and labelled classes (labels.py). The result is redacted again until nothing changes
(a mask can bring a code into its word's reach), so `text(text(x)) == text(x)` by construction; a mask is never split.
Every string the redactor returns is marked as redacted under the current known values, so a later sink skips it
(one pass per value); any edit of the string, or a new known value, drops the mark."""

import bisect
import re
from typing import Any, Iterable
from urllib.parse import urlsplit

from ferb.sanitize.formats import (_H, PRIVATE_KEY_END, PRIVATE_KEY_START, SECRET_PATTERNS, first_format,  # noqa: F401
                                   format_hits, holds_value, payment_card, secret_param, urls)
from ferb.sanitize.known import (REDACTED, Registry, SealedWord, escaped_forms, holds_sealed, id_bodies,  # noqa: F401
                                 known_spans, sealed, sealed_spans, word_char)
from ferb.sanitize.labels import (CARD_AUTOCOMPLETE, CODE_LINK, CODE_REACH, CODE_VALUE, CODE_WORD,  # noqa: F401
                                  FIELD_METADATA, INPUT_TYPES, INTERNAL_ID_KEYS, LINE_BREAK, OTP_LABEL,
                                  PASSWORD_AUTOCOMPLETE, code_alone, code_words, content_lines, hidden_answer,
                                  label_class, labelled_code, labelled_secret, labelled_spans, labelled_value,
                                  line_code_spans, list_line, password_field, secret_key, secret_words)

__all__ = ["CODE_LINK", "CODE_REACH", "CODE_VALUE", "CODE_WORD", "EXCLUDED", "INTERNAL_ID_KEYS", "NODE_TEXT",
           "OTP_LABEL", "PRIVATE_KEY_END", "PRIVATE_KEY_START", "REDACTED", "SAVED_SECRET", "SECRET_PATTERNS",
           "RedactionStream", "Redacted", "Redactor", "SealedWord", "_H", "_code_alone", "code_words", "escaped_forms",
           "first_format", "holds_sealed", "id_bodies", "label_class", "mask", "password_field", "payment_card",
           "sealed", "sealed_spans", "secret_key", "secret_param", "secret_words", "word_char"]

EXCLUDED = "[excluded]"
SAVED_SECRET = "saved secret"                  # `Redactor.kinds`' name for a known value: one of the owner's own
# A part `Redactor.kinds` names as the whole it belongs to, when that whole is in the same text (structural: a card's
# expiry and CVV are the card; a Twilio auth token comes with its account key). A card's part alone names no kind.
_PART_OF = {"card expiry": "payment card", "CVV": "payment card", "Twilio auth token": "Twilio key"}
_ONLY_A_PART = frozenset({"card expiry", "CVV"})
_MASKS = re.compile(r"\[redacted\]|\[excluded\]")
_DIGIT = re.compile(r"[0-9]")
_code_alone = code_alone                       # the OCR path's name for it
INPUT_TEXT_FIELDS = {"text", "unmodifiedText", "key", "code"}
# Where a page or accessibility node holds its visible text, in order of preference (sibling codes, C1).
NODE_TEXT = ("value", "text", "name", "label", "title", "content")
# Keys whose values are FERB's own ids: never read as a sibling line or masked by that reading (L29 re-review 5, V1).
_SIBLING_ID_KEYS = frozenset(INTERNAL_ID_KEYS | {"id", "intent_id", "episode", "evidence"})
_FIXED_POINT = 8                               # passes at most; two in practice (one masks, one reads around it)
_KEY_CACHE = 4096


class Redacted(str):
    """A string the redactor returned. Each Redactor makes its own subclass per set of known values and exclusions
    (`Redactor.marked`), so only a mark made under the current ones is trusted. Pickled as a plain string."""
    __slots__ = ()

    def __reduce__(self) -> tuple[Any, ...]:
        return (str, (str(self),))


def mask(s: str, spans: Iterable[tuple[int, int, str]]) -> str:
    """`s` with each span replaced; overlapping or adjacent spans as one (EXCLUDED wins), a mask already there never
    split."""
    found = sorted((a, b, rep) for a, b, rep in spans if b > a)
    if not found:
        return s
    masks = [m.span() for m in _MASKS.finditer(s)] if "[" in s else []
    starts = [a for a, _ in masks]
    out, kept, cur = [], 0, None
    for a, b, rep in found:
        if masks:                                   # a span reaching into a mask takes the whole mask
            i = bisect.bisect_right(starts, a) - 1
            if i >= 0 and masks[i][1] > a:
                a = masks[i][0]
            j = bisect.bisect_left(starts, b) - 1
            if j >= 0 and masks[j][1] > b:
                b = masks[j][1]
        if cur is not None and a <= cur[1]:        # overlapping or adjacent: one mask
            cur = (cur[0], max(cur[1], b), EXCLUDED if EXCLUDED in (cur[2], rep) else cur[2])
            continue
        if cur is not None:
            out.append(s[kept:cur[0]] + cur[2])
            kept = cur[1]
        cur = (a, b, rep)
    assert cur is not None
    out.append(s[kept:cur[0]] + cur[2])
    return "".join(out) + s[cur[1]:]


def _around_masks(s: str) -> list[tuple[int, int]]:
    """The stretches of `s` a later pass reads: each line holding a mask, with the lines that aren't blank just above
    and below it (and up a list to its label), merged."""
    found = [m.start() for m in _MASKS.finditer(s)]
    if not found:
        return []
    starts = [0] + [m.end() for m in LINE_BREAK.finditer(s)]
    lines = [(a, (starts[i + 1] if i + 1 < len(starts) else len(s))) for i, a in enumerate(starts)]

    def content(i: int) -> bool:
        return bool(s[lines[i][0]:lines[i][1]].strip())
    out: list[tuple[int, int]] = []
    for pos in found:
        i = bisect.bisect_right(starts, pos) - 1
        up, down = i - 1, i + 1
        while up > 0 and not content(up):
            up -= 1
        while up > 0 and list_line(s[lines[up][0]:lines[up][1]].rstrip("\r\n")):
            up -= 1
        while down < len(lines) - 1 and not content(down):
            down += 1
        a, b = lines[max(up, 0)][0], lines[min(down, len(lines) - 1)][1]
        if out and a <= out[-1][1]:
            out[-1] = (out[-1][0], max(out[-1][1], b))
        else:
            out.append((a, b))
    return out


class Redactor:
    def __init__(self, exclude_domains: set[str] | None = None, exclude_apps: set[str] | None = None) -> None:
        self.registry = Registry()
        self._config = 0
        self._mark_key: tuple[int, int] | None = None
        self._mark: type[Redacted] = Redacted
        self._keys: dict[str, str] = {}
        self.configure(exclude_domains, exclude_apps)

    # ---------- configuration and known values ----------

    def configure(self, exclude_domains: set[str] | None = None, exclude_apps: set[str] | None = None) -> None:
        """Change the exclusions in place: the journal, spans and gateway hold this instance, so it must stay one."""
        self.exclude_domains = {d.lower().lstrip(".") for d in (exclude_domains or set())}
        self.exclude_apps = {a.lower() for a in (exclude_apps or set())}
        self._config += 1

    def add_secret_value(self, value: str) -> None:
        """A value FERB handled (a fill it typed, a login code, a proxy token): hidden if a page or error echoes it."""
        self.registry.add(value)

    def remember_hidden_secret(self, value: str) -> None:
        """A sealed secret (a hidden input, a stored one revealed for a fill, a preloaded Keychain value), whatever its
        length. A typed answer to an ordinary question never comes here (L18)."""
        self.registry.remember(value)

    def preload(self, values: Iterable[str]) -> int:
        """The values read from FERB's Keychain handles and owner secrets at start; how many were added."""
        before = self.registry.lasting()
        for value in values:
            self.registry.remember(value)
        return self.registry.lasting() - before

    def forget_secret_value(self, value: str) -> None:
        self.registry.forget(value)

    def retain_secret(self, value: str) -> None:
        self.registry.retain(value)

    def release_secret(self, value: str) -> None:
        self.registry.release(value)

    def secret_values(self) -> frozenset[str]:
        """Every known value as kept (a `SealedWord` for an ordinary word), and its escaped forms."""
        return self.registry.values()

    def excluded_host(self, host: str) -> bool:
        """A host on the owner's exclusion list (a domain covers its subdomains)."""
        host = host.lower()
        return any(host == d or host.endswith("." + d) for d in self.exclude_domains)

    def stream(self) -> "RedactionStream":
        """A separate decoder and buffer for each process; raw partial secrets never leave it."""
        return RedactionStream(self)

    # ---------- the marker ----------

    def marked(self) -> type[Redacted]:
        """This redactor's mark under the current known values and exclusions."""
        key = (self.registry.version, self._config)
        if key != self._mark_key:
            self._mark = type("Redacted", (Redacted,), {"__slots__": ()})
            self._mark_key, self._keys = key, {}
        return self._mark

    def is_redacted(self, value: Any) -> bool:
        return type(value) is self.marked()

    # ---------- text ----------

    def spans(self, s: str, *, keyed: bool = False) -> list[tuple[int, int, str]]:
        """Every span to mask in `s`, found on `s` as given."""
        found = urls(s)
        out = [(a, b, REDACTED) for a, b in known_spans(s, self.registry.values(), keyed=keyed)]
        out += [(h.start, h.end, REDACTED) for h in format_hits(s, found)]
        out += [(a, b, REDACTED) for a, b in labelled_spans(s, found)]
        if self.exclude_domains:
            for u in found:
                try:
                    host = urlsplit(u.group()).hostname or ""
                except ValueError:
                    continue
                if self.excluded_host(host):
                    out.append((u.start(), u.end(), EXCLUDED))
        return out

    def _fixed(self, s: str, keyed: bool = False) -> str:
        """`s` masked, then masked again until nothing changes. A mask changes only its own line's reading and its
        neighbours' (the rules relate a line to the next one, a list line to the list above it), so each later pass
        reads only those lines (`_around_masks`)."""
        out = mask(s, self.spans(s, keyed=keyed))
        for _ in range(_FIXED_POINT):
            if out == s:
                break
            s = out
            found = [(a + x, a + y, rep) for a, b in _around_masks(s) for x, y, rep in self.spans(s[a:b], keyed=keyed)]
            out = mask(s, found)
        return out

    def text(self, s: str) -> str:
        """Free text: an exception message, a page, a command's output."""
        mark = self.marked()
        if type(s) is mark:
            return s
        if self.exclude_apps and s.lower() in self.exclude_apps:
            return mark(EXCLUDED)
        return mark(self._fixed(s))

    def ident(self, s: str) -> str:
        """A value under one of FERB's own id keys: any generated id body keeps its form (a known value strictly
        inside one is no hit). An excluded app's name stays excluded."""
        if self.exclude_apps and s.lower() in self.exclude_apps:
            return self.marked()(EXCLUDED)
        return self.marked()(self._fixed(s, keyed=True))

    def hit(self, s: str) -> str | None:
        """A specific secret in `s`, by kind, for a gate: a known value or a vendor format; None otherwise. Never
        "redaction would change it": an ordinary labelled-looking text is no hit."""
        if known_spans(s, self.registry.values()):
            return "a saved secret"
        return first_format(s)

    def kinds(self, s: str) -> list[str]:
        """Each kind of specific secret in `s`, once, in the order they stand: a known value (SAVED_SECRET) or a named
        format; names only, never a value. A format inside a known value's place or an earlier format's is that one
        (an Anthropic key is not also an OpenAI key); a part is named as its whole (`_PART_OF`), and a card's expiry
        or CVV with no card names nothing ("valid until 03/27"). Labelled text is no kind: as for `hit`. What is
        masked is unchanged: these are only the names."""
        taken = [(a, b, SAVED_SECRET) for a, b in known_spans(s, self.registry.values())]
        for h in format_hits(s):
            if holds_value(s, h) and not any(a <= h.start and h.end <= b for a, b, _ in taken):
                taken.append((h.start, h.end, h.kind))
        found = [kind for _, _, kind in sorted(taken)]
        return list(dict.fromkeys(_PART_OF[k] if _PART_OF.get(k) in found else k for k in found
                                  if k not in _ONLY_A_PART or _PART_OF[k] in found))

    def field(self, text: str, *, role: str = "", label: str = "", secure_input: bool = False,
              concealed: bool = False, metadata: tuple[str, ...] = (), kind: str | None = None) -> str:
        """Capture boundary for AX/DOM/clipboard fields; metadata never grants action permission. `kind` is the
        field's input type or role for the password rule (its role when not given); none named, no password rule."""
        kind = role if kind is None else kind
        if (secure_input or concealed or "secure" in role.casefold() or role.casefold() == "password"
                or bool(kind) and password_field(kind, label, *metadata) or labelled_code(label, text)
                or labelled_secret(kind, label, text)):
            return REDACTED if text else text
        return self.text(text)

    def answer(self, text: str, *, question: str, reply: bool = False) -> str:
        """The owner's typed answer to a question (no field): to a sign-in password, passcode or PIN question it is
        hidden whole; to a question with a code word its 4-8 digit codes are masked. `reply`: a chat message while
        the question is open."""
        if not text.strip():
            return text
        if hidden_answer(question, text, reply=reply):
            return REDACTED
        out = self.text(text)
        return self.marked()(CODE_VALUE.sub(REDACTED, out)) if code_words(question) else out

    # ---------- structures ----------

    def __call__(self, obj: Any) -> Any:
        return self._walk(obj)

    def _walk(self, obj: Any) -> Any:
        if isinstance(obj, str):
            return self.text(obj)
        if isinstance(obj, dict):
            return self._dict(obj)
        if isinstance(obj, (list, tuple)):
            return self._list(list(obj))
        if obj is None or isinstance(obj, (bool, float)):
            return obj
        if isinstance(obj, int):
            digits = str(abs(obj))
            return REDACTED if payment_card(digits) or known_spans(digits, self.registry.values()) else obj
        if isinstance(obj, (bytes, bytearray)):
            raw = bytes(obj).decode("utf-8", "surrogateescape")
            out = self.text(raw)
            return obj if out == raw else out.encode("utf-8", "surrogateescape")
        return obj

    def _key(self, key: str) -> str:
        """A dict key redacted as text (a key can be a secret too), cached for the current known values."""
        mark = self.marked()
        if type(key) is mark:
            return key
        if (out := self._keys.get(key)) is None:
            out = self.text(key)
            if len(self._keys) < _KEY_CACHE:
                self._keys[key] = out
        return out

    def _dict(self, d: dict[Any, Any]) -> dict[Any, Any]:
        kind_type = str(d.get("type") or "")
        kind = str(d.get("input_type") or (kind_type if kind_type.lower() in INPUT_TYPES else "")   # an input type
                   or d.get("role") or "")                                                          # before a role
        autocomplete = str(d.get("autocomplete", ""))
        secret_field = (kind_type.lower() == "password" or "one-time-code" in autocomplete.lower()
                        or PASSWORD_AUTOCOMPLETE.search(autocomplete) is not None
                        or CARD_AUTOCOMPLETE.search(autocomplete) is not None
                        or password_field(kind, *(d.get(k) for k in FIELD_METADATA)))
        input_event = isinstance(d.get("method"), str) and d["method"].startswith("Input.")
        out: dict[Any, Any] = {}
        order: list[tuple[str, Any]] = []
        for k, v in d.items():
            ks = str(k)
            cls = label_class(ks)
            if cls == "secret":
                value: Any = self._hidden(v)
            elif cls in ("code", "credential") and isinstance(v, (str, int)) and not isinstance(v, bool) \
                    and labelled_value(cls, str(v)):
                value = REDACTED
            elif ks in INTERNAL_ID_KEYS and isinstance(v, (str, list, tuple)):
                value = self.ident(v) if isinstance(v, str) else [
                    self.ident(item) if isinstance(item, str) else self._walk(item) for item in v]
            elif secret_field and ks == "value" and v not in (None, ""):
                value = REDACTED
            elif input_event and ks == "params" and isinstance(v, dict):
                value = {}
                for pk, pv in v.items():
                    self._put(value, pk, REDACTED if pk in INPUT_TEXT_FIELDS else self._walk(pv))
            elif ks in ("value", "text", "content") and isinstance(v, str):
                value = self.field(v, role=str(d.get("role", d.get("type", ""))), kind=kind,
                                   label=str(d.get("label", d.get("name", d.get("autocomplete", "")))))
            else:
                value = self._walk(v)
            order.append((ks, self._put(out, k, value)))
        return self._dict_siblings(d, out, order) if len(d) > 1 else out

    def _dict_siblings(self, d: dict[Any, Any], out: dict[Any, Any], order: list[tuple[str, Any]]) -> dict[Any, Any]:
        """A code its previous value labels (a body's "PIN" then "4821", L29 re-review Q1): an object's neighbouring
        scalar values, in key order, read as sibling lines; FERB's own ids are neither read nor masked."""
        pairs = sorted(((str(key), key, ks) for ks, key in order), key=lambda p: p[0])
        lines = [str(out[key]) if (isinstance(out[key], str) or type(out[key]) is int) and ks not in _SIBLING_ID_KEYS
                 else "" for _, key, ks in pairs]
        if not any(len(t) < 64 and _DIGIT.search(t) for t in lines):
            return out
        for i in self._sibling_hits(lines, [d.get(ks, "") if isinstance(d.get(ks), str) else ""
                                            for _, _, ks in pairs]):
            if isinstance(out[pairs[i][1]], (str, int)):
                out[pairs[i][1]] = REDACTED
        return out

    def _put(self, out: dict[Any, Any], k: Any, value: Any) -> Any:
        """`value` under `k` redacted; two keys that redact alike both stay ("#2" on the later one)."""
        key = self._key(k) if isinstance(k, str) else k
        if key in out:
            n = 2
            while f"{key}#{n}" in out:
                n += 1
            key = self.marked()(f"{key}#{n}")
        out[key] = value
        return key

    def _hidden(self, value: Any) -> Any:
        """A value a secret key names: every text and number in it masked, its shape (keys, items) kept."""
        if isinstance(value, dict):
            out: dict[Any, Any] = {}
            for k, v in value.items():
                self._put(out, k, self._hidden(v))
            return out
        if isinstance(value, (list, tuple)):
            return [self._hidden(v) for v in value]
        return value if value in (None, "") or isinstance(value, bool) else REDACTED

    def _list(self, raw: list[Any]) -> list[Any]:
        out = [self._walk(v) for v in raw]
        if len(raw) < 2:
            return out
        mark = self.marked()
        if all(type(v) is mark for v in raw if isinstance(v, str)) and not any(isinstance(v, dict) for v in raw):
            return out                             # already related as siblings when it was redacted
        return self._siblings(raw, out)

    def _siblings(self, raw: list[Any], out: list[Any]) -> list[Any]:
        """Secrets that only neighbouring items show: a code split from its label across sibling nodes (a page's
        "Your verification code" heading and the code in the next node, C1), read as lines; and a private key whose
        lines are separate items."""
        keys: list[str | None] = []
        lines: list[str] = []
        for item in out:
            key = next((k for k in NODE_TEXT if isinstance(item, dict) and isinstance(item.get(k), str)
                        and item[k].strip()), None)
            keys.append(key)
            lines.append(item if isinstance(item, str) else str(item) if type(item) is int
                         else str(item[key]) if key else "")
        raw_lines = [item if isinstance(item, str) else "" for item in raw]
        for i in self._sibling_hits(lines, raw_lines):
            if isinstance(out[i], (str, int)):
                out[i] = REDACTED
            elif keys[i] is not None and isinstance(out[i], dict):
                out[i] = {**out[i], keys[i]: REDACTED}
        return out

    def _sibling_hits(self, lines: list[str], raw: list[str]) -> set[int]:
        """Which of neighbouring texts read as lines hold a secret only together: a code alone under its label line
        or over its naming line (read from the redacted texts `lines`, so a second redaction reads the same), and a
        private key's lines (read from the `raw` texts: redacted, each line alone hides the block)."""
        hits: set[int] = set()
        if any(code_alone(line) for line in lines if line):
            joined, starts = "", []
            for line in lines:
                starts.append(len(joined))
                joined += " ".join(line.split()) + "\n"
            for a, _ in line_code_spans(joined, content_lines(joined)):
                hits.add(bisect.bisect_right(starts, a) - 1)
        if any("-----BEGIN" in line for line in raw):
            joined, starts = "", []
            for line in raw:
                starts.append(len(joined))
                joined += line + "\n"
            for a, b, _ in self.spans(joined):
                first, last = bisect.bisect_right(starts, a) - 1, bisect.bisect_right(starts, b - 1) - 1
                if first != last:
                    hits.update(range(first, last + 1))
        return hits


from ferb.sanitize.stream import RedactionStream  # noqa: E402  (the stream redacts through a Redactor)
