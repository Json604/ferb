"""Rework phase 3, offline: the one-walk snapshot (ferb.act.mac.accessibility) on a fake element tree — controls by
available actions, not by a role list; roles and states as facts; sheets / menus / dialogs included; truncation
reported with its reason; password fields found without their value; the learned profile seeded from a probe; the
full key table."""

from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

import pytest

from ferb.act.mac import accessibility as ax
from ferb.act.mac import input as inp
from ferb.act.mac.profile import Profiles


@dataclass
class El:
    role: str
    attrs: dict[str, Any] = field(default_factory=dict)
    acts: list[str] = field(default_factory=list)
    children: list["El"] = field(default_factory=list)
    subrole: str = ""

    def __post_init__(self) -> None:
        self.attrs.setdefault("AXRole", self.role)
        self.attrs.setdefault("AXSubrole", self.subrole or None)
        self.attrs.setdefault("AXChildren", self.children)
        self.attrs.setdefault("AXPosition", (1.0, 2.0))
        self.attrs.setdefault("AXSize", (30.0, 10.0))


def fake_ax(mp: pytest.MonkeyPatch) -> None:
    mp.setattr(ax, "_attr", lambda el, name: el.attrs.get(name) if el is not None else None)
    mp.setattr(ax, "actions", lambda el: list(el.acts) if el is not None else [])
    mp.setattr(ax, "_point", lambda v, kind: tuple(v) if v else (0.0, 0.0))


def walk(root: El) -> ax.AxTree:
    tree, texts = ax.AxTree(), []
    ax._walk(root, tree, texts, 0, None, [0])
    tree.text = " ".join(texts)
    tree.truncated = bool(tree.truncated_by)
    return tree


def test_controls_are_what_exposes_an_action_whatever_the_role(monkeypatch: pytest.MonkeyPatch) -> None:
    fake_ax(monkeypatch)
    win = El("AXWindow", {"AXTitle": "Chats"}, children=[
        El("AXGroup", {"AXDescription": "Rahul"}, acts=["AXPress", "AXShowMenu"]),              # Electron row
        El("AXGroup", children=[El("AXStaticText", {"AXValue": "Priya"}),                        # unlabeled row
                                El("AXStaticText", {"AXValue": "running late"})], acts=["AXPress"]),
        El("AXGroup", children=[El("AXStaticText", {"AXValue": "just a container"})]),         # no action: not one
        El("AXImage", {"AXDescription": "avatar"}, acts=["AXPress"]),
        El("AXCell", acts=["AXPress"], attrs={"AXSelected": True}),
        El("AXPopUpButton", {"AXTitle": "Format", "AXValue": "Rich Text"}, acts=["AXPress", "AXShowMenu"]),
        El("AXTextField", {"AXPlaceholderValue": "Search"}, acts=["AXConfirm"], subrole="AXSearchField"),
        El("AXSlider", {"AXValue": 3}, acts=["AXIncrement", "AXDecrement"]),
        El("AXDisclosureTriangle", {"AXTitle": "More", "AXExpanded": False}, acts=["AXPress"]),
        El("AXButton", {"AXTitle": "Send", "AXEnabled": False}, acts=["AXPress"]),
        El("AXHeading", {"AXValue": "Today"}),
        El("AXStaticText", {"AXValue": "hello there"}),
    ])
    t = walk(win)
    by = {(n.role, n.label): n for n in t.nodes}
    assert ("group", "Rahul") in by and by[("group", "Rahul")].actions == ("press", "menu")
    assert by[("group", "Rahul")].ax_role == "AXGroup" and by[("group", "Rahul")].rect == (1.0, 2.0, 30.0, 10.0)
    assert ("group", "Priya running late") in by                       # named by its own text
    assert not [n for n in t.nodes if n.label == "just a container"]   # an actionless group isn't a control
    assert ("image", "avatar") in by and by[("cell", "")].selected is True
    assert by[("popup", "Format")].value == "Rich Text"                # a pop-up button, not a combobox
    assert by[("searchbox", "Search")].actions == ("confirm",)         # the AXSearchField subrole
    assert by[("slider", "")].actions == ("increment", "decrement") and by[("slider", "")].value == "3"
    assert by[("button", "More")].expanded is False
    assert by[("button", "Send")].enabled is False
    assert t.headings == ["Today"] and "hello there" in t.text and "Today" in t.text
    assert t.secure is False and not t.truncated


def test_sheets_menus_and_dialogs_are_in_the_snapshot_with_their_place(monkeypatch: pytest.MonkeyPatch) -> None:
    """A save sheet appeared under the window's AXChildren (role AXSheet) while AXSheets was still empty (probe,
    TextEdit 26.6): its buttons are listed, marked as being in the sheet, so they can be pressed by title."""
    fake_ax(monkeypatch)
    win = El("AXWindow", {"AXTitle": "Untitled"}, children=[
        El("AXTextArea", {"AXValue": "draft"}, acts=["AXShowMenu"]),
        El("AXSheet", children=[El("AXStaticText", {"AXValue": "Do you want to keep this new document?"}),
                                El("AXButton", {"AXTitle": "Delete"}, acts=["AXPress"]),
                                El("AXButton", {"AXTitle": "Cancel"}, acts=["AXPress"]),
                                El("AXButton", {"AXTitle": "Save"}, acts=["AXPress"])]),
    ])
    t = walk(win)
    assert t.containers == ["sheet"]
    assert {n.label: n.container for n in t.nodes} == {"": None, "Delete": "sheet", "Cancel": "sheet", "Save": "sheet"}
    assert "keep this new document" in t.text
    # state changes show in the signature; element order and refs don't matter to it
    t2 = walk(win)
    assert t.signature() == t2.signature()
    win.children[0].attrs["AXValue"] = "draft edited"
    assert walk(win).signature() != t.signature()


def test_password_fields_are_found_but_never_read(monkeypatch: pytest.MonkeyPatch) -> None:
    fake_ax(monkeypatch)
    win = El("AXWindow", children=[El("AXTextField", {"AXValue": "me@example.com"}),
                                   El("AXTextField", {"AXValue": "hunter2", "AXNumberOfCharacters": 7},
                                      subrole="AXSecureTextField")])
    t = walk(win)
    assert t.secure is True
    pw = next(n for n in t.nodes if n.secret)
    assert pw.value == "filled" and "hunter2" not in str(t.signature())
    assert ax.has_secure_field(win) is True


def test_truncation_says_why_and_the_password_scan_goes_on_past_the_element_cap(
        monkeypatch: pytest.MonkeyPatch) -> None:
    fake_ax(monkeypatch)
    monkeypatch.setattr(ax, "MAX_ELEMENTS", 5)
    many = El("AXWindow", children=[El("AXButton", {"AXTitle": f"b{i}"}, acts=["AXPress"]) for i in range(8)]
              + [El("AXTextField", subrole="AXSecureTextField")])
    t = walk(many)
    assert len(t.nodes) == 5 and t.truncated and t.truncated_by == "elements"
    assert t.secure is True                                             # seen beyond the cap
    monkeypatch.setattr(ax, "MAX_DEPTH", 2)
    deep = El("AXWindow", children=[El("AXGroup", children=[El("AXGroup", children=[
        El("AXGroup", children=[El("AXButton", {"AXTitle": "deep"}, acts=["AXPress"])])])])])
    t = walk(deep)
    assert t.nodes == [] and t.truncated_by == "depth"


def test_the_profile_is_seeded_from_a_probe_and_learns_from_outcomes(tmp_path: Path) -> None:
    p = Profiles(tmp_path / "apps")
    probe = {"bundle": "com.apple.TextEdit", "ax_press_ok": False, "ax_set_value_ok": True, "ax_selected_text_ok": True,
             "ax_post_key_ok": False, "pid_keys_ok": True, "pid_keycode_ok": True, "pid_mouse_click_ok": False,
             "pid_scroll_ok": False, "menu_press_ok": True, "app_hit_test_ok": False, "ax_focus_ok": True,
             "pid_mouse_drag_ok": None}
    written = p.seed("com.apple.TextEdit", probe)
    assert "ax_keyboard" in written and "pid_mouse" in written and "pid_drag" not in written
    assert p.prefers("com.apple.TextEdit", "pid_keys") and p.status("com.apple.TextEdit", "ax_keyboard") == "failed"
    for _ in range(2):
        p.record("com.apple.TextEdit", "ax_keyboard", False)
    assert p.get("com.apple.TextEdit").mechanisms["ax_keyboard"].streak == 3
    p.record("com.apple.TextEdit", "ax_keyboard", True)                 # a later success forgets the streak
    assert p.get("com.apple.TextEdit").mechanisms["ax_keyboard"].streak == 0
    again = Profiles(tmp_path / "apps")
    assert again.get("com.apple.TextEdit").seeded_from == "probe"
    assert again.get("com.apple.TextEdit").mechanisms["ax_keyboard"].failed == 3
    assert Profiles(None).untried("x", "y")                             # memory only: still answers


def test_the_key_table_covers_letters_digits_function_and_navigation_keys() -> None:
    for combo in ("a", "z", "0", "9", "f1", "f12", "home", "end", "pageup", "pagedown", "forwarddelete", ",", "/",
                  "cmd+shift+s", "ctrl+alt+delete", "Cmd+Return", "enter", "esc", "pgdn", "option+right"):
        assert inp.valid_combo(combo), combo
    assert inp.normalize("Cmd+Shift+S") == ["cmd", "shift", "s"] and inp.normalize("pgdn") == ["pagedown"]
    assert inp.normalize("Option+Return") == ["alt", "return"]
    assert not inp.valid_combo("hyper+a") and not inp.valid_combo("f99") and not inp.valid_combo("")
    assert inp.is_shortcut("cmd+a") and not inp.is_shortcut("shift+a")
    assert inp.switches_apps("cmd+tab") and inp.switches_apps("ctrl+right") and inp.switches_apps("cmd+space")
    assert inp.quits_or_hides("cmd+q") and inp.quits_or_hides("alt+cmd+h") and not inp.quits_or_hides("cmd+shift+m")
    assert inp.is_arrow("shift+left") and inp.is_enter("return") and not inp.is_enter("cmd+return")
    assert all(0 <= k < 128 for k in inp.KEYCODES.values())
