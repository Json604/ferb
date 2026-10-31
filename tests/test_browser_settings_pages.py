"""Live 2026-10-26 (task 7c1eca3a): a worker switched off the owner's "Check Slot Alerts" extension on Chrome's
chrome://extensions page, unasked. The owner had said Yes to "bring Chrome to the front", which covers Chrome for the
task, and to typing the page's address; no question said the extension would be turned off. Input on a browser's own
pages (chrome://, brave://, edge://, arc://), by the address the browser reports for the target, or typing such an
address, now asks the owner every time, naming the page (rule 5: a where-boundary by the page's address)."""

from typing import Any

import pytest

from ferb.act.mac.apps import RunningApp
from ferb.act.mac.tools import MacAct, MacSnapshot
from ferb.policy import confirm
from tests.conftest import RunningDaemon, gateway_request, gateway_setup, gateway_tool_call
from tests.support.confirm import FerbAnswers
from tests.support.terminal_ax import TerminalApps, TerminalAx, terminal_mac

CHROME = "com.google.Chrome"
WINDOW = [{"role": "AXWindow", "title": "Extensions - Google Chrome - Sam (Home_Personal)"},
          {"role": "AXApplication", "title": "Google Chrome"}]
TOGGLE = [{"role": "AXCheckBox", "description": "Check Slot Alerts - BookSlots"}, {"role": "AXGroup"},
          {"role": "AXWebArea", "title": "Extensions", "url": "chrome://extensions/"}] + WINDOW
SEARCH = [{"role": "AXButton", "description": "Google Search"},
          {"role": "AXWebArea", "title": "Google", "url": "https://www.google.com/"}] + WINDOW
ADDRESS = [{"role": "AXTextField", "description": "Address and search bar"}, {"role": "AXToolbar"}] + WINDOW


def _chrome(d: RunningDaemon, mp: pytest.MonkeyPatch, focus: str | None = None) -> tuple[Any, TerminalAx, dict[str, str]]:
    ax = TerminalAx({"toggle": TOGGLE, "search": SEARCH, "address": ADDRESS}, focused=focus,
                    labels={"toggle": "Check Slot Alerts", "search": "Google Search", "address": "Address bar"})
    driver = terminal_mac(mp, ax, TerminalApps(RunningApp(CHROME, "Google Chrome", 11522, False)))
    grant, _ = gateway_setup(d, {"mac:read", "mac:act"}, gateway_request("check the permit portal"),
                             [MacSnapshot(driver), MacAct(driver)])
    out = gateway_tool_call(d, grant, "mac.snapshot", app="Google Chrome")
    assert not out["is_error"], out
    return grant, ax, {e["label"]: e["id"] for e in out["content"]["elements"]}


def test_a_click_on_chromes_extensions_page_asks_every_time_naming_the_page(
        daemon: RunningDaemon, monkeypatch: pytest.MonkeyPatch) -> None:
    grant, ax, ids = _chrome(daemon, monkeypatch)
    with FerbAnswers(daemon) as ferb:
        ferb.expect(confirm.FOREGROUND).expect(confirm.BROWSER_PAGE, 2)
        for _ in range(2):                                    # the task's earlier Yes never covers the next one
            out = gateway_tool_call(daemon, grant, "mac.act", app="Google Chrome", action="press",
                                    element=ids["Check Slot Alerts"])
            assert not out["is_error"], out
        assert "chrome://extensions/" in ferb.asked[0] and "chrome://extensions/" in ferb.asked[-1]
        ferb.verify()


def test_a_no_leaves_the_browsers_own_page_untouched(daemon: RunningDaemon, monkeypatch: pytest.MonkeyPatch) -> None:
    grant, ax, ids = _chrome(daemon, monkeypatch, focus="toggle")
    with FerbAnswers(daemon, yes=False) as ferb:
        ferb.expect(confirm.FOREGROUND).expect(confirm.BROWSER_PAGE)
        out = gateway_tool_call(daemon, grant, "mac.act", app="Google Chrome", action="key", text="space")
        assert out["is_error"]
        ferb.verify()
    assert ax.landed == []                                    # the focused toggle: nothing pressed


def test_an_ordinary_page_asks_only_the_once_per_task_screen_question(
        daemon: RunningDaemon, monkeypatch: pytest.MonkeyPatch) -> None:
    grant, ax, ids = _chrome(daemon, monkeypatch)
    with FerbAnswers(daemon) as ferb:
        ferb.expect(confirm.FOREGROUND)
        for _ in range(2):
            out = gateway_tool_call(daemon, grant, "mac.act", app="Google Chrome", action="press",
                                    element=ids["Google Search"])
            assert not out["is_error"], out
        ferb.verify()


def test_typing_a_browser_settings_address_asks(daemon: RunningDaemon, monkeypatch: pytest.MonkeyPatch) -> None:
    grant, ax, ids = _chrome(daemon, monkeypatch)
    with FerbAnswers(daemon) as ferb:
        ferb.expect(confirm.FOREGROUND).expect(confirm.BROWSER_PAGE)
        out = gateway_tool_call(daemon, grant, "mac.act", app="Google Chrome", action="set_value",
                                element=ids["Address bar"], text="chrome://extensions/?id=beepaenfejnphdgnkmccjcfiieihhogl")
        assert not out["is_error"], out
        assert "chrome://extensions/?id=beepaenfejnphdgnkmccjcfiieihhogl" in ferb.asked[0]
        ferb.verify()


def test_the_question_names_the_browser_and_its_page() -> None:
    need = confirm.browser_page_need("Google Chrome", "chrome://extensions/")
    assert need.rule == confirm.BROWSER_PAGE and need.always
    text = confirm.summary([need])
    assert "chrome://extensions/" in text and "Google Chrome" in text
    assert confirm.browser_page_need("Google Chrome", "chrome://settings/").key != need.key


def test_the_page_address_is_read_from_the_web_area_above_the_element(monkeypatch: pytest.MonkeyPatch) -> None:
    """Chromium reports a page's address as AXURL (an NSURL) on its AXWebArea; the first browser-own page on the way
    up counts, else the outermost page; nothing above the window is read."""
    import ferb.act.mac.accessibility as axm

    class Url:
        def __init__(self, s: str) -> None:
            self.s = s

        def absoluteString(self) -> str:  # noqa: N802  (AppKit's name)
            return self.s
    tree = {"box": {"AXRole": "AXCheckBox", "AXParent": "frame"},
            "frame": {"AXRole": "AXWebArea", "AXURL": Url("https://ads.example/"), "AXParent": "page"},
            "page": {"AXRole": "AXWebArea", "AXURL": Url("chrome://extensions/"), "AXParent": "win"},
            "win": {"AXRole": "AXWindow", "AXParent": "app"},
            "plain": {"AXRole": "AXButton", "AXParent": "top"},
            "top": {"AXRole": "AXWebArea", "AXURL": Url("https://www.google.com/"), "AXParent": "win"},
            "native": {"AXRole": "AXButton", "AXParent": "win"}}
    monkeypatch.setattr(axm, "_attr", lambda el, name: tree.get(el, {}).get(name))
    assert axm.page_url("box") == "chrome://extensions/"
    assert axm.page_url("plain") == "https://www.google.com/"
    assert axm.page_url("native") is None
