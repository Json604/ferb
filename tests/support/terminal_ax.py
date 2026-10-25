"""Shared fakes for the terminal floor (D31, rule 10): apps whose Accessibility trees hold terminals, with the facts
the live trees showed. No native input is sent: typed text and keys are recorded with the element they land in.

VS Code's facts come from task 6f5b85894b3140e0b434910301e088a6 (2026-10-25 07:04 and 07:11): its snapshots showed the
terminal input as a textbox labelled "Terminal 108, zsh Use ⌥F1 for terminal accessibility help" (s6e695), and a
read-only Accessibility read of the same VS Code window showed what is above it: an AXTextField with DOM class
`xterm-helper-textarea`, inside groups with classes `xterm-helpers`, `xterm-screen`, `terminal xterm …`,
`terminal-xterm-host` and the panel with DOM id `terminal`. JetBrains IDEs, Xcode and Nova expose no DOM classes;
their facts here are the labels their tool windows and text views carry."""

from dataclasses import dataclass, field
from typing import Any

import pytest

import ferb.act.mac.driver as drv
from ferb.act.gui_lock import GuiLock
from ferb.act.mac.accessibility import AxNode, AxTree
from ferb.act.mac.apps import RunningApp
from ferb.act.mac.driver import MacComputerDriver
from ferb.act.mac.input import RealInput, Step

CODE = "com.microsoft.VSCode"
PYCHARM = "com.jetbrains.pycharm"
TERMINAL_LABEL = "Terminal 108, zsh Use ⌥F1 for terminal accessibility help"

# VS Code's workbench above both the editor and the terminal panel
WORKBENCH = [{"role": "AXGroup", "dom_classes": ("monaco-split-view2", "horizontal")},
             {"role": "AXGroup", "dom_classes": ("split-view-container",)},
             {"role": "AXWebArea", "title": "FERB", "dom_classes": ("monaco-workbench", "mac")},
             {"role": "AXWindow", "title": "FERB"},
             {"role": "AXApplication", "title": "Code"}]
XTERM_HOST = [{"role": "AXGroup", "dom_classes": ("terminal-xterm-host",)},
              {"role": "AXGroup", "dom_classes": ("terminal-groups-container",)},
              {"role": "AXGroup", "dom_classes": ("pane-body", "shell-integration", "integrated-terminal", "wide")},
              {"role": "AXGroup", "dom_classes": ("composite", "panel"), "dom_identifier": "terminal"}] + WORKBENCH


def xterm_input(label: str) -> list[dict[str, Any]]:
    """An xterm.js terminal's input and what is above it, as VS Code's live tree showed them."""
    return [{"role": "AXTextField", "role_description": "text field", "dom_classes": ("xterm-helper-textarea",),
             "description": label},
            {"role": "AXGroup", "dom_classes": ("xterm-helpers",)},
            {"role": "AXGroup", "dom_classes": ("xterm-screen",)},
            {"role": "AXGroup", "dom_classes": ("terminal", "xterm", "xterm-dom-renderer-owner-84", "focus")}
            ] + XTERM_HOST


TERMINAL = xterm_input(TERMINAL_LABEL)
CLAUDE_TERMINAL = xterm_input("Terminal 57, claude Use ⌥F1 for terminal accessibility help")
EDITOR = [{"role": "AXTextArea", "role_description": "editor", "dom_classes": ("inputarea", "monaco-mouse-cursor-text"),
           "description": "Editor content;Press Alt+F1 for Accessibility Options."},
          {"role": "AXGroup", "dom_classes": ("overflow-guard",)},
          {"role": "AXGroup", "dom_classes": ("monaco-editor", "no-user-select", "mac", "vs-dark")},
          {"role": "AXGroup", "dom_classes": ("editor-instance",)},
          {"role": "AXGroup", "dom_classes": ("editor-group-container", "active")}] + WORKBENCH
JETBRAINS_TERMINAL = [{"role": "AXTextArea", "role_description": "text entry area", "description": "Local"},
                      {"role": "AXScrollArea"},
                      {"role": "AXGroup", "title": "Terminal Tool Window"},
                      {"role": "AXSplitGroup"},
                      {"role": "AXWindow", "title": "FERB – main.py"},
                      {"role": "AXApplication", "title": "PyCharm"}]
JETBRAINS_EDITOR = [{"role": "AXTextArea", "role_description": "text entry area", "description": "Editor for main.py"},
                    {"role": "AXScrollArea"},
                    {"role": "AXGroup", "title": "Editor"},
                    {"role": "AXWindow", "title": "FERB – terminal notes.md"},     # a window title is never read
                    {"role": "AXApplication", "title": "PyCharm"}]
XCODE_CONSOLE = [{"role": "AXTextArea", "description": "Console"}, {"role": "AXScrollArea"},
                 {"role": "AXGroup", "description": "Debug area"}, {"role": "AXWindow", "title": "App"},
                 {"role": "AXApplication", "title": "Xcode"}]
NOVA_TERMINAL = [{"role": "AXTextArea", "description": "Terminal"}, {"role": "AXGroup"},
                 {"role": "AXWindow", "title": "site"}, {"role": "AXApplication", "title": "Nova"}]
PAGE_SHELL_BOX = [{"role": "AXTextArea", "description": "Shell command", "dom_classes": ("cmd-box",)},
                  {"role": "AXGroup", "dom_classes": ("docs",)}, {"role": "AXWebArea", "title": "Docs"},
                  {"role": "AXWindow", "title": "Docs"}, {"role": "AXApplication", "title": "Google Chrome"}]
PAGE_XTERM = xterm_input("Terminal input")[:4] + [{"role": "AXWebArea", "title": "Cloud Shell"},
                                                  {"role": "AXWindow", "title": "Cloud Shell"},
                                                  {"role": "AXApplication", "title": "Google Chrome"}]

PASTE = ("V", 0)                                          # ⌘V
NEW_TERMINAL = ("`", 1 | 4 | 8)                           # ⌃⇧`, no ⌘


@dataclass
class TerminalAx:
    """One app window: elements are strings, each with its chain of facts. `focused` is the app's keyboard focus;
    focusing a container moves it to the container's input (`inner`). Typing and keys land where the focus is."""
    chains: dict[str, list[dict[str, Any]]]
    focused: str | None = None
    labels: dict[str, str] = field(default_factory=dict)
    inner: dict[str, str] = field(default_factory=dict)          # container -> the input focusing it reaches
    parent: dict[str, str] = field(default_factory=dict)
    shortcuts: dict[str, tuple[str, int]] = field(default_factory=dict)      # "Edit > Paste" -> (char, modifiers)
    keeps_focus: bool = True                                     # False: focusing an element doesn't take
    values: dict[str, str] = field(default_factory=dict)
    landed: list[tuple[str | None, str]] = field(default_factory=list)       # (element, what) for every input
    menus_pressed: list[str] = field(default_factory=list)
    windows: dict[str, str] = field(default_factory=dict)        # element -> its window's title (default "FERB")
    on_char: Any = None                                          # called after each typed character (a test's hook)

    def typed(self, ch: str) -> None:
        """One typed character lands where the focus is; consecutive ones into one element read as one "type"."""
        if self.landed and self.landed[-1][0] == self.focused and self.landed[-1][1].startswith("type ") and \
                getattr(self, "_typing", False):
            self.landed[-1] = (self.focused, self.landed[-1][1] + ch)
        else:
            self.landed.append((self.focused, "type " + ch))
        self._typing = True
        if self.on_char is not None:
            self.on_char(self)

    hits: dict[tuple[int, int], str] = field(default_factory=dict)     # screen point -> the element under it
    selections: dict[str, str] = field(default_factory=dict)          # element -> its selected text

    def hit_element(self, pid: int, x: float, y: float) -> Any:
        return self.hits.get((int(x), int(y)))

    def role_of(self, el: Any) -> str:
        return str(self.chains.get(el, [{}])[0].get("role", ""))

    def selected_text(self, el: Any) -> str | None:
        return self.selections.get(el)

    documents: dict[str, str] = field(default_factory=dict)           # element -> its window's document (a path)

    def document_of(self, el: Any) -> str | None:
        return self.documents.get(el)

    def place_of(self, el: Any) -> tuple[str, str]:
        chain = self.chains.get(el, [{}])
        return self.windows.get(el, "FERB"), self.labels.get(el, str(chain[0].get("description") or ""))

    def read_tree(self, pid: int, browser: bool = False, window: Any = None, offset: int = 0) -> AxTree:
        nodes = [AxNode(el, "menuitem" if chain[0]["role"] == "AXMenuItem" else "textbox",
                        self.labels.get(el, str(chain[0].get("description") or chain[0].get("title") or el)),
                        self.values.get(el, ""), (0, 10 * i, 100, 10), False, True, chain[0]["role"])
                 for i, (el, chain) in enumerate(self.chains.items())]
        return AxTree(nodes, "", "FERB")

    def expose_tree(self, pid: int) -> bool:
        return True

    def app_window(self, pid: int) -> Any:
        return "win"

    def window_title(self, win: Any) -> str:
        return "FERB"

    def web_areas_of(self, win: Any) -> list[Any]:
        return []

    def open_menus(self, pid: int) -> list[Any]:
        return []

    def surface_facts(self, el: Any) -> list[dict[str, Any]]:
        return [dict(f) for f in self.chains.get(el, [])]

    def window_facts(self, el: Any) -> list[dict[str, Any]]:
        """Each element's own facts in `el`'s window (elements share a window by `windows`, default "FERB")."""
        where = self.windows.get(el, "FERB")
        return [dict(chain[0]) for other, chain in self.chains.items() if self.windows.get(other, "FERB") == where]

    def focused_element(self, pid: int) -> Any:
        return self.focused

    def same(self, a: Any, b: Any) -> bool:
        return a is not None and a == b

    def focus_of(self, el: str) -> str:
        return self.inner.get(el, el)

    def has_focus(self, pid: int, el: Any) -> bool | None:
        if self.focused is None:
            return None
        node: str | None = self.focused
        while node is not None:
            if node == el:
                return True
            node = self.parent.get(node)
        return False

    def focus(self, el: Any) -> bool:
        if self.keeps_focus:
            self.focused = self.focus_of(el)
        return True

    def value(self, el: Any) -> str | None:
        return self.values.get(el)

    def post_text(self, pid: int, text: str) -> bool:
        self.landed.append((self.focused, f"ax keys {text}"))
        return True

    def post_key(self, pid: int, keycode: int, mods: list[str]) -> bool:
        self.landed.append((self.focused, f"ax key {keycode}"))
        return True

    def set_value(self, el: Any, text: str) -> bool:
        self.landed.append((el, f"value {text}"))
        self.values[el] = text
        return True

    def selected_range(self, el: Any) -> tuple[int, int] | None:
        return (len(self.values.get(el, "")), 0)

    def set_selected_range(self, el: Any, start: int, length: int) -> bool:
        return True

    def set_selected_text(self, el: Any, text: str) -> bool:
        self.landed.append((el, f"edit {text}"))
        self.values[el] = self.values.get(el, "") + text
        return True

    def press(self, el: Any) -> bool:
        self.landed.append((self.focused, f"press {el}"))
        return True

    def item_shortcut(self, el: Any) -> tuple[str, int] | None:
        return self.shortcuts.get(el)

    def menu_item_shortcut(self, pid: int, path: list[str]) -> tuple[str, int] | None:
        return self.shortcuts.get(" > ".join(path))

    def press_menu(self, pid: int, path: list[str]) -> bool:
        item = " > ".join(path)
        self.menus_pressed.append(item)
        if item == "Terminal > New Terminal":                # the new terminal takes the keyboard focus
            self.focused = "term108"
        elif self.shortcuts.get(item) == PASTE:
            self.landed.append((self.focused, "paste"))
        return True

    def menu_item_for_shortcut(self, pid: int, key: str, mods: list[str]) -> Any:
        if key == "v" and mods == ["cmd"]:
            return ("Edit > Paste", "Edit > Paste", True)
        return None

    def center(self, n: AxNode) -> tuple[float, float]:
        return 10.0, 10.0

    def cancel(self, el: Any) -> bool:
        return True


RUN_BUTTON = [{"role": "AXButton", "description": "Run Python File",
               "dom_classes": ("action-label", "codicon", "codicon-play")}] + WORKBENCH
EXPLORER_BUTTON = [{"role": "AXButton", "description": "Explorer (⇧⌘E)",
                    "dom_classes": ("action-label", "codicon", "codicon-explorer-view-icon")}] + WORKBENCH


def vscode_ax(focus: str | None = "term57") -> TerminalAx:
    """VS Code with its editor, the owner's Claude terminal (57) and FERB's new terminal (108); Edit > Paste is ⌘V and
    the terminal's context menu offers Paste."""
    return TerminalAx({"editor": EDITOR, "term57": CLAUDE_TERMINAL, "term108": TERMINAL,
                       "termhost": TERMINAL[4:], "ctxpaste": [{"role": "AXMenuItem", "title": "Paste"}]},
                      focused=focus, labels={"term108": TERMINAL_LABEL, "termhost": "terminal host",
                                           "editor": "Editor content", "ctxpaste": "Paste"},
                      inner={"termhost": "term108"}, parent={"term108": "termhost"},
                      shortcuts={"Edit > Paste": PASTE, "ctxpaste": PASTE, "Terminal > New Terminal": NEW_TERMINAL})


class TerminalApps:
    def __init__(self, *apps: RunningApp, shells: frozenset[str] = frozenset()) -> None:
        self.apps = list(apps) or [RunningApp(CODE, "Code", 41154, False)]
        self.shells = shells
        self.activated: list[str] = []

    def find(self, q: str) -> RunningApp | None:
        hits = [a for a in self.apps if q.lower() in (a.bundle_id.lower(), a.name.lower(), f"pid:{a.pid}")]
        return hits[0] if len(hits) == 1 else None

    def candidates(self, q: str) -> list[RunningApp]:
        return [a for a in self.apps if q.lower() in (a.bundle_id.lower(), a.name.lower(), f"pid:{a.pid}")]

    def running(self) -> list[RunningApp]:
        return self.apps

    def pids_of(self, bundle_ids: Any) -> set[int]:
        return {a.pid for a in self.apps if a.bundle_id.casefold() in bundle_ids}

    def frontmost(self) -> RunningApp | None:
        return next((a for a in self.apps if a.active), None)

    def activate(self, bid: str, pid: int | None = None) -> bool:
        self.activated.append(bid)
        self.apps = [RunningApp(a.bundle_id, a.name, a.pid, a.bundle_id == bid) for a in self.apps]
        return True

    def declares_shell(self, bundle_id: str) -> bool:
        return bundle_id in self.shells


def terminal_mac(mp: pytest.MonkeyPatch, ax: TerminalAx, apps: TerminalApps | None = None,
                 clipboard: tuple[int, str | None, bool] = (7, "codex", False)) -> MacComputerDriver:
    """A real driver over the fake app: process-directed and foreground typing and keys land where `ax.focused` is."""
    async def no_sleep(s: float) -> None:
        return None

    def typed(text: str, pid: int | None = None) -> list[Step]:
        ax._typing = False                                    # type: ignore[attr-defined]   (a new run of typing)
        return [Step(f"type {i + 1}/{len(text)}", lambda ch=ch: ax.typed(ch)) for i, ch in enumerate(text)]

    def keyed(combo: str, pid: int | None = None) -> list[Step]:
        return [Step(combo, lambda: ax.landed.append((ax.focused, f"key {combo}")))]
    mp.setattr(drv, "type_steps", typed)
    mp.setattr(drv, "key_steps", keyed)
    mp.setattr(drv, "click_steps", lambda x, y, pid=None: [Step("click", lambda: ax.landed.append((ax.focused, "click")))])
    mp.setattr(drv, "SETTLE_S", 0)
    d = MacComputerDriver("dev", RealInput(lambda: False, GuiLock(), sleep=no_sleep), ax=ax,
                          apps=apps or TerminalApps())
    board = [clipboard]
    d.clipboard = lambda: board[0]
    d.board = board                                           # type: ignore[attr-defined]   (a test copies anew)
    return d


FINDER = "com.apple.finder"


def finder_ax(files: dict[str, str], selected: tuple[str, ...] = ()) -> TerminalAx:
    """A Finder window: each element is a file item (an AXImage whose AXURL is the file), `selected` the selection."""
    ax = TerminalAx({el: [{"role": "AXImage", "title": path.rsplit("/", 1)[-1]}, {"role": "AXGroup"},
                          {"role": "AXWindow", "title": "Desktop"}, {"role": "AXApplication", "title": "Finder"}]
                     for el, path in files.items()}, focused=None,
                    labels={el: path.rsplit("/", 1)[-1] for el, path in files.items()})
    ax.file_paths = dict(files)                                   # type: ignore[attr-defined]
    ax.selection = list(selected)                                 # type: ignore[attr-defined]
    ax.file_of = lambda el: ax.file_paths.get(el)                 # type: ignore[attr-defined]
    ax.selected_items = lambda pid: list(ax.selection)            # type: ignore[attr-defined]
    return ax
