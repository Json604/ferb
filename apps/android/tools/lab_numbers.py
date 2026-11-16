#!/usr/bin/env python3
"""Reads the glass lab's design numbers and writes them as one JSON file that :design's tests pin Kotlin against.

The lab (~/Desktop/ferb-glass-lab, read only) is never built or changed: this script only reads its Swift and
tokens.json. The Kotlin tokens are transcribed by hand (com.ferb.design.*); LabNumbersTest fails when a value
drifts from this file, and `--check` says whether the lab has moved on since the file was written.

    python3 -I apps/android/tools/lab_numbers.py            # rewrite design/src/test/resources/lab/lab-numbers.json
    python3 -I apps/android/tools/lab_numbers.py --check    # exit 1 if the lab's numbers differ from the file
"""

import hashlib
import json
import os
import re
import subprocess
import sys

LAB = os.path.expanduser(os.environ.get("FERB_GLASS_LAB", "~/Desktop/ferb-glass-lab"))
OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "design", "src", "test", "resources", "lab",
                   "lab-numbers.json")

# The files the phone scene draws from. Every colour literal in them must exist in Kotlin (LabColors).
PHONE_FILES = [
    "Sources/FerbGlass/Theme.swift",
    "Sources/FerbGlass/Sky.swift",
    "Sources/FerbGlass/AstraSky.swift",
    "Sources/FerbGlass/Data.swift",
    "Sources/FerbGlassLab/Mobile.swift",
    "Sources/FerbGlassLab/MobileScreens.swift",
    "Sources/FerbGlassLab/Conversation.swift",
    "Sources/FerbGlassLab/Weather.swift",
]


def read(rel):
    with open(os.path.join(LAB, rel), encoding="utf-8") as f:
        return f.read()


def enum_body(text, name):
    m = re.search(r"enum " + name + r"\b[^{]*\{(.*?)\n\}", text, re.S)
    if not m:
        raise SystemExit(f"enum {name} not found")
    return m.group(1)


def colours(body):
    """`static let name = Color(hex: 0xRRGGBB[, opacity: x])` or `Color.black/white` -> {name: [hex, opacity]}."""
    out = {}
    for m in re.finditer(r"static let (\w+) = (Color(?:\(hex: 0x([0-9A-Fa-f]{6})(?:, opacity: ([0-9.]+))?\)|\.black(?:\.opacity\(([0-9.]+)\))?|\.white))", body):
        name, _, hexv, op, blackop = m.groups()
        expr = m.group(2)
        if hexv:
            out[name] = ["#" + hexv.upper(), float(op) if op else 1.0]
        elif expr.startswith("Color.black"):
            out[name] = ["#000000", float(blackop) if blackop else 1.0]
        else:
            out[name] = ["#FFFFFF", 1.0]
    return out


def literal_hexes():
    found = {}
    for rel in PHONE_FILES:
        for m in re.finditer(r"0x([0-9A-Fa-f]{6})\b", read(rel)):
            found.setdefault("#" + m.group(1).upper(), set()).add(os.path.basename(rel))
    return {k: sorted(v) for k, v in sorted(found.items())}


def android_frame(mobile):
    """The OS extension's `self == .ios ? a : b` numbers, the Android side."""
    body = re.search(r"extension OS \{(.*?)\n\}", mobile, re.S).group(1)
    out = {}
    for m in re.finditer(r"var (\w+): \w+ \{ self == \.ios \? ([^:]+?) : ([^}]+?) \}", body):
        out[m.group(1)] = m.group(3).strip()
    return out


def lab_commit():
    try:
        return subprocess.run(["git", "-C", LAB, "rev-parse", "HEAD"], capture_output=True, text=True, check=True).stdout.strip()
    except (OSError, subprocess.CalledProcessError):
        return "unknown"


def build():
    theme = read("Sources/FerbGlass/Theme.swift")
    mobile = read("Sources/FerbGlassLab/Mobile.swift")
    tokens = json.loads(read("tokens.json"))
    return {
        "labCommit": lab_commit(),
        "files": {rel: hashlib.sha256(read(rel).encode()).hexdigest() for rel in PHONE_FILES + ["tokens.json"]},
        "ink": colours(enum_body(mobile, "Ink")),
        "tone": colours(enum_body(theme, "Tone")),
        "lamp": colours(enum_body(theme, "Lamp")),
        "literalColours": literal_hexes(),
        "androidFrame": android_frame(mobile),
        "radius": {k: v for k, v in tokens["radius"].items() if not isinstance(v, list)},
        "space": tokens["space"],
        "type": {k: v for k, v in tokens["type"].items() if isinstance(v, dict)},
        "fileBadge": tokens["fileBadge"],
    }


def main():
    data = build()
    text = json.dumps(data, indent=1, sort_keys=True) + "\n"
    if "--check" in sys.argv:
        with open(OUT, encoding="utf-8") as f:
            old = json.load(f)
        old.pop("labCommit", None)
        data.pop("labCommit", None)
        if old != data:
            print("The lab's numbers differ from lab-numbers.json: rerun without --check and update the Kotlin tokens.")
            return 1
        print("lab-numbers.json matches the lab.")
        return 0
    with open(OUT, "w", encoding="utf-8") as f:
        f.write(text)
    print(f"wrote {os.path.normpath(OUT)} (lab {data['labCommit'][:7]})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
