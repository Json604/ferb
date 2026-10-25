"""Typing into any terminal is enforced by code (D31, CLAUDE.md rule 10): FERB always asks the owner first, showing the
exact command, every time, with no standing permission; a No types nothing.

Live evidence, task 6f5b85894b3140e0b434910301e088a6 (2026-10-25): FERB's sandbox blocked `ps` and the Codex control
socket, and the worker turned to the owner's VS Code terminal: at 07:04 `mac.act type "codex app-server daemon restart"`
into its terminal (refused only because another element held the keyboard focus), then at 07:11 `mac.menu Terminal >
New Terminal`, `mac.act type "codex"` and `mac.act key ENTER`, which ran a command in the owner's terminal unasked.
The terminal-app floor knew Terminal, iTerm2… by bundle id, not a terminal inside an editor.

Here: which elements code counts as terminals (policy/terminal.py), the driver's refusal of every input path without
the owner's Yes (pid, ax_keyboard, set_value / ax_value, edit, foreground, keys, ⌘V and menu paste), and the gateway's
question, asked before every input, with the exact text."""

import asyncio
import json
import subprocess
from pathlib import Path
from typing import Any

import pytest

from ferb.act.mac.apps import RunningApp, shell_role
from ferb.act.mac.tools import MacAct, MacMenu, MacSnapshot
from ferb.core.errors import Refused
from ferb.core.types import ComputerAction, TerminalGrant, TerminalInput
from ferb.daemon.gateway import CONFIRM_ASK, NO, YES
from ferb.policy import confirm, terminal
from ferb.policy.protection import DEFAULT_TERMINAL_APPS
from tests.browserkit import bd, call  # noqa: F401  (the fixture and its helper)
from tests.chromelab import SHOP, _node, _ready
from tests.conftest import RunningDaemon, gateway_request, gateway_setup, gateway_tool_call
from tests.support.terminal_ax import (CLAUDE_TERMINAL, CODE, EDITOR, JETBRAINS_EDITOR, JETBRAINS_TERMINAL,
                                       NOVA_TERMINAL, PAGE_SHELL_BOX, PAGE_XTERM, PYCHARM, TERMINAL, TERMINAL_LABEL,
                                       XCODE_CONSOLE, TerminalApps, TerminalAx, terminal_mac, vscode_ax)

UNASKED = "FERB types into a terminal only after the owner's Yes to that exact input"


# ---------- what counts as a terminal: code, from structural facts (rule 5) ----------

@pytest.mark.parametrize("bundle", [
    "com.apple.Terminal", "com.googlecode.iterm2", "dev.warp.Warp-Stable", "com.mitchellh.ghostty", "org.alacritty",
    "net.kovidgoyal.kitty", "com.github.wez.wezterm", "co.zeit.hyper", "org.tabby", "com.termius-dmg.mac",
    "com.raphaelamorim.rio", "org.contourterminal.contour"])
def test_terminal_apps_are_terminals_by_bundle_id_whatever_the_element(bundle: str) -> None:
    assert bundle in DEFAULT_TERMINAL_APPS
    assert terminal.surface(bundle, []) == f"a terminal app ({bundle})"
    assert terminal.surface(bundle.upper(), EDITOR) is not None                    # any case, any element


def test_the_owners_terminal_apps_and_an_app_that_declares_the_shell_role_count_too() -> None:
    assert terminal.surface("com.example.Term", [], owner_apps=["com.example.term"]) is not None
    assert terminal.surface("com.example.Term", []) is None
    assert "Info.plist" in (terminal.surface("com.example.Shelly", [], shell_app=True) or "")
    # Terminal.app's Info.plist (this Mac, 2026-10-25) declares the Shell role; VS Code's declares Editor for scripts
    terminal_app = {"CFBundleDocumentTypes": [
        {"CFBundleTypeRole": "Shell", "CFBundleTypeName": "Terminal shell script",
         "LSItemContentTypes": ["com.apple.terminal.shell-script"]},
        {"CFBundleTypeRole": "Shell", "LSItemContentTypes": ["public.unix-executable"]}]}
    vscode = {"CFBundleDocumentTypes": [{"CFBundleTypeRole": "Editor", "CFBundleTypeName": "Shell script",
                                         "CFBundleTypeExtensions": ["sh", "zsh"]}]}
    assert shell_role(terminal_app) and not shell_role(vscode) and not shell_role({}) and not shell_role(None)


def test_vscodes_terminal_is_a_terminal_by_xtermjs_dom_classes_and_its_editor_is_not() -> None:
    assert terminal.surface(CODE, TERMINAL) == "a terminal in the page (xterm.js: DOM class xterm-helper-textarea)"
    assert terminal.surface(CODE, TERMINAL[1:]) == "a terminal in the page (xterm.js: DOM class xterm)"
    assert terminal.surface(CODE, EDITOR) is None
    # Cursor and Windsurf are VS Code builds: the same xterm.js facts, whatever the bundle id
    assert terminal.surface("com.todesktop.230313mzl4w4u92", TERMINAL) is not None
    assert terminal.surface("com.exafunction.windsurf", CLAUDE_TERMINAL) is not None


def test_in_web_content_only_a_terminal_librarys_classes_decide_never_the_pages_words() -> None:
    assert terminal.surface("com.google.Chrome", PAGE_XTERM) is not None           # a cloud shell in a page
    assert terminal.surface("com.google.Chrome", PAGE_SHELL_BOX) is None           # a box labelled "Shell command"


def test_native_terminals_by_identifier_kind_or_label_and_a_label_errs_toward_asking() -> None:
    assert "Terminal Tool Window" in (terminal.surface(PYCHARM, JETBRAINS_TERMINAL) or "")
    assert "a label only" in (terminal.surface(PYCHARM, JETBRAINS_TERMINAL) or "")
    assert terminal.surface(PYCHARM, JETBRAINS_EDITOR) is None                     # a window title is never read
    assert terminal.surface("com.apple.dt.Xcode", XCODE_CONSOLE) is not None       # the debugger's console
    assert terminal.surface("com.panic.Nova", NOVA_TERMINAL) is not None
    assert terminal.surface("com.example.ide", [{"role": "AXTextArea", "identifier": "TerminalView"}]) is not None
    assert terminal.surface("com.example.ide", [{"role": "AXTextArea", "role_description": "shell"}]) is not None
    assert terminal.surface("com.example.ide", [{"role": "AXTextArea", "identifier": "EditorView"}]) is None
    assert terminal.surface("com.example.ide", [{"role": "AXTextArea", "description": "Terminally online"}]) is None


def test_an_app_whose_terminal_panel_exposes_no_facts_asks_for_any_typing() -> None:
    assert "Zed" in (terminal.surface("dev.zed.Zed", []) or "")
    assert terminal.surface("com.apple.TextEdit", []) is None


# ---------- the driver: nothing reaches a terminal without the owner's Yes to exactly it ----------

def _ids(d: Any, app: str = "Code") -> dict[str, str]:
    _, out = asyncio.run(d.snapshot(app))
    return {e["label"]: e["id"] for e in out["elements"]}


def _act(d: Any, kind: str, element: str | None = None, text: str | None = None, mechanism: str | None = None,
         grant: TerminalGrant | None = None, task: str = "t1", **kw: Any) -> Any:
    action = ComputerAction(kind, element, text, mechanism=mechanism, **kw)  # type: ignore[arg-type]
    return asyncio.run(d.act("Code", action, "w1", task, **({"terminal": grant} if grant else {})))


def _grant(inp: TerminalInput | None, cid: str = "c1") -> TerminalGrant:
    assert inp is not None
    return TerminalGrant(inp, cid)


@pytest.mark.parametrize("kind,mechanism,extra", [
    ("type", "pid", {}), ("type", "ax_keyboard", {}), ("set_value", "ax", {}), ("set_value", "ax_value", {}),
    ("edit", "ax", {}), ("type", "foreground", {"allow_foreground": True})])
def test_text_into_a_terminal_without_the_owners_yes_is_refused_on_every_mechanism(monkeypatch: Any, kind: str,
                                                                                    mechanism: str, extra: Any) -> None:
    ax = vscode_ax(focus="term108")
    d = terminal_mac(monkeypatch, ax)
    ids = _ids(d)
    with pytest.raises(Refused) as e:
        _act(d, kind, ids[TERMINAL_LABEL], "codex", mechanism, **extra)
    assert UNASKED in str(e.value) and "shell tool" in str(e.value) and "escape hatch" in str(e.value)
    assert ax.landed == []                                                        # nothing typed


@pytest.mark.parametrize("combo,mechanism", [("ENTER", "pid"), ("return", "ax_keyboard"), ("ctrl+c", "pid"),
                                             ("cmd+v", "ax"), ("cmd+v", "pid"), ("up", "foreground")])
def test_keys_into_a_focused_terminal_without_the_owners_yes_are_refused(monkeypatch: Any, combo: str,
                                                                         mechanism: str) -> None:
    ax = vscode_ax(focus="term108")
    d = terminal_mac(monkeypatch, ax)
    with pytest.raises(Refused, match=UNASKED):
        _act(d, "key", text=combo, mechanism=mechanism, allow_foreground=True)
    assert ax.landed == []


def test_pasting_by_menu_or_context_menu_into_a_focused_terminal_is_refused_unasked(monkeypatch: Any) -> None:
    ax = vscode_ax(focus="term108")
    d = terminal_mac(monkeypatch, ax)
    with pytest.raises(Refused, match=UNASKED):
        asyncio.run(d.press_menu("Code", ["Edit", "Paste"]))
    with pytest.raises(Refused, match=UNASKED):
        _act(d, "press", _ids(d)["Paste"], mechanism="ax")
    assert ax.landed == [] and ax.menus_pressed == []
    asyncio.run(d.press_menu("Code", ["Terminal", "New Terminal"]))               # opening a terminal runs nothing
    assert ax.menus_pressed == ["Terminal > New Terminal"]


def test_typing_that_lands_in_a_terminal_through_a_container_is_refused(monkeypatch: Any) -> None:
    """The element named is the terminal's host group; focusing it puts the keyboard focus in the terminal's input:
    the keystrokes' real destination is checked, not only the element named."""
    ax = vscode_ax(focus="editor")
    d = terminal_mac(monkeypatch, ax)
    with pytest.raises(Refused, match=UNASKED):
        _act(d, "type", _ids(d)["terminal host"], "codex", "pid")
    assert ax.landed == []


def test_the_editor_and_a_click_into_the_terminal_need_no_yes(monkeypatch: Any) -> None:
    ax = vscode_ax(focus="term57")
    d = terminal_mac(monkeypatch, ax)
    ids = _ids(d)
    assert d.terminal_input("Code", ComputerAction("type", ids["Editor content"], "print(1)")) is None
    assert _act(d, "type", ids["Editor content"], "print(1)", "pid").ok
    assert _act(d, "press", ids[TERMINAL_LABEL], mechanism="ax").ok              # a press is a click: allowed
    assert _act(d, "focus", ids[TERMINAL_LABEL]).ok
    assert ("editor", "type print(1)") in ax.landed


def test_the_yes_delivers_exactly_that_text_once_and_its_one_return(monkeypatch: Any) -> None:
    ax = vscode_ax(focus="term108")
    d = terminal_mac(monkeypatch, ax)
    box = _ids(d)[TERMINAL_LABEL]
    inp = d.terminal_input("Code", ComputerAction("type", box, "codex", mechanism="pid"), "t1")
    assert inp is not None and inp.text == "codex" and inp.enter and "xterm" in inp.surface
    with pytest.raises(Refused, match=UNASKED):                                   # another text: not this Yes
        _act(d, "type", box, "codex --yolo", "pid", grant=_grant(inp))
    assert _act(d, "type", box, "codex", "pid", grant=_grant(inp)).ok
    ret = d.terminal_input("Code", ComputerAction("key", None, "ENTER"), "t1")
    assert ret is not None and ret.covered_by == "c1"                             # covered by the Yes to "codex"
    assert d.terminal_input("Code", ComputerAction("key", None, "ENTER"), "other-task") is not None
    assert d.terminal_input("Code", ComputerAction("key", None, "ENTER"), "other-task").covered_by is None
    assert _act(d, "key", text="ENTER", mechanism="pid", grant=_grant(ret)).ok
    assert ax.landed == [("term108", "type codex"), ("term108", "key ENTER")]
    again = d.terminal_input("Code", ComputerAction("key", None, "ENTER"), "t1")
    assert again is not None and again.covered_by is None                          # one Return only
    with pytest.raises(Refused, match=UNASKED):
        _act(d, "key", text="ENTER", mechanism="pid", grant=_grant(ret))          # its grant is spent
    assert len(ax.landed) == 2


def test_the_covered_return_goes_only_to_the_terminal_the_text_went_into(monkeypatch: Any) -> None:
    """The owner's Claude terminal (57) holds their half-typed words: a Return FERB's Yes covered for terminal 108
    must not submit them when the focus moved there meanwhile."""
    ax = vscode_ax(focus="term108")
    d = terminal_mac(monkeypatch, ax)
    box = _ids(d)[TERMINAL_LABEL]
    inp = d.terminal_input("Code", ComputerAction("type", box, "codex"), "t1")
    assert _act(d, "type", box, "codex", "pid", grant=_grant(inp)).ok
    ret = d.terminal_input("Code", ComputerAction("key", None, "return"), "t1")
    ax.focused = "term57"                                                         # the owner clicked their terminal
    with pytest.raises(Refused, match=UNASKED):
        _act(d, "key", text="return", mechanism="pid", grant=_grant(ret))
    assert ax.landed == [("term108", "type codex")]


def test_text_that_holds_its_own_return_covers_no_second_one(monkeypatch: Any) -> None:
    ax = vscode_ax(focus="term108")
    d = terminal_mac(monkeypatch, ax)
    box = _ids(d)[TERMINAL_LABEL]
    inp = d.terminal_input("Code", ComputerAction("type", box, "ls\n"), "t1")
    assert inp is not None and not inp.enter
    assert _act(d, "type", box, "ls\n", "pid", grant=_grant(inp)).ok
    assert d.terminal_input("Code", ComputerAction("key", None, "return"), "t1").covered_by is None


def test_a_paste_is_asked_with_the_clipboards_text_and_a_new_copy_voids_the_yes(monkeypatch: Any) -> None:
    ax = vscode_ax(focus="term108")
    d = terminal_mac(monkeypatch, ax, clipboard=(7, "rm -rf build", False))
    inp = d.terminal_input("Code", ComputerAction("key", None, "cmd+v"), "t1")
    assert inp is not None and inp.paste and inp.text == "rm -rf build" and inp.clipboard == 7
    assert d.menu_terminal_input("Code", ["Edit", "Paste"]) == inp
    d.board[0] = (8, "something else", False)                                     # copied anew after the question
    with pytest.raises(Refused, match=UNASKED):
        _act(d, "key", text="cmd+v", mechanism="pid", grant=_grant(inp))
    d.board[0] = (7, "rm -rf build", False)
    assert asyncio.run(d.press_menu("Code", ["Edit", "Paste"], terminal=_grant(inp), task="t1"))
    assert ax.landed == [("term108", "paste")]
    secret = terminal_mac(monkeypatch, vscode_ax(focus="term108"), clipboard=(9, None, True))
    hidden = secret.terminal_input("Code", ComputerAction("key", None, "cmd+v"), "t1")
    assert hidden is not None and hidden.concealed and hidden.text is None       # a password manager's item: unshown


def test_a_jetbrains_terminal_and_a_terminal_app_ask_and_their_editors_do_not(monkeypatch: Any) -> None:
    ax = TerminalAx({"term": JETBRAINS_TERMINAL, "editor": JETBRAINS_EDITOR}, focused="term",
                    labels={"term": "Local", "editor": "Editor for main.py"})
    apps = TerminalApps(RunningApp(PYCHARM, "PyCharm", 501, False), RunningApp("com.apple.Terminal", "Terminal", 502,
                                                                             False))
    d = terminal_mac(monkeypatch, ax, apps)
    _, out = asyncio.run(d.snapshot("PyCharm"))
    ids = {e["label"]: e["id"] for e in out["elements"]}
    assert d.terminal_input("PyCharm", ComputerAction("type", ids["Local"], "ls")) is not None
    assert d.terminal_input("PyCharm", ComputerAction("type", ids["Editor for main.py"], "ls")) is None
    with pytest.raises(Refused, match=UNASKED):
        asyncio.run(d.act("PyCharm", ComputerAction("type", ids["Local"], "ls", mechanism="pid"), "w1", "t1"))
    assert d.terminal_input("Terminal", ComputerAction("key", None, "return")) is not None   # focus unknown: the app


# ---------- the gateway: FERB asks before every input, with the exact command ----------

class Owner:
    """Answers FERB's questions in order (True: Yes, False: No) and keeps each question's text and summary."""

    def __init__(self, d: RunningDaemon, answers: list[bool]) -> None:
        self.d, self.answers, self.asked, self.summaries = d, list(answers), [], []
        self._task = asyncio.run_coroutine_threadsafe(self._loop(), d.loop)

    async def _loop(self) -> None:
        approvals = self.d.daemon.state.approvals
        while True:
            for ap in approvals.pending():
                if ap.capability != CONFIRM_ASK or not self.answers:
                    continue
                yes = self.answers.pop(0)
                approvals.resolve(ap.id, True, ap.options.index(YES if yes else NO) + 1)
                self.asked.append(ap.reason)
                self.summaries.append(ap.summary)
            await asyncio.sleep(0.02)

    def stop(self) -> None:
        self._task.cancel()


def _live(d: RunningDaemon, monkeypatch: Any, focus: str = "term57") -> tuple[Any, TerminalAx]:
    """The real mac.snapshot / mac.menu / mac.act over VS Code as the 07:11 trace saw it: the owner's Claude terminal
    holds the keyboard focus until FERB opens a new terminal."""
    ax = vscode_ax(focus=focus)
    driver = terminal_mac(monkeypatch, ax)
    grant, _ = gateway_setup(d, {"mac:read", "mac:act"}, gateway_request("check that plain codex starts"),
                             [MacSnapshot(driver), MacMenu(driver), MacAct(driver)])
    return grant, ax


def _snap(d: RunningDaemon, grant: Any) -> dict[str, str]:
    out = gateway_tool_call(d, grant, "mac.snapshot", app="Code")
    assert not out["is_error"], out
    return {e["label"]: e["id"] for e in out["content"]["elements"]}


def _questions(d: RunningDaemon) -> list[dict[str, Any]]:
    async def go() -> list[dict[str, Any]]:
        rows = d.daemon.db.conn.execute(
            "SELECT input_json, output_json FROM spans WHERE kind = 'tool.call' AND "
            "json_extract(input_json, '$.tool') = 'owner.ask' AND json_extract(input_json, '$.ferb_question') = 1 "
            "ORDER BY rowid").fetchall()
        return [json.loads(r["input_json"])["args"] | {"answer": json.loads(r["output_json"] or "{}").get("answer")}
                for r in rows]
    return d.run(go())                                                            # type: ignore[no-any-return]


def test_the_live_sequence_asks_first_and_a_yes_types_once_and_sends_its_return(daemon: RunningDaemon,
                                                                                monkeypatch: Any) -> None:
    grant, ax = _live(daemon, monkeypatch)
    owner = Owner(daemon, [True])
    try:
        menu = gateway_tool_call(daemon, grant, "mac.menu", app="Code", path="Terminal > New Terminal")
        assert not menu["is_error"] and ax.focused == "term108" and _questions(daemon) == []   # runs nothing
        typed = gateway_tool_call(daemon, grant, "mac.act", app="Code", element=_snap(daemon, grant)[TERMINAL_LABEL],
                                  action="type", mechanism="pid", text="codex",
                                  why="Enter plain `codex` in the clean FERB terminal for the final startup check.")
        assert not typed["is_error"], typed
        enter = gateway_tool_call(daemon, grant, "mac.act", app="Code", element=_snap(daemon, grant)[TERMINAL_LABEL],
                                  action="key", mechanism="pid", text="ENTER", why="Run plain Codex.")
        assert not enter["is_error"], enter
    finally:
        owner.stop()
    assert ax.landed == [("term108", "type codex"), ("term108", "key ENTER")]     # once each, in FERB's terminal
    asked = _questions(daemon)
    assert len(asked) == 1 and asked[0]["confirm"][0]["rule"] == confirm.TERMINAL_INPUT and asked[0]["answer"] == YES
    assert owner.summaries == ['FERB wants to type "codex" into the terminal in Code, then press Return there (it '
                               "runs as you, outside FERB's sandbox)."]
    assert '"codex"' in asked[0]["question"] and "xterm" in asked[0]["question"]
    assert "this Yes covers this one input only" in asked[0]["question"]


def test_a_second_identical_command_asks_again_and_a_no_types_nothing(daemon: RunningDaemon, monkeypatch: Any) -> None:
    grant, ax = _live(daemon, monkeypatch, focus="term108")
    owner = Owner(daemon, [True, False])
    try:
        box = _snap(daemon, grant)[TERMINAL_LABEL]
        assert not gateway_tool_call(daemon, grant, "mac.act", app="Code", element=box, action="type", mechanism="pid",
                                     text="codex")["is_error"]
        box = _snap(daemon, grant)[TERMINAL_LABEL]
        no = gateway_tool_call(daemon, grant, "mac.act", app="Code", element=box, action="type", mechanism="pid",
                               text="codex", why="once more")
    finally:
        owner.stop()
    assert [q["answer"] for q in _questions(daemon)] == [YES, NO]                  # asked again, the same text
    assert no["is_error"] and "the owner said no" in no["content"]["error"]
    assert no["content"]["next"].startswith("The owner decided")
    assert ax.landed == [("term108", "type codex")]                               # the No typed nothing
    box = _snap(daemon, grant)[TERMINAL_LABEL]
    again = gateway_tool_call(daemon, grant, "mac.act", app="Code", element=box, action="type", mechanism="pid",
                              text="codex")
    assert again["is_error"] and "already said no" in again["content"]["error"]    # the No stands for it
    assert len(_questions(daemon)) == 2 and ax.landed == [("term108", "type codex")]


def test_a_retry_asks_again(daemon: RunningDaemon, monkeypatch: Any) -> None:
    """The owner said Yes, but the keyboard focus was elsewhere, so nothing was typed; the worker's retry of the very
    same call is asked again: a Yes covers one input, never a later attempt."""
    grant, ax = _live(daemon, monkeypatch, focus="term108")
    ax.keeps_focus = False
    ax.focused = "editor"
    owner = Owner(daemon, [True, True])
    try:
        box = _snap(daemon, grant)[TERMINAL_LABEL]
        first = gateway_tool_call(daemon, grant, "mac.act", app="Code", element=box, action="type", mechanism="pid",
                                  text="codex app-server daemon restart")
        assert first["is_error"] and "keyboard focus" in first["content"]["error"]
        ax.keeps_focus = True
        retry = gateway_tool_call(daemon, grant, "mac.act", app="Code", element=box, action="type", mechanism="pid",
                                  text="codex app-server daemon restart")
        assert not retry["is_error"], retry
    finally:
        owner.stop()
    assert [q["answer"] for q in _questions(daemon)] == [YES, YES]
    assert ax.landed == [("term108", "type codex app-server daemon restart")]


def test_a_return_ferb_did_not_type_for_asks_and_says_what_it_runs(daemon: RunningDaemon, monkeypatch: Any) -> None:
    grant, ax = _live(daemon, monkeypatch, focus="term57")
    owner = Owner(daemon, [False])
    try:
        out = gateway_tool_call(daemon, grant, "mac.act", app="Code", action="key", mechanism="pid", text="return")
    finally:
        owner.stop()
    assert out["is_error"] and ax.landed == []
    assert owner.summaries == ["FERB wants to press Return in the terminal in Code (it runs what is on its command "
                               "line, which FERB didn't type there)."]


def test_the_editor_needs_no_question(daemon: RunningDaemon, monkeypatch: Any) -> None:
    grant, ax = _live(daemon, monkeypatch, focus="editor")
    out = gateway_tool_call(daemon, grant, "mac.act", app="Code", element=_snap(daemon, grant)["Editor content"],
                            action="type", mechanism="pid", text="print('hi')")
    assert not out["is_error"], out
    assert _questions(daemon) == [] and ax.landed == [("editor", "type print('hi')")]


def test_a_menu_paste_into_the_focused_terminal_asks_with_the_clipboards_text(daemon: RunningDaemon,
                                                                              monkeypatch: Any) -> None:
    grant, ax = _live(daemon, monkeypatch, focus="term108")
    owner = Owner(daemon, [True])
    try:
        out = gateway_tool_call(daemon, grant, "mac.menu", app="Code", path="Edit > Paste")
    finally:
        owner.stop()
    assert not out["is_error"], out
    assert owner.summaries == ["FERB wants to paste the clipboard's text \"codex\" into the terminal in Code, then "
                               "press Return there (it runs as you, outside FERB's sandbox)."]
    assert ax.landed == [("term108", "paste")]


# ---------- a web terminal in a browser page (xterm.js): browser.act asks the same way ----------


def test_typing_into_a_web_terminal_asks_and_a_yes_covers_its_return(bd: Any) -> None:
    d, launched = bd
    ext, g = _ready(d, launched)
    tab = call(d, g, "browser.open", url=f"{SHOP}/terminal")["content"]["tab"]
    snap = call(d, g, "browser.snapshot", tab=tab)["content"]
    term, docs = (next(e["id"] for e in snap["elements"] if e["label"] == label)
                  for label in ("Terminal input", "Search docs"))
    owner = Owner(d, [True, False])
    try:
        assert not call(d, g, "browser.act", tab=tab, element=docs, action="type", text="xterm")["is_error"]
        assert _questions(d) == []                                                # an ordinary box: no question
        typed = call(d, g, "browser.act", tab=tab, element=term, action="type", text="gcloud config list")
        assert not typed["is_error"], typed
        enter = call(d, g, "browser.act", tab=tab, element=term, action="press", text="Enter")
        assert not enter["is_error"], enter                                       # covered by that Yes
        again = call(d, g, "browser.act", tab=tab, element=term, action="press", text="Enter")
    finally:
        owner.stop()
    assert [q["answer"] for q in _questions(d)] == [YES, NO]                      # the second Return asked: No
    assert owner.summaries[0] == ('FERB wants to type "gcloud config list" into the terminal in the page on '
                                  "shop.example, then press Return there (it runs as you, outside FERB's sandbox).")
    assert again["is_error"] and "the owner said no" in again["content"]["error"]
    sent = [(a["kind"], a["text"]) for a in ext.acts]
    assert sent == [("type", "xterm"), ("type", "gcloud config list"), ("press", "Enter")]   # the No sent nothing


def test_the_extension_marks_an_element_inside_xtermjs() -> None:
    """content.js's own describe(): `terminal` for xterm.js's input and anything inside its root, by the classes
    xterm.js sets; never for a page box whose words say terminal."""
    source = Path("extensions/chromium/content.js").read_text()
    fn = source[source.index("  function terminalOf("):source.index("  // Visible text nearest")]
    script = fn + r'''
const el = (classes, inside) => ({classList: {contains: c => classes.includes(c)},
                                  closest: sel => (sel === ".xterm" && inside) ? {} : null});
console.log(JSON.stringify([terminalOf(el(["xterm-helper-textarea"], true)), terminalOf(el([], true)),
                            terminalOf(el(["terminal", "shell"], false)), terminalOf({})]));
'''
    out = subprocess.run([_node(), "-e", script], capture_output=True, text=True, check=True).stdout
    assert json.loads(out) == [True, True, False, False]


def test_scrolling_a_terminals_text_view_sends_it_no_page_keys(monkeypatch: Any) -> None:
    """Page Up / Page Down are keys: in a terminal they go to the program running there, so FERB doesn't send them
    unasked; the wheel (mac.pointer) scrolls it instead."""
    ax = TerminalAx({"term": JETBRAINS_TERMINAL}, focused="term", labels={"term": "Local"})
    ax.scroll_area_of = lambda el: "area"                                          # type: ignore[attr-defined]
    ax.scroll_content = lambda area: "content"                                     # type: ignore[attr-defined]
    ax.scroll_bar = lambda area, vertical: None                                    # type: ignore[attr-defined]
    ax.scroll_position = lambda area, content: (0, 0)                              # type: ignore[attr-defined]
    apps = TerminalApps(RunningApp(PYCHARM, "PyCharm", 501, False))
    d = terminal_mac(monkeypatch, ax, apps)
    _, out = asyncio.run(d.snapshot("PyCharm"))
    r = asyncio.run(d.act("PyCharm", ComputerAction("scroll", out["elements"][0]["id"], direction="down",
                                                    mechanism="pid"), "w1", "t1"))
    assert not r.ok and "no Page keys" in r.detail and ax.landed == []
