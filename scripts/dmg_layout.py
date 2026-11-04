"""The disk image's Finder window (REL-408, REL-409): one spec for scripts/make_dmg_background.py, which renders the
background, and for scripts/build-dmg.sh, which writes the window's .DS_Store onto the mounted image with this script.

The image holds FERB.app, the Applications link and the hidden background (plus the volume icon and the .DS_Store,
both hidden). Two variants differ only in the background's words: before the Developer ID the window says how to get
past Gatekeeper the first time; after it (signed with a Developer ID and notarized) only the drag line stays.

    python scripts/dmg_layout.py spec VARIANT       the JSON the background renderer draws from
    python scripts/dmg_layout.py ds-store VOLUME    write VOLUME/.DS_Store (the mounted image's root)

The .DS_Store is written with ds_store and mac_alias (MIT, the libraries dmgbuild uses; uv dev group, pinned in
uv.lock with hashes), never by driving Finder with AppleScript."""

from __future__ import annotations

import json
import sys
from pathlib import Path
from typing import Any

APP = "FERB.app"
LINK = "Applications"
BACKGROUND = ".background/FERB.tiff"         # a two-resolution TIFF (1x and 2x) made from the committed PNGs
BACKGROUNDS = Path(__file__).resolve().parent.parent / "assets" / "dmg"
VARIANTS = ("before-devid", "after-devid")
MOUNTED_AT = "/Volumes/FERB"                 # where macOS mounts the image (build-dmg.sh names its volume FERB)

WINDOW = (680, 440)                          # the window's content, in points: the background's 1x size
# Finder's WindowBounds is the whole window, title bar included (32 points on macOS 26 and later, 28 before), so the
# content holds the whole background and Finder never scrolls; the origin is from the screen's bottom left. The window
# fits a 1440 x 900 screen with the menu bar and the Dock.
TITLE_BAR = 32
WINDOW_ORIGIN = (200, 180)
ICON_SIZE = 128
ICONS = {APP: (180, 210), LINK: (500, 210)}  # icon centers, from the content's top left
# Finder draws icon labels in black over a background picture, whatever the appearance, and no view option changes
# that (checked on macOS 27.2: icvp textColor in any form is ignored). So the background carries the names in white,
# and Finder's own labels, at its smallest text size, fall on a pool of the stage's black that the background keeps
# right under each icon: black on black, unseen. Labels to the right of the icons would hide as well, but Finder then
# reserves a wide label area that makes the window scroll sideways. LABEL_CENTER is where Finder centers a label below
# the icon's center at that size, measured on macOS 27.2.
LABEL_SIZE = 10
LABEL_CENTER = 82
NAME_SIZE = 13
NAMES = {APP: "FERB", LINK: "Applications"}  # as Finder shows them (it hides the .app extension)

HEADING = "Drag FERB to Applications"
# Gatekeeper's first-open path for an app that isn't notarized, as Apple's guides give it for macOS 15 and later
# ("Safely open apps on your Mac"; Mac User Guide "Open a Mac app from an unknown developer").
# No-break spaces keep each name on one line.
FIRST_OPEN = ("The first time you open FERB, macOS blocks it.",
              "Open System Settings → Privacy & Security, scroll down to Security and click "
              "Open Anyway, then confirm with your password.")


def spec(variant: str) -> dict[str, Any]:
    if variant not in VARIANTS:
        raise ValueError(f"unknown variant {variant!r} (one of {', '.join(VARIANTS)})")
    return {
        "variant": variant,
        "window": list(WINDOW),
        "icon_size": ICON_SIZE,
        "icons": {name: list(center) for name, center in ICONS.items()},
        "app": APP,
        "link": LINK,
        "background": BACKGROUND,
        "heading": HEADING,
        "note": list(FIRST_OPEN) if variant == "before-devid" else [],
        "names": dict(NAMES),
        "label_size": LABEL_SIZE,
        "label_center": LABEL_CENTER,
        "name_size": NAME_SIZE,
    }


def background_pngs(variant: str) -> tuple[Path, Path]:
    """The committed 1x and 2x backgrounds of a variant (scripts/make_dmg_background.py writes them)."""
    spec(variant)
    return BACKGROUNDS / f"background-{variant}.png", BACKGROUNDS / f"background-{variant}@2x.png"


def ds_store_entries() -> dict[str, dict[str, Any]]:
    """The window's records, by item ('.' is the volume's own window), before the background's alias is added."""
    (width, height), (x, y) = WINDOW, WINDOW_ORIGIN
    window: dict[str, Any] = {
        "WindowBounds": f"{{{{{x}, {y}}}, {{{width}, {height + TITLE_BAR}}}}}",
        "ShowToolbar": False, "ShowSidebar": False, "ShowStatusBar": False, "ShowPathbar": False,
        "ShowTabView": False, "ContainerShowSidebar": False, "PreviewPaneVisibility": False, "SidebarWidth": 0,
    }
    icons: dict[str, Any] = {
        "viewOptionsVersion": 1, "arrangeBy": "none", "iconSize": float(ICON_SIZE), "textSize": float(LABEL_SIZE),
        "labelOnBottom": True, "showIconPreview": True, "showItemInfo": False, "gridSpacing": 100.0,
        "gridOffsetX": 0.0, "gridOffsetY": 0.0, "scrollPositionX": 0.0, "scrollPositionY": 0.0,
        "backgroundType": 2,                 # an image (backgroundImageAlias, added when written)
        "backgroundColorRed": 0.0, "backgroundColorGreen": 0.0, "backgroundColorBlue": 0.0,
    }
    entries: dict[str, dict[str, Any]] = {".": {"bwsp": window, "icvp": icons, "vSrn": ("long", 1),
                                                 "icvl": ("type", b"icnv")}}
    for name, center in ICONS.items():
        entries[name] = {"Iloc": center}
    return entries


def write_ds_store(volume: Path) -> None:
    from ds_store import DSStore
    from mac_alias import Alias, Bookmark

    background = volume / BACKGROUND
    missing = [n for n in (APP, LINK, BACKGROUND) if not (volume / n).exists() and not (volume / n).is_symlink()]
    if missing:
        raise SystemExit(f"dmg_layout.py: {volume} has no {', '.join(missing)}")
    from mac_alias.bookmark import URL, kBookmarkVolumePath, kBookmarkVolumeURL

    entries = ds_store_entries()
    # Both records name where the volume is mounted; the build mounts it in its own folder, so they say where macOS
    # mounts the downloaded image instead (and the image carries no path of the Mac that built it)
    alias = Alias.for_file(str(background))
    alias.volume.posix_path = MOUNTED_AT
    entries["."]["icvp"]["backgroundImageAlias"] = alias.to_bytes()
    bookmark = Bookmark.for_file(str(background))
    for _, toc in bookmark.tocs:
        toc[kBookmarkVolumePath] = MOUNTED_AT
        toc[kBookmarkVolumeURL] = URL("file://" + MOUNTED_AT)
    entries["."]["pBBk"] = bookmark
    store = volume / ".DS_Store"
    store.unlink(missing_ok=True)
    with DSStore.open(str(store), "w+") as d:
        for name, records in entries.items():
            for code, value in records.items():
                d[name][code] = value


def main(argv: list[str]) -> int:
    if len(argv) == 2 and argv[0] == "spec":
        print(json.dumps(spec(argv[1]), ensure_ascii=False))
        return 0
    if len(argv) == 2 and argv[0] == "ds-store":
        write_ds_store(Path(argv[1]))
        return 0
    print(__doc__, file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
