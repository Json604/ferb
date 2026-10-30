"""Own incremental redaction of process output (a PTY, a shell, a streamed reply): text is released as it comes and
masked exactly as `Redactor.text` masks the whole. Must not release raw secret bytes, even partially.

The text rules are line-local except four relations between neighbouring lines (a label line over a code alone, a
code alone over its naming line, "Password:" over its value, a backup-code list's label over its codes). So the
stream releases complete lines at once, and the line being read up to its last word, except while later text can
still change a mask in it: the line is held whole while it holds a class word, a code that can still join its word,
or sits under a relation; a code alone on its line waits for the next line; digits that may still form a card, a
private key's block and a known value's start wait for the rest. Released text is masked by `Redactor.text` over it
and the few lines before it that it can relate to (`_context`)."""

import codecs
import re
from typing import TYPE_CHECKING

from ferb.sanitize.formats import _H, PRIVATE_KEY_END, PRIVATE_KEY_START, private_key_hits, urls
from ferb.sanitize.known import REDACTED
from ferb.sanitize.labels import (_TAG, CODE_LINK, CODE_REACH, CODE_VALUE, CODE_WORD, KEY_LABEL, LINE_BREAK, OTP_LABEL,
                                  SECRET_AFTER, SECRET_WORD, code_alone, label_class, label_line, link_line,
                                  list_label_line, list_line, marks_from, quote_only, secret_words)

if TYPE_CHECKING:
    from ferb.sanitize.redact import Redactor

# Words that make a line hold a value released later in it: the vocabulary's key-only words, card labels, an OTP
# introducer and a list label's first words (labels.py has the rest: code words, secret words).
HOLD_WORDS = re.compile(rf"(?i)(?<![A-Za-z0-9])(?:cookie|session|auth(?:orization)?|credentials?|bearer|basic|"
                        rf"cvv2?|cvc2?|csc|exp(?:iry|ires|iration)?|valid|good[{_H}]+thru|backup|recovery|"
                        rf"two[{_H}_-]?factor|security|login|sign[{_H}_-]?in|one[{_H}_-]?time|access|"
                        rf"authentication|mysql|mariadb)")
# What can still become a link after a code at the end of what has been read: marks, then an unfinished tag or part
# of a word. Each mark reads one way only, taken possessively.
_UNFINISHED = r"</?(?:[A-Za-z][A-Za-z0-9]*(?:[ \t][^<>\r\n]*|/)?)?"
_LINK_START = re.compile(rf"(?:[\s*_`'\"]|{_TAG})*+(?:{_UNFINISHED}|\S*)")
_UNFINISHED_TAG = re.compile(_UNFINISHED)
_CODE_LEAD_START = re.compile(r"(?:>[\s>]*)?(?:#+(?:\s+|$))?(?:[-*•·–—](?:\s|$))?")
_CODE_START = re.compile(rf"(?:[A-Z]-?)?(?:\d|$)|{_UNFINISHED}$")
_LIST_LINE_START = re.compile(r"[\s,]*(?:(?:\d+[.)]|[*•])(?:\s|$)\s*)?[A-Za-z0-9-]*(?:[\s,]|$)")
# The digits ending the line being read that a read must not split: a run of digits and separators that reaches the
# end, or ends in the line's last word at a mark a card can end at ("4111 1111 1111 1111**").
_NUMERIC_TAIL = re.compile(rf"[{_H},.-]*+(?=\d[\d{_H},.-]*+(?:[^\s0-9A-Za-z_]\S*+)?$)")
_NUMERIC_START = re.compile(rf"(?<![\d{_H},.-])[\d{_H},.-]")
_NUMERIC_RUN = re.compile(rf"[\d{_H},.-]*+")
_LAST_WORD = re.compile(r"\S*+")
_HEAD_WORDS = 2 * CODE_REACH + 2           # a released line's start kept as context: every relation reaches less
_HEAD_CHARS = 4096
_LONG_LINE = 2048                           # a relation line longer than this is kept as its last words


def _secret_colon_end(line: str) -> bool:
    """A line ending in a secret word and its ":" or "=" ("Password:"), whose value may be on the next line."""
    for word in secret_words(line):
        after = SECRET_AFTER.match(line, word.end())
        if after is not None and after.end() >= len(line.rstrip()) and re.search("[:=]", after.group("link")):
            return True
    return False


def _code_alone_start(line: str) -> bool:
    """Whether the line being read can still become a code alone: its lead and marks, then a digit, a "G-" prefix,
    an unfinished tag or nothing yet."""
    a = _CODE_LEAD_START.match(line, len(line) - len(line.lstrip())).end()     # type: ignore[union-attr]
    return _CODE_START.match(line, marks_from(line, a, len(line))) is not None


def _code_alone_so_far(line: str) -> bool:
    """A code alone so far, an unfinished tag at its end allowed ("(482913) <b" can still get its naming line)."""
    lt = line.rfind("<")
    return code_alone(line[:lt] if lt >= 0 and _UNFINISHED_TAG.fullmatch(line, lt) else line)


def _numeric_tail(tail: str) -> int | None:
    """Where the digits that end `tail` start; only the runs reaching the last word are tried, each once."""
    reverse = tail[::-1]
    word = len(tail) - _LAST_WORD.match(reverse).end()                       # type: ignore[union-attr]
    start = word - _NUMERIC_RUN.match(reverse, len(tail) - word).end()       # type: ignore[union-attr]
    for run in _NUMERIC_START.finditer(tail, start):
        if (digits := _NUMERIC_TAIL.match(tail, run.start())) is not None:
            return digits.end()
    return None


def _head(line: str) -> str:
    """A released line's start as context: its last words (every relation within a line reaches fewer)."""
    tail = line[-_HEAD_CHARS:]
    found = list(re.finditer(r"\S+", tail))
    return tail[found[-_HEAD_WORDS].start():] if len(found) > _HEAD_WORDS else tail


def _lines(s: str) -> list[tuple[int, int, int]]:
    """The complete lines of `s`: (start, end, end of its line break)."""
    out, begin = [], 0
    for m in LINE_BREAK.finditer(s):
        out.append((begin, m.start(), m.end()))
        begin = m.end()
    return out


def _relation(lines: list[tuple[int, int, int]], s: str) -> tuple[int, set[str]]:
    """How the line after `lines` relates to the last of them that isn't blank: where that line starts (or where the
    list label of its list starts), and its relations ("label", "secret", "list"); (len(s), set()) when none."""
    i = len(lines) - 1
    while i >= 0 and not s[lines[i][0]:lines[i][1]].strip():
        i -= 1
    quoted = False
    while i >= 0 and quote_only(s[lines[i][0]:lines[i][1]]):
        quoted, i = True, i - 1
        while i >= 0 and not s[lines[i][0]:lines[i][1]].strip():
            i -= 1
    if i < 0:
        return len(s), set()
    line = s[lines[i][0]:lines[i][1]]
    kinds = {"label"} if label_line(line) else set()
    if quoted:
        return lines[i][0], kinds
    if _secret_colon_end(line):
        kinds.add("secret")
    if list_label_line(line):
        kinds.add("list")
    elif list_line(line):
        j = i - 1
        while j >= 0:
            above = s[lines[j][0]:lines[j][1]]
            if above.strip() and not list_line(above):
                if list_label_line(above):
                    return lines[j][0], kinds | {"list"}
                break
            j -= 1
    return lines[i][0], kinds


def _context(raw: str) -> str:
    """What of the released text `raw` later text can relate to: the last line that isn't blank (with a list's label
    above it), the blank or quote lines after it, and the start of the line being read."""
    lines = _lines(raw)
    tail = raw[lines[-1][2]:] if lines else raw
    head = _head(tail) if tail else ""
    if not lines:
        return head
    start, kinds = _relation(lines, raw)
    if start >= len(raw):
        return head
    kept = raw[start:lines[-1][2]]
    label_end = next(e for a, _, e in lines if a == start)
    after = [a for a, b, _ in lines if a >= label_end and raw[a:b].strip()]
    if "list" in kinds and after:                    # a list's label, then its last list line and what follows it
        kept = raw[start:label_end] + raw[after[-1]:lines[-1][2]]
    elif len(kept) > _LONG_LINE and "list" not in kinds:
        end = next(b for a, b, _ in lines if a == start)
        kept = _head(raw[start:end]) + raw[end:lines[-1][2]][-_LONG_LINE:]
    return kept + head


class RedactionStream:
    """Incremental UTF-8 redaction. Emit complete words, retaining possible secrets across chunks.

    An oversized unfinished record is replaced by an explicit marker rather than spilling raw bytes to disk.
    Private-key blocks are suppressed until their end marker, even when they exceed the buffer bound.
    """
    MAX_PENDING = 65536
    PRIVATE_START = PRIVATE_KEY_START
    PRIVATE_END = PRIVATE_KEY_END

    def __init__(self, redactor: "Redactor") -> None:
        self.redactor = redactor
        self.decoder = codecs.getincrementaldecoder("utf-8")(errors="replace")
        self.pending = ""
        self.suppress: str | None = None
        self.context = ""          # released raw text later text can still relate to (`_context`)

    def _unfinished_key(self) -> int | None:
        """Where the first BEGIN line with no END line after it starts (one END serves every BEGIN before it)."""
        end = None
        for opening in self.PRIVATE_START.finditer(self.pending):
            if end is None or end.start() < opening.end():
                if (end := self.PRIVATE_END.search(self.pending, opening.end())) is None:
                    return opening.start()
        return None

    def _hold_line(self, line: str, kinds: set[str]) -> bool:
        """Whether later text in the line being read can still change a mask in what has been read of it."""
        if (CODE_WORD.search(line) or SECRET_WORD.search(line) or HOLD_WORDS.search(line) or OTP_LABEL.search(line)
                or _code_alone_so_far(line)):
            return True
        if any(label_class(m.group("key").lstrip("-")) for m in KEY_LABEL.finditer(line)):
            return True
        if line.strip() and ("secret" in kinds or ("list" in kinds and _LIST_LINE_START.match(line) is not None)
                             or ("label" in kinds and _code_alone_start(line))):
            return True
        url_spans = [u.span() for u in urls(line)]       # a URL's own digits are never a code
        return any((CODE_LINK.match(line, m.end()) is not None or _LINK_START.fullmatch(line, m.end()))
                   and not any(a <= m.start() < b for a, b in url_spans) for m in CODE_VALUE.finditer(line))

    def _safe_cut(self) -> int:
        p = self.pending
        lines = _lines(p)
        if lines and lines[-1][2] == len(p) and p.endswith("\r"):
            lines.pop()                                  # a "\r" that ends the text may be half of "\r\n"
        line_start = lines[-1][2] if lines else 0
        cut = len(p)
        released = self.context
        released_lines = _lines(released)
        head = released[released_lines[-1][2]:] if released_lines else released     # the released start of a line
        tail = p[line_start:]
        if tail:
            line = (head if line_start == 0 else "") + tail
            above = released + p[:line_start]
            kinds = _relation(_lines(above), above)[1]
            if self._hold_line(line, kinds):
                cut = line_start
                if len(tail) > self.MAX_PENDING // 2 and "secret" not in kinds:
                    found = list(re.finditer(r"\S+", tail))   # a very long line: all but its last words
                    if len(found) > _HEAD_WORDS:
                        cut = line_start + found[-_HEAD_WORDS].start()
            else:
                last = re.search(r"\s(?=\S*$)", tail)
                cut = line_start + (last.end() if last else 0)
                if (numeric := _numeric_tail(tail)) is not None:
                    start = line_start + numeric
                    while start > line_start and not p[start - 1].isspace():
                        start -= 1
                    cut = min(cut, start)
            if (begin := tail.find("-----BEGIN")) >= 0:
                cut = min(cut, line_start + begin)
        if (opening := self._unfinished_key()) is not None:
            cut = min(cut, opening)
        blocks = private_key_hits(p) if "-----BEGIN" in p else []
        while True:                                      # each guard only moves the cut back: until none moves it
            before = cut
            for hit in blocks:                           # never inside a whole key block
                if hit.start < cut < hit.end:
                    cut = hit.start
            cut = self._known_cut(cut)
            while 0 < cut < len(p) and not p[cut - 1].isspace() and not p[cut].isspace():
                cut -= 1                                 # never inside a token: its rest may still mask its start
            cut = self._line_cut(cut, head)
            cut = self._code_alone_cut(cut, head)
            if cut == before:
                return cut

    def _code_alone_cut(self, cut: int, head: str) -> int:
        """A code alone on the last line before the cut waits while the line after it is still missing, or is
        released with it when that line names it ("482913" over "is your verification code")."""
        p = self.pending
        before = [(a, b) for a, b, _ in _lines(p[:cut]) if p[a:b].strip() or (a == 0 and head.strip())]
        if not before:
            return cut
        a, b = before[-1]
        if not code_alone((head if a == 0 else "") + p[a:b]):
            return cut
        after = [(x, y) for x, y, _ in _lines(p) if x >= cut and p[x:y].strip()]
        return a if not after or link_line(p[after[0][0]:after[0][1]]) else cut

    def _line_cut(self, cut: int, head: str) -> int:
        """A cut a guard moved inside a line (a key's BEGIN, a known value) goes back to the line's start when the line
        holds anything a later word in it can change (`_hold_line`); a very long line is cut by its own rule."""
        p = self.pending
        if cut <= 0 or cut >= len(p) or p[cut - 1] in "\r\n":
            return cut
        start = max(p.rfind("\n", 0, cut), p.rfind("\r", 0, cut)) + 1
        end = min((i for i in (p.find("\n", cut), p.find("\r", cut)) if i >= 0), default=len(p))
        if end - start > self.MAX_PENDING // 2:          # a very long line keeps only its last words (`_safe_cut`)
            return cut
        above = self.context + p[:start]
        line = (head if start == 0 else "") + p[start:end]
        return start if self._hold_line(line, _relation(_lines(above), above)[1]) else cut

    def _known_cut(self, cut: int) -> int:
        """Never inside a known value, nor after the start of one that may still be coming."""
        p = self.pending
        for value in self.redactor.secret_values():
            start = p.find(value)
            while start >= 0 and start < cut:
                if cut < start + len(value):
                    cut = start
                start = p.find(value, start + 1)
            for length in range(min(len(value) - 1, len(p)), 0, -1):
                if p.endswith(value[:length]):
                    cut = min(cut, len(p) - length)
                    break
        return cut

    def _release(self, n: int) -> str:
        """`pending[:n]` masked as `Redactor.text` masks it after the released text it can relate to."""
        released, self.pending = self.pending[:n], self.pending[n:]
        if not released:
            return ""
        text = self.redactor.text
        if self.context:
            before, whole = text(self.context), text(self.context + released)
            out = whole[len(before):] if whole.startswith(before) else text(released)
        else:
            out = text(released)
        self.context = _context(self.context + released)
        return str(out)

    def feed(self, chunk: bytes, final: bool = False) -> bytes:
        self.pending += self.decoder.decode(chunk, final=final)
        if self.suppress:
            end = (self.PRIVATE_END.search(self.pending) if self.suppress == "private"
                   else re.search(r"\n", self.pending))
            if end is None:
                self.pending = self.pending[-128:] if self.suppress == "private" and not final else ""
                return b""
            self.pending = self.pending[end.end():]
            self.suppress, self.context = None, ""
        if final:
            return self._release(len(self.pending)).encode("utf-8")
        out = self._release(self._safe_cut())
        bound = max(self.MAX_PENDING, max((len(v) for v in self.redactor.secret_values()), default=0) + 1)
        if len(self.pending) > bound:
            opening = self._unfinished_key()
            if opening is not None:
                out += self._release(opening) + REDACTED
                self.pending, self.suppress, self.context = self.pending[-128:], "private", ""
            else:
                out += "[redacted: oversized unfinished record]\n"
                self.pending, self.suppress, self.context = "", "record", ""
        return out.encode("utf-8")
