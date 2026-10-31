"""Owns token estimates and recoverable context assembly. Must not infer intent, choose a target or claim semantic success from transport facts.

Budget complete context blocks and expose omissions; never shorten requests or generated artifacts.

Counts use the cached o200k_base tokenizer as an estimate, not a claim about every provider's tokenizer.
Loading is local only: FERB's own pinned copy under its home (`tokenizer_path`, put there by `ferb setup`), else a
tiktoken cache, each checked against the pinned SHA-256. macOS cleans the temporary folder, so FERB never relies on
it alone. A missing tokenizer uses a conservative UTF-8 byte ceiling and says so (`token_method`, `ferb doctor`)."""

import base64
import hashlib
import json
import os
import tempfile
from bisect import bisect_right
from dataclasses import dataclass
from functools import lru_cache
from pathlib import Path
from typing import Any, Callable, Sequence

from ferb.core.errors import InputTooLarge, ModelLimitsUnknown
from ferb.core.types import ModelLimits

_ENCODING_URL = "https://openaipublic.blob.core.windows.net/encodings/o200k_base.tiktoken"
_ENCODING_SHA256 = "446a9538cb6c348e3516120d7c08b09f57c36495e2acfffe59a5bf8b0cfb1a2d"
# o200k_base's public tokenizer pattern (OpenAI tiktoken, MIT); ranks remain in its verified local cache.
_PATTERN = "|".join([
    r"[^\r\n\p{L}\p{N}]?[\p{Lu}\p{Lt}\p{Lm}\p{Lo}\p{M}]*[\p{Ll}\p{Lm}\p{Lo}\p{M}]+(?i:'s|'t|'re|'ve|'m|'ll|'d)?",
    r"[^\r\n\p{L}\p{N}]?[\p{Lu}\p{Lt}\p{Lm}\p{Lo}\p{M}]+[\p{Ll}\p{Lm}\p{Lo}\p{M}]*(?i:'s|'t|'re|'ve|'m|'ll|'d)?",
    r"\p{N}{1,3}", r" ?[^\s\p{L}\p{N}]+[\r\n/]*", r"\s*[\r\n]+", r"\s+(?!\S)", r"\s+",
])


TOKENIZER_URL, TOKENIZER_SHA256 = _ENCODING_URL, _ENCODING_SHA256


def tokenizer_path(home: Path | None = None) -> Path:
    """FERB's own pinned copy of the o200k_base ranks, under FERB's home ($FERB_HOME or ~/.ferb), never in temp."""
    return (home or Path(os.environ.get("FERB_HOME", Path.home() / ".ferb"))) / "cache" / "tokenizer" / \
        "o200k_base.tiktoken"


def tokenizer_sources() -> list[Path]:
    """Where a copy may be, in order: FERB's own, then tiktoken's cache (its environment variable, else temp)."""
    name = hashlib.sha1(_ENCODING_URL.encode()).hexdigest()
    caches = [os.environ.get(v) for v in ("TIKTOKEN_CACHE_DIR", "DATA_GYM_CACHE_DIR")]
    return [tokenizer_path(), *(Path(c) / name for c in caches if c), Path(tempfile.gettempdir()) / "data-gym-cache" / name]


def verified_tokenizer() -> bytes | None:
    """The first copy whose bytes match the pinned SHA-256, or None."""
    for path in tokenizer_sources():
        try:
            data = path.read_bytes()
        except OSError:
            continue
        if hashlib.sha256(data).hexdigest() == _ENCODING_SHA256:
            return data
    return None


@lru_cache(maxsize=1)
def _encoding() -> Any:
    try:
        import tiktoken
        data = verified_tokenizer()
        if data is None:
            return None
        ranks = {base64.b64decode(token): int(rank) for token, rank in
                 (line.split() for line in data.splitlines() if line)}
        return tiktoken.Encoding(name="o200k_base", pat_str=_PATTERN, mergeable_ranks=ranks,
                                 special_tokens={"<|endoftext|>": 199999, "<|endofprompt|>": 200018})
    except (ImportError, OSError, ValueError):
        return None


def token_method() -> str:
    return "o200k_base_chunked8192_estimate_provider_may_differ" if _encoding() is not None else "utf8_byte_ceiling_tokenizer_unavailable"


TOKEN_SLICE = 8192        # count_tokens counts each slice of this many characters on its own


def count_tokens(text: str) -> int:
    encoding = _encoding()
    if encoding is None:
        return len(text.encode("utf-8"))
    # Bound tokenizer work on huge unbroken source strings without dropping a character.
    # Chunk boundaries make this an explicit estimate even for a provider using o200k_base.
    return sum(len(encoding.encode_ordinary(text[start:start + TOKEN_SLICE]))
               for start in range(0, len(text), TOKEN_SLICE))


# Counting without reading a whole text again (P1/D10, perf plan 2026-10-23). count_tokens(text) is the sum, over the
# text's TOKEN_SLICE-character slices, of each slice counted alone (a UTF-8 byte count adds up the same way).
#
# 1. A text that only grows keeps the count of every complete slice; only its last, partial slice is counted again
#    (GrowingCount). Exact by construction.
#
# 2. o200k never puts a comma and a space right after it in one piece. The tokenizer cuts a text into pieces with
#    _PATTERN, left to right, each piece being the first alternative that matches there (greedy quantifiers), and counts
#    each piece on its own. Every character starts some alternative's match, so the pieces are contiguous. A comma can
#    be in no letter piece (alternatives 1-2 take one non-letter only when a letter follows it, and a space follows),
#    no digit piece and no whitespace piece, so it is in alternative 4's run of [^\s\p{L}\p{N}]: greedy, the run goes
#    on to the comma and stops at the space (\s), and [\r\n/]* then matches nothing. The piece ends right after the
#    comma. The pattern looks only ahead, and one character at most ((?!\S) after a whitespace run), so the pieces after
#    a piece's end depend on the text after it alone; and no attempt starting before the comma's piece reaches past the
#    comma (a letter, digit or whitespace run can't contain it; a punctuation run that reaches it is the comma's piece).
#    So within a slice, the text cut right after any such commas counts as the sum of its parts, each counted alone.
#
# 3. Hence a lower bound (uncut_count): an atom (the text between two such cuts) that no slice edge falls inside adds
#    exactly its own count; every other part adds at least 0. FERB uses it only to skip counting a text that is surely
#    over a budget, and counts the text whole whenever the bound can't decide. tests/test_batcher_cost.py pins
#    _PATTERN to the one proven here.


class GrowingCount:
    """count_tokens of a text that only grows at its end, kept exact without counting it again (point 1 above).
    `count` is count_tokens or another count that adds up over TOKEN_SLICE-character slices (len does)."""

    def __init__(self, count: Callable[[str], int] = count_tokens) -> None:
        self.count = count
        self.closed = 0           # the counts of the complete slices
        self.tail = ""            # the text after them, shorter than TOKEN_SLICE

    def with_suffix(self, suffix: str = "") -> int:
        """count_tokens(text + suffix); the text itself is unchanged."""
        rest = self.tail + suffix
        return self.closed + sum(self.count(rest[i:i + TOKEN_SLICE]) for i in range(0, len(rest), TOKEN_SLICE))

    def append(self, more: str) -> None:
        rest = self.tail + more
        whole = len(rest) - len(rest) % TOKEN_SLICE
        self.closed += sum(self.count(rest[i:i + TOKEN_SLICE]) for i in range(0, whole, TOKEN_SLICE))
        self.tail = rest[whole:]


def comma_bounds(text: str) -> list[int]:
    """Each position right after a comma that a space follows: a piece always ends there (point 2 above)."""
    bounds, at = [], text.find(", ")
    while at >= 0:
        bounds.append(at + 1)
        at = text.find(", ", at + 1)
    return bounds


def uncut_count(ends: Sequence[int], totals: Sequence[int], lo: int, hi: int, shift: int) -> int:
    """At most what atoms lo..hi-1 add to count_tokens of the text they're in (point 3 above). Atom i is the text from
    shift + ends[i] to shift + ends[i + 1], both cuts of point 2 (or the text's start or end); totals[i] is the sum of
    the counts of the atoms before i, each counted alone (0 for an atom longer than TOKEN_SLICE). An atom that a slice
    edge falls inside is left out."""
    if hi <= lo:
        return 0
    bound = totals[hi] - totals[lo]
    edge, end, cut = (shift + ends[lo]) // TOKEN_SLICE * TOKEN_SLICE + TOKEN_SLICE, shift + ends[hi], -1
    while edge < end:
        i = bisect_right(ends, edge - shift, lo, hi) - 1               # ends[i] <= edge - shift < ends[i + 1]
        if shift + ends[i] < edge and i != cut:
            bound -= totals[i + 1] - totals[i]
            cut = i
        edge += TOKEN_SLICE
    return bound


def runtime_limits(runtime: Any) -> ModelLimits:
    """A runtime's documented model limits (L28): the context budget is their window minus room for the reply. A runtime
    that reports none raises ModelLimitsUnknown; FERB never substitutes a number of its own."""
    limits = getattr(runtime, "limits", None)
    if not isinstance(limits, ModelLimits):
        raise ModelLimitsUnknown(f"the {getattr(runtime, 'name', 'model')} runtime doesn't report its model's "
                                 "documented context window")
    return limits


@dataclass(frozen=True)
class ContextBlock:
    text: str
    reference: str
    protected: bool = False  # always included, whatever the budget (the situation, the request)
    cache: bool = False      # repeats across this role's calls; the cacheable prefix ends after the last leading one (P20)
    priority: int = 0        # optional layer allocation; original presentation order is preserved
    origin: str = ""          # supplied by the source, never inferred from model-written text
    source_refs: tuple[str, ...] = ()
    payload: str | None = None  # source data without presentation/catalogue metadata; None preserves legacy text
    history: bool = False    # one conversation entry: filled newest-first after the protected blocks, never with gaps
    fold: str = ""           # one run of optional blocks (its retrieval route): its lowest markers fold into a note


@dataclass(frozen=True)
class Assembly:
    parts: list[str]                     # the context in order, the budget report last
    cacheable: int                       # leading parts that repeat across this role's calls
    shown: tuple[bool, ...]              # per non-empty block, in order: handed to the model in full
    omitted_history: int                 # older conversation entries that did not fit
    tokens: int = 0                      # the body's token estimate, before the budget report
    chars: int = 0                       # characters of the whole text the parts make, the report included


def assemble(blocks: list[ContextBlock], *, budget_tokens: int, reserved_tokens: int = 0,
             max_chars: int | None = None) -> Assembly:
    """Reserve prompts/tools first; then every protected block; then the conversation entries newest-first while they
    fit, the older ones reported in one omission note; then optional blocks by priority (omitted whole, their retrieval
    references kept visible). Each block is counted once (L28); the report counts the final body once.

Every optional block's marker is charged before any block is added. When the markers of a run (`fold`, one retrieval
route, e.g. a task's recorded calls) can't all fit beside the protected text, its lowest-priority blocks fold, one by
one until the rest fit, into one note in the first one's place that counts them and names the first and last of them
and the route (fix wave 6b round 1). Then, as the optional blocks are added by priority, one that doesn't fit folds
more of the lower-priority run markers still listed, lowest first, until it fits; when even all of them wouldn't make
room, none is folded for it (L29 Task 5 fix round 1). With one run, or runs in disjoint priority bands (every current
caller: a worker's earlier runs, the planner's procedure lines, the checker's actions above its observations), a block
is never left out while lower-priority markers that could make room for it are still listed. When runs interleave in
priority, folding stays lowest first and a run's first fold also costs its note, so a block can be left out that
folding only some of those markers would have fitted. Blocks without `fold` keep their markers as before, and a
context without a run is assembled exactly as before.

Protected source text can exceed the budget. Reporting that fact is preferable to changing it.
The budget is a context allocation (the model's window minus room for its reply), not a task/output/token-spend limit.
`max_chars`: the runtime's documented per-input character cap (ModelLimits.max_input_chars), for the whole text the
parts make (they are sent as one input; instructions go apart). Chat and optional blocks are kept only while both fit;
protected text over the cap is refused (InputTooLarge) rather than sent to be rejected.
"""
    if budget_tokens <= 0 or reserved_tokens < 0 or (max_chars is not None and max_chars <= 0):
        raise ValueError("context budget must be positive and reserve nonnegative")
    active = [block for block in blocks if block.text]
    omission = [f"[Context omitted; fetch full text: {block.reference}]" for block in active]
    history = [index for index, block in enumerate(active) if block.history and not block.protected]
    kept = {index for index, block in enumerate(active) if block.protected}
    separator = count_tokens("\n\n")
    cost = {index: count_tokens(active[index].text) for index in range(len(active))
            if index in kept or index in history}
    gap = {index: count_tokens(omission[index]) for index in range(len(active))
           if index not in kept and index not in history}
    # the report and a history note are small; they are counted into the decision as their longest form
    note_cost = count_tokens(_note(len(history), "chat.history")) + separator if history else 0
    report_cost = count_tokens(_report(budget_tokens, reserved_tokens, 10 ** 9, True, len(active))) + separator
    used = (reserved_tokens + report_cost + note_cost + sum(cost[index] + separator for index in kept) +
            sum(gap[index] + separator for index in gap))
    # characters, as the runtime counts them (Unicode code points), each part with its separator; the report and the
    # history note at their longest
    cap = max_chars if max_chars is not None else float("inf")
    note_chars = len(_note(len(history), active[history[0]].reference.split(" ", 1)[0])) + 2 if history else 0
    used_chars = (len(_report(budget_tokens, reserved_tokens, 10 ** 9, True, len(active))) + 2 + note_chars +
                  sum(len(active[index].text) + 2 for index in kept) + sum(len(omission[index]) + 2 for index in gap))
    folded: dict[str, list[int]] = {}        # fold route -> its folded blocks, lowest priority first

    def note(route: str) -> tuple[int, int]:
        """A run's note at its longest (two of its longest listed references), charged when its first block folds."""
        run = [i for i in gap if active[i].fold == route and i not in kept]
        return (count_tokens(_fold(10 ** 9, "", "", route)) + 2 * max(gap[i] for i in run) + separator,
                len(_fold(10 ** 9, "", "", route)) + 2 * max(len(active[i].reference) for i in run) + 2)

    def fold(index: int) -> None:
        nonlocal used, used_chars
        route = active[index].fold
        if route not in folded:
            tokens, chars = note(route)
            used, used_chars = used + tokens, used_chars + chars
            folded[route] = []
        folded[route].append(index)
        used -= gap.pop(index) + separator
        used_chars -= len(omission[index]) + 2

    def room(index: int, extra: int, extra_chars: int) -> list[int] | None:
        """The listed run markers of lower priority than `index` that, folded lowest first, make room for it whole:
        the fewest that do; None when even all of them wouldn't."""
        tokens, chars, noted, chosen = used + extra, used_chars + extra_chars, set(folded), []
        for i in sorted((i for i in gap if active[i].fold and i not in kept
                         and active[i].priority < active[index].priority), key=lambda i: active[i].priority):
            if active[i].fold not in noted:
                note_tokens, note_chars = note(active[i].fold)
                tokens, chars = tokens + note_tokens, chars + note_chars
                noted.add(active[i].fold)
            tokens, chars = tokens - gap[i] - separator, chars - len(omission[i]) - 2
            chosen.append(i)
            if tokens <= budget_tokens and chars <= cap:
                return chosen
        return None

    for index in sorted((i for i in gap if active[i].fold), key=lambda i: active[i].priority):
        if used <= budget_tokens and used_chars <= cap:
            break
        fold(index)
    for index in reversed(history):
        chars = len(active[index].text) + 2
        if used + cost[index] + separator > budget_tokens or used_chars + chars > cap:
            break
        kept.add(index)
        used += cost[index] + separator
        used_chars += chars
    for index in sorted(gap, key=lambda i: -active[i].priority):
        if index not in gap:                 # folded meanwhile to make room for a higher-priority block
            continue
        block = active[index]
        extra, extra_chars = count_tokens(block.text) - gap[index], len(block.text) - len(omission[index])
        if used + extra > budget_tokens or used_chars + extra_chars > cap:
            make_room = room(index, extra, extra_chars)
            if make_room is None:
                continue
            for i in make_room:
                fold(i)
        kept.add(index)
        used += extra
        used_chars += extra_chars
    first_folded = {min(run) for run in folded.values()}
    in_folds = {index for run in folded.values() for index in run}
    omitted_history = sum(1 for index in history if index not in kept)
    parts: list[str] = []
    repeats: list[bool] = []             # per part: repeats across this role's calls
    noted = False
    for index, block in enumerate(active):
        if index in kept:
            parts.append(block.text)
            repeats.append(block.cache)
        elif index in history:
            if not noted:                # the older entries collapse into one note where the first one stood
                parts.append(_note(omitted_history, block.reference.split(" ", 1)[0]))
                repeats.append(block.cache)
                noted = True
        elif index in in_folds:
            if index in first_folded:    # a run's folded blocks: one note where the first one stood
                run = sorted(folded[block.fold])
                parts.append(_fold(len(run), active[run[0]].reference, active[run[-1]].reference, block.fold))
                repeats.append(False)
        else:
            parts.append(omission[index])
            repeats.append(False)        # an omitted block ends the cacheable prefix
    parts = parts or [""]
    body = "\n\n".join(parts)
    content = count_tokens(body)
    over = content + reserved_tokens > budget_tokens
    report = _report(budget_tokens, reserved_tokens, content, over, len(active) - len(kept))
    # Header/omission references also consume context. Report their unavoidable overhead truthfully.
    if not over and content + count_tokens(report) + separator + reserved_tokens > budget_tokens:
        report = _report(budget_tokens, reserved_tokens, content, True, len(active) - len(kept))
    parts.append(report)
    chars = len("\n\n".join(parts))
    if max_chars is not None and chars > max_chars:     # protected text alone is over the cap: refused, never sent
        raise InputTooLarge(chars, max_chars)
    cacheable = 0
    for repeat in repeats:
        if not repeat:
            break
        cacheable += 1
    return Assembly(parts, cacheable, tuple(index in kept for index in range(len(active))), omitted_history,
                    content, chars)


def _note(count: int, reference: str) -> str:
    return f"[{count} older entries not shown here; fetch full text: {reference}]"


def _fold(count: int, first: str, last: str, route: str) -> str:
    return f"[{count} more not shown or listed one by one; fetch full text: {route} — from {first} to {last}]"


def _report(budget: int, reserved: int, content: int, over: bool, omitted: int) -> str:
    return "Context budget: " + json.dumps({"budget_tokens": budget, "reserved_tokens": reserved,
                                            "content_tokens_estimate": content, "method": token_method(),
                                            "over_budget": over, "omitted_blocks": omitted})


def overhead_tokens(budget_tokens: int, reserved_tokens: int = 0) -> int:
    """What `assemble` adds around one protected block at most (its budget report and separator): what a caller that
    splits work to fit a window keeps free beside its own text."""
    return count_tokens(_report(budget_tokens, reserved_tokens, 10 ** 9, True, 1)) + 2 * count_tokens("\n\n")


def overhead_chars(budget_tokens: int, reserved_tokens: int = 0) -> int:
    """`overhead_tokens` in characters: what a caller splitting work to fit a per-input character cap keeps free."""
    return len(_report(budget_tokens, reserved_tokens, 10 ** 9, True, 1)) + 2 * len("\n\n")
