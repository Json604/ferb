"""Owns which on-screen element is a terminal (D31, AGENTS.md rule 10) and the owner's question before FERB types into
one. A where/what boundary decided by code from the app and from the element's own structural Accessibility facts and
its ancestors' (rule 5). Must not read what an element shows, judge what is typed or decide intent (rule 6): any input
into a terminal asks, whatever it is.

Typing into any terminal (terminal apps and terminals inside other apps, e.g. VS Code's) is enforced by code: FERB
always asks the owner first, showing the exact command, every time, with no standing permission; a No types nothing.
The gateway asks (`need`); the driver delivers only the input the owner's Yes names.

A terminal is recognized, strongest fact first (`surface`):
1. The app is a terminal: a bundle id in TERMINAL_APPS (protection's DEFAULT_TERMINAL_APPS) or in the owner's
   [protection] terminal_apps, or an app whose Info.plist declares the Shell role for executables or shell scripts, as
   Terminal.app does (`apps.declares_shell`).
2. Web content, where Chromium and WebKit expose each node's DOM classes: an element inside a terminal library's own
   nodes. xterm.js marks its root `xterm` and its input `xterm-helper-textarea`: VS Code, Cursor, Windsurf, VSCodium,
   JupyterLab, Theia, Hyper, Tabby and web terminals in a browser page. In web content only these decide: a page's
   words never do.
3. Native content: an element on the path whose role, subrole or role description, or whose own identifier (the
   developer's AXIdentifier), names a terminal, console or shell as a word.
4. Native content with none of those facts: a label (title, description) on the element or an ancestor below its
   window that names a terminal, console or shell as a word (JetBrains IDEs' Terminal tool window, Xcode's debug
   console, Nova's terminal). A label is weak evidence, so this errs toward asking: a false match costs one question,
   a miss would type unasked. Window titles (documents' names) are not read.
5. An app whose terminal panel exposes no Accessibility facts (Zed, NO_FACTS_HOSTS): any typing or key there asks.
6. Fails closed: in an app that can host a terminal (a terminal app, NO_FACTS_HOSTS, an editor in EDITOR_APPS), input
   whose receiving element FERB can't read (`unread`) counts as terminal input and asks.
Limit: a custom terminal panel that exposes none of these facts, in an app not listed, is not recognized."""

import json
import re
import unicodedata
from typing import Any, Collection, Mapping, Sequence

from ferb.core.types import TerminalInput
from ferb.policy import confirm
from ferb.policy.protection import DEFAULT_TERMINAL_APPS

RULE = confirm.TERMINAL_INPUT
# What a driver says when input would reach a terminal without the owner's Yes to exactly it (nothing was typed).
UNASKED = ("FERB types into a terminal only after the owner's Yes to that exact input, every time (D31): this input "
           "would reach {why} in {app}, and the call carries no Yes for it, so nothing was typed. Run commands with "
           "FERB's shell tool; if FERB's sandbox blocks a command you need, request the escape hatch (shell.rules "
           "lists what it lifts), which asks the owner once")
RETURN_KEYS = frozenset({"return", "enter"})        # a key that runs a terminal's command line
TERMINAL_APPS = frozenset(b.casefold() for b in DEFAULT_TERMINAL_APPS)
# Apps that run code when a script tells them to (review rounds 1 and 2: System Events, O1, O4): a script for any of
# them asks, showing the script. Data, one entry per app, by bundle id (casefolded).
SYSTEM_EVENTS = "com.apple.systemevents"
CODE_RUNNERS = {SYSTEM_EVENTS: "System Events, whose keystrokes type into whatever has the keyboard focus",
                "com.apple.finder": ("Finder, which opens files with their default apps (a .command runs in "
                                     "Terminal) and copies files with its own access, past FERB's protection lists"),
                "com.apple.scripteditor2": "Script Editor, which runs scripts",
                "com.apple.automator": "Automator, which runs workflows",
                "com.apple.shortcuts.events": "Shortcuts Events, which runs shortcuts",
                "com.apple.shortcuts": "Shortcuts, which runs shortcuts"}
# Round 3, O1: apps whose scripts open files with their default apps; a file a script names only indirectly (a path
# built at run time, an item of a folder) FERB can't resolve, and the question says so
OPENS_FILES = frozenset({"com.apple.finder", SYSTEM_EVENTS})
# Apps with a terminal panel whose focus Accessibility can't show (GPUI exposes no element tree): typing there asks.
NO_FACTS_HOSTS = {"dev.zed.zed": "Zed", "dev.zed.zed-preview": "Zed Preview"}
# Review round 1, finding 2: a menu-bar menu that operates an app's terminal or runs its code (the VS Code family's
# Terminal and Run menus, JetBrains' Run), by the menu's own title as Accessibility shows it (Electron and Swing menus
# carry no identifier): pressing any of its items asks, as typing does.
RUN_MENUS = re.compile(r"(?i)^\s*(terminal|run|console|shell)\s*$")
# A control that runs code: an editor's own run or debug-start icon class on its button (VS Code's codicons), or, in
# native content, a button whose label starts with Run or Debug (a label rule: errs toward asking).
RUN_CLASSES = frozenset({"codicon-play", "codicon-run", "codicon-run-all", "codicon-run-above", "codicon-run-below",
                         "codicon-run-errors", "codicon-debug-start", "codicon-debug-alt", "codicon-debug-alt-small"})
_RUN_LABEL = re.compile(r"(?i)^\s*(run|debug)(?![a-z])")
_CONTROLS = frozenset({"AXButton", "AXMenuButton", "AXLink", "AXMenuItem"})
# Review round 2, N6: editors, by bundle id (the JetBrains family by its prefix), where a Run… or Debug… label on a
# button or a context-menu item means running code in the app; elsewhere such a label decides nothing
EDITOR_APPS = frozenset({"com.microsoft.vscode", "com.microsoft.vscodeinsiders", "com.todesktop.230313mzl4w4u92",
                         "com.exafunction.windsurf", "com.vscodium", "com.visualstudio.code.oss",
                         "com.google.android.studio", "com.apple.dt.xcode", "dev.zed.zed", "dev.zed.zed-preview",
                         "com.panic.nova", "com.sublimetext.4"})
TERMINAL_MENUS = re.compile(r"(?i)^\s*(terminal|console|shell)\s*$")       # a menu that operates the terminal itself
# DOM classes a terminal library puts on its own nodes: xterm.js's root and its input textarea. Data, one entry per
# library; a page's text never reaches this check.
TERMINAL_CLASSES = frozenset({"xterm", "xterm-helper-textarea"})
_WORD = re.compile(r"(?i)(?<![a-z])(terminal|console|shell)(?![a-z])")
_CAMEL = re.compile(r"(?<=[a-z0-9])(?=[A-Z])")
_LABELS = ("title", "description")
_KINDS = ("role", "subrole", "role_description")
_NOT_READ = {"AXWindow", "AXApplication"}           # a window's title is a document's name, not the element's
SHOWN = 60                                          # display only: a label quoted in the trace's reason
LABEL_SHOWN = 30                                    # display only: the terminal's own label in the question


def _quote(v: str) -> str:
    one = " ".join(v.split())
    return json.dumps(one if len(one) <= SHOWN else one[:SHOWN - 1] + "…", ensure_ascii=False)


def _names(value: Any) -> bool:
    """Whether an identifier or a kind names a terminal as a word, camelCase split ("TerminalView", "shell-input")."""
    return isinstance(value, str) and bool(_WORD.search(_CAMEL.sub(" ", value)))


def can_host(bundle_id: str, owner_apps: Collection[str] = (), shell_app: bool = False) -> bool:
    """Whether an app can hold a terminal somewhere in it: a terminal app (built-in, the owner's, or one whose
    Info.plist declares the Shell role), an app whose terminal panel shows no facts (NO_FACTS_HOSTS), or an editor
    (`editor_app`: their terminal panels)."""
    bid = bundle_id.casefold()
    return (bid in TERMINAL_APPS or bid in {a.casefold() for a in owner_apps} or shell_app or bid in NO_FACTS_HOSTS
            or editor_app(bundle_id))


def surface(bundle_id: str, chain: Sequence[Mapping[str, Any]], owner_apps: Collection[str] = (),
            shell_app: bool = False, unread: bool = False) -> str | None:
    """Why code counts this a terminal, in words for the owner's question and the trace, else None. `chain`: the
    element's facts then its ancestors' (accessibility.surface_facts; empty when no element can be read); `owner_apps`:
    the owner's [protection] terminal_apps; `shell_app`: the app's Info.plist declares the Shell role; `unread`: the
    element input would reach couldn't be read (no focused element, no facts), so in an app that can host a terminal
    it counts as one (fails closed)."""
    bid = bundle_id.casefold()
    if bid in TERMINAL_APPS or bid in {a.casefold() for a in owner_apps}:
        return f"a terminal app ({bundle_id})"
    if shell_app:
        return f"an app whose Info.plist declares it a shell ({bundle_id})"
    if bid in NO_FACTS_HOSTS:
        return f"{NO_FACTS_HOSTS[bid]}, whose terminal panel Accessibility can't show, so any typing there asks"
    if unread and not chain and editor_app(bundle_id):
        return (f"an editor ({bundle_id}) whose focused element FERB couldn't read: it could be the editor's terminal, "
                "so FERB asks rather than type unchecked")
    web = any(f.get("dom_classes") or f.get("dom_identifier") for f in chain)
    for f in chain:
        hit = TERMINAL_CLASSES & {str(c).casefold() for c in f.get("dom_classes") or ()}
        if hit:
            return f"a terminal in the page (xterm.js: DOM class {sorted(hit)[-1]})"
    if web:
        return None
    for f in chain:
        for k in _KINDS:
            if _names(f.get(k)):
                return f"an element whose {k.replace('_', ' ')} is {_quote(str(f[k]))}"
        if _names(f.get("identifier")):
            return f"an element identified as {_quote(str(f['identifier']))}"
    for f in chain:
        if f.get("role") in _NOT_READ:
            continue
        for k in _LABELS:
            v = f.get(k)
            if isinstance(v, str) and _WORD.search(v):
                return f"an element labelled {_quote(v)} (a label only: FERB asks rather than guess)"
    return None


def _size(text: str) -> str:
    """A selection's size as FERB read it (round 3: Monaco may hold a shortened copy of a very large selection, so the
    owner can compare it with what they selected)."""
    lines = len(text.splitlines()) or 1
    return f"{len(text)} characters on {lines} line{'' if lines == 1 else 's'}"


def structural(f: Mapping[str, Any]) -> bool:
    """Whether one element is a terminal by its structure alone (round 3, N6): an xterm.js class, or, for a native
    element (no DOM facts), a role, subrole, role description or identifier naming a terminal. A label never counts."""
    if TERMINAL_CLASSES & {str(c).casefold() for c in f.get("dom_classes") or ()}:
        return True
    if f.get("dom_classes") or f.get("dom_identifier"):
        return False
    return any(_names(f.get(k)) for k in (*_KINDS, "identifier"))


def run_menu(path: Sequence[str]) -> bool:
    """Whether a menu path is under a menu-bar menu that operates the app's terminal or runs its code."""
    return bool(path) and bool(RUN_MENUS.match(path[0]))


def editor_app(bundle_id: str) -> bool:
    """An editor app (N6): the VS Code family, JetBrains IDEs, Xcode, Zed, Nova, Sublime Text."""
    b = bundle_id.casefold()
    return b in EDITOR_APPS or b.startswith("com.jetbrains.")


def run_control(chain: Sequence[Mapping[str, Any]], editor: bool = False) -> str | None:
    """Why pressing this control runs code: its run icon class (any app), or, in an editor (`editor_app`, or a window
    that hosts a terminal by structure: round 3, N6), a button without DOM classes or a menu item (a context menu's
    "Run Python File in Terminal") labelled Run… or Debug…. Else None."""
    if not chain or chain[0].get("role") not in _CONTROLS:
        return None
    f = chain[0]
    hit = RUN_CLASSES & {str(c).casefold() for c in f.get("dom_classes") or ()}
    if hit:
        return f"a control that runs code (class {sorted(hit)[0]})"
    if editor and (f.get("role") == "AXMenuItem" or not f.get("dom_classes")):
        for k in _LABELS:
            v = f.get(k)
            if isinstance(v, str) and _RUN_LABEL.match(v):
                return f"a control labelled {_quote(v)} (a label only: FERB asks rather than guess)"
    return None


# Characters that print but draw blank (Hangul fillers, the halfwidth filler, the braille blank): escaped too.
BLANK_GLYPHS = frozenset("\u3164\u115f\u1160\uffa0\u2800")


def _hidden(c: str, before: str) -> bool:
    """Whether a character would read as nothing (or as something else) in the question: non-printing ones (C0/C1,
    DEL, line and paragraph separators, format characters, non-ASCII spaces), blank glyphs, and a combining mark
    with no letter or digit before it (round 2: Unicode facts, never a meaning)."""
    if not c.isprintable() or c in BLANK_GLYPHS or unicodedata.category(c) == "Cf":
        return True
    return unicodedata.category(c) == "Mn" and not (before and unicodedata.category(before)[0] in "LN")


def visible(text: str, quote: bool = True) -> str:
    """Text as the owner must see it to know what goes in (review rounds 1 and 2, finding 5): JSON-quoted (`quote`),
    and every character that would read as nothing written as a visible \\uXXXX escape (`_hidden`). printable()
    would drop some silently; spaces stay as typed."""
    s = json.dumps(text, ensure_ascii=False) if quote else text
    out, before = [], ""
    for c in s:
        if _hidden(c, before):
            out.append(f"\\u{ord(c):04x}" if ord(c) <= 0xFFFF else f"\\U{ord(c):08x}")
            before = ""
        else:
            out.append(c)
            before = c
    return "".join(out)


def newline(text: str) -> bool:
    """Whether typed text holds its own Return (it runs as it is typed)."""
    return "\n" in text or "\r" in text


def need(inp: TerminalInput) -> confirm.Need:
    """The owner's question for one terminal input: always asked (a Yes covers this one run, `always`). The key names
    the exact input, so a No is final for it in this task tree (until the owner's own follow-up reopens it); a Yes is
    spent by the call it covers and never covers another."""
    shown = visible(inp.text) if inp.text is not None else ""
    label = " ".join(inp.label.split())
    label = label if len(label) <= LABEL_SHOWN else label[:LABEL_SHOWN - 1].rstrip() + "…"
    values = {"app": visible(inp.app, False), "surface": visible(inp.surface, False), "text": shown,
              "keys": visible(inp.keys or "", False), "window": visible(inp.window) if inp.window else "",
              "label": visible(label) if label else "", "menu": visible(inp.menu) if inp.menu else "",
              "secret": visible(inp.secret) if inp.secret else "", "script": visible(inp.script) if inp.script else "",
              "drag": "yes" if inp.drag else "", "runs": "yes" if inp.runs else "",
              "control": visible(inp.control) if inp.control else "",
              "selection": visible(inp.selection) if inp.selection else "",
              "selection_size": _size(inp.selection) if inp.selection else "",
              "file": visible(inp.file) if inp.file else "", "runs_where": visible(inp.runs_where, False),
              "files": "; ".join(f"{visible(path)}, which can run when opened and holds {visible(text)}"
                                 for path, _, text in inp.files),
              "named": "; ".join(f"{visible(path)}, which can't run when opened" for path in inp.named),
              "paste": "yes" if inp.paste else "", "concealed": "yes" if inp.concealed else "",
              "enter": "yes" if inp.enter else "", "opens": "yes" if inp.opens else ""}
    return confirm.Need(RULE, confirm.key(RULE, inp.bundle_id.casefold(), inp.text, inp.keys, inp.paste, inp.menu,
                                                  inp.secret, inp.script, inp.drag, inp.control, inp.selection,
                                                  inp.file, [(p, h) for p, h, _ in inp.files]),
                        values, always=True)
