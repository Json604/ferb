"""C1 (re-review 6, rule 14): the redactors run on the daemon's event loop (every shell and PTY read, every streamed
model reply, every captured text), so no input may make one call cost more than in proportion to its size. A pattern
that can match one run of characters in several ways backtracks exponentially when the match then fails ("482913",
20 spaces, two words: 80 ms per read; FERB's own proxy.py line 69: past 20 s), and a scan that restarts at every
position of a run is quadratic.

These tests are structural, not wall-clock bounds: each doubles a pathological run (spaces, tabs, no-break spaces,
markup characters, quote marks, digits) and checks the time per call about doubles.
A call cheaper than `FLOOR` is never judged (timer noise), and a suspect ratio is measured again, best of five."""
import pathlib
import re
import time
from typing import Callable

import pytest

from ferb.sanitize import images, redact
from ferb.sanitize.redact import Redactor

FLOOR = 0.0002              # seconds: below this a ratio is timer noise
GIVE_UP = 1.0               # seconds: one call this slow ends the doubling (it has already failed)
MAX_CHARS = 8192            # the longest run built per case


def _best(call: Callable[[], object], repeat: int) -> float:
    best = float("inf")
    for _ in range(repeat):
        start = time.perf_counter()
        call()
        best = min(best, time.perf_counter() - start)
    return best


def _superlinear(run: Callable[[str], object], make: Callable[[int], str], unit: int) -> str | None:
    """None when the time of `run(make(n))` grows about in proportion to n as the run of n units doubles from one
    unit; else what grew. A doubling may cost at most 8 times the size before (an exponential blows past that within a
    step or two, while the calls are still cheap), and a quadrupling at most 8 times (a quadratic costs 16)."""
    times: dict[int, float] = {}
    n = 1
    while n * unit <= MAX_CHARS:
        text = make(n)
        times[n] = t = _best(lambda: run(text), 1)
        for back in (2, 4):
            if n // back in times and n % back == 0 and t > FLOOR and t > 8 * times[n // back]:
                small = make(n // back)
                t = times[n] = _best(lambda: run(text), 5)
                before = times[n // back] = _best(lambda: run(small), 5)
                if t > FLOOR and t > 8 * before:
                    return f"{n // back} -> {n} units: {before * 1000:.3f} ms -> {t * 1000:.3f} ms"
        if t > GIVE_UP:
            return f"{n} units: {t:.1f} s"
        n *= 2
    return None


# (prefix, run unit, suffix): the run repeated n times between them. The suffix ends the text inside a word, so a
# pattern anchored at the end fails and backtracks.
RUNS = [" ", "\t", "\xa0", " \t\xa0\u2003", "*", "_", "`", "'", '"', "'\"", " *", "<b>", " <b>", "<b ", "</b> ", "1",
        "1 ", "1.5,", "4821 ", "4821 is ", "\r\n", " \n", ">\n", "> ", "#", ".", ". ", ":", "-", "(", ")", "["]
PREFIXES = ["", "482913", "one", "backup", "api", "my", "Your code:", "password is", "verification", "bearer"]
SUFFIX = " ab cd efgh"
SEARCHING = {"search", "finditer", "findall", "sub", "subn", "split"}
SOURCE = "\n".join(pathlib.Path(module.__file__ or "").read_text() for module in (redact, images))


def _patterns() -> list[tuple[str, re.Pattern[str], set[str]]]:
    """Every compiled pattern in the redaction modules (module constants, the key patterns, the stream's own), and how
    the modules call it: searching through a text, or matching at a position. A pattern whose calls the source scan
    can't see is tried every way."""
    found: dict[int, tuple[str, re.Pattern[str]]] = {}
    for module in (redact, images):
        for name, value in vars(module).items():
            if isinstance(value, re.Pattern):
                found.setdefault(id(value), (name, value))
    for i, value in enumerate(redact.SECRET_PATTERNS):
        found.setdefault(id(value), (f"SECRET_PATTERNS[{i}]", value))
    for name, value in vars(redact.RedactionStream).items():
        if isinstance(value, re.Pattern):
            found.setdefault(id(value), (f"RedactionStream.{name}", value))
    out = []
    for name, pattern in found.values():
        short = name.split(".")[-1].split("[")[0]
        calls = set(re.findall(rf"\b{re.escape(short)}(?:\[\w*\])?\.(\w+)\(", SOURCE))
        if short == "SECRET_PATTERNS":
            calls = {"sub", "finditer"}
        ways = ({"search"} if calls & SEARCHING else set()) | (calls & {"match", "fullmatch"})
        out.append((name, pattern, ways or {"search", "match", "fullmatch"}))
    return sorted(out, key=lambda item: item[0])


@pytest.mark.parametrize("name,pattern,ways", _patterns(), ids=lambda value: value if isinstance(value, str) else "")
def test_every_redaction_pattern_is_linear_on_pathological_runs(name, pattern, ways) -> None:
    """The pattern searched through (when the modules search with it) or matched from the run's start of (when they
    match with it) every pathological text."""
    slow = []
    for prefix in PREFIXES:
        for unit in RUNS:
            def make(n: int, prefix: str = prefix, unit: str = unit) -> str:
                return prefix + unit * n + SUFFIX
            at = len(prefix)
            calls = {"search": lambda s: pattern.search(s), "match": lambda s: pattern.match(s, at),
                     "fullmatch": lambda s: pattern.fullmatch(s, at)}
            for how in sorted(ways):
                if (grew := _superlinear(calls[how], make, len(unit))) is not None:
                    slow.append(f"{name}.{how} on {prefix!r} + {unit!r} * n: {grew}")
    assert slow == [], "\n".join(slow)


# Whole texts through both redactors: `Redactor.text`, and one stream read ending inside the text's last line.
TEXTS: list[tuple[str, str, str]] = [
    ("482913", " ", " ab cd"), ("482913", "\t", " ab cd"), ("482913", "\xa0", " ab cd"),
    ("482913", " \t\xa0\u2003", " ab cd"), ("482913 ", "*", " ab cd"), ("482913 ", "_", " ab cd"),
    ("482913 ", "`", " ab cd"), ("482913 ", "'\"", " ab cd"), ("482913", " *", " ab cd"), ("482913", " <b>", " ab cd"),
    ("482913 <span ", "a ", "b"), ("MAX_UNAUTH = 1024", " ", "# connections not yet authenticated held at once"),
    ("", "1 ", "abc def"), ("", "1.5,", "abc def"), ("", "4821" + " " * 9, "ab cd"),
    ("one", " ", "x y zzz"), ("backup", " ", "x y zzz"), ("api", "_", " x yzw"), ("access ", "-", " x yzw"),
    ("my", " ", "wifi x y z"), ("password is", " ", "x y"), ("password:", "\r\n", "x y"), ("Your code:", "\r\n", "x"),
    ("", "<b ", "482913 x"), ("> Your code:\n", ">\n", "> 482913 x"), ("", "(", "482913) x"), ("", " ", "482913 ab cd"),
]


@pytest.mark.parametrize("prefix,unit,suffix", TEXTS)
def test_both_redactors_are_linear_on_pathological_lines(prefix, unit, suffix) -> None:
    redactor = Redactor()

    def make(n: int) -> str:
        return prefix + unit * n + suffix

    def read(text: str) -> object:
        data = text.encode()
        return redactor.stream().feed(data[:len(data) - 2])          # the read ends inside the last word

    assert (grew := _superlinear(redactor.text, make, len(unit))) is None, f"Redactor.text: {grew}"
    assert (grew := _superlinear(read, make, len(unit))) is None, f"stream read: {grew}"
