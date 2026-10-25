"""L34 Phase G: each release publishes one appcast per architecture with one item: the build Sparkle compares, the
version people read, the minimum macOS, and the release's DMG with FERB's EdDSA signature. Inputs are checked."""

import importlib.util
import subprocess
import sys
import xml.etree.ElementTree as ET
from datetime import datetime, timezone
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parent.parent
SCRIPT = ROOT / "scripts" / "make_appcast.py"
spec = importlib.util.spec_from_file_location("make_appcast", SCRIPT)
assert spec is not None and spec.loader is not None
make_appcast = importlib.util.module_from_spec(spec)
spec.loader.exec_module(make_appcast)

SPARKLE = "{http://www.andymatuschak.org/xml-namespaces/sparkle}"
SIGNED = 'sparkle:edSignature="QUJDRA==" length="734003200"'
WHEN = datetime(2026, 11, 1, 9, tzinfo=timezone.utc)


def test_one_item_pointing_at_the_releases_dmg() -> None:
    xml = make_appcast.appcast("0.28.0", "202611010900", "v0.28.0", "FERB-AppleSilicon.dmg", SIGNED, "14.2", WHEN)
    channel = ET.fromstring(xml).find("channel")
    assert channel is not None and len(channel.findall("item")) == 1
    item = channel.find("item")
    assert item is not None
    assert item.findtext(SPARKLE + "version") == "202611010900"
    assert item.findtext(SPARKLE + "shortVersionString") == "0.28.0"
    assert item.findtext(SPARKLE + "minimumSystemVersion") == "14.2"
    enclosure = item.find("enclosure")
    assert enclosure is not None
    assert enclosure.get("url") == "https://github.com/Json604/ferb/releases/download/v0.28.0/FERB-AppleSilicon.dmg"
    assert enclosure.get("length") == "734003200" and enclosure.get(SPARKLE + "edSignature") == "QUJDRA=="


@pytest.mark.parametrize("args", [
    ("0.28", "202611010900", "v0.28.0", "FERB-AppleSilicon.dmg", SIGNED, "14.2"),
    ("0.28.0", "2026", "v0.28.0", "FERB-AppleSilicon.dmg", SIGNED, "14.2"),
    ("0.28.0", "202611010900", "0.28.0", "FERB-AppleSilicon.dmg", SIGNED, "14.2"),
    ("0.28.0", "202611010900", "v0.28.0", "../FERB.dmg", SIGNED, "14.2"),
    ("0.28.0", "202611010900", "v0.28.0", "FERB-Intel.dmg", "not a signature", "14.2"),
    ("0.28.0", "202611010900", "v0.28.0", "FERB-Intel.dmg", SIGNED, "fourteen"),
])
def test_bad_inputs_are_refused(args: tuple[str, ...]) -> None:
    with pytest.raises(ValueError):
        make_appcast.appcast(*args, WHEN)


def test_the_command_writes_the_file_and_refuses_bad_input(tmp_path: Path) -> None:
    out = tmp_path / "appcast-x86_64.xml"
    base = [sys.executable, "-I", str(SCRIPT), "--version", "0.28.0", "--build", "202611010900", "--tag", "v0.28.0",
            "--dmg", "FERB-Intel.dmg", "--min-macos", "14.2", "--out", str(out)]
    assert subprocess.run(base + ["--signature", SIGNED]).returncode == 0 and out.exists()
    bad = subprocess.run(base + ["--signature", "nope"], capture_output=True, text=True)
    assert bad.returncode == 1 and "signature" in bad.stderr


def test_release_appcast_refuses_an_unknown_architecture(tmp_path: Path) -> None:
    done = subprocess.run([str(ROOT / "scripts" / "release-appcast.sh"), "v0.28.0", str(tmp_path / "x.dmg"), "ppc"],
                          capture_output=True, text=True)
    assert done.returncode == 1 and "arm64 or x86_64" in done.stderr


def test_the_feed_names_agree_across_the_build_the_release_and_the_app() -> None:
    assert "appcast-$ARCH.xml" in (ROOT / "scripts" / "build-app.sh").read_text()
    assert "appcast-$ARCH.xml" in (ROOT / "scripts" / "release-appcast.sh").read_text()
    swift = (ROOT / "ui" / "mac" / "FERBNotch" / "Sources" / "NotchCore" / "UpdatePlan.swift").read_text()
    assert 'feedPrefix = "https://github.com/Json604/ferb/releases/latest/download/appcast-"' in swift
