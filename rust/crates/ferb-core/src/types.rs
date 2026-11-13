//! Shared task, action and observation data (ferb/core/types.py). Pure data: no I/O.
//!
//! Each serde type writes the JSON of Python's `dataclasses.asdict` form, fields in the dataclass's order, `None` as
//! null, tuples as lists, enums by value, datetimes as `isoformat()` (ferb/store/spans.py:521-525 `node_json`).
//! `MemoryItem` is the exception: its one whole-item form goes to a worker with `default=str`
//! (ferb/orchestrator/evidence.py:360,501), so its datetimes have a space. Types holding picture bytes, a secret or a
//! callback (`ModelImage`, `ModelRequest`, `RunHandle`, `EscapeGrant`, `ToolContext`, `ToolResult`, `BrowserAction`)
//! have no JSON form in Python either and get none here.

use std::collections::BTreeSet;
use std::fmt;
use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::when::{self, DateTime};

/// A JSON object in insertion order, as a Python dict (`serde_json` is built with `preserve_order`).
pub type Dict = serde_json::Map<String, Value>;

macro_rules! words {
    ($(#[$meta:meta])* $name:ident { $($variant:ident = $text:literal),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        pub enum $name {
            $(#[serde(rename = $text)] $variant),+
        }

        impl $name {
            pub const ALL: &'static [$name] = &[$($name::$variant),+];

            pub fn as_str(self) -> &'static str {
                match self { $($name::$variant => $text),+ }
            }
        }
    };
}

words!(
    /// types.py:13: whose profile and credentials a task uses.
    Identity { Ferb = "ferb", Owner = "owner" }
);

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecretRef {
    pub key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecretSpec {
    #[serde(rename = "ref")]
    pub secret: SecretRef,
    pub allowed_origins: Vec<String>,
    pub account_hint: String,
}

/// types.py:30: written as its number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(into = "u8", try_from = "u8")]
pub enum RiskLevel {
    Low = 1,
    Medium = 2,
    High = 3,
}

impl From<RiskLevel> for u8 {
    fn from(r: RiskLevel) -> u8 {
        r as u8
    }
}

impl TryFrom<u8> for RiskLevel {
    type Error = String;

    fn try_from(n: u8) -> Result<Self, String> {
        match n {
            1 => Ok(RiskLevel::Low),
            2 => Ok(RiskLevel::Medium),
            3 => Ok(RiskLevel::High),
            _ => Err(format!("{n} is not a valid RiskLevel")),
        }
    }
}

words!(
    /// Task.role: the execution purpose, not a tool permission class.
    TaskRole { Worker = "worker", Verifier = "verifier", Mixed = "mixed" }
);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Task {
    pub id: String,
    pub parent_id: Option<String>,
    pub goal: String,
    pub device_id: String,
    pub identity: Identity,
    /// A Python set: written here as a sorted list (Python never writes a Task as JSON).
    pub scopes: BTreeSet<String>,
    pub role: TaskRole,
    pub request_id: String,
    pub success_conditions: Vec<String>,
    #[serde(with = "when::iso_opt")]
    pub deadline: Option<DateTime>,
}

words!(OutcomeStatus {
    Done = "done",
    Failed = "failed",
    Unknown = "unknown",
    Cancelled = "cancelled",
    NeedsUser = "needs_user",
    Blocked = "blocked",
});

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Outcome {
    pub task_id: String,
    pub status: OutcomeStatus,
    pub summary: String,
    pub artifacts: Vec<String>,
    pub evidence: Vec<String>,
}

words!(IntentState {
    Intended = "intended",
    Executing = "executing",
    Done = "done",
    Failed = "failed",
    Unknown = "unknown",
});

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionIntent {
    pub id: String,
    pub task_id: String,
    pub capability: String,
    pub target: Dict,
    pub payload_hash: String,
    pub state: IntentState,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Span {
    pub id: String,
    pub parent_id: Option<String>,
    pub kind: String,
    /// None for daemon-level spans.
    pub task_id: Option<String>,
    pub device_id: Option<String>,
    #[serde(with = "when::iso")]
    pub started_at: DateTime,
    #[serde(with = "when::iso_opt")]
    pub ended_at: Option<DateTime>,
    pub input: Dict,
    pub output: Option<Dict>,
    pub cost_usd: Option<f64>,
    pub error: Option<String>,
    /// A tool call's result origins, comma-joined (`crate::origin`).
    pub origin: Option<String>,
    /// A tool call's one-line result summary.
    pub summary: Option<String>,
}

words!(Actor { Owner = "owner", Ferb = "ferb", Unknown = "unknown" });

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrailEvent {
    pub id: String,
    #[serde(with = "when::iso")]
    pub at: DateTime,
    pub device_id: String,
    pub actor: Actor,
    pub task_id: Option<String>,
    pub source: String,
    pub data: Dict,
}

words!(MemoryKind {
    Fact = "fact",
    Preference = "preference",
    Person = "person",
    Project = "project",
    Env = "env",
});
words!(MemoryOrigin {
    OwnerStated = "owner_stated",
    Observed = "observed",
    Inferred = "inferred",
    ThirdParty = "third_party",
});
words!(MemoryStatus { Proposed = "proposed", Active = "active", Retracted = "retracted" });

/// types.py:98. Datetimes are written as `str(datetime)` (see the module note).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryItem {
    pub id: String,
    pub kind: MemoryKind,
    pub subject: String,
    /// What it is about ("dentist"): versions of one fact share it.
    pub key: String,
    pub content: String,
    pub source_ids: Vec<String>,
    #[serde(with = "when::spaced")]
    pub observed_at: DateTime,
    #[serde(with = "when::spaced")]
    pub valid_from: DateTime,
    #[serde(with = "when::spaced_opt")]
    pub valid_to: Option<DateTime>,
    pub supersedes: Option<String>,
    pub confidence: f64,
    pub origin: MemoryOrigin,
    pub status: MemoryStatus,
    /// MEM-10: what the owner's words are about.
    pub about: String,
    #[serde(with = "when::spaced_opt")]
    pub review_at: Option<DateTime>,
    /// MEM-14: the owner's words FERB stopped using as over (kept).
    pub set_aside: bool,
}

// ---------- actions (descriptive execution metadata) ----------

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub account: Option<String>,
    pub site: Option<String>,
    pub recipients: Vec<String>,
    pub path: Option<String>,
    pub amount: Option<f64>,
    /// A payment's currency as the page shows it (display only).
    pub currency: Option<String>,
    pub contact: Option<String>,
    pub page: Option<String>,
    pub item: Option<String>,
}

words!(
    /// The kinds of input field a capability can report, from the field's own markup or accessibility role, never
    /// from its label's words (types.py:128-130).
    FieldKind {
        Password = "password",
        NewPassword = "new_password",
        OneTimeCode = "one_time_code",
        CardNumber = "card_number",
        CardCsc = "card_csc",
        CardExpiry = "card_expiry",
    }
);

/// What received an input, as the driver found it when it delivered the input.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReceivedBy {
    pub bundle_id: Option<String>,
    pub window: Option<String>,
    pub role: Option<String>,
}

/// Structural facts a capability reports about its call (GATE-202): read by code, never the worker's words and
/// never a judgment of intent. Every field is optional; None or empty means not known.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionFacts {
    pub submits_form: Option<bool>,
    pub form_action_host: Option<String>,
    pub form_method: Option<String>,
    pub field_kinds: Vec<FieldKind>,
    pub page_amount: Option<String>,
    pub page_merchant: Option<String>,
    /// Every money amount the page shows (GATE-578): only compared with an amount the worker declared.
    pub page_amounts: Vec<String>,
    pub received_by: Option<ReceivedBy>,
    pub secure_field: Option<bool>,
    pub terminal_target: Option<bool>,
    pub target_role: Option<String>,
    pub target_subrole: Option<String>,
    pub page_url: Option<String>,
    pub frame_host: Option<String>,
    pub tab_chain: Option<String>,
    pub deletes: Vec<String>,
    pub writes: Vec<String>,
    /// The page's own label for the control (display only: GATE-557, GATE-559; it never decides anything).
    pub control_label: Option<String>,
}

impl ActionFacts {
    /// types.py:176-186: the known facts only, for a call's record. None and empty lists are left out (a known
    /// `false` stays); `received_by` keeps only its known parts.
    pub fn recorded(&self) -> Dict {
        let Ok(Value::Object(all)) = serde_json::to_value(self) else {
            unreachable!("ActionFacts is a JSON object")
        };
        let mut out = Dict::new();
        for (name, value) in all {
            match value {
                Value::Null => {}
                Value::Array(ref items) if items.is_empty() => {}
                Value::Object(parts) => {
                    out.insert(
                        name,
                        Value::Object(parts.into_iter().filter(|(_, v)| !v.is_null()).collect()),
                    );
                }
                value => {
                    out.insert(name, value);
                }
            }
        }
        out
    }
}

/// `Capability.describe(args)`: what the call does (types.py:189-205). Every declared kind is kept and can only
/// add a confirmation, never remove one.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionSpec {
    pub kind: String,
    pub target: Target,
    pub content: Option<String>,
    pub declared: Option<String>,
    pub place: Option<String>,
    pub effect_kind: Option<String>,
    pub facts: Option<ActionFacts>,
}

impl ActionSpec {
    pub fn new(kind: &str) -> Self {
        ActionSpec {
            kind: kind.to_string(),
            ..Default::default()
        }
    }
}

// ---------- runtimes ----------

/// A picture handed to a model with its request (types.py:210): bytes already redacted, sent to the model only,
/// never into a span, the journal or a log. `Debug` leaves the bytes out, as Python's repr does.
#[derive(Clone, PartialEq, Eq)]
pub struct ModelImage {
    pub data: Vec<u8>,
    /// Its evidence-store row, which the trace records instead.
    pub reference: String,
    pub mime: String,
}

impl ModelImage {
    pub fn new(data: Vec<u8>, reference: &str) -> Self {
        ModelImage {
            data,
            reference: reference.to_string(),
            mime: "image/jpeg".to_string(),
        }
    }

    pub fn base64(&self) -> String {
        const ALPHABET: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::with_capacity(self.data.len().div_ceil(3) * 4);
        for chunk in self.data.chunks(3) {
            let n =
                chunk.iter().fold(0u32, |n, b| n << 8 | u32::from(*b)) << (8 * (3 - chunk.len()));
            for i in 0..4 {
                if i <= chunk.len() {
                    out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
                } else {
                    out.push('=');
                }
            }
        }
        out
    }

    pub fn data_url(&self) -> String {
        format!("data:{};base64,{}", self.mime, self.base64())
    }
}

impl fmt::Debug for ModelImage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ModelImage")
            .field("ref", &self.reference)
            .field("mime", &self.mime)
            .finish()
    }
}

/// A model's documented limits (types.py:225): provider facts, never picked or guessed by FERB.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelLimits {
    pub model: String,
    pub context_window: i64,
    pub max_output: i64,
    pub source: String,
    /// False: the provider documents an input limit, with the reply counted apart.
    pub window_includes_output: bool,
    pub max_input_chars: Option<i64>,
}

impl ModelLimits {
    /// Room for FERB's prompt: the window minus room for the reply (an input limit is already that room).
    pub fn context_budget(&self) -> i64 {
        if self.window_includes_output {
            self.context_window - self.max_output
        } else {
            self.context_window
        }
    }
}

/// What FERB sends any model runtime (types.py:243): instructions and the context in order, stable parts first.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModelRequest {
    pub system: String,
    pub parts: Vec<String>,
    /// `parts[..cacheable]` repeat across this role's calls; adapters cache them.
    pub cacheable: usize,
    pub images: Vec<ModelImage>,
}

impl ModelRequest {
    pub fn context(&self) -> String {
        self.parts.join("\n\n")
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RunHandle {
    pub id: String,
    pub runtime: String,
    pub task_id: String,
    /// Runtime-private (process, session, ...).
    pub state: Dict,
}

words!(
    /// RuntimeEvent.kind (types.py:266-276).
    RuntimeEventKind {
        Message = "message",
        TextDelta = "text_delta",
        ToolCall = "tool_call",
        Usage = "usage",
        Final = "final",
        Error = "error",
        Violation = "violation",
        InvalidAnswer = "invalid_answer",
        Retrying = "retrying",
        Progress = "progress",
        Compacted = "compacted",
    }
);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeEvent {
    pub kind: RuntimeEventKind,
    pub data: Dict,
}

// ---------- tools ----------

words!(
    /// What a call does to a file it names: the protection lists check each one.
    FileAccess { Read = "read", Write = "write", Remove = "remove", Metadata = "metadata" }
);

/// types.py:285. `Debug` leaves the secret out, as Python's repr does.
#[derive(Clone, PartialEq, Eq)]
pub struct EscapeGrant {
    pub rule: String,
    pub confirmation_id: String,
    pub command: String,
    pub cwd: String,
    pub secret: Option<String>,
}

impl fmt::Debug for EscapeGrant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EscapeGrant")
            .field("rule", &self.rule)
            .field("confirmation_id", &self.confirmation_id)
            .field("command", &self.command)
            .field("cwd", &self.cwd)
            .finish()
    }
}

/// Input about to reach a terminal (types.py:294; GATE-585), as code found it. Facts only.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TerminalInput {
    pub app: String,
    pub bundle_id: String,
    pub surface: String,
    pub text: Option<String>,
    pub keys: Option<String>,
    pub paste: bool,
    pub concealed: bool,
    pub clipboard: Option<i64>,
    pub enter: bool,
    pub covered_by: Option<String>,
    pub window: String,
    pub menu: Option<String>,
    /// A hidden secret filled in: its name, never its value.
    pub secret: Option<String>,
    pub script: Option<String>,
    pub drag: bool,
    pub runs: bool,
    pub control: Option<String>,
    pub selection: Option<String>,
    pub file: Option<String>,
    pub runs_where: String,
    /// Files the input would open that can run when opened: (path, sha256 of its contents, its text as shown).
    pub files: Vec<(String, String, String)>,
    pub label: String,
    pub opens: bool,
    pub named: Vec<String>,
}

/// The owner's Yes covering one TerminalInput for this call (types.py:330).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TerminalGrant {
    pub input: TerminalInput,
    pub confirmation_id: String,
}

/// The protection lists' check of a file as the tool really opened it and what it does to it: the error to return
/// when the lists close it, else None (types.py:355-357).
#[derive(Clone)]
pub struct PathCheck(pub Arc<PathCheckFn>);

/// (the file as opened, what the call does to it) -> the refusal, if the lists close it.
pub type PathCheckFn = dyn Fn(&Path, &str) -> Option<String> + Send + Sync;

impl fmt::Debug for PathCheck {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PathCheck")
    }
}

#[derive(Debug, Clone)]
pub struct ToolContext {
    pub task_id: String,
    pub worker_id: String,
    pub request_id: String,
    pub identity: Identity,
    pub scopes: BTreeSet<String>,
    pub device_id: String,
    pub span_id: Option<String>,
    pub workspace: String,
    pub owner_roots: Vec<String>,
    pub untrusted_roots: Vec<String>,
    /// Ledger row of this call when consequential.
    pub intent_id: Option<String>,
    /// The final checker's call: its tools keep it to observing.
    pub observe_only: bool,
    pub probe: bool,
    /// None when the daemon has no lists.
    pub path_closed: Option<PathCheck>,
    pub escape: Option<EscapeGrant>,
    pub terminal: Option<TerminalGrant>,
    /// The task's folders (HAND-204): the only visible folders FERB's shell may change.
    pub folders: Vec<String>,
}

/// types.py:366. `Debug` leaves the image bytes out (never logged).
#[derive(Clone, Default, PartialEq)]
pub struct ToolResult {
    pub content: Dict,
    pub is_error: bool,
    /// (source kind, id) annotations for the data read by this call.
    pub read_sources: Vec<(String, String)>,
    pub image: Option<Vec<u8>>,
    pub evidence: Option<Dict>,
    pub facts: Option<ActionFacts>,
    /// How long the call's own steps took (ms): its span only, never the model (HAND-436).
    pub timing: Option<Dict>,
}

impl fmt::Debug for ToolResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ToolResult")
            .field("content", &self.content)
            .field("is_error", &self.is_error)
            .field("read_sources", &self.read_sources)
            .field(
                "image",
                &self.image.as_ref().map(|i| format!("<{} bytes>", i.len())),
            )
            .field("evidence", &self.evidence)
            .field("facts", &self.facts)
            .field("timing", &self.timing)
            .finish()
    }
}

// ---------- browsers ----------

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TabRef {
    pub device_id: String,
    pub browser: String,
    pub profile: String,
    /// Connection generation: a reconnect makes older refs stale.
    pub generation: i64,
    pub tab_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ElementRef {
    pub tab: TabRef,
    pub frame_id: String,
    pub snapshot_gen: i64,
    pub element_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DriverFeatures {
    pub trusted_input_background: bool,
    pub parallel_actions: bool,
    pub network_events: bool,
    pub dom_snapshot: bool,
    pub screenshot_background: bool,
}

words!(BrowserActionKind {
    Click = "click",
    Dblclick = "dblclick",
    Rightclick = "rightclick",
    Hover = "hover",
    Type = "type",
    Select = "select",
    Press = "press",
    Scroll = "scroll",
    Upload = "upload",
    Drag = "drag",
    Paste = "paste",
});
words!(DialogAnswer { Accept = "accept", Dismiss = "dismiss" });
words!(TypeMode { Replace = "replace", Insert = "insert" });
words!(Modifier { Shift = "shift", Cmd = "cmd", Ctrl = "ctrl", Alt = "alt" });

/// types.py:409. In Python `files_closed` and `terminal` take no part in equality; here the type is not compared.
#[derive(Debug, Clone)]
pub struct BrowserAction {
    pub kind: BrowserActionKind,
    pub text: Option<String>,
    pub trusted: bool,
    /// Scripted (untrusted) page events instead of real input.
    pub page_mode: bool,
    pub expect_dialog: Option<DialogAnswer>,
    pub dialog_text: Option<String>,
    pub files: Vec<String>,
    pub end_element: Option<String>,
    pub files_closed: Option<PathCheck>,
    pub terminal: Option<TerminalGrant>,
    /// type (HAND-505): None is by the field's structure.
    pub mode: Option<TypeMode>,
    /// paste (HAND-506): the same content as text/html.
    pub html: Option<String>,
    /// click (HAND-514): keys held with it.
    pub modifiers: Vec<Modifier>,
}

impl BrowserAction {
    pub fn new(kind: BrowserActionKind) -> Self {
        BrowserAction {
            kind,
            text: None,
            trusted: false,
            page_mode: false,
            expect_dialog: None,
            dialog_text: None,
            files: Vec::new(),
            end_element: None,
            files_closed: None,
            terminal: None,
            mode: None,
            html: None,
            modifiers: Vec::new(),
        }
    }
}

words!(WaitKind { Load = "load", Text = "text", Url = "url" });

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WaitCondition {
    pub kind: WaitKind,
    pub value: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionResult {
    pub ok: bool,
    pub detail: String,
    pub facts: Dict,
    /// Structural facts code read around the action (GATE-202).
    pub observed: Option<ActionFacts>,
}

/// What submitting from a page control would do, from the form's own markup (the extension, protocol 18).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FormSubmit {
    pub on_click: bool,
    pub on_enter: bool,
    pub action_host: Option<String>,
    pub method: Option<String>,
    pub field_kinds: Vec<String>,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PageElement {
    pub id: String,
    pub frame_id: String,
    pub role: String,
    pub label: String,
    pub value: Option<String>,
    /// x, y, width, height (viewport of its frame).
    pub rect: [f64; 4],
    pub visible: bool,
    pub input_type: Option<String>,
    pub href: Option<String>,
    pub form: Option<String>,
    /// Password / OTP field: value never leaves the driver.
    pub secret: bool,
    pub code_fill: bool,
    pub in_view: bool,
    pub offscreen: Option<String>,
    pub occluded: bool,
    pub container: Option<String>,
    pub state: Vec<String>,
    pub level: Option<i64>,
    pub terminal: bool,
    pub field_kind: Option<String>,
    pub submit: Option<FormSubmit>,
    pub path: String,
    pub backend_node: Option<i64>,
    pub in_dialog: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PageSnapshot {
    pub tab: TabRef,
    pub generation: i64,
    pub url: String,
    pub title: String,
    pub elements: Vec<PageElement>,
    pub text: String,
    pub truncated: bool,
    /// JS dialogs dismissed since the last snapshot.
    pub dialogs: Vec<String>,
    pub scroll: Option<Dict>,
    pub scrollers: Vec<Dict>,
    pub media: Vec<Dict>,
    pub frame_urls: Vec<String>,
    pub loading: bool,
    pub content_type: Option<String>,
}

// ---------- Mac / computer ----------

words!(ComputerActionKind {
    Press = "press",
    SetValue = "set_value",
    Focus = "focus",
    Click = "click",
    Type = "type",
    Key = "key",
    Edit = "edit",
    Scroll = "scroll",
    Select = "select",
    Cancel = "cancel",
});
words!(Mechanism { Ax = "ax", Pid = "pid", AxValue = "ax_value", Foreground = "foreground" });

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComputerAction {
    pub kind: ComputerActionKind,
    pub element: Option<String>,
    pub text: Option<String>,
    /// edit: [start, length] | "all" | "end" | null (at the caret).
    pub select: Value,
    pub direction: Option<String>,
    pub amount: Option<i64>,
    pub window: Option<String>,
    /// Task-authorized permission, not a fallback instruction.
    pub allow_foreground: bool,
    pub mechanism: Option<Mechanism>,
}
