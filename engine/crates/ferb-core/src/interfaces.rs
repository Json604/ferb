//! Typed boundaries shared by drivers, runtimes and services (ferb/core/interfaces.py). The contract only: the
//! adapters, drivers and stores behind these traits are later ports.
//!
//! Async methods return a boxed future so each trait can be used as `dyn Trait`. A Python capability's pydantic
//! `Input` model becomes `input_schema()` (its JSON schema) plus arguments as JSON, checked by the gateway.

use std::collections::BTreeSet;
use std::future::Future;
use std::pin::Pin;

use serde_json::Value;
use tokio::sync::mpsc;

use crate::errors::FerbError;
use crate::types::{
    ActionResult, ActionSpec, BrowserAction, ComputerAction, Dict, DriverFeatures, ElementRef,
    Identity, ModelRequest, PageSnapshot, RiskLevel, RunHandle, RuntimeEvent, SecretRef,
    SecretSpec, TabRef, Task, ToolContext, ToolResult, WaitCondition,
};
use crate::when::DateTime;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// interfaces.py:15.
pub trait Clock: Send + Sync {
    /// Timezone-aware wall time: timestamps, schedules.
    fn now(&self) -> DateTime;
    /// Seconds for elapsed time and timeouts; unaffected by clock changes.
    fn monotonic(&self) -> f64;
    fn sleep_until(&self, t: DateTime) -> BoxFuture<'_, ()>;
}

/// interfaces.py:21: injected id generation, so any run can be replayed with deterministic ids.
pub trait IdSource: Send + Sync {
    fn new_id(&self) -> String;
}

/// interfaces.py:26: one model call for one role; everything model-specific lives in its adapter.
pub trait AgentRuntime: Send + Sync {
    fn name(&self) -> &str;
    fn start<'a>(
        &'a self,
        task: &'a Task,
        request: &'a ModelRequest,
        run_token: &'a str,
    ) -> BoxFuture<'a, Result<RunHandle, FerbError>>;
    /// The run's events, in order, until the channel closes (Python: an async iterator).
    fn events(&self, run: &RunHandle) -> mpsc::Receiver<RuntimeEvent>;
    /// Same-run input accepted, or `Unsupported` if closed. (Python: `continue_`.)
    fn continue_run<'a>(
        &'a self,
        run: &'a RunHandle,
        message: &'a str,
    ) -> BoxFuture<'a, Result<(), FerbError>>;
    fn cancel<'a>(&'a self, run: &'a RunHandle) -> BoxFuture<'a, ()>;
}

/// interfaces.py:36.
pub trait Capability: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn scopes(&self) -> BTreeSet<String>;
    /// Which identities may call it.
    fn identities(&self) -> Vec<Identity>;
    fn risk(&self) -> RiskLevel;
    fn reversible(&self) -> bool;
    /// The JSON schema of its arguments (Python: the `Input` model).
    fn input_schema(&self) -> Value;
    fn run<'a>(
        &'a self,
        args: &'a Value,
        ctx: &'a ToolContext,
    ) -> BoxFuture<'a, Result<ToolResult, FerbError>>;
    /// Kind, target, content, data labels for policy.
    fn describe(&self, args: &Value) -> ActionSpec;
}

/// interfaces.py:48: FERB identity only. No reveal method for callers: filling happens in the extension of the
/// FERB profile, which re-checks the frame's origin in the same call that sets the value, and forgets the value.
pub trait SecretStore: Send + Sync {
    fn has(&self, secret: &SecretRef) -> bool;
    fn spec(&self, secret: &SecretRef) -> Result<SecretSpec, FerbError>;
}

/// interfaces.py:56.
pub trait BrowserDriver: Send + Sync {
    /// "ext:chrome" | "ext:brave" | "ax:<browser>"
    fn name(&self) -> &str;
    fn identity(&self) -> Identity;
    fn features(&self) -> DriverFeatures;
    fn tabs(&self) -> BoxFuture<'_, Result<Vec<TabRef>, FerbError>>;
    fn open<'a>(
        &'a self,
        url: &'a str,
        background: bool,
    ) -> BoxFuture<'a, Result<TabRef, FerbError>>;
    /// Structured, sanitized, has a generation.
    fn snapshot<'a>(&'a self, tab: &'a TabRef) -> BoxFuture<'a, Result<PageSnapshot, FerbError>>;
    fn act<'a>(
        &'a self,
        el: &'a ElementRef,
        action: &'a BrowserAction,
        intent_id: &'a str,
    ) -> BoxFuture<'a, Result<ActionResult, FerbError>>;
    fn wait<'a>(
        &'a self,
        tab: &'a TabRef,
        until: &'a WaitCondition,
        timeout: f64,
    ) -> BoxFuture<'a, Result<bool, FerbError>>;
    fn screenshot<'a>(&'a self, tab: &'a TabRef) -> BoxFuture<'a, Result<Vec<u8>, FerbError>>;
    fn close<'a>(
        &'a self,
        tab: &'a TabRef,
        leave_page: Option<&'a str>,
    ) -> BoxFuture<'a, Result<Option<Dict>, FerbError>>;
}

/// interfaces.py:69: OS-neutral.
pub trait ComputerDriver: Send + Sync {
    fn device_id(&self) -> &str;
    /// "macos" | "windows" | "linux"
    fn os(&self) -> &str;
    fn act<'a>(
        &'a self,
        app: &'a str,
        action: &'a ComputerAction,
    ) -> BoxFuture<'a, Result<ActionResult, FerbError>>;
}
