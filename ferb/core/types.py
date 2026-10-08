"""Owns shared task, action and observation data types. Must not infer intent, choose a target or claim semantic success from transport facts.

Core execution data (see ARCHITECTURE.md). Pure data: no I/O, imports nothing from ferb except core."""

from dataclasses import dataclass, field
from datetime import datetime
from enum import Enum
from typing import Any, Literal


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


@dataclass(frozen=True)
class ActionSpec:
    """Capability.describe(args): what the call does, recorded for tracing and result inspection."""
    kind: str
    target: Target = Target()
    content: str | None = None


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
class ModelRequest:
    """What FERB sends any model runtime (D32): its instructions and the context in order, stable parts first (P20)."""
    system: str
    parts: tuple[str, ...]
    cacheable: int = 0                       # parts[:cacheable] repeat across this role's calls; adapters cache them

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
    kind: Literal["message", "tool_call", "usage", "final", "error", "violation"]
    data: dict[str, Any]


# ---------- tools ----------

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


@dataclass(frozen=True)
class ToolResult:
    content: dict[str, Any]
    is_error: bool = False
    read_sources: tuple[tuple[str, str], ...] = ()   # (source kind, id) annotations for the data read by this call
    image: bytes | None = None                        # JPEG for the model only: never redacted text, logged or stored
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
    in_view: bool = True                     # on screen now (rect meets the viewport and nothing covers its centre)
    offscreen: str | None = None             # "above" | "below" | "left" | "right" when not on screen
    occluded: bool = False                   # on screen but covered by something else
    container: str | None = None             # id of its scroll container (see PageSnapshot.scrollers), None = window
    state: tuple[str, ...] = ()              # "disabled", "expanded", "collapsed", "selected"
    level: int | None = None                 # headings


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
