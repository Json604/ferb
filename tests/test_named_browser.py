"""The browser the owner names is the one FERB uses (owner's report 2026-10-01: "my scaler gmail in my chrome" was
opened in their Brave instead). If it isn't connected, FERB says so and how to connect it — never another browser —
and `ferb browsers pair` walks them through installing the extension where it's missing."""

import io
import json
from pathlib import Path
from typing import Any

from ferb.act.browser.ext_protocol import FERB_EXTENSION_ID
from ferb.act.browser.profiles import BrowserProfile, has_extension
from ferb.cli.admin import browsers
from ferb.core.types import Identity
from tests.conftest import RunningDaemon
from tests.browserkit import OWNER, bd, call, pair, task  # noqa: F401  (bd is a fixture)

KNOWN = [BrowserProfile("chrome", "Default", "Kartikey", "kartikey@gmail.com"),
         BrowserProfile("chrome", "Profile 2", "scaler", "kartikey.24bcs@sst.scaler.com"),
         BrowserProfile("brave", "Default", "Work", "")]


def test_whether_the_extension_is_installed_is_read_from_the_profile(tmp_path: Path) -> None:
    (tmp_path / "Profile 2").mkdir()
    p = KNOWN[1]
    assert not has_extension(p, frozenset({FERB_EXTENSION_ID}), tmp_path)
    (tmp_path / "Profile 2" / "Secure Preferences").write_text(json.dumps(
        {"extensions": {"settings": {FERB_EXTENSION_ID: {"location": 4}}}}))
    assert has_extension(p, frozenset({FERB_EXTENSION_ID}), tmp_path)


def test_an_explicit_profile_selects_that_profile(bd: Any) -> None:     # noqa: F811
    d, launched = bd
    pair(d, launched, "Work")
    g = task(d, "w", OWNER, {"research"})
    r = call(d, g, "browser.open", url="https://mail.google.com/", profile="Work")
    assert not r["is_error"] and r["content"]["profile"] == "chrome/Profile 1"


class FakeClient:
    def __init__(self) -> None:
        self.installed_after, self.calls = 2, []

    def call(self, method: str, params: dict[str, Any] | None = None) -> dict[str, Any]:
        self.calls.append(method)
        if method == "browsers.pair" and self.calls.count("browsers.pair") == 1:
            return {"paired": False, "needs_install": True, "browser": "chrome", "folder": "Profile 2",
                    "name": "scaler", "extension_dir": "/repo/extensions/chromium"}
        if method == "browsers.installed":
            self.installed_after -= 1
            return {"installed": self.installed_after <= 0}
        return {"paired": True, "browser": "chrome", "folder": "Profile 2", "name": "scaler"}


def test_pairing_walks_the_owner_through_installing_the_extension_then_pairs() -> None:
    c, out = FakeClient(), io.StringIO()
    assert browsers(c, "pair", "scaler", None, out, sleep=lambda s: None) == 0   # type: ignore[arg-type]
    text = out.getvalue()
    assert "isn't installed in Chrome “scaler”" in text and "Load unpacked" in text
    assert "/repo/extensions/chromium" in text and "Paired chrome profile “scaler”" in text
    assert c.calls == ["browsers.pair", "browsers.installed", "browsers.installed", "browsers.pair"]


def test_finder_names_and_home_folders(tmp_path: Path, monkeypatch: Any) -> None:
    """macOS stores a Finder name "clg/uni" as clg:uni on disk; "Desktop/…" is the owner's Desktop, not FERB's
    workspace (owner's report 2026-10-01: FERB made a new empty clg/uni instead)."""
    from ferb.act.fs.files import normalize_path, owner_path
    monkeypatch.setattr(Path, "home", lambda: tmp_path)
    monkeypatch.setenv("HOME", str(tmp_path))                               # ~ too
    (tmp_path / "Desktop" / "clg:uni").mkdir(parents=True)
    (tmp_path / "Desktop" / "clg").mkdir()                                  # a plain clg folder doesn't hide it
    assert owner_path("Desktop/clg/uni", "/ws") == tmp_path / "Desktop" / "clg:uni"
    assert normalize_path("~/Desktop/clg/uni", "/ws") == str(tmp_path / "Desktop" / "clg:uni")
    assert owner_path("notes.md", "/ws") == Path("/ws/notes.md")            # other relative paths: the workspace
    assert owner_path("Desktop/new/x", "/ws") == tmp_path / "Desktop" / "new" / "x"   # nothing there: as written


def test_mac_kinds_are_facts_about_the_control() -> None:
    """Typing is input (a search box: a search); pressing a control is input unless its label hits the floor words;
    Return is a submit; opening a chat row or a link is plain input, left to the worker's declaration."""
    from ferb.act.mac.accessibility import AxNode
    from ferb.act.mac.tools import MacAct
    from ferb.policy.rules import Kind
    nodes = {"s1e1": AxNode("r", "button", "SST Students Community", None, (0, 0, 1, 1), False, True),
             "s1e2": AxNode("r", "button", "Delete chat", None, (0, 0, 1, 1), False, True),
             "s1e3": AxNode("r", "button", "Send", None, (0, 0, 1, 1), False, True),
             "s1e4": AxNode("r", "button", "", None, (0, 0, 1, 1), False, True),
             "s1e5": AxNode("r", "button", "Voice call", None, (0, 0, 1, 1), False, True),
             "s1e6": AxNode("r", "link", "https://docs.google.com/document/d/x", None, (0, 0, 1, 1), False, True),
             "s1e7": AxNode("r", "searchbox", "Search", None, (0, 0, 1, 1), False, True),
             "s1e8": AxNode("r", "textbox", "Type a message", None, (0, 0, 1, 1), False, True)}

    class Driver:
        def node(self, bundle: str, el: str) -> AxNode:
            return nodes[el]
    tool = MacAct.__new__(MacAct)
    tool.driver = Driver()                                                       # type: ignore[assignment]
    tool._bundle = lambda args: "net.whatsapp.WhatsApp"                          # type: ignore[method-assign]
    kind = lambda el, action="press", text=None: tool._kind(                     # noqa: E731
        tool.Input(app="WhatsApp", action=action, element=el, text=text))
    assert kind("s1e1") == kind("s1e6") == kind("s1e4") == Kind.MAC_INPUT       # a chat row, a link, unlabeled
    assert kind("s1e2") == Kind.DELETE_PERMANENT and kind("s1e3") == Kind.SEND and kind("s1e5") == Kind.CALL
    assert kind("s1e7", "set_value", "syllabus") == Kind.SEARCH                 # a search box: a search
    assert kind("s1e8", "set_value", "hi") == Kind.MAC_INPUT                    # a message box: input
    assert kind(None, "key", "return") == Kind.SUBMIT and kind(None, "key", "cmd+s") == Kind.MAC_INPUT
    assert kind("s1e8", "focus") == Kind.NAVIGATE


def test_mac_menu_lists_freely_and_pressing_an_item_is_what_its_label_says() -> None:
    """Apps keep commands in their menus (WhatsApp: File > Search). Listing is reading; pressing needs mac:act and
    is input unless the item's label hits the floor words."""
    from ferb.act.mac.tools import MacMenu
    from ferb.policy.rules import Kind
    tool = MacMenu.__new__(MacMenu)
    tool._bundle = lambda args: "net.whatsapp.WhatsApp"                          # type: ignore[method-assign]
    inp = lambda path: tool.Input(app="WhatsApp", path=path)                      # noqa: E731
    assert tool.describe(inp(None)).kind == Kind.READ
    assert tool.describe(inp("File > Search")).kind == Kind.MAC_INPUT
    assert tool.describe(inp("Chat > Delete Chat")).kind == Kind.DELETE_PERMANENT
    assert tool.describe(inp("Chat > Voice Call")).kind == Kind.CALL


def test_page_kinds_an_email_row_is_a_click_and_security_pages_always_ask() -> None:
    """Owner's run: opening an email in Gmail was refused as a "submit". An email row, a list item or a harmless
    button is a click (edit, the worker declares it); Send / Submit are what they say; a security page is on the
    floor first."""
    from ferb.act.browser.tools import BrowserAct
    from ferb.core.types import PageElement
    from ferb.policy.rules import Kind

    def pe(i: str, role: str, label: str, **k: Any) -> PageElement:
        return PageElement(i, "f0", role, label, None, (0, 0, 1, 1), True, **k)
    els = {"row": pe("e1", "row", "Student Alert Computer Vision Course – Kaggle Assignment 2 new post"),
           "send": pe("e2", "button", "Send"), "archive": pe("e3", "button", "Archive"),
           "link": pe("e4", "link", "View here", href="https://forms.gle/x"),
           "submitbtn": pe("e5", "button", "Submit", form="other"), "field": pe("e6", "textbox", "To")}

    class Driver:
        url = "https://mail.google.com/mail/u/0/#inbox"

        def element_info(self, tab: str, el: str) -> PageElement:
            return els[el]

        def tab_url(self, tab: str) -> str:
            return self.url

    tool = BrowserAct.__new__(BrowserAct)
    tool.driver = Driver()                                                     # type: ignore[assignment]
    tool.family = type("F", (), {"identity": Identity.OWNER})()                # type: ignore[assignment]
    click = lambda el: tool.Input(tab="t1", element=el, action="click")        # noqa: E731
    assert tool.describe(click("row")).kind == Kind.CLICK                      # opening the email: a click
    assert tool.describe(click("archive")).kind == Kind.CLICK
    assert tool.describe(click("link")).kind == Kind.NAVIGATE
    assert tool.describe(click("send")).kind == Kind.SEND
    assert tool.describe(click("submitbtn")).kind == Kind.SUBMIT
    assert tool.describe(tool.Input(tab="t1", element="field", action="type", text="hi")).kind == Kind.FILL_FORM
    tool.driver.url = "https://myaccount.google.com/security"
    assert tool.describe(click("row")).kind == Kind.ACCOUNT_SECURITY           # security pages: always ask, first
