"""HAND-518, offline: a capture's two looks read only the masking inventory, and the look before walks alongside the
shot. The light walk gives the same masks as the whole walk on the same window (a password field, a text field named
a password field, none, a dialog beside the window, fields past MAX_DEPTH, web areas, a walk cut by its budget that
leaves the picture to text recognition); it never reads an element's value; the worker's cached walk is untouched by
FERB's own captures and preview frames."""

import asyncio
import threading
from dataclasses import dataclass, field
from typing import Any

import pytest

from ferb.act.mac import accessibility as ax
from ferb.act.mac.driver import WINDOW_UNREAD, window_masks


@dataclass(eq=False)
class El:
    role: str
    attrs: dict[str, Any] = field(default_factory=dict)
    children: list["El"] = field(default_factory=list)
    subrole: str = ""
    acts: list[str] = field(default_factory=lambda: ["AXPress"])

    def __post_init__(self) -> None:
        self.attrs.setdefault("AXRole", self.role)
        self.attrs.setdefault("AXSubrole", self.subrole or None)
        self.attrs.setdefault("AXChildren", self.children)
        self.attrs.setdefault("AXPosition", (1.0, 2.0))
        self.attrs.setdefault("AXSize", (30.0, 10.0))
        self.attrs.setdefault("AXValue", "the element's own text")


def _fake_ax(mp: pytest.MonkeyPatch, app: El) -> list[str]:
    """The AX module over a fake element tree; returns every attribute (and action list) read, in order."""
    read: list[str] = []

    def attr(el: Any, name: str) -> Any:
        if el is None:
            return None
        read.append(name)
        return el.attrs.get(name)

    def acts(el: Any) -> list[str]:
        read.append("actions")
        return list(el.acts)
    mp.setattr(ax, "_attr", attr)
    mp.setattr(ax, "actions", acts)
    mp.setattr(ax, "_point", lambda v, kind: tuple(v) if v else (0.0, 0.0))
    mp.setattr(ax, "_app", lambda pid: app)
    mp.setattr(ax, "_same", lambda a, b: a is b)
    mp.setattr(ax, "windows", lambda pid, win=None: [])
    clock = [0.0]                                               # a browser's empty page is re-read for ~2 s: virtual
    mp.setattr(ax, "time", type("Clock", (), {"monotonic": staticmethod(lambda: clock[0]),
                                              "sleep": staticmethod(lambda s: clock.__setitem__(0, clock[0] + s))}))
    return read


def _app(*windows: El) -> El:
    return El("AXApplication", children=list(windows))


def _masks(tree: ax.AxTree, browser: bool) -> tuple[list[Any], str]:
    return window_masks((tree, None), (tree, None), browser)


def _window(secret: bool) -> El:
    deep = El("AXGroup")
    tip = deep
    for _ in range(6):                                          # past MAX_DEPTH (patched to 3): the inventory goes on
        nxt = El("AXGroup")
        tip.children.append(nxt)
        tip.attrs["AXChildren"] = tip.children
        tip = nxt
    tip.children += [El("AXWebArea", children=[El("AXStaticText")], attrs={"AXPosition": (5.0, 6.0)})]
    if secret:
        tip.children.append(El("AXTextField", {"AXPosition": (70.0, 80.0)}, subrole="AXSecureTextField"))
    tip.attrs["AXChildren"] = tip.children
    shared = El("AXButton", {"AXTitle": "listed twice"})
    kids = [El("AXTextField", {"AXTitle": "Email"}), shared, shared, deep,
            El("AXWebArea", attrs={"AXPosition": (100.0, 130.0), "AXSize": (800.0, 520.0)}),   # an empty one
            El("AXStaticText", {"AXValue": "hello"})]
    if secret:
        kids += [El("AXSecureTextField", {"AXPosition": (40.0, 50.0)}),
                 El("AXTextField", {"AXPlaceholderValue": "Password", "AXPosition": (10.0, 90.0)})]
    return El("AXWindow", {"AXTitle": "Sign in"}, children=kids)


# decision: HAND-518
@pytest.mark.parametrize("secret", [True, False])
def test_the_light_look_gives_the_same_masks_as_the_whole_walk_and_never_reads_a_value(
        monkeypatch: pytest.MonkeyPatch, secret: bool) -> None:
    monkeypatch.setattr(ax, "MAX_DEPTH", 3)
    win = _window(secret)
    dialog = El("AXWindow", {"AXSubrole": "AXDialog"},
                children=[El("AXSecureTextField", {"AXPosition": (300.0, 300.0)})] if secret else [])
    read = _fake_ax(monkeypatch, _app(win, dialog))
    for browser in (False, True):
        whole = ax.read_tree(1, browser, win)
        read.clear()
        light = ax.read_tree(1, browser, win, masks_only=True)
        assert light.masks_only and not light.nodes and not light.text and not light.title
        assert "AXValue" not in read and "actions" not in read and "AXNumberOfCharacters" not in read
        for part in ("secure", "secret_rects", "web_areas", "empty_web_areas", "has_web", "web_empty",
                     "containers", "repeated_nodes"):
            assert getattr(light, part) == getattr(whole, part), part
        assert _masks(light, browser) == _masks(whole, browser)
    assert whole.secure is secret and len(whole.secret_rects or []) == (4 if secret else 0)
    assert whole.repeated_nodes == 1 and len(whole.web_areas) == 2


# decision: HAND-518
def test_a_light_look_cut_by_its_budget_leaves_the_picture_to_text_recognition_as_the_whole_walk_does(
        monkeypatch: pytest.MonkeyPatch) -> None:
    win = _window(True)
    _fake_ax(monkeypatch, _app(win))
    monkeypatch.setattr(ax, "WALK_MAX_NODES", 4)
    whole, light = ax.read_tree(1, False, win), ax.read_tree(1, False, win, masks_only=True)
    assert "budget" in whole.truncated_by and "budget" in light.truncated_by
    assert whole.secure is None and light.secure is None
    assert _masks(light, False) == _masks(whole, False) and WINDOW_UNREAD in _masks(light, False)[1]


class _Ax:
    """The vision fake (tests/mackit.py) that records each walk's kind and, when `gate` is set, holds the first walk
    until the window has been grabbed."""

    def __init__(self, base: Any) -> None:
        self.base, self.walks, self.gate = base, [], None   # type: ignore[var-annotated]

    def __getattr__(self, name: str) -> Any:
        return getattr(self.base, name)

    def read_tree(self, pid: int, browser: bool = False, window: Any = None, budget_s: Any = None,
                  masks_only: bool = False) -> ax.AxTree:
        self.walks.append("masks" if masks_only else "whole")
        if self.gate is not None and len(self.walks) == 1:
            assert self.gate.wait(10), "the shot waited for the look before it"
        return self.base.read_tree(pid, browser, window, budget_s)  # type: ignore[no-any-return]


# decision: HAND-518
def test_the_look_before_walks_alongside_the_shot_and_only_a_workers_screenshot_keeps_a_whole_walk(
        monkeypatch: pytest.MonkeyPatch) -> None:
    from ferb.act.point import PREVIEW_DEADLINE
    from tests.mackit import vision
    import time
    d, base, shots, *_ = vision(monkeypatch)
    fake = _Ax(base)
    d.ax = fake
    grabbed = threading.Event()
    shots.on_capture = grabbed.set
    fake.gate = grabbed

    async def check() -> None:
        _, shot, _ = await d.screenshot("Notes")                   # the walk before could end only after the grab
        assert fake.walks == ["masks", "whole"]
        state = d._state["com.apple.Notes"]
        tree = state.tree
        assert tree is not None and not tree.masks_only and shot.signature == tree.signature()
        fake.gate, fake.walks[:] = None, []
        await d.window_image("Notes")                               # FERB's own picture: two light looks
        _, capture = d.recording_source("Notes", shot.id)
        PREVIEW_DEADLINE.set(time.monotonic() + 8)
        await capture()                                             # a preview frame: two light looks
        assert fake.walks == ["masks"] * 4
        assert state.tree is tree and state.shot is shot           # the worker's walk and shot are still its own
    asyncio.run(check())
