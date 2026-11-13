//! One `Tool` trait for every capability the gateway runs (the Python `Capability` protocol and the attributes the
//! gateway reads with `getattr`/`hasattr`), and the registry (`ferb/registry.py`).
//!
//! The tools themselves are other crates (ferb-fs, ferb-shell, ferb-browser, ferb-clock, ferb-memory, Mac hands); each
//! registers through this trait. A tool in the frozen catalog takes its schema, description, scopes, identities and
//! facts from it by default, so a Rust tool can't serve a schema the models were never given.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;

use ferb_core::interfaces::BoxFuture;
use ferb_core::types::{ActionSpec, TerminalInput, ToolContext, ToolResult};
use serde_json::{Map, Value};

use crate::catalog::{self, Live};
use crate::pydantic::Hooks;
use crate::validators::ToolHooks;

/// Validated arguments, as `model_dump(mode="json")` gives them (field order, defaults filled in).
pub type Args = Map<String, Value>;

/// The facts the gateway reads off a capability (`tools.json` `facts`). Absent: Python's `getattr` default.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Facts {
    pub risk: u8,
    pub reversible: bool,
    /// Only observes (V3, W4): the final checker may use it.
    pub observation: bool,
    /// A fixed origin label for what it returns (S3).
    pub origin: Option<String>,
    /// A click, typing, an app's control: its declared effect and destination are checked (W2).
    pub generic_input: bool,
    /// May change what is on the owner's screen (DMN-5).
    pub on_screen: bool,
    /// May be a loop's probe (L31).
    pub probe_safe: bool,
    pub describe_in_thread: bool,
    /// Only ever creates a new file of its own: no recovery copy (S1).
    pub creates_new_only: bool,
    /// The ledger key sibling tools share (P19), else its name.
    pub ledger_key: Option<String>,
    /// How a call is delivered, left out of P19's hash.
    pub method_args: Vec<String>,
    /// Argument keys a normalized Target field already binds.
    pub target_args: Vec<String>,
    /// Target fields filled from the agent's notes, left out of P19's hash.
    pub noted_targets: Vec<String>,
}

fn list(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

impl Facts {
    /// What the frozen catalog says of a tool.
    pub fn from_catalog(name: &str) -> Option<Facts> {
        let e = catalog::catalog().entry(name)?;
        let f = &e.facts;
        let flag = |k: &str| f.get(k).and_then(Value::as_bool).unwrap_or(false);
        Some(Facts {
            risk: f.get("risk").and_then(Value::as_u64).unwrap_or(1) as u8,
            reversible: f.get("reversible").and_then(Value::as_bool).unwrap_or(true),
            observation: flag("observation"),
            origin: f.get("origin").and_then(Value::as_str).map(str::to_string),
            generic_input: flag("generic_input"),
            on_screen: flag("on_screen"),
            probe_safe: flag("probe_safe"),
            describe_in_thread: flag("describe_in_thread"),
            creates_new_only: flag("creates_new_only"),
            ledger_key: f
                .get("ledger_key")
                .and_then(Value::as_str)
                .map(str::to_string),
            method_args: list(f.get("method_args")),
            target_args: list(f.get("target_args")),
            noted_targets: list(f.get("noted_targets")),
        })
    }
}

/// What a tool's run raised (Python: an exception out of `cap.run`).
#[derive(Debug, Clone, PartialEq)]
pub struct ToolError {
    /// Its type's name (`type(e).__name__`), as the result's error says it.
    pub class: String,
    /// `str(e)`.
    pub message: String,
    /// It failed before it acted (`before_acting`: a refusal, invalid arguments, `attempted = False`): failed, not
    /// unknown (P19).
    pub before_acting: bool,
}

impl ToolError {
    /// A safety check's refusal (`Refused`): nothing was attempted.
    pub fn refused(message: impl Into<String>) -> ToolError {
        ToolError {
            class: "Refused".into(),
            message: message.into(),
            before_acting: true,
        }
    }

    /// Any other failure: the action may have started, its outcome is unknown.
    pub fn failed(class: impl Into<String>, message: impl Into<String>) -> ToolError {
        ToolError {
            class: class.into(),
            message: message.into(),
            before_acting: false,
        }
    }
}

/// A tool's result, and which of its values are its own measurements.
#[derive(Debug, Clone, Default)]
pub struct ToolOutput {
    pub result: ToolResult,
    /// JSON pointers (RFC 6901, into `result.content`) of numbers the tool's code itself produced (a file's size,
    /// its `mtime_ns`, an inode): no outside text, so the result's redaction never reads them as a card number. Only
    /// a JSON number at such a place is kept as it is; anything else there is redacted like the rest.
    pub measured: Vec<String>,
}

impl From<ToolResult> for ToolOutput {
    fn from(result: ToolResult) -> ToolOutput {
        ToolOutput {
            result,
            measured: Vec::new(),
        }
    }
}

/// The live preview's surface one call works on (`ferb/act/point.py` `PreviewTarget`): its scope (the same scope is
/// the same surface) and a short label; how it is captured is the capability's.
#[derive(Clone)]
pub struct PreviewTarget {
    pub scope: String,
    pub label: String,
    pub capture: Option<std::sync::Arc<dyn std::any::Any + Send + Sync>>,
}

impl std::fmt::Debug for PreviewTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreviewTarget")
            .field("scope", &self.scope)
            .field("label", &self.label)
            .finish_non_exhaustive()
    }
}

/// `Point` (L33): the element an action is about to touch, in global screen points, on a display.
#[derive(Debug, Clone, PartialEq)]
pub struct Point {
    /// click, type, scroll, press or drag.
    pub kind: String,
    pub rect: [f64; 4],
    pub display: i64,
}

/// Why a picture couldn't be taken (`NoTarget`: no window yet; `Withheld`: its secret fields couldn't be painted
/// over, FERB's words; anything else).
#[derive(Debug, Clone, PartialEq)]
pub enum Shot {
    NoTarget,
    Withheld(String),
    Failed,
}

/// What a tool's real file is (`real_path` hook): the path, or that it can't be told.
pub type RealPath = Result<PathBuf, ()>;

/// A capability (`ferb/core/interfaces.py` `Capability`, plus what the gateway reads off it).
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;

    /// The catalog's facts by default.
    fn facts(&self) -> Facts {
        Facts::from_catalog(self.name()).unwrap_or(Facts {
            risk: 1,
            reversible: true,
            ..Facts::default()
        })
    }

    fn scopes(&self) -> BTreeSet<String> {
        catalog::catalog()
            .entry(self.name())
            .map(|e| e.scopes.clone())
            .unwrap_or_default()
    }

    /// Which identities may call it ("owner", "ferb").
    fn identities(&self) -> BTreeSet<String> {
        catalog::catalog()
            .entry(self.name())
            .map(|e| e.identities.clone())
            .unwrap_or_default()
    }

    /// Its description as listed now (`live`: what a description reads when the listing is made).
    fn description(&self, live: &Live) -> String {
        catalog::description(self.name(), live).unwrap_or_default()
    }

    /// Its `Input` model's schema (the frozen one by default).
    fn input_schema(&self) -> Value {
        catalog::catalog()
            .entry(self.name())
            .map(|e| e.schema.clone())
            .unwrap_or(Value::Null)
    }

    /// Its model's own validators (by default the catalog tool's, `validators.rs`).
    fn hooks(&self) -> Option<Box<dyn Hooks + '_>> {
        Some(Box::new(ToolHooks(self.name())))
    }

    /// `describe(args)`: kind, target, content, facts.
    fn describe(&self, args: &Args) -> ActionSpec;

    /// `describe(args)` as Python may raise it: `Err` is what it raised. A call then ends with an internal error,
    /// as Python's exception leaves `Gateway.call`; a probe answers it as the tool's failure. By default `describe`
    /// never fails.
    fn try_describe(&self, args: &Args) -> Result<ActionSpec, ToolError> {
        Ok(self.describe(args))
    }

    /// `precheck(args)`: a refusal before anything else, else None.
    fn precheck(&self, _args: &Args) -> Option<String> {
        None
    }

    /// `unavailable(args)`: what macOS already refuses (GATE-560), as the worker's fact.
    fn unavailable<'a>(&'a self, _args: &'a Args) -> BoxFuture<'a, Option<Map<String, Value>>> {
        Box::pin(async { None })
    }

    /// `locks(args)`: the resources one call touches ("kind:name").
    fn locks(&self, _args: &Args) -> Vec<String> {
        Vec::new()
    }

    /// `file_paths(args)`: each file the call names and what it does to it (read / write / remove / metadata).
    /// `None`: it doesn't say (a tool with an `fs:` scope that doesn't is refused).
    fn file_paths(&self, _args: &Args) -> Option<Vec<(String, String)>> {
        None
    }

    /// `real_path(args, workspace)`: the file the call really acts on; `None`: no such hook.
    fn real_path(&self, _args: &Args, _workspace: &str) -> Option<RealPath> {
        None
    }

    /// `recovery_reason(args)`: why a write keeps a copy ("overwrite", "edit"), `Some(None)` when it can't change
    /// an existing file; `None`: it doesn't say.
    fn recovery_reason(&self, _args: &Args) -> Option<Option<String>> {
        None
    }

    /// `ledger_args(args, hashed)`: the stable form of arguments that name a target for one observation only.
    fn ledger_args(&self, _args: &Args, _hashed: &Args) -> Option<Args> {
        None
    }

    /// `terminal_input(args, task)`: what would go into a terminal (D31), from the driver's facts now.
    fn terminal_input<'a>(
        &'a self,
        _args: &'a Args,
        _task_id: &'a str,
    ) -> BoxFuture<'a, Option<TerminalInput>> {
        Box::pin(async { None })
    }

    /// `browser_page(args)`: a browser's own page the input would land on (rule 5).
    fn browser_page<'a>(&'a self, _args: &'a Args) -> BoxFuture<'a, Option<String>> {
        Box::pin(async { None })
    }

    /// GATE-579: one picture of the surface a payment call works on, for the owner's eyes only (`pay_picture.py`
    /// `capture`, taken under the call's locks by the gateway); None when it has none.
    fn pay_picture<'a>(
        &'a self,
        _args: &'a Args,
        _owner: &'a str,
    ) -> BoxFuture<'a, Option<Vec<u8>>> {
        Box::pin(async { None })
    }

    /// GATE-580: the tab's address and amounts read now (`pay_press.py` `read`); None when the call isn't on a
    /// browser tab the driver can read this way.
    fn pay_press<'a>(
        &'a self,
        _args: &'a Args,
        _owner: &'a str,
        _declared: Option<&'a str>,
    ) -> BoxFuture<'a, Option<ferb_policy::payments::Pressed>> {
        Box::pin(async { None })
    }

    /// `preview_target(args, owner, observed)` (kb area app): the live preview's surface for this call, as the
    /// capability declares it; `observed`: what a successful call's result shows. `Err`: the declaration failed
    /// (read as no surface).
    fn preview_target(
        &self,
        _args: &Args,
        _owner: &str,
        _observed: Option<&Map<String, Value>>,
    ) -> Result<Option<PreviewTarget>, String> {
        Ok(None)
    }

    /// `point(args, grant)` (L33): the element this action is about to touch, for FERB's cursor; `Ok(None)`: none
    /// (also a tool without the hook); `Err`: it failed (skipped).
    fn point<'a>(
        &'a self,
        _args: &'a Args,
        _grant: &'a ferb_rpc::auth::RunGrant,
    ) -> BoxFuture<'a, Result<Option<Point>, String>> {
        Box::pin(async { Ok(None) })
    }

    /// D6/D26: the task tab's picture now (`driver.screenshot(driver.owned_by(args.tab, task))`) with the page's
    /// masking note (`driver.masking(tab)`), for a browser snapshot's first picture.
    fn tab_picture<'a>(
        &'a self,
        _args: &'a Args,
        _task: &'a str,
    ) -> BoxFuture<'a, Result<(Vec<u8>, String), Shot>> {
        Box::pin(async { Err(Shot::Failed) })
    }

    /// `run(args, ctx)`.
    fn run<'a>(
        &'a self,
        args: &'a Args,
        ctx: &'a ToolContext,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>>;
}

/// The registry (`ferb/registry.py` `Registry`): tools by name, in registration order.
#[derive(Default, Clone)]
pub struct Registry {
    tools: Vec<Arc<dyn Tool>>,
}

impl Registry {
    pub fn new() -> Registry {
        Registry::default()
    }

    /// `add`: a name registered twice is refused.
    pub fn add(&mut self, tool: Arc<dyn Tool>) -> Result<(), String> {
        if self.get(tool.name()).is_some() {
            return Err(format!("capability {} registered twice", tool.name()));
        }
        self.tools.push(tool);
        Ok(())
    }

    /// Put a tool in, replacing one of the same name in place (tests swap a capability as Python's dict does).
    pub fn set(&mut self, tool: Arc<dyn Tool>) {
        match self.tools.iter().position(|t| t.name() == tool.name()) {
            Some(i) => self.tools[i] = tool,
            None => self.tools.push(tool),
        }
    }

    /// Unregister a tool (FERB's own browser detached, DMN-25).
    pub fn remove(&mut self, name: &str) {
        self.tools.retain(|t| t.name() != name);
    }

    pub fn get(&self, name: &str) -> Option<&Arc<dyn Tool>> {
        self.tools.iter().find(|t| t.name() == name)
    }

    pub fn tools(&self) -> &[Arc<dyn Tool>] {
        &self.tools
    }

    pub fn names(&self) -> Vec<String> {
        self.tools.iter().map(|t| t.name().to_string()).collect()
    }
}
