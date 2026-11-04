"""Render the disk image's backgrounds (REL-409) from FERB's startup drawing code: a still of the Gravity — Dense field
(ui/mac/FERBNotch/Sources/FERBNotch/StartupStudies.swift, GravityDenseStudy.swift) compiled with
scripts/dmg-background/main.swift, with the words of each variant in scripts/dmg_layout.py.

    uv run python scripts/make_dmg_background.py

Writes assets/dmg/background-{before-devid,after-devid}.png and @2x.png. They are committed (as the logo files are,
REL-3), so a disk-image build never renders: build-dmg.sh joins a variant's two PNGs into the image's TIFF. Run this
again after changing the layout, the words or the startup field.
"""

import json
import subprocess
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import dmg_layout  # noqa: E402

ROOT = Path(__file__).resolve().parent.parent
FERB_SOURCES = ROOT / "ui" / "mac" / "FERBNotch" / "Sources" / "FERBNotch"
SOURCES = [ROOT / "scripts" / "dmg-background" / "main.swift",
           FERB_SOURCES / "StartupStudies.swift", FERB_SOURCES / "GravityDenseStudy.swift"]


def main() -> int:
    dmg_layout.BACKGROUNDS.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory() as work:
        renderer = Path(work) / "render-dmg-background"
        subprocess.run(["xcrun", "swiftc", "-O", "-swift-version", "5", "-o", str(renderer), *map(str, SOURCES)],
                       check=True)
        for variant in dmg_layout.VARIANTS:
            spec = Path(work) / f"{variant}.json"
            spec.write_text(json.dumps(dmg_layout.spec(variant), ensure_ascii=False))
            one_x, _ = dmg_layout.background_pngs(variant)
            subprocess.run([str(renderer), "render", str(spec), str(one_x.with_suffix(""))], check=True)
            print(f"Wrote {one_x.relative_to(ROOT)} and its @2x")
    return 0


if __name__ == "__main__":
    sys.exit(main())
