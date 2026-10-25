"""Write FERB's one-item Sparkle appcast for one architecture (L34 Phase G). Sparkle reads the newest item only, so the
feed holds just the release being published. Its archive is that release's DMG on GitHub, signed with FERB's EdDSA
key (sign_update's output is passed in). Release tooling, stdlib only; never part of FERB.app.

    python3 -I scripts/make_appcast.py --version 0.28.0 --build 202611010900 --tag v0.28.0 \
        --dmg FERB-AppleSilicon.dmg --signature 'sparkle:edSignature="…" length="…"' --min-macos 14.2 \
        --out build/appcast-arm64.xml"""

import argparse
import re
import sys
from datetime import datetime, timezone
from email.utils import format_datetime
from pathlib import Path
from xml.sax.saxutils import quoteattr

REPO = "https://github.com/Json604/ferb"
SIGNED = re.compile(r'sparkle:edSignature="([A-Za-z0-9+/]+=*)"\s+length="(\d+)"')
CHECKS = (("version", re.compile(r"\d+\.\d+\.\d+")), ("build", re.compile(r"\d{12}")),
          ("tag", re.compile(r"v\d+\.\d+\.\d+")), ("DMG name", re.compile(r"FERB-(AppleSilicon|Intel)\.dmg")),
          ("minimum macOS", re.compile(r"\d+(\.\d+){0,2}")))


def appcast(version: str, build: str, tag: str, dmg: str, signature: str, min_macos: str, now: datetime) -> str:
    for (what, pattern), value in zip(CHECKS, (version, build, tag, dmg, min_macos)):
        if not pattern.fullmatch(value):
            raise ValueError(f"{what} {value!r} isn't valid")
    signed = SIGNED.search(signature)
    if signed is None:
        raise ValueError('the signature must be sign_update\'s output: sparkle:edSignature="…" length="…"')
    ed, length = signed.groups()
    url = f"{REPO}/releases/download/{tag}/{dmg}"
    return ('<?xml version="1.0" encoding="utf-8"?>\n'
            '<rss version="2.0" xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle">\n'
            '  <channel>\n'
            '    <title>FERB</title>\n'
            '    <item>\n'
            f'      <title>FERB {version}</title>\n'
            f'      <pubDate>{format_datetime(now)}</pubDate>\n'
            f'      <sparkle:version>{build}</sparkle:version>\n'
            f'      <sparkle:shortVersionString>{version}</sparkle:shortVersionString>\n'
            f'      <sparkle:minimumSystemVersion>{min_macos}</sparkle:minimumSystemVersion>\n'
            f'      <link>{REPO}/releases/tag/{tag}</link>\n'
            f'      <enclosure url={quoteattr(url)} length="{length}" type="application/octet-stream" '
            f'sparkle:edSignature="{ed}"/>\n'
            '    </item>\n'
            '  </channel>\n'
            '</rss>\n')


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description="FERB's Sparkle appcast for one architecture")
    for name in ("version", "build", "tag", "dmg", "signature", "min-macos", "out"):
        parser.add_argument("--" + name, required=True)
    args = parser.parse_args(argv)
    try:
        text = appcast(args.version, args.build, args.tag, args.dmg, args.signature, args.min_macos,
                       datetime.now(timezone.utc))
    except ValueError as e:
        print(f"make_appcast.py: {e}", file=sys.stderr)
        return 1
    Path(args.out).write_text(text)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
