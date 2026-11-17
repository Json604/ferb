#!/usr/bin/env python3
"""The launch's values at chosen moments (kb PHN-631), computed from motion.json's `launch` numbers by a third copy of
the formulas (Launch.kt and FerbDesign's Launch.swift are the other two), written into motion.json's `launch.samples`.
Android's LaunchTest and the iPhone's LaunchTests hold their code to these samples, so both phones move alike.

    python3 -I apps/android/tools/launch_samples.py           # rewrite launch.samples
    python3 -I apps/android/tools/launch_samples.py --check   # exit 1 when the file's samples differ
"""

import json
import math
import os
import sys

TABLE = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "design", "src", "test", "resources", "motion",
                     "motion.json")

# (t ms, ready at ms or None, reduced)
MOMENTS = [(t, None, False) for t in (0, 60, 150, 299, 330, 400, 470, 600, 900, 1350, 1800, 2700)] + \
          [(t, 0, False) for t in (0, 100, 220, 380)] + \
          [(t, 1350, False) for t in (1350, 1500, 1730)] + \
          [(t, None, True) for t in (0, 100, 300)] + \
          [(t, 300, True) for t in (400, 500)]


def bezier(x1, y1, x2, y2, x):
    """y at x on the cubic bezier (0,0) (x1,y1) (x2,y2) (1,1), by bisection on its x."""
    lo, hi = 0.0, 1.0
    for _ in range(60):
        s = (lo + hi) / 2
        bx = 3 * (1 - s) ** 2 * s * x1 + 3 * (1 - s) * s ** 2 * x2 + s ** 3
        lo, hi = (s, hi) if bx < x else (lo, s)
    s = (lo + hi) / 2
    return 3 * (1 - s) ** 2 * s * y1 + 3 * (1 - s) * s ** 2 * y2 + s ** 3


def free(L, t, reduced):
    # the whole mark at rest from the first frame (owner kb PHN-700: no drop, no hop, no give); reduced: still
    if reduced:
        return dict(dotY=0.0, dotAlpha=1.0, dotScale=1.0, fScale=1.0)
    b = L["breath"]
    k = 0.0 if t < b["after_ms"] else (1 - math.cos(2 * math.pi * (t - b["after_ms"]) / b["period_ms"])) / 2
    return dict(dotY=0.0, dotAlpha=1 - (1 - b["alpha"]) * k, dotScale=1 - (1 - b["scale"]) * k, fScale=1.0)


def at(L, t, ready, reduced):
    if ready is None or t < ready:
        v = free(L, t, reduced)
        v.update(handover=0.0, promptOk=False)
        return v
    a = free(L, ready, reduced)
    ms = L["reduced"]["ms"] if reduced else L["settle"]["ms"]
    u = min(max((t - ready) / ms, 0), 1)
    e = u if reduced else bezier(*L["settle"]["bezier"], u)
    rest = dict(dotY=0.0, dotAlpha=1.0, dotScale=1.0, fScale=1.0)
    v = {k: a[k] + (rest[k] - a[k]) * e for k in rest} if u < 1 else rest
    v.update(handover=e if u < 1 else 1.0,
             promptOk=(t - ready) >= (L["reduced"]["ms"] if reduced else L["settle"]["prompt_ms"]))
    return v


def main():
    with open(TABLE) as f:
        table = json.load(f)
    L = table["launch"]
    samples = []
    for t, ready, reduced in MOMENTS:
        v = at(L, t, ready, reduced)
        samples.append({"t": t, "ready": ready, "reduced": reduced,
                        **{k: (round(x, 6) if isinstance(x, float) else x) for k, x in v.items()}})
    if "--check" in sys.argv:
        if L["samples"] != samples:
            print("launch.samples differ from the formulas; run without --check")
            sys.exit(1)
        return
    # only the samples' lines change: the rest of the file keeps its hand-written layout
    with open(TABLE) as f:
        text = f.read()
    start = text.index('"samples": [', text.index('"launch"'))
    rows = ",\n".join("      " + json.dumps(s) for s in samples)
    text = text[:start] + '"samples": [\n' + rows + "\n    ]" + text[_close(text, start) + 1:]
    with open(TABLE, "w") as f:
        f.write(text)


def _close(text, start):
    """The index of the ] that closes the array opened at text[start:]."""
    depth = 0
    for i in range(text.index("[", start), len(text)):
        if text[i] == "[":
            depth += 1
        elif text[i] == "]":
            depth -= 1
            if depth == 0:
                return i
    raise ValueError("unclosed samples")


if __name__ == "__main__":
    main()
