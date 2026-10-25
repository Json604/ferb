"""Owns which on-screen element is a terminal (D31, CLAUDE.md rule 10) and the owner's question before FERB types into
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
Limit: a custom terminal panel that exposes none of these facts, in an app not listed, is not recognized."""

import json
import re
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
# Apps with a terminal panel whose focus Accessibility can't show (GPUI exposes no element tree): typing there asks.
NO_FACTS_HOSTS = {"dev.zed.zed": "Zed", "dev.zed.zed-preview": "Zed Preview"}
# DOM classes a terminal library puts on its own nodes: xterm.js's root and its input textarea. Data, one entry per
# library; a page's text never reaches this check.
TERMINAL_CLASSES = frozenset({"xterm", "xterm-helper-textarea"})
_WORD = re.compile(r"(?i)(?<![a-z])(terminal|console|shell)(?![a-z])")
_CAMEL = re.compile(r"(?<=[a-z0-9])(?=[A-Z])")
_LABELS = ("title", "description")
_KINDS = ("role", "subrole", "role_description")
_NOT_READ = {"AXWindow", "AXApplication"}           # a window's title is a document's name, not the element's
SHOWN = 60                                          # display only: a label quoted in the trace's reason


def _quote(v: str) -> str:
    one = " ".join(v.split())
    return json.dumps(one if len(one) <= SHOWN else one[:SHOWN - 1] + "…", ensure_ascii=False)


def _names(value: Any) -> bool:
    """Whether an identifier or a kind names a terminal as a word, camelCase split ("TerminalView", "shell-input")."""
    return isinstance(value, str) and bool(_WORD.search(_CAMEL.sub(" ", value)))


def surface(bundle_id: str, chain: Sequence[Mapping[str, Any]], owner_apps: Collection[str] = (),
            shell_app: bool = False) -> str | None:
    """Why code counts this a terminal, in words for the owner's question and the trace, else None. `chain`: the
    element's facts then its ancestors' (accessibility.surface_facts; empty when no element can be read); `owner_apps`:
    the owner's [protection] terminal_apps; `shell_app`: the app's Info.plist declares the Shell role."""
    bid = bundle_id.casefold()
    if bid in TERMINAL_APPS or bid in {a.casefold() for a in owner_apps}:
        return f"a terminal app ({bundle_id})"
    if shell_app:
        return f"an app whose Info.plist declares it a shell ({bundle_id})"
    if bid in NO_FACTS_HOSTS:
        return f"{NO_FACTS_HOSTS[bid]}, whose terminal panel Accessibility can't show, so any typing there asks"
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


def newline(text: str) -> bool:
    """Whether typed text holds its own Return (it runs as it is typed)."""
    return "\n" in text or "\r" in text


def need(inp: TerminalInput) -> confirm.Need:
    """The owner's question for one terminal input: always asked (a Yes covers this one run, `always`). The key names
    the exact input, so a No is final for it in this task tree (until the owner's own follow-up reopens it); a Yes is
    spent by the call it covers and never covers another."""
    shown = json.dumps(inp.text, ensure_ascii=False) if inp.text is not None else ""
    values = {"app": inp.app, "surface": inp.surface, "text": shown, "keys": inp.keys or "",
              "paste": "yes" if inp.paste else "", "concealed": "yes" if inp.concealed else "",
              "enter": "yes" if inp.enter else ""}
    return confirm.Need(RULE, confirm.key(RULE, inp.bundle_id.casefold(), inp.text, inp.keys, inp.paste),
                        values, always=True)
