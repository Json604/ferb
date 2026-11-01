"""The checks every FERB release passes before its appcast is written (release-appcast.sh runs this): the version was
bumped, it isn't a development build, the tag names it, and the Chrome Web Store already serves an extension this
FERB accepts. Release tooling, stdlib only; never part of FERB.app.

    python3 -I scripts/release_checks.py --version 0.28.0 --tag v0.28.0 --released v0.27.0 v0.26.1 \
        --store-extension 0.19.0 --min-extension 0.19.0"""

import argparse
import re
import sys

VERSION = re.compile(r"\d+\.\d+\.\d+")


def _parts(v: str) -> tuple[int, ...]:
    return tuple(int(p) for p in re.findall(r"\d+", v)[:3])


def problems(version: str, tag: str, released: list[str], store_extension: str, min_extension: str) -> list[str]:
    """Why this can't be released yet; empty when it can. `version` is FERB's full version (ferb/__init__.py),
    `released` the tags already published, `store_extension` the version the Web Store serves ("" if unknown)."""
    out = []
    if not VERSION.fullmatch(version):
        out.append(f"FERB {version} is a development version: set the release version in ferb/__init__.py and "
                   "pyproject.toml, then rebuild")
    if tag != f"v{version}":
        out.append(f"the tag {tag} doesn't name FERB {version} (expected v{version})")
    newest = max((_parts(t) for t in released if VERSION.search(t)), default=None)
    if tag in released or (newest is not None and VERSION.fullmatch(version) and _parts(version) <= newest):
        out.append(f"FERB {version} isn't newer than what's already released: bump the version in ferb/__init__.py "
                   "and pyproject.toml, then rebuild")
    if not store_extension:
        out.append("couldn't read the extension version the Chrome Web Store serves")
    elif _parts(store_extension) < _parts(min_extension):
        out.append(f"the Chrome Web Store serves extension {store_extension} but this FERB needs {min_extension}: "
                   "upload build/store/ferb-extension-*.zip (uv run ferb browsers extension package --out build/store) "
                   "and wait until it is published")
    return out


def main(argv: list[str]) -> int:
    p = argparse.ArgumentParser()
    p.add_argument("--version", required=True)
    p.add_argument("--tag", required=True)
    p.add_argument("--released", nargs="*", default=[])
    p.add_argument("--store-extension", default="")
    p.add_argument("--min-extension", required=True)
    a = p.parse_args(argv)
    found = problems(a.version, a.tag, a.released, a.store_extension, a.min_extension)
    for line in found:
        print(f"release_checks.py: {line}", file=sys.stderr)
    return 1 if found else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
