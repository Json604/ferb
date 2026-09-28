"""Spike 5: FERB's Safari extension (inside the no-UI FERB.app container) + the AX/real-input fallback.

Runs against the owner's Safari, **spare profile only** (the extension is enabled there and nowhere else).
Real site: Wikipedia search (public, no login). The AX test brings Safari to the front and types real keys:
don't touch the keyboard while it runs.
"""

import json
import secrets
import time
from pathlib import Path

import pytest

import safari_run as S

WIKI = "https://en.wikipedia.org/wiki/Main_Page"
QUERY = "Ada Lovelace"


@pytest.fixture(scope="module")
def ext():
    h = S.Harness()
    try:
        h.wait_hello()
        yield h
    finally:
        h.close()


def token_url() -> str:
    return f"{WIKI}?ferbtoken={secrets.token_hex(6)}"


def search_box(snap: dict) -> dict:
    return next(e for e in snap["elements"] if e["tag"] == "input" and e["name"] == "search" and e["visible"])


def search_button(snap: dict, box: dict) -> dict:
    later = [e for e in snap["elements"] if e["id"] > box["id"] and e["visible"]
             and e["tag"] == "button" and "search" in e["label"].lower()]
    return later[0]


def poll(ext, tab, done, timeout=30):
    end, snap = time.monotonic() + timeout, None
    while time.monotonic() < end:
        try:
            snap = ext.cmd("snapshot", tabId=tab)
            if done(snap):
                return snap
        except S.ExtError:
            pass
        time.sleep(0.5)
    return snap


def test_extension_connects_with_expected_version(ext):
    on_disk = json.loads((Path(__file__).parent / "ext" / "manifest.json").read_text())["version"]
    assert ext.hello["version"] == on_disk
    assert "Safari" in ext.hello["ua"]


def test_background_tab_snapshot(ext):
    tab = ext.cmd("open", url=token_url())["tabId"]
    try:
        snap = ext.cmd("snapshot", tabId=tab)
        assert snap["visibility"] == "hidden"
        assert snap["webdriver"] is False
        assert search_box(snap)
    finally:
        ext.cmd("close", tabId=tab)


def test_script_click_on_real_site(ext):
    tab = ext.cmd("open", url=token_url())["tabId"]
    try:
        snap = ext.cmd("snapshot", tabId=tab)
        box = search_box(snap)
        assert ext.cmd("script_set_value", tabId=tab, id=box["id"], value=QUERY)["ok"]
        snap = ext.cmd("snapshot", tabId=tab)
        assert ext.cmd("script_click", tabId=tab, id=search_button(snap, search_box(snap))["id"])["ok"]
        snap = poll(ext, tab, lambda s: "Ada_Lovelace" in s["url"])
        assert "Ada_Lovelace" in snap["url"], snap and snap["url"]
    finally:
        ext.cmd("close", tabId=tab)


# ---------- AX + real input fallback (foreground) ----------

needs_ax = pytest.mark.skipif(not S.ax_trusted(), reason="Accessibility not granted to this process")


def bring_forward(ext, tab: int, url: str):
    ext.cmd("activate", tabId=tab)
    end = time.monotonic() + 10
    while time.monotonic() < end:
        area = S.focused_web_area() if S.frontmost_is_safari() else None
        if area is not None and S.web_area_url(area) == url:
            return area
        time.sleep(0.2)
    return None


@needs_ax
def test_ax_refuses_when_focused_window_is_not_the_target(ext):
    """AX must prove it is looking at the tab the extension opened; a different tab in front -> refuse."""
    target_url, other_url = token_url(), token_url()
    target = ext.cmd("open", url=target_url)["tabId"]
    other = ext.cmd("open", url=other_url)["tabId"]
    try:
        assert bring_forward(ext, other, other_url) is not None
        area = S.focused_web_area()
        assert S.web_area_url(area) != target_url  # the check a real fallback runs before acting -> refuse
    finally:
        ext.cmd("close", tabId=target)
        ext.cmd("close", tabId=other)


@needs_ax
def test_ax_and_real_input_on_same_site(ext):
    url = token_url()
    tab = ext.cmd("open", url=url)["tabId"]
    try:
        area = bring_forward(ext, tab, url)
        assert area is not None, "could not verify the focused Safari window shows the target tab"
        field = S.find_text_field(area, "search")
        assert field is not None, "search field not found in the AX tree"
        assert S.AX.AXUIElementSetAttributeValue(field, "AXFocused", True) == 0
        time.sleep(0.3)
        assert S.frontmost_is_safari() and S.web_area_url(S.focused_web_area()) == url  # re-check right before typing
        S.type_real(QUERY)
        S.press_return()
        snap = poll(ext, tab, lambda s: "Ada_Lovelace" in s["url"])
        assert "Ada_Lovelace" in snap["url"], snap and snap["url"]
    finally:
        ext.cmd("close", tabId=tab)
