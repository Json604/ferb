"""Fixtures for the Rust port of prompt context and structured answers (ENG-3): ferb/core/context.py,
ferb/core/prompts.py, ferb/core/structured.py, ferb/orchestrator/schemas.py and ferb/runtimes/schema_shape.py, run on
invented inputs. rust/fixtures/catalog holds the frozen schemas and repair notes; this adds what it doesn't cover.

Run from the repo root:  uv run python rust/fixtures/context/make.py

A wider run that leaves the tracked files alone: `make.py --seed 7 --scale 5 --out /some/temp/folder`, then
`FERB_CONTEXT_FIXTURES=/some/temp/folder cargo test --test parity` (manifest path as in AGENT-BRIEF.md).

Writes rust/fixtures/context/*.json and rust/crates/ferb-context/src/generated.rs (Python 3.12's `str.isprintable`
table, for `repr`). FERB_HOME and HOME point into a temporary folder; the o200k_base ranks are copied there from the
tiktoken-rs crate the Rust port embeds (checked against context.py's pinned SHA-256), so Python and Rust count with
the same file. Nothing is sent anywhere. The random source is seeded: the same Python build writes the same files.

Files:
- tokens.json: count_tokens on invented strings (ASCII, contractions, CJK, emoji, combining marks, long whitespace,
  random code points, random bytes decoded with replacement, texts across the 8,192-character slices), GrowingCount,
  comma_bounds and uncut_count.
- assemble.json: assemble() over invented block sets at many budgets, reserves and character caps (o200k and the
  UTF-8 fallback), and overhead_tokens/overhead_chars.
- structured.json: check_final on answers made by mutating valid answers of every answer schema (the 86 the catalog
  names and schemas.py's parts), on texts that aren't JSON, on answers mapped back from Anthropic's form; schema_shape's
  functions on invented schemas; the JSON-level readers of schemas.py.
- prompts.json: load_prompt on an invented package (links, suffixes, line endings, placeholders) and the SHA-256 of
  every Markdown prompt of the real package as load_prompt reads it.
"""

import argparse
import base64
import glob
import hashlib
import importlib.metadata
import json
import os
import random
import shutil
import sys
import tempfile
from pathlib import Path
from typing import Any, Callable

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[2]
CRATE = REPO / "rust" / "crates" / "ferb-context"
sys.dont_write_bytecode = True
sys.path.insert(0, str(REPO))

PINNED = "446a9538cb6c348e3516120d7c08b09f57c36495e2acfffe59a5bf8b0cfb1a2d"


def find_ranks() -> bytes:
    """o200k_base.tiktoken as the tiktoken-rs crate carries it (before HOME moves to the temporary folder)."""
    pattern = str(Path.home() / ".cargo" / "registry" / "src" / "*" / "tiktoken-rs-0.12.1" / "assets" /
                  "o200k_base.tiktoken")
    for path in sorted(glob.glob(pattern)):
        data = Path(path).read_bytes()
        if hashlib.sha256(data).hexdigest() == PINNED:
            return data
    raise SystemExit("o200k_base.tiktoken from tiktoken-rs 0.12.1 not found: build ferb-context once (cargo fetch)")


RANKS = find_ranks()
TMP = Path(tempfile.mkdtemp(prefix="ferbctx-"))
os.environ["FERB_HOME"] = str(TMP / "home")
os.environ["HOME"] = str(TMP / "user")
os.environ["TZ"] = "UTC"
(TMP / "user").mkdir()
(TMP / "home" / "cache" / "tokenizer").mkdir(parents=True)
(TMP / "home" / "cache" / "tokenizer" / "o200k_base.tiktoken").write_bytes(RANKS)
tempfile.tempdir = str(TMP)

import ferb.core.context as C  # noqa: E402
import ferb.core.prompts as P  # noqa: E402
from ferb.core.errors import InputTooLarge  # noqa: E402
from ferb.core.structured import check_final  # noqa: E402
from ferb.orchestrator import schemas as S  # noqa: E402
from ferb.runtimes import schema_shape as SS  # noqa: E402

from tests.test_schema_shape import as_wire, sample  # noqa: E402   (the test's own answer generators)

assert C.token_method() == "o200k_base_chunked8192_estimate_provider_may_differ", "the tokenizer did not load"

# the gate's secrets check (tools/kb/gate.py): no generated text may look like a key
sys.path.insert(0, str(REPO / "tools" / "kb"))
import re  # noqa: E402
from gate import SECRET_PATTERNS  # noqa: E402
SECRET_RX = [re.compile(p) for p in SECRET_PATTERNS.values()]


def clean(text: str) -> bool:
    return not any(rx.search(text) for rx in SECRET_RX)


ARGS = argparse.ArgumentParser()
ARGS.add_argument("--seed", type=int, default=20261113)
ARGS.add_argument("--scale", type=int, default=1)
ARGS.add_argument("--out", type=Path, default=HERE)
OPTS = ARGS.parse_args()
RNG = random.Random(OPTS.seed)
SCALE = OPTS.scale


def write(name: str, value: dict[str, Any]) -> int:
    """One top-level key per line; a list's items one per line (small, and a changed case is a one-line diff)."""
    lines = ["{"]
    for n, (key, item) in enumerate(value.items()):
        end = "," if n < len(value) - 1 else ""
        if isinstance(item, list) and item:
            lines.append(f" {json.dumps(key)}: [")
            lines += [f"  {json.dumps(x, ensure_ascii=False)}" + ("," if i < len(item) - 1 else "")
                      for i, x in enumerate(item)]
            lines.append(" ]" + end)
        else:
            lines.append(f" {json.dumps(key)}: {json.dumps(item, ensure_ascii=False)}{end}")
    text = "\n".join(lines) + "\n}\n"
    OPTS.out.mkdir(parents=True, exist_ok=True)
    (OPTS.out / name).write_text(text, encoding="utf-8")
    return len(text.encode())


def sha(text: str) -> str:
    return hashlib.sha256(text.encode("utf-8")).hexdigest()


# ---------------------------------------------------------------------------------------------------------------
# generated.rs: str.isprintable
# ---------------------------------------------------------------------------------------------------------------

def gen_tables() -> None:
    ranges: list[tuple[int, int]] = []
    for cp in range(0x110000):
        if 0xD800 <= cp <= 0xDFFF or chr(cp).isprintable():
            continue
        if ranges and ranges[-1][1] == cp - 1:
            ranges[-1] = (ranges[-1][0], cp)
        else:
            ranges.append((cp, cp))
    lines = ["// Generated by rust/fixtures/context/make.py from Python "
             f"{sys.version_info[0]}.{sys.version_info[1]}.{sys.version_info[2]} "
             "(do not edit): the code points whose `str.isprintable()` is false (surrogates left out: a Rust char "
             "is never one).", "",
             "/// Inclusive ranges, sorted.",
             "pub const NOT_PRINTABLE: &[(u32, u32)] = &["]
    for lo, hi in ranges:
        lines.append(f"    (0x{lo:04X}, 0x{hi:04X}),")
    lines.append("];")
    (CRATE / "src" / "generated.rs").write_text("\n".join(lines) + "\n", encoding="utf-8")


# ---------------------------------------------------------------------------------------------------------------
# invented text
# ---------------------------------------------------------------------------------------------------------------

WORDS = ("the owner asked FERB to sort the invoices folder by month and tell them which ones are missing "
         "a quick note about Kyoto trip plans résumé naïve façade coöperate Zürich São Paulo").split()
CONTRACTIONS = ["don't", "I'LL", "it's", "WE'RE", "they've", "She'd", "you'Re", "I'm", "o'clock", "'s", "'S",
                "can't", "ISN'T", "y'all", "rock'n'roll"]
PUNCT = list(",.;:!?-–—()[]{}<>/\\|@#$%^&*_+=~`'\"…«»“”‘’")
WS = [" ", "  ", "\t", "\n", "\r\n", "\r", "\u00a0", "\u3000", "\u2028", "\u2029", "\u000b", "\u000c", "\u0085",
      "\u200b", "\u001c", "\u001f", " \n ", "\n\n\n", "    \n", "\t\t \r\n"]
EMOJI = ["😀", "👍🏽", "👨‍👩‍👧‍👦", "🇯🇵", "🇺🇸", "❤️", "✔︎", "🧑🏿‍💻", "🏳️‍🌈", "⌚", "🫠", "🪿", "©", "™", "🔟", "#️⃣"]


def rand_cp_text(n: int, lo: int, hi: int) -> str:
    out = []
    while len(out) < n:
        cp = RNG.randint(lo, hi)
        if 0xD800 <= cp <= 0xDFFF:
            continue
        out.append(chr(cp))
    return "".join(out)


def ascii_text(n: int) -> str:
    parts = []
    while sum(map(len, parts)) < n:
        r = RNG.random()
        if r < 0.5:
            parts.append(RNG.choice(WORDS))
        elif r < 0.6:
            parts.append(RNG.choice(CONTRACTIONS))
        elif r < 0.7:
            parts.append(str(RNG.randint(0, 10 ** RNG.randint(1, 14))))
        elif r < 0.85:
            parts.append(RNG.choice(PUNCT) * RNG.randint(1, 3))
        else:
            parts.append(RNG.choice(WORDS).upper())
        parts.append(RNG.choice([" ", " ", " ", "", ", ", "\n", "  "]))
    return "".join(parts)


def mixed_text(n: int) -> str:
    pick = RNG.random()
    if pick < 0.2:
        return ascii_text(n)
    if pick < 0.3:
        return rand_cp_text(n, 0x4E00, 0x9FFF) + ascii_text(RNG.randint(0, 20))
    if pick < 0.36:
        return rand_cp_text(n, 0x3040, 0x30FF)
    if pick < 0.42:
        return rand_cp_text(n, 0xAC00, 0xD7A3)
    if pick < 0.5:
        return "".join(RNG.choice(EMOJI + WORDS + [" "]) for _ in range(max(1, n // 3)))
    if pick < 0.58:
        return "".join(RNG.choice(WS + WORDS[:5]) for _ in range(max(1, n // 2)))
    if pick < 0.66:
        return rand_cp_text(n, 0, 0x10FFFF)
    if pick < 0.72:
        return rand_cp_text(n, 0, 0x2FFF)
    if pick < 0.8:
        return RNG.randbytes(n).decode("utf-8", "replace")
    if pick < 0.86:
        base = "aeiouAEIOUxyzßǅǈ"
        marks = [chr(c) for c in range(0x300, 0x370)] + ["\u0e31", "\u094d", "\u20dd", "\ufe0f"]
        return "".join(RNG.choice(base) + "".join(RNG.choice(marks) for _ in range(RNG.randint(0, 3)))
                       for _ in range(max(1, n // 2)))
    if pick < 0.92:
        return "".join(RNG.choice(["'s", "'S", "'t", "'RE", "'Ve", "'m", "'LL", "'d", "'x", "’s", "'"]) +
                       RNG.choice(["", "a", "A", "é", "1", " ", "\n"]) for _ in range(max(1, n // 3)))
    return "".join(RNG.choice(WS + PUNCT + CONTRACTIONS + EMOJI + WORDS + ["1234567", "٣٤٥", "Ⅻ", "½", "𝟘𝟙"])
                   for _ in range(max(1, n // 3)))


def text_of(n: int) -> str:
    while True:
        t = mixed_text(n)
        if clean(t):
            return t


# ---------------------------------------------------------------------------------------------------------------
# tokens.json
# ---------------------------------------------------------------------------------------------------------------

def long_recipes(short: list[str]) -> list[list[list[Any]]]:
    """Long texts as recipes, to keep the file small: each piece is [text, times] or [case index] (a short case's
    text); the text is the pieces joined."""
    S_ = C.TOKEN_SLICE
    out: list[list[list[Any]]] = [
        [[" ", 9000]], [["\n", 8200]], [[" \n", 5000]], [["a", 20000]], [["1", 9000]], [["é", 8193]],
        [["😀", 8193]], [["x", 8191], ["😀", 1], ["y", 100]], [["x", 8190], ["'s", 1], ["z", 1]], [["word ", 4000]],
        [["a", S_ - 1], [" b", 1]], [["a", S_], [" b", 1]], [["a", S_ + 1], [" b", 1]], [["ab ", 5461], ["a", 1]],
        [["\t", 3 * S_]], [["一丁七万丈三上下不与", 1700]], [["\r\n", 4100], ["end", 1]], [["\u3000 ", 4500]],
        [["😀 ", 4097]], [["x", 8191], ["'S", 1]], [["1", 8191], ["23", 1]],
    ]
    for _ in range(14 * SCALE):
        pieces: list[list[Any]] = []
        n = 0
        target = RNG.choice([S_ - 1, S_, S_ + 1, 2 * S_ - 1, 2 * S_ + 3, RNG.randint(S_, 3 * S_)])
        while n < target:
            i = RNG.randrange(len(short))
            pieces.append([i])
            n += len(short[i])
        out.append(pieces)
    return out


def from_recipe(recipe: list[list[Any]], short: list[str]) -> str:
    return "".join(short[p[0]] if len(p) == 1 else p[0] * p[1] for p in recipe)


def tokens_fixture() -> dict[str, Any]:
    texts = ["", " ", "a", "\n\n", "Hello, world!", "I'm here; you're there.", "HELLO'S", "  leading", "trailing  ",
             "a\r\nb", "1234567890", "日本語のテキスト", "👨‍👩‍👧‍👦", "\u00a0\u3000", "x\u0301\u0302", "Context budget: {}",
             "[Context omitted; fetch full text: chat.history 3]"]
    for _ in range(3500 * SCALE):
        texts.append(text_of(RNG.choice([1, 2, 3, 5, 8, 13, 30, 60, 120, 250])))
    cases: list[dict[str, Any]] = [{"text": t, "tokens": C.count_tokens(t)} for t in texts]
    for recipe in long_recipes(texts):
        cases.append({"recipe": recipe, "tokens": C.count_tokens(from_recipe(recipe, texts))})
    every = [c["text"] if "text" in c else from_recipe(c["recipe"], texts) for c in cases]
    # the fallback (one token per UTF-8 byte) is asserted directly in Rust
    growing = []
    for _ in range(40 * SCALE):
        g = C.GrowingCount()
        steps = []
        for _ in range(RNG.randint(1, 6)):
            a = RNG.randrange(len(every))
            g.append(every[a])
            suffix = RNG.randrange(len(every))
            steps.append({"append": a, "suffix": suffix, "with_suffix": g.with_suffix(every[suffix]),
                          "closed": g.closed, "tail_chars": len(g.tail)})
        growing.append(steps)
    bounds = []
    for _ in range(100 * SCALE):
        t = text_of(RNG.choice([20, 200])) + ", " + text_of(10) + RNG.choice([",", ", ", ",,  ", " ,"])
        bounds.append({"text": t, "bounds": C.comma_bounds(t)})
    uncut = []
    for _ in range(60 * SCALE):
        t = "".join(text_of(RNG.randint(20, 3000)) + ", " for _ in range(RNG.randint(1, 12)))
        cuts = C.comma_bounds(t)
        ends = [0, *cuts, len(t)]
        atoms = [t[ends[i]:ends[i + 1]] for i in range(len(ends) - 1)]
        totals = [0]
        for a in atoms:
            totals.append(totals[-1] + (C.count_tokens(a) if len(a) <= C.TOKEN_SLICE else 0))
        for _ in range(5):
            lo = RNG.randint(0, len(atoms))
            hi = RNG.randint(lo, len(atoms))
            shift = RNG.choice([0, RNG.randint(0, 30000)])
            uncut.append({"ends": ends, "totals": totals, "lo": lo, "hi": hi, "shift": shift,
                          "out": C.uncut_count(ends, totals, lo, hi, shift)})
    return {"about": "count_tokens (o200k_base, the pinned ranks), GrowingCount (`append`/`suffix` index the cases), "
                     "comma_bounds, uncut_count; a case is a `text` or a `recipe` (see long_recipes)",
            "token_slice": C.TOKEN_SLICE, "method": C.token_method(), "pattern": C._PATTERN,
            "case_count": len(cases), "cases": cases, "growing": growing, "comma_bounds": bounds, "uncut": uncut}


# ---------------------------------------------------------------------------------------------------------------
# assemble.json
# ---------------------------------------------------------------------------------------------------------------

def block_set(kind: str) -> list[dict[str, Any]]:
    n = RNG.randint(0, 14)
    routes = ["", "", "calls", "procedures", "task.evidence t-7"] if kind != "fold" else ["calls", "calls", "steps", ""]
    blocks = []
    for i in range(n):
        size = RNG.choice([0, 3, 20, 80, 250, 600]) if RNG.random() > 0.05 else 0
        text = text_of(size) if size else ""
        hist = RNG.random() < (0.35 if kind == "chat" else 0.12)
        blocks.append({
            "text": text,
            "reference": RNG.choice([f"memory.read m-{i}", f"chat.history e{i} 2026-01-0{1 + i % 9}",
                                     f"task.evidence t-{i}", f"ref{i}", f"résumé ✓ {i}", ""]),
            "protected": RNG.random() < (0.2 if kind != "fold" else 0.1),
            "cache": RNG.random() < 0.5 if i < 3 else RNG.random() < 0.1,
            "priority": RNG.randint(-3, 6),
            "history": hist,
            "fold": "" if hist else RNG.choice(routes),
        })
    return blocks


def run_assemble(blocks: list[dict[str, Any]], budget: int, reserved: int, max_chars: int | None) -> dict[str, Any]:
    objs = [C.ContextBlock(text=b["text"], reference=b["reference"], protected=b["protected"], cache=b["cache"],
                           priority=b["priority"], history=b["history"], fold=b["fold"]) for b in blocks]
    try:
        a = C.assemble(objs, budget_tokens=budget, reserved_tokens=reserved, max_chars=max_chars)
    except InputTooLarge as e:
        return {"error": "InputTooLarge", "message": str(e)}
    except ValueError as e:
        return {"error": "ValueError", "message": str(e)}
    return {"parts_sha256": sha(json.dumps(a.parts)), "part_count": len(a.parts), "cacheable": a.cacheable, "shown": list(a.shown), "omitted_history": a.omitted_history,
            "tokens": a.tokens, "chars": a.chars}


def assemble_fixture() -> dict[str, Any]:
    sets = [block_set(RNG.choice(["plain", "chat", "fold", "fold"])) for _ in range(110 * SCALE)]
    cases = []
    for si, blocks in enumerate(sets):
        total = sum(C.count_tokens(b["text"]) for b in blocks) + 120
        chars = sum(len(b["text"]) for b in blocks) + 300
        budgets = sorted({1, 40, 150, 260, *(RNG.randint(1, max(2, total)) for _ in range(7)), total, total * 3})
        for budget in budgets:
            reserved = RNG.choice([0, 0, 10, 100, RNG.randint(0, max(1, budget))])
            max_chars = RNG.choice([None, None, None, RNG.randint(100, max(101, chars)), chars * 2])
            cases.append({"set": si, "budget": budget, "reserved": reserved, "max_chars": max_chars,
                          "tokenizer": "o200k", "out": run_assemble(blocks, budget, reserved, max_chars)})
    for budget, reserved, max_chars in ((0, 0, None), (-5, 0, None), (10, -1, None), (10, 0, 0), (10, 0, -3)):
        cases.append({"set": 0, "budget": budget, "reserved": reserved, "max_chars": max_chars, "tokenizer": "o200k",
                      "out": run_assemble(sets[0], budget, reserved, max_chars)})
    # the UTF-8 fallback: count_tokens with no tokenizer
    keep = C._encoding
    C._encoding = lambda: None   # type: ignore[assignment]
    try:
        assert C.token_method() == "utf8_byte_ceiling_tokenizer_unavailable"
        for si in RNG.sample(range(len(sets)), 25):
            total = sum(len(b["text"].encode()) for b in sets[si]) + 300
            for budget in sorted({60, RNG.randint(1, total), total}):
                reserved = RNG.choice([0, 25])
                cases.append({"set": si, "budget": budget, "reserved": reserved, "max_chars": None,
                              "tokenizer": "utf8", "out": run_assemble(sets[si], budget, reserved, None)})
        overhead_utf8 = [{"budget": b, "reserved": r, "tokens": C.overhead_tokens(b, r), "chars": C.overhead_chars(b, r)}
                         for b, r in ((1, 0), (200000, 30000))]
    finally:
        C._encoding = keep   # type: ignore[assignment]
    overhead = [{"budget": b, "reserved": r, "tokens": C.overhead_tokens(b, r), "chars": C.overhead_chars(b, r)}
                for b, r in ((1, 0), (8000, 3000), (200000, 30000), (10 ** 12, 7), (123456789, 98765))]
    return {"about": "assemble() on invented block sets; `set` indexes `sets`; `parts_sha256` is the SHA-256 of "
                     "json.dumps(parts)",
            "sets": sets,
            "case_count": len(cases), "cases": cases, "overhead": overhead, "overhead_utf8": overhead_utf8}


# ---------------------------------------------------------------------------------------------------------------
# structured.json
# ---------------------------------------------------------------------------------------------------------------

def all_schemas() -> dict[str, dict[str, Any]]:
    from ferb.daemon.areas.intro import SUGGEST_SCHEMA
    from ferb.memory.distill import DISTILL_SCHEMA
    from ferb.memory.import_extract import RECALL_SCHEMA, VERIFY_SCHEMA
    from ferb.memory.mac_profile import MAC_SCHEMA
    from ferb.memory.playbooks import SCHEMA as PLAYBOOK_SCHEMA
    from ferb.memory.profile import PROFILE_SCHEMA
    from ferb.runtimes.check import IMAGE_SCHEMA
    catalog = json.loads((REPO / "rust" / "fixtures" / "catalog" / "answer_schemas.json").read_text())["schemas"]
    out = {n: getattr(S, n) for n in dir(S) if n.endswith("_SCHEMA") and isinstance(getattr(S, n), dict)}
    out |= {"ATTENTION_SCHEMA": catalog["ATTENTION_SCHEMA"]["core"], "DISTILL_SCHEMA": DISTILL_SCHEMA,
            "RECALL_SCHEMA": RECALL_SCHEMA, "VERIFY_SCHEMA": VERIFY_SCHEMA, "MAC_SCHEMA": MAC_SCHEMA,
            "PLAYBOOK_SCHEMA": PLAYBOOK_SCHEMA, "PROFILE_SCHEMA": PROFILE_SCHEMA, "SUGGEST_SCHEMA": SUGGEST_SCHEMA,
            "IMAGE_SCHEMA": IMAGE_SCHEMA}
    from ferb.clock.when import WHEN_SCHEMA
    out["WHEN_SCHEMA"] = WHEN_SCHEMA
    flags = ("inputs", "proposals", "conversation", "watch", "loop", "parts")
    for bits in range(64):
        on = {f: bool(bits >> i & 1) for i, f in enumerate(flags)}
        out["worker_schema(" + ",".join(f for f in flags if on[f]) + ")"] = S.worker_schema(**on)
    return out


ODD = ["z" * 260, "it's", 'say "hi"', "both ' and \"", "tab\there", "nl\nx", "\x00\x7f\x9f", "é ✓ 日本 😀",
       "\u200b\u2028\u00ad\ufeff", "\\back\\slash", "", " ", "\U0010fffd", "\ue000"]
WRONG = [7, -0, 2.0, 1.5, 1e16, -1e-7, True, False, None, [], {}, [1, "a", None], {"k": [None, True, 2.5]},
         "seven", 12345678901234567890123, *ODD]


def paths(node: dict[str, Any], value: Any, at: tuple = ()) -> list[tuple[tuple, dict[str, Any], Any]]:
    """Every place in a valid answer with the schema node it answers."""
    out = [(at, node, value)]
    if isinstance(value, dict):
        props = node.get("properties") or {}
        branch = node
        if "anyOf" in node:
            branch = next((b for b in node["anyOf"] if b.get("type") == "object"), node)
            props = branch.get("properties") or {}
        for k, v in value.items():
            if k in props:
                out += paths(props[k], v, at + (k,))
    elif isinstance(value, list):
        items = node.get("items")
        if "anyOf" in node:
            items = next((b.get("items") for b in node["anyOf"] if b.get("type") == "array"), None)
        if isinstance(items, dict):
            for i, v in enumerate(value):
                out += paths(items, v, at + (i,))
    return out


def set_at(value: Any, at: tuple, new: Any) -> Any:
    if not at:
        return new
    head, rest = at[0], at[1:]
    if isinstance(value, dict):
        return {k: (set_at(v, rest, new) if k == head else v) for k, v in value.items()}
    return [set_at(v, rest, new) if i == head else v for i, v in enumerate(value)]


def get_at(value: Any, at: tuple) -> Any:
    for k in at:
        value = value[k]
    return value


def varied(node: Any) -> Any:
    """A valid answer with random strings, list lengths and nullable choices."""
    if not isinstance(node, dict):
        return None
    if SS.nullable(node) and RNG.random() < 0.4:
        return None
    if "anyOf" in node:
        options = [b for b in node["anyOf"] if b.get("type") != "null"]
        return varied(RNG.choice(options))
    if "enum" in node:
        return RNG.choice([v for v in node["enum"] if v is not None] or [None])
    kind = node.get("type")
    if isinstance(kind, list):
        kind = RNG.choice([k for k in kind if k != "null"] or ["null"])
    if kind == "object":
        return {k: varied(v) for k, v in (node.get("properties") or {}).items()}
    if kind == "array":
        return [varied(node.get("items")) for _ in range(max(node.get("minItems", 0), RNG.choice([0, 1, 1, 2, 3])))]
    if kind == "string":
        return RNG.choice(["x", "a note", "résumé ✓", "it's", "日本", ""])
    if kind == "integer":
        return RNG.choice([0, 1, 42, -3, 2.0])
    if kind == "number":
        return RNG.choice([0, 1.5, 7, 1e16, -0.25])
    if kind == "boolean":
        return RNG.choice([True, False])
    return None


def mutate(schema: dict[str, Any], good: Any) -> Any:
    places = paths(schema, good)
    for _ in range(RNG.choice([1, 1, 1, 2, 3])):
        places = paths(schema, good)
        at, node, value = RNG.choice(places)
        r = RNG.random()
        if isinstance(value, dict) and r < 0.3:
            req = [k for k in value]
            if req and RNG.random() < 0.6:
                drop = RNG.choice(req)
                good = set_at(good, at, {k: v for k, v in value.items() if k != drop})
            else:
                extra = RNG.choice(["unasked", "zeta", "Alpha", "a'b", 'x"y', "é", "0", "", "kind2"])
                good = set_at(good, at, {**value, extra: RNG.choice(WRONG)})
        elif isinstance(value, list) and r < 0.5:
            if value and RNG.random() < 0.5:
                good = set_at(good, at + (RNG.randrange(len(value)),), RNG.choice(WRONG))
            else:
                good = set_at(good, at, value + [RNG.choice(WRONG)])
        elif "enum" in node and r < 0.7:
            good = set_at(good, at, RNG.choice(ODD + ["DONE", "none ", "ask"]))
        else:
            good = set_at(good, at, RNG.choice(WRONG))
    return good


def targeted(schema: dict[str, Any], good: Any) -> list[Any]:
    """Answers wrong where best_match has to choose: an empty list under minItems, a key missing or a value wrong
    inside a nullable object (anyOf's context), and two problems at the same depth."""
    out = []
    for at, node, value in paths(schema, good):
        if isinstance(value, list) and node.get("minItems"):
            out.append(set_at(good, at, []))
        if "anyOf" in node and isinstance(value, dict) and value:
            keys = list(value)
            out.append(set_at(good, at, {k: v for k, v in value.items() if k != RNG.choice(keys)}))
            k = RNG.choice(keys)
            out.append(set_at(good, at, {**value, k: RNG.choice(WRONG)}))
            if len(keys) > 1:
                a, b = RNG.sample(keys, 2)
                out.append(set_at(good, at, {**value, a: RNG.choice(WRONG), b: RNG.choice(WRONG)}))
    if isinstance(good, dict) and len(good) > 1:
        a, b = RNG.sample(list(good), 2)
        out.append({**good, a: RNG.choice(WRONG), b: RNG.choice(WRONG)})
        out.append({k: v for k, v in good.items() if k not in (a, b)} | {"Zextra": 1, "aextra": 2})
    return out


def as_text(value: Any) -> str:
    r = RNG.random()
    if r < 0.6:
        return json.dumps(value)
    if r < 0.85:
        return json.dumps(value, ensure_ascii=False)
    if r < 0.95:
        return json.dumps(value, indent=RNG.choice([1, 2]))
    return json.dumps(value, separators=(",", ":"))


def judge(text: str, schema: Any) -> dict[str, Any]:
    try:
        assert check_final(text, schema) == text
        return {"ok": True}
    except ValueError as e:
        return {"ok": False, "error": str(e)}


def not_json_texts() -> list[str]:
    base = ['{"answer": "x"}', '{"a": [1, 2, {"b": null}], "c": "d"}', '[true, false, null]', '"text"', '12.5e3']
    out = ["", "   ", "{", "}", "[", "]", "[1,]", "{\"a\":}", "{\"a\" 1}", "{\"a\": 1 \"b\": 2}", "nul", "tru",
           "-", "-x", "01", "1.", "1.e5", ".5", "+1", "0x10", "1e", "1e+", "\"\\u12\"", "\"\\uZZZZ\"",
           "\"a\\qb\"", "\"tab\there\"", "\"\x01\"", "'single'", "{'a': 1}", "{a: 1}", "[1 2]", "\ufeff{}",
           "\ufeff", "{\"a\": 1}}", "{} {}", "NaN", "Infinity", "-Infinity", "nan", "inf", "[NaN, Infinity]",
           "1" * 4300, "1" * 4301, "-" + "1" * 4301, "[" + "9" * 5000 + "]", "[" + "9" * 5000 + ", }",
           "[1, " + "2" * 4400 + " x]", "{\"a\": 1, }", "\"\\ud83d\\ude00\"", "\"\\\"\"", "[\"\\/\"]",
           "{\"a\":1,\"a\":2}", "[" * 3000 + "]" * 3000, "[" * 3000 + "]" * 2999, "{\"k\": \"v\"}\n\n",
           "\n\t {\"k\" : [ 1 , 2 ] } \r\n", "\"\u2028\u2029\"", "[1.0, 1E400, -0, -0.0, 1e-400]"]
    for _ in range(80 * SCALE):
        b = RNG.choice(base)
        i = RNG.randint(0, len(b))
        r = RNG.random()
        if r < 0.4:
            out.append(b[:i])
        elif r < 0.8:
            out.append(b[:i] + RNG.choice([",", "]", "}", "\"", ":", "\\", "x", " ", "\x02", "\n", "{", "["]) + b[i:])
        else:
            out.append(b[:i] + b[i + 1:])
    return out


def mapped_back(schemas: dict[str, Any]) -> list[dict[str, Any]]:
    out = []
    for name, schema in schemas.items():
        shape = SS.for_anthropic(schema)
        for fill in (False, True):
            answer = sample(schema, fill)
            wire = as_wire(schema, shape.wire, answer)
            variants = [("as written", wire)]
            if isinstance(wire, dict):
                required = set(shape.wire.get("required", []))
                variants.append(("every optional left out", {k: v for k, v in wire.items() if k in required}))
                variants.append(("an unknown property kept", {**wire, "unasked": "é"}))
                lists = [k for k, v in wire.items() if isinstance(v, list) and
                         (shape.wire["properties"].get(k) or {}).get("description", "").endswith(SS.WRAPPED_NOTE)]
                for k in lists[:3]:
                    item = wire[k][0] if wire[k] else {}
                    variants.append((f"two items for {k}", {**wire, k: [item, item]}))
                    variants.append((f"no item for {k}", {**wire, k: []}))
                    variants.append((f"a string for {k}", {**wire, k: "x"}))
                    variants.append((f"null for {k}", {**wire, k: None}))
            for _ in range(2 * SCALE):
                variants.append(("mutated", mutate(shape.wire, wire)))
            for what, w in variants:
                text = json.dumps(w, ensure_ascii=RNG.random() < 0.5)
                core = shape.answer_text(text)
                out.append({"schema": name, "what": what, "mapped": shape.mapped, "wire_answer": text,
                            "core_answer": core, **judge(core, schema)})
    for text in ("oops {", "[1, 2]", "null", "\"s\"", "{\"task\": [{}], \"x\": 1}", "1" * 4301):
        shape = SS.for_anthropic(S.SUPERVISOR_SCHEMA)
        out.append({"schema": "SUPERVISOR_SCHEMA", "what": "odd text", "mapped": shape.mapped, "wire_answer": text,
                    "core_answer": shape.answer_text(text), **judge(shape.answer_text(text), S.SUPERVISOR_SCHEMA)})
    return out


def synthetic_schemas() -> list[dict[str, Any]]:
    """Invented schemas for schema_shape's functions: nullable objects and scalars at every depth, type lists with
    descriptions and enums, enums holding null, $defs, allOf, items, over-limit shapes."""
    def leaf() -> dict[str, Any]:
        r = RNG.random()
        if r < 0.15:
            return {"type": ["string", "null"], "description": RNG.choice(["a note", "", "née ✓"])}
        if r < 0.25:
            return {"type": ["string", "null"], "enum": ["a", "b", None]}
        if r < 0.32:
            return {"description": "first", "type": ["integer", "null"], "enum": [1, 2, None]}
        if r < 0.4:
            return {"anyOf": [{"type": "string"}, {"type": "integer"}]}
        if r < 0.47:
            return {"anyOf": [{"type": "null"}, {"type": "number"}], "description": "maybe"}
        if r < 0.52:
            return {"enum": ["x", None]}
        if r < 0.56:
            return {"type": ["string", "integer", "null"]}
        if r < 0.6:
            return {"type": "string", "enum": ["p", "q"], "description": "pick"}
        return {"type": RNG.choice(["string", "integer", "number", "boolean"])}

    def obj(depth: int) -> dict[str, Any]:
        props: dict[str, Any] = {}
        for i in range(RNG.randint(1, 6)):
            r = RNG.random()
            if depth < 3 and r < 0.25:
                props[f"o{i}"] = {"anyOf": [{"type": "null"}, obj(depth + 1)], **({"description": "an optional object"} if RNG.random() < 0.5 else {})}
            elif depth < 3 and r < 0.35:
                props[f"t{i}"] = {**obj(depth + 1), "type": ["object", "null"]}
            elif depth < 3 and r < 0.5:
                props[f"a{i}"] = {"type": "array", "items": obj(depth + 1) if RNG.random() < 0.5 else leaf(),
                                  **({"minItems": 1} if RNG.random() < 0.3 else {})}
            else:
                props[f"p{i}"] = leaf()
        required = [k for k in props if RNG.random() < 0.8]
        order = RNG.random()
        if order < 0.5:
            return {"type": "object", "additionalProperties": False, "required": required, "properties": props}
        if order < 0.8:
            return {"type": "object", "properties": props, "required": required, "additionalProperties": False}
        return {"properties": props, "type": "object", "required": required}

    out = [obj(0) for _ in range(32 * SCALE)]
    out.append({"type": "object", "$defs": {"D": obj(1)}, "properties": {"d": {"$ref": "#/$defs/D"}},
                "allOf": [obj(1)], "required": []})
    props = {f"f{i}": {"type": "object", "additionalProperties": False, "required": ["v"],
                       "properties": {"v": {"anyOf": [{"type": "string"}, {"type": "integer"}]}}} for i in range(20)}
    out.append({"type": "object", "additionalProperties": False, "required": list(props), "properties": props})
    out.append({"type": "object", "properties": {f"n{i}": {"type": ["string", "null"]} for i in range(40)},
                "required": [f"n{i}" for i in range(40)]})
    out.append({"type": "object", "properties": {"e": {"type": ["string", "integer", "null"], "enum": ["a", 1, None]}}})
    out.append({"type": "object", "properties": {"e": {"enum": ["a", None]}}})
    out.append({"type": "string", "description": "√ non-ASCII \u2028 description", "enum": ["a\"b", "c\\d"]})
    return out


def shape_cases() -> list[dict[str, Any]]:
    cases = []
    for schema in synthetic_schemas():
        entry: dict[str, Any] = {"schema": schema, "complexity": list(SS.complexity(schema)),
                                 "keywords": sorted(SS.keywords(schema)), "enum_with_null": SS.enum_with_null(schema),
                                 "stated_sha256": sha(SS.stated(schema)),
                                 "refusal_key": SS.Refusals.key("gemini", None, schema)}
        nodes = [schema, *(schema.get("properties") or {}).values()]
        entry["nullable"] = [SS.nullable(n) for n in nodes]
        entry["is_union"] = [SS.is_union(n) for n in nodes]
        try:
            shape = SS.for_anthropic(schema)
            entry["anthropic"] = {"wire": shape.wire, "mapped": shape.mapped,
                                  "wire_default_sha256": sha(json.dumps(shape.wire))}
            answers = []
            for _ in range(2):
                good = varied(shape.wire)
                text = json.dumps(good)
                core = shape.answer_text(text)
                field = {k: json.dumps(shape.field(k, v)) for k, v in good.items()} if isinstance(good, dict) else {}
                answers.append({"wire_answer": text, "core_answer": core, "fields": field,
                                **judge(core, schema)})
            entry["anthropic"]["answers"] = answers
        except SS.SchemaTooComplex as e:
            entry["anthropic"] = {"refused": str(e)}
        try:
            g = SS.for_gemini(schema)
            entry["gemini"] = {"wire": g.wire, "wire_default_sha256": sha(json.dumps(g.wire))}
        except SS.SchemaTooComplex as e:
            entry["gemini"] = {"refused": str(e)}
        cases.append(entry)
    return cases


def VALIDATORS(model: Any) -> dict[str, Callable[[Any], Any]]:
    return {k: v.func for k, v in model.__pydantic_decorators__.model_validators.items()}


def readers() -> dict[str, Any]:
    sup = S.SUPERVISOR_SCHEMA
    answers = []
    for _ in range(14 * SCALE):
        a = varied(sup)
        if RNG.random() < 0.5 and isinstance(a, dict):
            a = S.ungrouped(a)
        if RNG.random() < 0.3:
            a = {**a, "schedule": varied(S.SCHEDULE_SCHEMA) if RNG.random() < 0.7 else None}
        if RNG.random() < 0.2:
            a = {**a, "show": RNG.choice([["schedules"], [], "schedules", None]), "about_tasks": []}
        if RNG.random() < 0.3:
            a = {**a, "kind": "task", "task": None, "control": None}
        answers.append(a)
    answers += [None, 5, "text", [], {}, {"side_entries": None}, {"side_entries": {"remember": [1]}, "remember": []},
                {"kind": "task", "task": None, "control": None, "schedule": {"request": "x"}},
                {"kind": "task", "task": None, "control": None, "side_entries": {"show_schedules": True}},
                {"kind": "task", "task": {"goal": "g"}, "control": {"action": "stop"}},
                {"kind": "task", "task": {"goal": "g"}, "control": {"action": "continue"}},
                {"kind": "reply", "task": {"goal": "g"}}]
    out = []
    for a in answers:
        row: dict[str, Any] = {"answer": a, "ungrouped": S.ungrouped(a)}
        if isinstance(a, dict):
            # pydantic runs `mode="before"` validators last-defined first: _entries_only, then _one_schedule
            row["entries_only"] = VALIDATORS(S.SupervisorOutcome)["_entries_only"](a)
            row["one_schedule"] = VALIDATORS(S.SupervisorOutcome)["_one_schedule"](row["entries_only"])
            try:
                row["grouped"] = S.grouped(a)
            except Exception as e:   # noqa: BLE001  (a value grouped can't read: recorded, not ported)
                row["grouped_error"] = type(e).__name__
            kind, task, control = a.get("kind"), a.get("task"), a.get("control")
            row["new_task"] = (task if kind == "task" and task is not None and
                               (control is None or (isinstance(control, dict) and control.get("action") in ("stop", "pause")))
                               else None)
        out.append(row)
    checks = [{"in": c, "out": VALIDATORS(S.Check)["_legacy_ledger_done"](c)} for c in
              ({"kind": "ledger_done", "path": None}, {"kind": "none"}, "x", None)]
    conds = [{"in": c, "out": VALIDATORS(S.Condition)["_no_code_check"](c)} for c in
             ({"text": "t", "check": None}, {"text": "t"}, {"text": "t", "check": {"kind": "sent"}}, [])]
    untils = [{"in": c, "out": VALIDATORS(S.ScheduleUntil)["_unused"](c)} for c in
              ({"quote": "", "day": "", "date": "2026-01-02", "in_days": 0, "time": "", "in_minutes": 0.0},
               {"quote": "until 6", "day": "friday", "in_days": 3, "in_minutes": False, "time": None}, 7)]
    return {"supervisor": out, "check": checks, "condition": conds, "schedule_until": untils}


def structured_fixture() -> dict[str, Any]:
    schemas = all_schemas()
    cases = []
    for name, schema in schemas.items():
        big = name in ("SUPERVISOR_SCHEMA", "TASK_SCHEMA", "SIDE_ENTRIES_SCHEMA", "SCHEDULE_SCHEMA", "PLAN_SCHEMA")
        per = (5 if name.startswith("worker_schema(") else 15 if big else 11) * SCALE
        for i in range(per):
            good = varied(schema) if i % 3 else sample(schema, bool(i % 2))
            bad = mutate(schema, good) if i % 7 else good
            text = as_text(bad)
            if not clean(text):
                continue
            cases.append({"schema": name, "answer": text, **judge(text, schema)})
    for name, schema in schemas.items():
        if name.startswith("worker_schema(") and RNG.random() < 0.7:
            continue
        good = sample(schema, True)
        for bad in targeted(schema, good)[:12]:
            text = as_text(bad)
            if clean(text):
                cases.append({"schema": name, "answer": text, **judge(text, schema)})
    for text in not_json_texts():
        if clean(text):
            cases.append({"schema": "FIX_SCHEMA", "answer": text, **judge(text, S.FIX_SCHEMA)})
    deep = {"schema": "FIX_SCHEMA", "answer": "{\"answer\": " + "[" * 2000 + "]" * 2000 + "}"}
    cases.append({**deep, **judge(deep["answer"], S.FIX_SCHEMA)})
    for text in ('{"answer": "x"}', "anything", ""):
        cases.append({"schema": None, "answer": text, **judge(text, None)})
        cases.append({"schema": "{}", "answer": text, **judge(text, {})})
    fences = ['```json\n{"a": 1}\n```', '```\n{}\n```', '```JSON\n[1]\n```', '```Json\n{}\n```', '``` json \n{}\n```',
              '\n\t```json\n {"a": 1} \n```\u3000', '```json```', '``````', '`````', '```\n```', '```json\n```',
              '```json\r\n{}\r\n```', 'x```json\n{}\n```', '```json\n{}\n```x', '```py\n1\n```',
              '```json\n{"a": "```"}\n```', '```\u00a0json\n{}\n```', '```json\n\n\n```', '````json\n{}\n````']
    return {"about": "check_final on invented answers (`schema` names a schema in `schemas`), schema_shape on "
                     "invented schemas, schemas.py's JSON-level readers",
            "jsonschema": importlib.metadata.version("jsonschema"),
            "schemas": {n: s for n, s in schemas.items() if not n.startswith("worker_schema(")},
            "case_count": len(cases), "invalid_count": sum(1 for c in cases if not c["ok"]),
            "cases": cases, "mapped_back": mapped_back({n: schemas[n] for n in
                                                         ("SUPERVISOR_SCHEMA", "WORKER_ALL_SCHEMA", "CHECKER_SCHEMA",
                                                          "WORKER_WATCH_SCHEMA", "DISTILL_SCHEMA", "TASK_SCHEMA",
                                                          "SIDE_ENTRIES_SCHEMA", "worker_schema(watch,loop)",
                                                          "STEP_SCHEMA")}),
            "shapes": shape_cases(), "readers": readers(),
            "unfenced": [{"text": t, "out": SS.unfenced(t), "repair_note": SS.repair_note(t)} for t in fences],
            "refusal_keys": [{"provider": p, "model": m, "schema": n,
                              "key": SS.Refusals.key(p, m, SS.for_anthropic(schemas[n]).wire)}
                             for p, m, n in (("anthropic", None, "FIX_SCHEMA"), ("anthropic", "", "CHECKER_SCHEMA"),
                                             ("claude_sdk", "claude-x ✓", "SUPERVISOR_SCHEMA"),
                                             ("api", "m\u2028", "WORKER_ALL_SCHEMA"))]}


# ---------------------------------------------------------------------------------------------------------------
# prompts.json
# ---------------------------------------------------------------------------------------------------------------

def prompts_fixture() -> dict[str, Any]:
    pkg = TMP / "pkg" / "ferb"
    files: dict[str, bytes] = {
        "a.md": b"  \n# Title {name}\n\nBody {name} and {other} {name}\n\n\t ",
        "crlf.md": b"line one\r\nline two\rline three\r\n\r\n",
        "bom.md": "\ufeff  hello\u3000".encode(),
        "ws.md": "\u001c\u00a0 inner \u2028\u001f".encode(),
        "dir/deep.md": "{a}{b}{a}é".encode(),
        "x.MD": b"upper suffix",
        ".md": b"hidden",
        "a.": b"dot",
        "t.tar.md": b"double suffix",
        "notes.txt": b"not markdown",
        "bad.md": b"\xff\xfe broken",
        "empty.md": b"",
        "dir.md/inner.md": b"in a folder named like a file",
    }
    outside = {"../outside.md": b"outside the package"}
    for rel, data in {**files, **outside}.items():
        path = (pkg / rel).resolve()
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)
    links = {"link.md": "a.md", "out.md": "../outside.md", "dirlink": "dir", "loop1.md": "loop2.md",
             "loop2.md": "loop1.md", "abs.md": str(TMP / "pkg" / "outside.md")}
    for name, target in links.items():
        os.symlink(target, pkg / name)
    keep = P.PACKAGE
    P.PACKAGE = pkg.resolve()
    calls = [("a.md", None), ("a.md", {"name": "N", "other": "{name}"}), ("a.md", {"other": "{name}", "name": "N"}),
             ("a.md", {"": "E", "name": ""}), ("crlf.md", None), ("bom.md", None), ("ws.md", None),
             ("dir/deep.md", {"a": "1", "b": "{a}"}), ("dir/../a.md", None), ("./a.md", None), ("x.MD", None),
             (".md", None), ("a.", None), ("t.tar.md", None), ("notes.txt", None), ("bad.md", None), ("empty.md", None),
             ("dir.md", None), ("dir.md/inner.md", None), ("../outside.md", None), ("missing.md", None),
             ("link.md", None), ("out.md", None), ("dirlink/deep.md", {"a": "x"}), ("loop1.md", None),
             ("abs.md", None), (str(pkg / "a.md"), None), ("/etc/hosts", None), ("dir/missing/../deep.md", None)]
    cases = []
    for rel, repl in calls:
        try:
            out: dict[str, Any] = {"text": P.load_prompt(rel, repl)}
        except ValueError as e:
            out = {"error": type(e).__name__, "message": str(e)}
        except (OSError, RuntimeError) as e:     # RuntimeError: Path.resolve's "Symlink loop"
            out = {"error": type(e).__name__}
        cases.append({"path": rel.replace(str(pkg), "<PKG>"), "replacements": list((repl or {}).items()), **out})
    P.PACKAGE = keep
    real = []
    for path in sorted((REPO / "ferb").rglob("*.md")):
        rel = str(path.relative_to(REPO / "ferb"))
        text = P.load_prompt(rel)
        names = sorted(set(re.findall(r"\{([a-z_]+)\}", text)))
        repl = {n: f"<{n} ✓>" for n in names}
        real.append({"path": rel, "sha256": sha(text), "chars": len(text),
                     "replacements": list(repl.items()), "replaced_sha256": sha(P.load_prompt(rel, repl))})
    return {"about": "load_prompt on an invented package (`files` and `links`, written by the test into a temp "
                     "folder named ferb, with outside.md beside it) and on every Markdown file of the real package",
            "files": {k: base64.b64encode(v).decode() for k, v in {**files, **outside}.items()},
            "links": links | {"abs.md": "<TMP>/pkg/outside.md"}, "cases": cases, "real": real}


def main() -> None:
    sizes = {}
    if OPTS.out == HERE:
        gen_tables()
    for name, make in (("tokens.json", tokens_fixture), ("assemble.json", assemble_fixture),
                       ("structured.json", structured_fixture), ("prompts.json", prompts_fixture)):
        value = make()
        sizes[name] = write(name, value)
    manifest = {"seed": OPTS.seed, "scale": SCALE, "files": sizes}
    write("manifest.json", manifest)
    shutil.rmtree(TMP, ignore_errors=True)
    print(json.dumps(manifest, indent=1))


main()
