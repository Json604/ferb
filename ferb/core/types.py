"""Owns shared task, action and observation data types. Must not infer intent, choose a target or claim semantic success from transport facts.

Core execution data (see ARCHITECTURE.md). Pure data: no I/O, imports nothing from ferb except core."""

from dataclasses import dataclass, field
from datetime import datetime
from enum import Enum
from pathlib import Path
from typing import Any, Callable, Literal


class Identity(Enum):
    FERB = "ferb"      # FERB's own profile + credentials
    OWNER = "owner"    # owner's logged-in browsers/apps; no credentials ever


@dataclass(frozen=True)
class SecretRef:
    key: str           # e.g. "ferb/email/password" (FERB identity only)


@dataclass(frozen=True)
class SecretSpec:
    ref: SecretRef
    allowed_origins: list[str]
    account_hint: str


class RiskLevel(Enum):
    LOW = 1
    MEDIUM = 2
    HIGH = 3


@dataclass
class Task:
    id: str
    parent_id: str | None
    goal: str
    device_id: str
    identity: Identity
    scopes: set[str]
    role: Literal["worker", "verifier", "mixed"]   # execution purpose, not a tool permission class
    request_id: str
    success_conditions: list[str]
    deadline: datetime | None                # an explicit deadline if supplied; ordinary tasks have none


@dataclass
class Outcome:
    task_id: str
    status: Literal["done", "failed", "unknown", "cancelled", "needs_user", "blocked"]
    summary: str
    artifacts: list[str]
    evidence: list[str]


@dataclass
class ActionIntent:
    id: str
    task_id: str
    capability: str
    target: dict[str, Any]
    payload_hash: str
    state: Literal["intended", "executing", "done", "failed", "unknown"]


@dataclass
class Span:
    id: str
    parent_id: str | None
    kind: str
    task_id: str | None        # None for daemon-level spans
    device_id: str | None
    started_at: datetime
    ended_at: datetime | None
    input: dict[str, Any]
    output: dict[str, Any] | None
    cost_usd: float | None
    error: str | None
    origin: str | None = None  # a tool call's result origins, comma-joined (ferb/core/origin.py)
    summary: str | None = None  # a tool call's one-line result summary (ferb/trace/lines.py)


@dataclass
class TrailEvent:
    id: str
    at: datetime
    device_id: str
    actor: Literal["owner", "ferb", "unknown"]
    task_id: str | None
    source: str
    data: dict[str, Any]


@dataclass
class MemoryItem:
    id: str
    kind: Literal["fact", "preference", "person", "project", "env"]
    subject: str
    key: str                                  # what it's about ("dentist"): versions of one fact share it
    content: str
    source_ids: list[str]
    observed_at: datetime
    valid_from: datetime
    valid_to: datetime | None
    supersedes: str | None
    confidence: float
    origin: Literal["owner_stated", "observed", "inferred", "third_party"]
    status: Literal["proposed", "active", "retracted"]


# ---------- actions (descriptive execution metadata) ----------

@dataclass(frozen=True)
class Target:
    account: str | None = None               # e.g. "gmail:me@example.com"
    site: str | None = None                  # origin/host, e.g. "mail.google.com"
    recipients: tuple[str, ...] = ()
    path: str | None = None
    amount: float | None = None
    contact: str | None = None               # the exact contact a call / message goes to, as shown
    page: str | None = None                  # the page a browser input acts on, as its tab shows it (a fact)
    item: str | None = None                  # what a declared delete removes, as the agent says it is shown (a note)


@dataclass(frozen=True)
class ActionSpec:
    """Capability.describe(args): what the call does, recorded for tracing and result inspection. `kind` is the
    physical operation the capability derives from its arguments; `declared` a kind the agent's stated purpose adds
    (a click it says sends or calls); `effect_kind` the kind the agent's declared effect adds (ferb/policy/rules.py
    `declare`). Every declared kind is kept, and can only add a confirmation, never remove one (W2). `place` is
    where the call happens as the owner knows it (an app's name), for FERB's questions; else the target's site."""
    kind: str
    target: Target = Target()
    content: str | None = None
    declared: str | None = None
    place: str | None = None
    effect_kind: str | None = None


# ---------- runtimes ----------

@dataclass(frozen=True)
class Capabilities:
    """What a model runtime supports (D32's capability table): facts for planning and for `ferb runtimes`."""
    tools: Literal["direct", "deferred", "none"]      # how FERB's gateway tools reach the model
    cache: Literal["prefix", "markers"]               # automatic on an identical start, or FERB marks the cacheable prefix
    cache_key: bool                                   # FERB keeps one cache-routing key per role
    structured_output: Literal["native", "tool"]      # the provider enforces the schema, or a final-answer tool carries it
    reasoning_effort: bool
    image_input: bool                                 # image tool results reach the model
    continuation: bool
    live_messaging: bool
    billing: Literal["subscription", "api"]
    native_tools_contained: bool = True               # must be True to be registered


@dataclass(frozen=True)
class ModelImage:
    """A picture handed to a model with its request (L12): bytes already redacted (rule 14), sent to the model only,
    never into a span, the journal or a log. `ref` names its evidence-store row, which the trace records instead."""
    data: bytes = field(repr=False)
    ref: str
    mime: str = "image/jpeg"

    def base64(self) -> str:
        import base64
        return base64.b64encode(self.data).decode()

    def data_url(self) -> str:
        return f"data:{self.mime};base64,{self.base64()}"


@dataclass(frozen=True)
class ModelLimits:
    """A model's documented limits (L28): its token window and reply limit, and the runtime's per-input character cap
    where it documents one; provider facts, each adapter citing where it found them. FERB never picks or guesses these;
    a runtime that can't report its window fails explicitly (ModelLimitsUnknown)."""
    model: str
    context_window: int                  # tokens the model holds
    max_output: int                      # the longest reply it can write
    source: str                          # where the numbers are documented
    window_includes_output: bool = True  # False: the provider documents an input limit, with the reply counted apart
    max_input_chars: int | None = None   # characters one input may hold, where the runtime documents a cap (Codex)

    @property
    def context_budget(self) -> int:
        """Room for FERB's prompt: the window minus room for the reply (an input limit is already that room)."""
        return self.context_window - self.max_output if self.window_includes_output else self.context_window


@dataclass(frozen=True)
class ModelRequest:
    """What FERB sends any model runtime (D32): its instructions and the context in order, stable parts first (P20),
    and any images, which each adapter places after the text so the cacheable prefix stays the same."""
    system: str
    parts: tuple[str, ...]
    cacheable: int = 0                       # parts[:cacheable] repeat across this role's calls; adapters cache them
    images: tuple[ModelImage, ...] = ()

    @property
    def context(self) -> str:
        return "\n\n".join(self.parts)


@dataclass
class RunHandle:
    id: str
    runtime: str
    task_id: str
    state: dict[str, Any]                    # runtime-private (process, session, ...)


@dataclass(frozen=True)
class RuntimeEvent:
    # invalid_answer: the model answered, but its final answer doesn't match the requested schema (format repair, not
    # a failed call); data["message"] says where. retrying: the runtime hit an error it retries itself (Codex
    # willRetry); the run goes on; data["message"] is the runtime's redacted error
    kind: Literal["message", "text_delta", "tool_call", "usage", "final", "error", "violation", "invalid_answer",
                  "retrying"]
    data: dict[str, Any]


# ---------- tools ----------

# What a call does to a file it names (a capability's `file_paths`): the protection lists check each one (rule 10).
FileAccess = Literal["read", "write", "remove", "metadata"]


@dataclass(frozen=True)
class EscapeGrant:
    rule: str
    confirmation_id: str
    command: str
    cwd: str
    secret: str | None = field(default=None, repr=False)


@dataclass(frozen=True)
class TerminalInput:
    """Input about to reach a terminal (D31, rule 10: code asks the owner first, every time), as code found it: the
    app, why it is a terminal (ferb/policy/terminal.py), and exactly what goes in. Facts only; never a judgement of
    what the text does."""
    app: str                                 # the app's name as the owner knows it
    bundle_id: str
    surface: str                             # why code counts it a terminal, for the question and the trace
    text: str | None = None                  # the exact text typed, or pasted (`paste`)
    keys: str | None = None                  # a key or combo pressed ("ctrl+c", "return")
    paste: bool = False                      # the text is the clipboard's, put there by a paste
    concealed: bool = False                  # paste: the clipboard item is marked secret; its text isn't shown
    clipboard: int | None = None             # paste: the pasteboard's change count when asked (a new copy asks again)
    enter: bool = False                      # the owner's Yes also covers one Return after this text
    covered_by: str | None = None            # a Return covered by the Yes to the text FERB just typed there
    window: str = ""                         # the title of the window it is in
    menu: str | None = None                  # a menu command of a terminal app, by its path or title
    secret: str | None = None                # a hidden secret filled in (mac.secret): its name, never its value
    script: str | None = None                # an AppleScript for a terminal app or System Events (mac.applescript)
    drag: bool = False                       # dragged onto it (`text`: what was selected where the drag began)
    runs: bool = False                       # a menu item or a control that runs things in the app's terminal
    control: str | None = None               # such a control, by its label ("Run Python File")
    selection: str | None = None             # the editor's selected text such a press would run
    file: str | None = None                  # the active file such a press would run
    runs_where: str = ""                     # where such a press runs code: the app, or its terminal
    # files the input would open in a terminal app (a .command): (path, sha256 of its contents, its text as shown)
    files: tuple[tuple[str, str, str], ...] = ()
    label: str = ""                          # the terminal element's own label (VS Code: "Terminal 108, zsh …")
    opens: bool = False                      # a script for an app that opens files by their default app (round 3, O1)


@dataclass(frozen=True)
class TerminalGrant:
    """The owner's Yes to one TerminalInput (one run: the next input asks again). The driver delivers only the input
    the grant names and refuses any other input that would reach a terminal."""
    input: TerminalInput
    confirmation_id: str


@dataclass(frozen=True)
class ToolContext:
    task_id: str
    worker_id: str
    request_id: str
    identity: Identity
    scopes: frozenset[str]
    device_id: str
    span_id: str | None
    workspace: str
    owner_roots: tuple[str, ...]
    untrusted_roots: tuple[str, ...]
    intent_id: str | None = None             # ledger row of this call when consequential (drivers tag with it)
    observe_only: bool = False               # the final checker's call (V3, W4): its tools keep it to observing
    # a loop's probe read between rounds (L31): what a tool hands over once (a tab's new downloads and tabs) stays for
    # the next round
    probe: bool = False
    # The protection lists' check of a file as the tool really opened it (macOS F_GETPATH) and what it does to it:
    # the error to return when the lists close it, else None. None when the daemon has no lists (rule 10).
    path_closed: Callable[[Path, str], str | None] | None = None
    escape: EscapeGrant | None = None
    terminal: TerminalGrant | None = None    # the owner's Yes to the one terminal input this call makes (D31)


@dataclass(frozen=True)
class ToolResult:
    content: dict[str, Any]
    is_error: bool = False
    read_sources: tuple[tuple[str, str], ...] = ()   # (source kind, id) annotations for the data read by this call
    image: bytes | None = None                        # JPEG for the model and the evidence store (V1); never logged
    evidence: dict[str, Any] | None = None            # source observations used by result inspection


# ---------- browsers ----------

@dataclass(frozen=True)
class TabRef:
    device_id: str
    browser: str                             # "chrome" | "brave" | ...
    profile: str                             # profile folder, e.g. "Profile 22"
    generation: int                          # connection generation: a reconnect makes older refs stale
    tab_id: str


@dataclass(frozen=True)
class ElementRef:
    tab: TabRef
    frame_id: str
    snapshot_gen: int                        # the snapshot the id came from; stale after navigation / newer snapshot
    element_id: str


@dataclass(frozen=True)
class DriverFeatures:
    trusted_input_background: bool
    parallel_actions: bool
    network_events: bool
    dom_snapshot: bool
    screenshot_background: bool


@dataclass(frozen=True)
class BrowserAction:
    kind: Literal["click", "dblclick", "rightclick", "hover", "type", "select", "press", "scroll", "upload", "drag"]
    text: str | None = None                  # for type: the text; select: option label or value; press: key ("cmd+a");
                                             # scroll: up|down
    trusted: bool = False                    # kept for callers that insist; real input is the default for click/type/press
    page_mode: bool = False                  # scripted (untrusted) page events instead of real input
    expect_dialog: Literal["accept", "dismiss"] | None = None   # a JS dialog this action is expected to open
    dialog_text: str | None = None           # prompt() answer when accepting
    files: tuple[str, ...] = ()              # upload: absolute paths
    end_element: str | None = None           # drag: another screenshot-resolved point in the same tab
    # upload: the protection lists' check of each file as the driver really opened it (ToolContext.path_closed)
    files_closed: Callable[[Path, str], str | None] | None = field(default=None, compare=False, repr=False)
    terminal: TerminalGrant | None = field(default=None, compare=False)   # the owner's Yes to this input (D31)


@dataclass(frozen=True)
class WaitCondition:
    kind: Literal["load", "text", "url"]
    value: str | None = None


@dataclass(frozen=True)
class ActionResult:
    ok: bool
    detail: str = ""
    facts: dict[str, Any] = field(default_factory=dict)


@dataclass(frozen=True)
class PageElement:
    id: str
    frame_id: str
    role: str
    label: str
    value: str | None
    rect: tuple[float, float, float, float]  # x, y, width, height (viewport of its frame)
    visible: bool
    input_type: str | None = None
    href: str | None = None
    form: str | None = None                  # "search" | "login" | "other" | None: what submitting it would do
    secret: bool = False                     # password / OTP field: value never leaves the driver
    code_fill: bool = False                  # a login code may be sealed into it (the extension's own fill rule)
    in_view: bool = True                     # on screen now (rect meets the viewport and nothing covers its centre)
    offscreen: str | None = None             # "above" | "below" | "left" | "right" when not on screen
    occluded: bool = False                   # on screen but covered by something else
    container: str | None = None             # id of its scroll container (see PageSnapshot.scrollers), None = window
    state: tuple[str, ...] = ()              # "disabled", "expanded", "collapsed", "selected"
    level: int | None = None                 # headings
    terminal: bool = False                   # inside a terminal library's own nodes (xterm.js), D31


@dataclass(frozen=True)
class PageSnapshot:
    tab: TabRef
    generation: int
    url: str
    title: str
    elements: list[PageElement] = field(default_factory=list)
    text: str = ""
    truncated: bool = False
    dialogs: list[str] = field(default_factory=list)   # JS dialogs dismissed since the last snapshot
    scroll: dict[str, Any] | None = None     # the window scroller: {y, height, viewport_h, at_end}
    scrollers: list[dict[str, Any]] = field(default_factory=list)   # containers: {id, label, y, height, viewport_h, at_end}
    # video/audio elements (L26): {index, playing (its time moved, I5), paused, current_time, duration, muted, volume,
    # in_view[, frame]}
    media: list[dict[str, Any]] = field(default_factory=list)


# ---------- Mac / computer ----------

@dataclass(frozen=True)
class ComputerAction:
    kind: Literal["press", "set_value", "focus", "click", "type", "key", "edit", "scroll", "select", "cancel"]
    element: str | None = None               # element id from the latest snapshot of that app
    text: str | None = None                  # set_value / type / edit: the text; key: a combo like "cmd+s";
                                             # select: the pop-up item's title
    select: Any = None                       # edit: (start, length) | "all" | "end" | None (at the caret)
    direction: str | None = None             # scroll: up / down / left / right
    amount: int | None = None                # scroll: pages (keys) or steps (slider / scroll bar)
    window: str | None = None                # scroll with no element: the window (title or id) to scroll
    allow_foreground: bool = False           # task-authorized permission, not a fallback instruction
    mechanism: Literal['ax', 'pid', 'ax_keyboard', 'ax_value', 'foreground'] | None = None
