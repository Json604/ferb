#!/usr/bin/env python3
"""Names the functions in a FERB hang report (the daemon's watchdog writes `~/.ferb/logs/hang-<time>.txt` with
`/usr/bin/sample` before it restarts a stuck daemon). What ships is stripped, so the report shows only offsets into
ferbd; build-app-rust.sh keeps a map of ferbd's symbols in the build folder, named by the binary's UUID, which the
report prints in its Binary Images. Each `??? (in FERB) load address A + OFF` line gets the function at OFF.

    python3 -I scripts/symbolicate-hang.py ~/.ferb/logs/hang-....txt [build/app-arm64/symbols]

Stdlib only; reads the two files, writes the named report to stdout."""

from __future__ import annotations

import bisect
import gzip
import re
import sys
from pathlib import Path

# ld64's __TEXT for an arm64 executable starts here: a map's addresses are this plus the offset sample prints
TEXT_BASE = 0x100000000
FRAME = re.compile(r"\?\?\?\s+\(in FERB\)\s+load address 0x[0-9a-f]+ \+ (0x[0-9a-f]+)")
IMAGE = re.compile(r"\+?com\.ferb\.daemon .*<([0-9A-F-]{36})>")
SYMBOL = re.compile(r"0x([0-9A-Fa-f]+)\s+0x([0-9A-Fa-f]+)\s+\[\s*\d+\]\s+(\S+)")


def readable(name: str) -> str:
    """A Rust v0 symbol's identifiers joined by `::` (enough to read a stack; not a full demangler)."""
    s = name.lstrip("_")
    if not s.startswith("R"):
        return name
    s, i, parts = s[1:], 0, []
    while i < len(s):
        c = s[i]
        if c.isdigit():
            m = re.match(r"(\d+)_?", s[i:])
            n, j = int(m.group(1)), i + len(m.group(0))
            parts.append(s[j : j + n])
            i = j + n
        elif c in "sB" and (m := re.match(r"[sB][0-9A-Za-z]*_", s[i:])):
            i += len(m.group(0))  # a disambiguator or a back-reference
        else:
            i += 1
    return "::".join(p for p in parts if p) or name


def symbols(map_text: str) -> tuple[list[int], list[tuple[int, int, str]]]:
    rows, found = [], False
    for line in map_text.splitlines():
        if line.startswith("# Symbols:"):
            found = True
        elif found and (m := SYMBOL.match(line)):
            rows.append((int(m.group(1), 16), int(m.group(2), 16), m.group(3)))
    rows.sort()
    return [r[0] for r in rows], rows


def name_at(index: list[int], rows: list[tuple[int, int, str]], address: int) -> str | None:
    at = bisect.bisect_right(index, address) - 1
    if at < 0:
        return None
    start, size, name = rows[at]
    return readable(name) if address < start + max(size, 1) else None


def symbolicate(report: str, map_text: str) -> str:
    index, rows = symbols(map_text)

    def named(m: re.Match) -> str:
        name = name_at(index, rows, TEXT_BASE + int(m.group(1), 16))
        return f"{name}  (in FERB) + {m.group(1)}" if name else m.group(0)

    return "\n".join(FRAME.sub(named, line) for line in report.splitlines()) + "\n"


def main(argv: list[str]) -> int:
    if len(argv) not in (2, 3):
        print(__doc__, file=sys.stderr)
        return 2
    report = Path(argv[1]).read_text(errors="replace")
    folder = Path(argv[2]) if len(argv) == 3 else Path(__file__).resolve().parents[1] / "build/app-arm64/symbols"
    uuid = next((m.group(1) for m in IMAGE.finditer(report)), None)
    if not uuid:
        print("the report names no ferbd (com.ferb.daemon) image", file=sys.stderr)
        return 1
    path = folder / f"ferbd-{uuid}.map.gz"
    if not path.exists():
        print(f"no symbol map for ferbd {uuid} in {folder}: it was built elsewhere or its map was pruned", file=sys.stderr)
        return 1
    sys.stdout.write(symbolicate(report, gzip.decompress(path.read_bytes()).decode(errors="replace")))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
