//! Building model runtimes per role (D32) from FERB's runtime table, and the runtime RPCs: Settings → AI's status,
//! FERB's own Claude sign-in, the owner's choice of runtime, model and effort (checked before it is saved), API keys
//! (GATE-543), the connection checks (T-1069) and the one-tap switch of a task stopped on a usage limit (ORCH-315).
//! Port of `ferb/daemon/areas/runtimes.py`, with `ferb/runtimes/keys.py` ([`keys`]), `check.py` ([`check`]),
//! `claude_login.py`'s sign-in ([`sign_in`]), `ferb/cli/runtimes.py`'s status and assign ([`cli`]) and the runtime
//! table's factories (what `_build_codex`, `_build_claude` and `_api` decide before an adapter is made). Never picks a
//! model the owner's config doesn't name (a runtime's own default aside).
//!
//! RPCs: runtimes.status, runtimes.sign_in, runtimes.use, runtimes.switch_task, runtimes.check, runtimes.models,
//! runtimes.model, keys.store, keys.delete, runtimes.check_calls. The rest of [`Runtimes`] is what the supervisor,
//! the worker and the daemon's start call (`make_runtime`, `runtime_available`, `runtime_account`, `worker_types`,
//! `api_key`, `prepare_codex`, `runtime_limits`, `runtime_status`, the Codex cache hooks).

pub mod check;
pub mod cli;
pub mod keys;
pub mod rows;
pub mod sign_in;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use ferb_core::errors::FerbError;
use ferb_core::types::{Identity, ModelImage, ModelRequest, RuntimeEventKind, Task, TaskRole};
use ferb_core::when::DateTime;
use ferb_identity::Secret;
use ferb_rpc::{BoxFuture, HandlerError, HandlerResult, Server};
use ferb_runtimes::catalog::{self, Config as _, SignInFacts, CATALOG, ROLES};
use ferb_runtimes::error::RuntimeError;
use ferb_runtimes::pyjson::{py_repr, repr_str};
use ferb_runtimes::run::{Adapter, RunControl};
use serde_json::{json, Map, Value};

use self::check::{CheckEcho, CheckPing, CheckTool, Line};
use self::cli::{text_or, AssignError, View};
use self::keys::{KeyCache, KeyFault, KeyHelper, Keys, Security};
use self::sign_in::ClaudeSignIn;
use super::core::{ok, refusal, Host, P};

// ---------- seams ----------

/// The supervisor's side of the one-tap switch (ORCH-315).
pub trait Supervisor: Send + Sync {
    /// `supervisor.runtime_offer(task_id)`: `{role, choices}` of a task waiting to switch, else None.
    fn runtime_offer(&self, task_id: &str) -> Option<Value>;
    /// `supervisor.answer_runtime_switch(task_id, runtime)`: the task resumes on `runtime`; returns `{role, ...}`.
    fn answer_runtime_switch(&self, task_id: &str, runtime: &str) -> Result<Value, HandlerError>;
}

/// One runtime to build for one call (catalog.py `Build`), after the table's own checks passed.
#[derive(Debug, Clone)]
pub struct Build {
    pub home: PathBuf,
    pub name: String,
    pub role: String,
    /// "supervisor" | "worker".
    pub mode: String,
    pub tool_names: Vec<String>,
    pub schema: Value,
    pub model: String,
    /// The effort this call sends (catalog.run_effort); None: the runtime's default.
    pub effort: Option<String>,
    /// The planner's chat turn (P20 cache, no overall limit, the API loop's answer tool).
    pub planner: bool,
    /// The call's one overall limit (catalog.call_limit).
    pub limit_s: Option<f64>,
    /// Codex: the pinned binary `_build_codex` resolved.
    pub codex_binary: Option<PathBuf>,
    /// Codex: "planner" when this turn resumes the cached planner thread (P20, RUN-401), else None.
    pub cache_role: Option<String>,
    /// Codex: the cache's scope (the device id).
    pub cache_scope: String,
}

/// A built adapter, and what Python reads off it besides (`getattr(rt, "retried", [])`).
pub struct Built {
    pub adapter: Arc<dyn Adapter>,
    /// The provider statuses the API loop retried after (RUN-439); empty for Codex and Claude.
    pub retried: Arc<dyn Fn() -> Vec<Value> + Send + Sync>,
}

/// Makes the adapter for a [`Build`]: Codex, Claude Code or the API loop with the daemon's MCP shim, redactor,
/// gateway bridge, keys, tool-list proxy and abandon/cache hooks (catalog.py RuntimeDeps), all model-specific
/// wiring kept there (rule 12).
pub trait Factory: Send + Sync {
    fn build(&self, b: &Build) -> Result<Built, RuntimeError>;
}

/// The programs FERB runs for its runtimes (the Mac's part).
pub trait Programs: Send + Sync {
    /// codex_exec.py `codex_binary(home, repo_root())`: the pinned Codex, or Unsupported.
    fn codex_binary(&self) -> Result<PathBuf, FerbError>;
    /// claude_login.py `cli_binary()`: the pinned Claude Code, or Unsupported.
    fn claude_cli(&self) -> Result<PathBuf, FerbError>;
    /// codex_exec.py `bundled_catalog(home)`: the catalog shipped with the pinned Codex, dumped once per version.
    fn bundled_catalog(&self);
    /// codex_exec.py `prepare(home, binary)`: the version checked and FERB's locked model catalog written.
    fn prepare_codex<'a>(&'a self, binary: &'a Path) -> BoxFuture<'a, Result<(), FerbError>>;
    /// codex_exec.py `check_version(binary)`.
    fn check_codex_version<'a>(&'a self, binary: &'a Path) -> BoxFuture<'a, Result<(), FerbError>>;
    /// claude_sdk.py `claude_pin_status(...)`: the pinned Claude Code's status line for doctor.
    fn claude_pin_status(&self) -> String;
    /// `os.environ`, the base of the Claude sign-in's environment.
    fn environ(&self) -> BTreeMap<String, String> {
        std::env::vars().collect()
    }
}

/// The gateway's capability registry, for the two tools that exist only while a check runs.
pub trait ToolRegistry: Send + Sync {
    fn add(&self, tool: Arc<dyn CheckTool>);
    fn remove(&self, name: &str);
}

/// What one check call holds while it runs (`_check_call`): its id, its span, a live task record and a run token
/// for its tools, and the gateway's per-holder call records.
pub trait CheckHost: Send + Sync {
    /// `self.ids.new_id()`.
    fn new_id(&self) -> String;
    /// `self.spans.start(kind, task_id=None, input=...)`: the span's id.
    fn span_start(&self, kind: &str, input: Value) -> String;
    /// `self.spans.finish(span, output=...)`.
    fn span_finish(&self, span_id: &str, output: Value);
    /// `self.tasks.hold(TaskRecord(task, started_at, span_id, task.id))`.
    fn hold_task(&self, task: &Task, started_at: &DateTime, span_id: &str);
    /// `self.tasks.remove(task_id)`.
    fn remove_task(&self, task_id: &str);
    /// `self.tokens.issue(task_id, holder, scopes, identity, ttl_s=, observe_only=, tools=, max_calls=)`.
    #[allow(clippy::too_many_arguments)]
    fn issue_token(
        &self,
        task_id: &str,
        holder: &str,
        scopes: &BTreeSet<String>,
        identity: Identity,
        ttl_s: f64,
        observe_only: bool,
        tools: &BTreeSet<String>,
        max_calls: u32,
    ) -> String;
    /// `self.tokens.revoke_worker(holder)`.
    fn revoke_token(&self, holder: &str);
    /// `self.gateway.forget_calls(holder)`.
    fn forget_calls(&self, holder: &str);
}

/// The journal's reads (ferb/memory/journal.py).
pub trait JournalReads: Send + Sync {
    /// `latest_kind(kinds, field, equals)`.
    fn latest_kind(&self, kinds: &[&str], field: &str, equals: &str) -> Option<String>;
    /// `of_kind(kind, field, equals)`: (at as isoformat, data), oldest first.
    fn of_kind(&self, kind: &str, field: &str, equals: &str) -> Vec<(String, Value)>;
}

/// A provider's model list (models.py `provider_models`), read with the stored key: the ids, or the provider's (or
/// transport's) words.
pub trait ModelLists: Send + Sync {
    fn provider_models<'a>(
        &'a self,
        provider: &'a str,
        key: &'a Secret,
    ) -> BoxFuture<'a, Result<Vec<String>, String>>;
}

/// The real list: the provider's models endpoint over HTTPS.
pub struct ProviderLists;

impl ModelLists for ProviderLists {
    fn provider_models<'a>(
        &'a self,
        provider: &'a str,
        key: &'a Secret,
    ) -> BoxFuture<'a, Result<Vec<String>, String>> {
        Box::pin(ferb_runtimes::models::provider_models(provider, key, None))
    }
}

/// The area's own state (the daemon's `_available_cache`, `_refusal_facts`, keys.py's memory, the sign-in), shared
/// by everything that holds these [`Deps`].
#[derive(Default)]
pub struct State {
    available: Mutex<HashMap<String, (String, bool)>>,
    refusals: Mutex<Option<HashMap<String, (String, String)>>>,
    keys: Arc<KeyCache>,
    pub sign_in: ClaudeSignIn,
}

impl State {
    /// Reads each runtime's standing refusal from the journal again at its next use (as after a restart).
    pub fn forget_refusals(&self) {
        *self.refusals.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

#[derive(Clone)]
pub struct Deps {
    pub supervisor: Arc<dyn Supervisor>,
    pub factory: Arc<dyn Factory>,
    pub programs: Arc<dyn Programs>,
    /// FERB's signed secret helper (keys: service "ferb", account `api.sealed/<provider>`): a
    /// [`ferb_identity::Helper`].
    pub helper: Arc<dyn KeyHelper>,
    /// `security`, only to find and move an older key item.
    pub security: Arc<dyn Security>,
    pub tools: Arc<dyn ToolRegistry>,
    pub checks: Arc<dyn CheckHost>,
    pub journal_reads: Arc<dyn JournalReads>,
    pub model_lists: Arc<dyn ModelLists>,
    pub state: Arc<State>,
}

/// The area over the daemon's shared state.
#[derive(Clone)]
pub struct Runtimes {
    pub host: Host,
    pub deps: Deps,
}

/// The planner's lookups (supervisor.py PLANNER_TOOLS, sorted) and scopes (PLANNER_SCOPES).
pub const PLANNER_TOOLS: [&str; 6] = [
    "about.read",
    "chat.history",
    "memory.read",
    "task.evidence",
    "trail.days",
    "trail.query",
];
pub const PLANNER_SCOPES: [&str; 2] = ["task:read", "trail:read"];
/// The journal kinds that hold a runtime's last refusal, and the one that ends it (APP-882).
pub const REFUSAL_KINDS: [&str; 3] = [
    "runtime.signed_out",
    "runtime.usage_limit",
    "runtime.answered",
];

fn supervisor_schema() -> &'static Value {
    static S: OnceLock<Value> = OnceLock::new();
    S.get_or_init(ferb_context::schemas::supervisor_schema)
}

fn fault(class: &str, message: impl Into<String>) -> HandlerError {
    HandlerError::fault(class, message)
}

fn key_fault(e: KeyFault) -> HandlerError {
    fault(&e.class, e.message)
}

fn join_fault(e: tokio::task::JoinError) -> HandlerError {
    fault("RuntimeError", e.to_string())
}

fn assign_error(e: AssignError) -> HandlerError {
    match e {
        AssignError::Value(words) => refusal(words),
        AssignError::Other(e) => fault(e.class(), e.to_string()),
    }
}

/// A runtime error said as Python's `f"{type(e).__name__}: {e}"`.
fn said(e: &RuntimeError) -> String {
    format!("{}: {e}", e.class())
}

struct Facts<'a>(&'a Runtimes);

impl SignInFacts for Facts<'_> {
    fn codex_binary(&self) -> bool {
        self.0.deps.programs.codex_binary().is_ok()
    }
    fn has_api_key(&self, provider: &str) -> bool {
        self.0.keys().has_api_key(provider)
    }
}

pub fn register(server: &Server, host: &Host, deps: &Deps) {
    let rt = Runtimes {
        host: host.clone(),
        deps: deps.clone(),
    };
    macro_rules! unary {
        ($method:literal, $handler:ident) => {{
            let r = rt.clone();
            server.register_unary($method, move |_s, params: Map<String, Value>| {
                let r = r.clone();
                async move { r.$handler(&params).await }
            });
        }};
    }
    unary!("runtimes.status", status);
    unary!("runtimes.sign_in", sign_in);
    unary!("runtimes.use", use_runtime);
    unary!("runtimes.switch_task", switch_task);
    unary!("runtimes.check", check);
    unary!("runtimes.models", models);
    unary!("runtimes.model", model);
    unary!("keys.store", keys_store);
    unary!("keys.delete", keys_delete);
    unary!("runtimes.check_calls", check_calls);
}

impl Runtimes {
    pub fn new(host: Host, deps: Deps) -> Runtimes {
        Runtimes { host, deps }
    }

    fn cfg(&self) -> View<'_> {
        View(&self.host.config)
    }

    fn keys(&self) -> Keys {
        Keys {
            helper: self.deps.helper.clone(),
            security: self.deps.security.clone(),
            cache: self.deps.state.keys.clone(),
        }
    }

    fn device(&self) -> String {
        text_or(&self.cfg(), "device", "id", "unknown-device")
    }

    fn forget_available(&self, name: &str) {
        self.deps
            .state
            .available
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(name);
    }

    /// Journal rows a handler writes after its change was made: a failed write is the handler's error.
    fn journal(&self, kind: &str, data: Value) -> Result<(), HandlerError> {
        self.host.journal(kind, data, None)
    }

    // ---------- roles, models, availability ----------

    /// `_runtime_for`: a planner-chosen worker type, or the runtime the owner chose for one task's checker
    /// ("checker:<runtime>", ORCH-315), else [runtimes] orchestrator / worker / checker.
    pub fn runtime_for(&self, role: &str) -> String {
        if (role.starts_with("worker:") || role.starts_with("checker:")) && role != "worker:default"
        {
            return role.split_once(':').map_or("", |x| x.1).to_string();
        }
        let cfg = self.cfg();
        if role == "orchestrator" {
            return text_or(&cfg, "runtimes", "orchestrator", "codex");
        }
        let worker = text_or(&cfg, "runtimes", "worker", "codex");
        if role == "checker" {
            text_or(&cfg, "runtimes", "checker", &worker)
        } else {
            worker
        }
    }

    /// `_model_for`: the model the owner set for a runtime; None when they set none.
    pub fn model_for(&self, name: &str) -> Option<String> {
        catalog::configured_model(&self.cfg(), name)
    }

    /// `_worker_types`: "default", then every runtime that can run here, in the table's order.
    pub fn worker_types(&self) -> Vec<String> {
        let mut out = vec!["default".to_string()];
        out.extend(
            CATALOG
                .iter()
                .filter(|r| self.runtime_available(r.name))
                .map(|r| r.name.to_string()),
        );
        out
    }

    /// `_runtime_available` (RUN-424): the table's availability as last known; read again only after an event that
    /// can change it (a choice, a sign-in, a status read, a check, a call refused for its sign-in) or a model change.
    pub fn runtime_available(&self, name: &str) -> bool {
        let model = catalog::model_for(&self.cfg(), name);
        if let Some((m, ok)) = self
            .deps
            .state
            .available
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(name)
        {
            if *m == model {
                return *ok;
            }
        }
        let ok = catalog::entry(name).is_some_and(|rt| {
            catalog::available(rt, &self.host.home, &self.cfg(), &Facts(self), Some(&model)).0
        });
        self.deps
            .state
            .available
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(name.to_string(), (model, ok));
        ok
    }

    /// `_runtime_account` (ORCH-315, RUN-424, APP-882): the account fact a call reported. "signed_out" counts the
    /// runtime as unavailable until it is read again; "signed_out" or "usage_limit" is kept as its last refusal,
    /// which a later call it answered ("") clears.
    pub fn runtime_account(&self, name: &str, account: &str) -> Result<(), HandlerError> {
        if account == "signed_out" {
            let model = catalog::model_for(&self.cfg(), name);
            self.deps
                .state
                .available
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(name.to_string(), (model, false));
        }
        self.load_refusals();
        let changed = {
            let mut held = self
                .deps
                .state
                .refusals
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let held = held.get_or_insert_with(HashMap::new);
            if !account.is_empty() {
                held.insert(name.to_string(), (account.to_string(), self.host.now_iso()));
                Some(format!("runtime.{account}"))
            } else if held.remove(name).is_some() {
                Some("runtime.answered".to_string())
            } else {
                None
            }
        };
        let Some(kind) = changed else {
            return Ok(());
        };
        self.journal(&kind, json!({"runtime": name}))?;
        self.runtimes_changed(
            name,
            if account.is_empty() {
                "answered"
            } else {
                "refused"
            },
        );
        Ok(())
    }

    /// `_refusals`: each runtime's last refusal still standing, read once from the journal, then kept in memory.
    fn load_refusals(&self) {
        let mut slot = self
            .deps
            .state
            .refusals
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if slot.is_some() {
            return;
        }
        let reads = &self.deps.journal_reads;
        let mut held = HashMap::new();
        for rt in CATALOG.iter() {
            let kind = reads.latest_kind(&REFUSAL_KINDS, "runtime", rt.name);
            if let Some(kind @ ("runtime.signed_out" | "runtime.usage_limit")) = kind.as_deref() {
                let rows = reads.of_kind(kind, "runtime", rt.name);
                let at = rows.last().map(|r| r.0.clone()).unwrap_or_default();
                let account = kind.split_once('.').map_or("", |x| x.1).to_string();
                held.insert(rt.name.to_string(), (account, at));
            }
        }
        *slot = Some(held);
    }

    fn refusal_of(&self, name: &str) -> Option<(String, String)> {
        self.load_refusals();
        self.deps
            .state
            .refusals
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .and_then(|h| h.get(name).cloned())
    }

    /// `_runtimes_changed` (APP-882, DMN-580): FERB.app's Settings → AI follows a change made anywhere.
    fn runtimes_changed(&self, runtime: &str, why: &str) {
        self.host
            .publish("runtimes.changed", json!({"runtime": runtime, "why": why}));
    }

    /// `_runtime_violation`: this run's tool-source violation, journaled.
    pub fn runtime_violation(&self, runtime: &str, detail: &str) -> Result<(), HandlerError> {
        self.journal(
            "runtime.failed",
            json!({"runtime": runtime, "detail": detail}),
        )
    }

    // ---------- building a runtime ----------

    /// `_make_runtime`: the runtime of `role`, built for one call. The planner's turn is the orchestrator's
    /// supervisor call with SUPERVISOR_SCHEMA (Python checks that object's identity; here its value).
    pub fn make_runtime(
        &self,
        role: &str,
        mode: &str,
        tool_names: &[String],
        schema: &Value,
        effort: Option<&str>,
    ) -> Result<(Build, Built), RuntimeError> {
        self.build_runtime(
            &self.runtime_for(role),
            role,
            mode,
            tool_names,
            schema,
            effort,
            "",
            "",
        )
    }

    /// `_build_runtime` with the table's factories' own checks (catalog.py `_build_codex`, `_build_claude`,
    /// `_api`): FERB's own sign-in present, a model chosen, the effort the call sends (catalog.run_effort, or
    /// exactly `fixed_effort`), its overall limit; then the adapter itself through the [`Factory`].
    #[allow(clippy::too_many_arguments)]
    pub fn build_runtime(
        &self,
        name: &str,
        role: &str,
        mode: &str,
        tool_names: &[String],
        schema: &Value,
        effort: Option<&str>,
        model: &str,
        fixed_effort: &str,
    ) -> Result<(Build, Built), RuntimeError> {
        let Some(entry) = catalog::entry(name) else {
            let words = if role.starts_with("worker:") {
                format!(
                    "there is no worker type {}; the worker types are {}",
                    repr_str(name),
                    self.worker_types().join(", ")
                )
            } else {
                let names: Vec<&str> = CATALOG.iter().map(|r| r.name).collect();
                format!(
                    "unknown runtime {} in config.toml [runtimes]; runtimes: {}",
                    repr_str(name),
                    names.join(", ")
                )
            };
            return Err(RuntimeError::Ferb(FerbError::Ferb(words)));
        };
        let cfg = self.cfg();
        let planner =
            role == "orchestrator" && mode == "supervisor" && schema == supervisor_schema();
        let sent = if fixed_effort.is_empty() {
            catalog::run_effort(&cfg, name, role, mode, planner, effort)
        } else {
            Some(fixed_effort.to_string())
        };
        let model = if model.is_empty() {
            catalog::role_model(&cfg, name, role)
        } else {
            model.to_string()
        };
        let mut b = Build {
            home: self.host.home.clone(),
            name: name.to_string(),
            role: role.to_string(),
            mode: mode.to_string(),
            tool_names: tool_names.to_vec(),
            schema: schema.clone(),
            model,
            effort: sent,
            planner,
            limit_s: catalog::call_limit(mode, planner),
            codex_binary: None,
            cache_role: None,
            cache_scope: self.device(),
        };
        match entry.sign_in {
            catalog::SignIn::Codex => {
                if ferb_runtimes::codex::codex_home(&self.host.home).is_none() {
                    return Err(RuntimeError::SignedOut {
                        message:
                            "Codex isn't signed in for FERB: sign in to ChatGPT in Settings → AI"
                                .into(),
                        runtime: "codex".into(),
                    });
                }
                b.codex_binary = Some(self.deps.programs.codex_binary()?);
                // P20 (RUN-401): on by default; `[runtimes] codex_planner_cache = false` turns it off
                let cache = cfg.0.get("runtimes", "codex_planner_cache").ok().flatten();
                if planner && cache != Some(Value::Bool(false)) {
                    b.cache_role = Some("planner".into());
                }
            }
            catalog::SignIn::Claude => {
                if sign_in::claude_home(&self.host.home).is_none() {
                    // never the owner's own ~/.claude login and billing
                    return Err(RuntimeError::SignedOut {
                        message:
                            "Claude isn't signed in for FERB: sign in to Claude in Settings → AI"
                                .into(),
                        runtime: "claude".into(),
                    });
                }
                if b.model.is_empty() {
                    return Err(RuntimeError::unsupported("no model is chosen for Claude"));
                }
            }
            catalog::SignIn::Key => {}
        }
        let built = self.deps.factory.build(&b)?;
        Ok((b, built))
    }

    /// worker.py `model_limits` on the built runtime: a model without documented limits fails before any call (L28).
    fn model_limits(&self, b: &Build) -> Result<(), RuntimeError> {
        match catalog::limits(&b.name, Some(&b.model), &self.host.home) {
            Ok(_) => Ok(()),
            Err(FerbError::ModelLimitsUnknown(e)) => {
                Err(RuntimeError::Raised(ferb_runtimes::Raised::new(
                    "ServiceError",
                    format!("no documented context window: {e}"),
                )))
            }
            Err(e) => Err(RuntimeError::Ferb(e)),
        }
    }

    // ---------- the Codex planner cache's hooks (P20, RUN-416) ----------

    /// `_codex_cache_refusal`: why this planner turn may not hold its stable context in the developer role; "" when
    /// it may (only an owner's chat turn may, not a scheduled run's).
    pub fn codex_cache_refusal(&self, task: &Task) -> String {
        let asked = self
            .deps
            .journal_reads
            .of_kind("chat.message", "request_id", &task.request_id);
        let Some((_, data)) = asked.last() else {
            return "not an owner's chat message".into();
        };
        let session = data
            .get("session_id")
            .map_or_else(|| "owner".to_string(), ferb_py::value::py_str);
        if session.starts_with(ferb_orchestrator::continuity::SCHEDULE_SESSION) {
            return "a scheduled run's turn".into();
        }
        String::new()
    }

    /// `_codex_cache_skipped`: one journal line when a planner turn kept its context in the user role.
    pub fn codex_cache_skipped(&self, request_id: &str, why: &str) -> Result<(), HandlerError> {
        self.journal(
            "runtime.cache_skipped",
            json!({"runtime": "codex", "role": "planner", "request_id": request_id, "why": why}),
        )
    }

    /// `_codex_cache_fallback`: one journal line when a cached planner turn took today's path instead.
    pub fn codex_cache_fallback(&self, request_id: &str, why: &str) -> Result<(), HandlerError> {
        self.journal(
            "runtime.cache_fallback",
            json!({"runtime": "codex", "role": "planner", "request_id": request_id, "why": why}),
        )
    }

    // ---------- keys ----------

    /// `_api_key`: from the Keychain at call time (through the helper); registered with the redactor so an echo
    /// never reaches a trace or the journal.
    pub async fn api_key(&self, provider: &str) -> Result<Option<Secret>, HandlerError> {
        let (keys, p) = (self.keys(), provider.to_string());
        let (found, problems) = tokio::task::spawn_blocking(move || {
            let mut problems = Vec::new();
            let found = keys.read_key(&p, &mut problems);
            (found, problems)
        })
        .await
        .map_err(join_fault)?;
        self.keys_problems(problems)?;
        let found = found.map_err(key_fault)?;
        if let Some(k) = &found {
            self.host.redactor.add_secret_value(k.expose());
        }
        Ok(found)
    }

    /// `_keys_problems` (GATE-547): an older key item FERB couldn't delete, journaled (provider only).
    fn keys_problems(&self, problems: Vec<Value>) -> Result<(), HandlerError> {
        for p in problems {
            self.journal("keys.old_item_not_deleted", p)?;
        }
        Ok(())
    }

    // ---------- RPCs ----------

    /// `_runtimes_status` (L34 C, APP-882, DMN-580): each runtime's availability read afresh (it also refreshes
    /// the last known availability), its last refusal, its effort, and each role's runtime, model and effort.
    pub async fn status(&self, _params: &Map<String, Value>) -> HandlerResult {
        let r = self.clone();
        let mut out =
            tokio::task::spawn_blocking(move || cli::status(&r.host.home, &r.cfg(), &Facts(&r)))
                .await
                .map_err(join_fault)?;
        let cfg = self.cfg();
        if let Some(Value::Array(rows)) = out.get_mut("runtimes") {
            for row in rows.iter_mut() {
                let Value::Object(row) = row else { continue };
                let name = ferb_py::value::py_str(&row["name"]);
                let model = ferb_py::value::py_str(&row["model"]);
                let usable = row["usable"] == Value::Bool(true);
                self.deps
                    .state
                    .available
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(name.clone(), (model, usable));
                if name == "claude" {
                    row.insert(
                        "sign_in".into(),
                        Value::Object(self.deps.state.sign_in.status()),
                    );
                }
                row.insert(
                    "effort".into(),
                    catalog::configured_effort(&cfg, &name).into(),
                );
                if let Some((account, at)) = self.refusal_of(&name) {
                    let fix = if account == "signed_out" {
                        catalog::sign_in_fix(&name)
                    } else {
                        "it can be used again when the plan's limit resets, or use another connection".into()
                    };
                    row.insert(
                        "refused".into(),
                        json!({"account": account, "at": at, "fix": fix}),
                    );
                }
            }
        }
        let mut details = Map::new();
        for role in ROLES {
            let name = self.runtime_for(role);
            details.insert(
                role.into(),
                json!({"runtime": name, "model": catalog::role_model(&cfg, &name, role),
                       "effort": catalog::role_effort(&cfg, &name, role)}),
            );
        }
        out.insert("role_details".into(), Value::Object(details));
        Ok(out)
    }

    /// `_runtimes_sign_in` (L34 C): FERB's own Claude sign-in for FERB.app: `{runtime: "claude"}` starts it (or
    /// reports the one waiting), `code` passes the page's code to it, `cancel: true` stops it. Journaled without
    /// the code or anything Claude Code printed.
    pub async fn sign_in(&self, params: &Map<String, Value>) -> HandlerResult {
        let p = P(params);
        if p.get("runtime") != Some(&Value::String("claude".into())) {
            return Err(refusal(
                "Only Claude signs in here; ChatGPT signs in with Settings → AI → Sign in with ChatGPT",
            ));
        }
        let sign_in = &self.deps.state.sign_in;
        if p.is_true("cancel") {
            return Ok(sign_in.cancel().await);
        }
        if let Some(code) = p.get("code") {
            let redactor = self.host.redactor.clone();
            return sign_in.send_code(&ferb_py::value::py_str(code), &|v| {
                redactor.add_secret_value(v)
            });
        }
        let redactor = self.host.redactor.clone();
        let me = self.clone();
        let hooks = sign_in::Hooks {
            ferb_home: self.host.home.clone(),
            cli: self.deps.programs.claude_cli(),
            environ: self.deps.programs.environ(),
            add_secret: Arc::new(move |v| redactor.add_secret_value(v)),
            done: Arc::new(move |signed| me.claude_signed_in(signed)),
        };
        let status = sign_in.start(hooks).await?;
        self.journal(
            "runtimes.sign_in",
            json!({"runtime": "claude", "state": status.get("state").cloned().unwrap_or(Value::Null)}),
        )?;
        Ok(status)
    }

    /// `_claude_signed_in`: FERB's own Claude sign-in ended: journaled, never with the credential or the code.
    fn claude_signed_in(&self, signed: bool) {
        self.forget_available("claude");
        let _ = self.journal(
            "runtimes.sign_in_finished",
            json!({"runtime": "claude", "signed_in": signed}),
        );
        self.runtimes_changed(
            "claude",
            if signed {
                "signed_in"
            } else {
                "sign_in_failed"
            },
        );
    }

    /// `_runtimes_use` (L34 C): the owner's AI service for the roles named (all three by default), and optionally
    /// its model, from its next call; one that can't run here yet is set anyway and says so.
    pub async fn use_runtime(&self, params: &Map<String, Value>) -> HandlerResult {
        let p = P(params);
        let runtime = p.text("runtime", "");
        let chosen: Vec<String> = match p.get("roles") {
            None => ROLES.iter().map(|r| r.to_string()).collect(),
            Some(Value::Array(items)) if items.iter().all(Value::is_string) => {
                items.iter().map(ferb_py::value::py_str).collect()
            }
            Some(_) => return Err(refusal("roles must be a list of role names")),
        };
        let model = match p.get("model") {
            None | Some(Value::Null) => None,
            Some(Value::String(m)) if !ferb_py::value::strip(m).is_empty() => Some(m.clone()),
            Some(_) => return Err(refusal("model must be a model name")),
        };
        let assigned = self.assign(&runtime, &chosen, model.clone()).await?;
        self.forget_available(&runtime);
        let mut data = json!({"runtime": runtime, "roles": chosen});
        if let Some(m) = &model {
            data["model"] = m.clone().into();
        }
        self.journal("runtimes.changed", data)?;
        self.runtimes_changed(&runtime, "roles");
        let r = self.clone();
        let rt = runtime.clone();
        let (usable, fix) = tokio::task::spawn_blocking(move || {
            catalog::entry(&rt).map_or((false, String::new()), |e| {
                catalog::available(e, &r.host.home, &r.cfg(), &Facts(&r), None)
            })
        })
        .await
        .map_err(join_fault)?;
        ok(json!({"roles": assigned, "usable": usable, "fix": fix}))
    }

    /// runtimes_cli.assign in a worker thread; its ValueError is the owner's refusal.
    async fn assign(
        &self,
        runtime: &str,
        chosen: &[String],
        model: Option<String>,
    ) -> Result<Map<String, Value>, HandlerError> {
        let (r, runtime, chosen) = (self.clone(), runtime.to_string(), chosen.to_vec());
        tokio::task::spawn_blocking(move || {
            let programs = r.deps.programs.clone();
            cli::assign(
                &r.host.home,
                &r.host.config,
                &runtime,
                &chosen,
                model.as_deref(),
                &|| programs.bundled_catalog(),
            )
        })
        .await
        .map_err(join_fault)?
        .map_err(assign_error)
    }

    /// `_runtimes_switch_task` (ORCH-315): a task stopped on a usage limit or a sign-in failure, answered with
    /// `runtime` for that task (`scope` "task"), or its role pointed at it first ("role"). Owner-only.
    pub async fn switch_task(&self, params: &Map<String, Value>) -> HandlerResult {
        let p = P(params);
        let (task_id, runtime) = (p.text("task_id", ""), p.text("runtime", ""));
        let scope = match p.get("scope") {
            None => "task".to_string(),
            Some(Value::String(s)) if s == "task" || s == "role" => s.clone(),
            Some(_) => return Err(refusal("scope must be \"task\" or \"role\"")),
        };
        let offer = self.deps.supervisor.runtime_offer(&task_id);
        let offered = offer.as_ref().is_some_and(|o| {
            o.get("choices")
                .and_then(Value::as_array)
                .is_some_and(|c| c.iter().any(|x| x.as_str() == Some(runtime.as_str())))
        });
        let (Some(offer), true) = (offer, offered) else {
            return Err(refusal(format!(
                "task {task_id} isn't waiting to switch to {runtime}"
            )));
        };
        if scope == "role" {
            let role = ferb_py::value::py_str(offer.get("role").unwrap_or(&Value::Null));
            self.assign(&runtime, std::slice::from_ref(&role), None)
                .await?;
            self.forget_available(&runtime);
            self.journal(
                "runtimes.changed",
                json!({"runtime": runtime, "roles": [role]}),
            )?;
            self.runtimes_changed(&runtime, "roles");
        }
        let answered = self
            .deps
            .supervisor
            .answer_runtime_switch(&task_id, &runtime)?;
        ok(
            json!({"task_id": task_id, "runtime": runtime, "scope": scope,
                  "role": answered.get("role").cloned().unwrap_or(Value::Null)}),
        )
    }

    /// `_runtimes_check`: one tiny typed call through a runtime (the key, model and network work).
    pub async fn check(&self, params: &Map<String, Value>) -> HandlerResult {
        let p = P(params);
        Ok(self
            .check_runtime(&p.text("runtime", "None"), &p.text_or_empty("model"), "")
            .await)
    }

    /// `_check_runtime`: `runtimes.check`'s call, with `model` instead of the runtime's own when given (RUN-429),
    /// made at exactly `effort` when given (RUN-463).
    ///
    /// Fixed against Python: an adapter whose event stream raises answers `{"ok": false, "detail": "<Class>:
    /// <words>"}` (redacted), as a build or start failure does; Python let it escape as an internal error.
    pub async fn check_runtime(&self, name: &str, model: &str, effort: &str) -> Map<String, Value> {
        let failed = |detail: String| -> Map<String, Value> {
            super::core::object(json!({"ok": false, "detail": detail}))
        };
        self.forget_available(name);
        let schema = json!({"type": "object", "properties": {"ok": {"type": "boolean"}}, "required": ["ok"],
                            "additionalProperties": false});
        let task = Task {
            id: "runtime-check".into(),
            parent_id: None,
            goal: "chat".into(),
            device_id: self.device(),
            identity: Identity::Owner,
            scopes: BTreeSet::new(),
            role: TaskRole::Mixed,
            request_id: "runtime-check".into(),
            success_conditions: vec![],
            deadline: None,
        };
        let request = ModelRequest {
            system: "Answer {\"ok\": true}.".into(),
            parts: vec!["Check.".into()],
            cacheable: 0,
            images: vec![],
        };
        let started = async {
            let (b, built) = self.build_runtime(
                name,
                "orchestrator",
                "supervisor",
                &[],
                &schema,
                None,
                model,
                effort,
            )?;
            self.model_limits(&b)?;
            built.adapter.start(&task, &request, "").await
        }
        .await;
        let mut started = match started {
            Ok(s) => s,
            Err(e) => return failed(self.host.redact_text(&said(&e))),
        };
        let mut detail = String::new();
        let mut result = None;
        while let Some(item) = started.events.next().await {
            let ev = match item {
                Ok(ev) => ev,
                Err(e) => {
                    result = Some(failed(self.host.redact_text(&said(&e))));
                    break;
                }
            };
            match ev.kind {
                RuntimeEventKind::Final => {
                    let answer = match ev.data.get("text") {
                        Some(Value::String(t)) => ferb_py::json::loads(t).ok(),
                        _ => None,
                    };
                    let Some(Value::Object(answer)) = answer else {
                        result = Some(failed("the answer was not {\"ok\": ...}".into()));
                        break;
                    };
                    let yes = answer.get("ok") == Some(&Value::Bool(true));
                    if yes {
                        // it answered: an earlier refusal no longer stands
                        let _ = self.runtime_account(name, "");
                    }
                    result = Some(super::core::object(
                        json!({"ok": yes, "detail": "answered"}),
                    ));
                    break;
                }
                RuntimeEventKind::Error | RuntimeEventKind::Violation => {
                    detail = self.host.redact_text(&py_repr(&Value::Object(ev.data)));
                }
                _ => {}
            }
        }
        started.control.cancel().await;
        result.unwrap_or_else(|| {
            failed(if detail.is_empty() {
                "no answer".into()
            } else {
                detail
            })
        })
    }

    /// `_runtimes_models` (RUN-429, RUN-464, DMN-580): the models a connection can use for Settings → AI's picker,
    /// the model it runs now and each role's own, and each model's effort levels and documented default.
    pub async fn models(&self, params: &Map<String, Value>) -> HandlerResult {
        let name = P(params).text("runtime", "");
        let Some(entry) = catalog::entry(&name) else {
            return Err(refusal(format!("unknown AI service {}", repr_str(&name))));
        };
        let cfg = self.cfg();
        let mut roles = Map::new();
        for role in ROLES {
            if let Some(row @ Value::Object(_)) = cfg.get("role_models", role) {
                roles.insert(role.into(), row);
            }
        }
        let model = catalog::model_for(&cfg, &name);
        let key_sign_in = entry.sign_in == catalog::SignIn::Key;
        let mut out = Map::new();
        out.insert("runtime".into(), name.clone().into());
        out.insert("model".into(), model.clone().into());
        out.insert("models".into(), json!([]));
        out.insert(
            "source".into(),
            if key_sign_in { "provider" } else { "known" }.into(),
        );
        out.insert(
            "effort".into(),
            catalog::configured_effort(&cfg, &name).into(),
        );
        out.insert("roles".into(), Value::Object(roles.clone()));
        let mut models: Vec<String> = Vec::new();
        if !key_sign_in {
            let (home, n) = (self.host.home.clone(), name.clone());
            models =
                tokio::task::spawn_blocking(move || ferb_runtimes::models::known_models(&n, &home))
                    .await
                    .map_err(join_fault)?;
        } else if let Some(key) = self.api_key(&name).await? {
            match self.deps.model_lists.provider_models(&name, &key).await {
                Ok(list) => models = list,
                Err(words) => {
                    out.insert("problem".into(), self.host.redact_text(&words).into());
                }
            }
        } else {
            out.insert(
                "problem".into(),
                format!("no {name} API key is saved").into(),
            );
        }
        out.insert("models".into(), json!(models));
        let mut shown: BTreeSet<String> = models.iter().cloned().collect();
        shown.insert(model);
        for row in roles.values() {
            shown.insert(match row.get("model") {
                Some(m) if ferb_py::value::truthy(m) => ferb_py::value::py_str(m),
                _ => String::new(),
            });
        }
        shown.remove("");
        let shown: Vec<String> = shown.into_iter().collect();
        let (home, n) = (self.host.home.clone(), name.clone());
        let (levels, defaults) =
            tokio::task::spawn_blocking(move || effort_levels(&n, &home, &shown))
                .await
                .map_err(join_fault)?;
        out.insert("levels".into(), Value::Object(levels));
        out.insert("default_efforts".into(), Value::Object(defaults));
        Ok(out)
    }

    /// `_runtimes_model` (RUN-429, RUN-463, DMN-580): the owner's model for a connection, or for one role on it
    /// (an empty model clears the role's own), with its effort ("" clears it, absent keeps it). Checked before it is
    /// saved: in the provider's own list for this key, else one tiny call (made at a new effort); a model or
    /// effort that fails isn't saved and the provider's words come back.
    pub async fn model(&self, params: &Map<String, Value>) -> HandlerResult {
        let p = P(params);
        let name = p.text("runtime", "");
        let model = ferb_py::value::squeeze(&p.text_or_empty("model"));
        let role = p.get("role").filter(|r| !r.is_null());
        let Some(entry) = catalog::entry(&name) else {
            return Err(refusal(format!("unknown AI service {}", repr_str(&name))));
        };
        let role: Option<String> = match role {
            None => None,
            Some(Value::String(r)) if ROLES.contains(&r.as_str()) => Some(r.clone()),
            Some(_) => return Err(refusal(format!("role must be one of {}", ROLES.join(", ")))),
        };
        if role.is_none() && model.is_empty() {
            return Err(refusal("model must be a model id"));
        }
        let given = p.has("effort");
        let mut effort = ferb_py::value::strip(&p.text_or_empty("effort")).to_string();
        if !effort.is_empty() {
            let problem = catalog::effort_problem(&name, &effort);
            if !problem.is_empty() {
                return Err(refusal(problem));
            }
        }
        let cfg = self.cfg();
        let before = match &role {
            Some(r) => match cfg.get("role_models", r) {
                Some(Value::Object(row))
                    if row.get("runtime").and_then(Value::as_str) == Some(&name) =>
                {
                    match row.get("effort") {
                        Some(e) if ferb_py::value::truthy(e) => ferb_py::value::py_str(e),
                        _ => String::new(),
                    }
                }
                _ => String::new(),
            },
            None => catalog::configured_effort(&cfg, &name),
        };
        if !given {
            effort = before.clone();
        }
        let mut checked = String::new();
        if !model.is_empty() && entry.sign_in == catalog::SignIn::Key {
            if let Some(key) = self.api_key(&name).await? {
                if let Ok(offered) = self.deps.model_lists.provider_models(&name, &key).await {
                    if !offered.contains(&model) {
                        return ok(json!({"ok": false,
                                         "detail": format!("{model} isn't in the models your {name} key can use")}));
                    }
                    checked = "in the provider's list".into();
                }
            }
        }
        let call =
            (!model.is_empty() && checked.is_empty()) || (!effort.is_empty() && effort != before);
        if call {
            let use_model = if model.is_empty() {
                catalog::model_for(&cfg, &name)
            } else {
                model.clone()
            };
            let result = self.check_runtime(&name, &use_model, &effort).await;
            if result.get("ok") != Some(&Value::Bool(true)) {
                let detail = match result.get("detail") {
                    Some(d) if ferb_py::value::truthy(d) => d.clone(),
                    _ => "no answer".into(),
                };
                return ok(json!({"ok": false, "detail": detail}));
            }
            checked = "answered".into();
        }
        match &role {
            Some(r) => {
                let written = if !model.is_empty() || !effort.is_empty() {
                    let mut chosen = Map::new();
                    chosen.insert("runtime".into(), name.clone().into());
                    chosen.insert("model".into(), model.clone().into());
                    if !effort.is_empty() {
                        chosen.insert("effort".into(), effort.clone().into());
                    }
                    rows::set_row(&self.host.config, "role_models", r, &chosen)
                } else {
                    rows::remove_row(&self.host.config, "role_models", r)
                };
                written.map_err(refusal)?;
            }
            None => {
                self.host
                    .set("models", &name, &Value::String(model.clone()))?;
                if !effort.is_empty() {
                    self.host
                        .set("efforts", &name, &Value::String(effort.clone()))?;
                } else {
                    self.host.remove("efforts", &name)?;
                }
            }
        }
        self.forget_available(&name);
        let or_none = |s: &str| if s.is_empty() { Value::Null } else { s.into() };
        let mut data = json!({"runtime": name, "model": or_none(&model), "effort": or_none(&effort),
                              "checked": checked});
        if let Some(r) = &role {
            data["role"] = r.clone().into();
        }
        self.journal("runtimes.model", data)?;
        self.runtimes_changed(&name, "model");
        let detail = if !checked.is_empty() {
            checked
        } else if role.is_some() && model.is_empty() {
            "uses the connection's model".into()
        } else {
            "saved".into()
        };
        ok(json!({"ok": true, "model": model, "effort": effort, "detail": detail}))
    }

    /// `_keys_store` (GATE-543, APP_ONLY): FERB.app's API key for a provider, stored through FERB's signed secret
    /// helper and read back; registered with the redactor first. The key is in no reply, span or journal entry.
    pub async fn keys_store(&self, params: &Map<String, Value>) -> HandlerResult {
        let p = P(params);
        let provider = p.text("provider", "");
        if !catalog::api_providers().contains(&provider.as_str()) {
            return Err(refusal(format!("unknown provider {}", repr_str(&provider))));
        }
        let key = match p.get("key") {
            Some(Value::String(k)) => Secret::new(ferb_py::value::strip(k).to_string()),
            _ => return Err(refusal("that doesn't look like an API key")),
        };
        if key.expose().is_empty() || key.expose().chars().any(ferb_py::value::is_py_space) {
            return Err(refusal("that doesn't look like an API key"));
        }
        self.host.redactor.add_secret_value(key.expose());
        let (keys, pr) = (self.keys(), provider.clone());
        let (stored, problems) = tokio::task::spawn_blocking(move || {
            let mut problems = Vec::new();
            let stored = keys.store_key(&pr, &key, &mut problems);
            (stored, problems)
        })
        .await
        .map_err(join_fault)?;
        self.keys_problems(problems)?;
        let stored = stored.map_err(key_fault)?;
        self.forget_available(&provider);
        self.journal(
            "keys.stored",
            json!({"provider": provider, "stored": stored}),
        )?;
        self.runtimes_changed(&provider, "key");
        ok(json!({"stored": stored}))
    }

    /// `_keys_delete` (GATE-543, APP_ONLY): the owner's Remove key: the provider's item (an older one too) goes.
    pub async fn keys_delete(&self, params: &Map<String, Value>) -> HandlerResult {
        let provider = P(params).text("provider", "");
        if !catalog::api_providers().contains(&provider.as_str()) {
            return Err(refusal(format!("unknown provider {}", repr_str(&provider))));
        }
        let (keys, pr) = (self.keys(), provider.clone());
        let (removed, problems) = tokio::task::spawn_blocking(move || {
            let mut problems = Vec::new();
            let removed = keys.delete_key(&pr, &mut problems);
            (removed, problems)
        })
        .await
        .map_err(join_fault)?;
        self.keys_problems(problems)?;
        let removed = removed.map_err(key_fault)?;
        self.forget_available(&provider);
        self.journal(
            "keys.deleted",
            json!({"provider": provider, "removed": removed}),
        )?;
        self.runtimes_changed(&provider, "key");
        ok(json!({"removed": removed}))
    }

    /// `_runtimes_check_calls` (T-1069, RUN-439): the call shapes FERB really makes, through the connection's real
    /// adapter and model: a planner call (its schema, its lookups offered), a worker call that calls a tool with no
    /// arguments and one with arguments, a checker call, and a call carrying a small generated picture. The two
    /// check tools exist only while it runs and touch nothing.
    pub async fn check_calls(&self, params: &Map<String, Value>) -> HandlerResult {
        let p = P(params);
        let name = p.text("runtime", "");
        let model = ferb_py::value::strip(&p.text_or_empty("model")).to_string();
        if catalog::entry(&name).is_none() {
            return Err(refusal(format!("unknown AI service {}", repr_str(&name))));
        }
        self.forget_available(&name);
        let (ping, echo) = (
            Arc::new(CheckPing::default()),
            Arc::new(CheckEcho::default()),
        );
        self.deps.tools.add(ping.clone());
        self.deps.tools.add(echo.clone());
        let request = |system: &str, images: Vec<ModelImage>| ModelRequest {
            system: system.into(),
            parts: vec!["Check.".into()],
            cacheable: 0,
            images,
        };
        let planner_tools: Vec<String> = PLANNER_TOOLS.iter().map(|s| s.to_string()).collect();
        let planner_scopes: BTreeSet<String> =
            PLANNER_SCOPES.iter().map(|s| s.to_string()).collect();
        let mut lines = vec![
            self.check_call(
                Line::new("Planner call (its real schema, lookups offered)"),
                &name,
                &model,
                "orchestrator",
                "supervisor",
                supervisor_schema(),
                &planner_tools,
                &planner_scopes,
                &request(check::PROMPT_PLANNER, vec![]),
            )
            .await,
        ];
        let worker = self
            .check_call(
                Line::new("Worker call (tools, its typed report)"),
                &name,
                &model,
                "worker",
                "worker",
                &ferb_context::schemas::worker_schema(Default::default()),
                &[check::PING.to_string(), check::ECHO.to_string()],
                &BTreeSet::new(),
                &request(check::PROMPT_WORKER, vec![]),
            )
            .await;
        // RUN-439: the two tool calls ran inside the worker call: its time, tokens and cost are on its line
        let pinged = ping.calls() > 0;
        let echoed = echo.texts().iter().any(|t| t == "hello");
        let part = |title: &str, ok: bool, missing: &str| Line {
            name: title.into(),
            ok,
            detail: if ok { String::new() } else { missing.into() },
            model: worker.model.clone(),
            typed: worker.typed.clone(),
            part_of: "worker call".into(),
            ..Line::default()
        };
        lines.push(part(
            "Tool call with no arguments",
            pinged,
            "the model didn't call check.ping",
        ));
        lines.push(part(
            "Tool call with arguments",
            echoed,
            "the model didn't call check.echo with hello",
        ));
        lines.push(worker);
        lines.push(
            self.check_call(
                Line::new("Checker call (its schema)"),
                &name,
                &model,
                "checker",
                "worker",
                &ferb_context::schemas::checker_schema(),
                &[],
                &BTreeSet::new(),
                &request(check::PROMPT_CHECKER, vec![]),
            )
            .await,
        );
        let picture = ModelImage {
            data: check::RED_PNG.to_vec(),
            reference: "connection-check".into(),
            mime: "image/png".into(),
        };
        lines.push(
            self.check_call(
                Line::new("A picture in the request"),
                &name,
                &model,
                "orchestrator",
                "supervisor",
                &check::image_schema(),
                &[],
                &BTreeSet::new(),
                &request(check::PROMPT_IMAGE, vec![picture]),
            )
            .await,
        );
        self.deps.tools.remove(check::PING);
        self.deps.tools.remove(check::ECHO);
        let costs: Vec<f64> = lines.iter().filter_map(|l| l.cost_usd).collect();
        let passed = lines.iter().filter(|l| l.ok).count();
        self.journal(
            "runtimes.checked",
            json!({"runtime": name, "passed": passed, "lines": lines.len()}),
        )?;
        let shown_model = if model.is_empty() {
            catalog::model_for(&self.cfg(), &name)
        } else {
            model
        };
        let total = if costs.is_empty() {
            Value::Null
        } else {
            json!(check::py_round(costs.iter().sum(), 6))
        };
        ok(
            json!({"runtime": name, "model": shown_model, "ok": lines.iter().all(|l| l.ok),
                  "lines": lines.iter().map(Line::row).collect::<Vec<_>>(), "total_cost_usd": total}),
        )
    }

    /// `_check_call`: one check call through the real runtime for `role`, offered `tools` through a run token for a
    /// live record held only for this call, its events read to the end (at most LINE_LIMIT_S). The line passes when
    /// the final answer fits `schema`; an error or a misfit says why, redacted.
    #[allow(clippy::too_many_arguments)]
    async fn check_call(
        &self,
        mut line: Line,
        name: &str,
        model: &str,
        role: &str,
        mode: &str,
        schema: &Value,
        tools: &[String],
        scopes: &BTreeSet<String>,
        request: &ModelRequest,
    ) -> Line {
        let checks = &self.deps.checks;
        let started_at = self.host.clock.monotonic();
        let task_id = format!("check-{}", checks.new_id());
        let mut holder = String::new();
        let task = Task {
            id: task_id.clone(),
            parent_id: None,
            goal: "FERB's connection check".into(),
            device_id: self.device(),
            identity: Identity::Owner,
            scopes: scopes.clone(),
            role: TaskRole::Mixed,
            request_id: task_id.clone(),
            success_conditions: vec![],
            deadline: None,
        };
        let span = checks.span_start(
            "runtime.check",
            json!({"runtime": name, "role": role, "check": line.name}),
        );
        let outcome: Result<(), RuntimeError> = async {
            let (_, built) =
                self.build_runtime(name, role, mode, tools, schema, None, model, "")?;
            let mut token = String::new();
            if !tools.is_empty() {
                holder = format!("check:{task_id}");
                checks.hold_task(&task, &self.host.now(), &span);
                let names: BTreeSet<String> = tools.iter().cloned().collect();
                token = checks.issue_token(
                    &task_id,
                    &holder,
                    scopes,
                    Identity::Owner,
                    600.0,
                    true,
                    &names,
                    10,
                );
            }
            let mut control: Option<Arc<dyn RunControl>> = None;
            let answered = tokio::time::timeout(
                Duration::from_secs_f64(check::LINE_LIMIT_S),
                answer(
                    &built,
                    &task,
                    request,
                    &token,
                    schema,
                    &mut line,
                    &mut control,
                ),
            )
            .await;
            let result = match answered {
                Ok(r) => r,
                Err(_) => {
                    line.detail = check::no_answer(
                        check::LINE_LIMIT_S,
                        &(built.retried)(),
                        line.retrying.as_deref().unwrap_or(""),
                    );
                    Ok(())
                }
            };
            if let Some(c) = control {
                c.cancel().await;
            }
            result
        }
        .await;
        if let Err(e) = outcome {
            // the provider's or FERB's words, never a crash of the check
            line.detail = said(&e);
        }
        if !holder.is_empty() {
            checks.revoke_token(&holder);
            checks.forget_calls(&holder);
            checks.remove_task(&task_id);
        }
        line.seconds = self.host.clock.monotonic() - started_at;
        line.detail = if !line.detail.is_empty() {
            self.host.redact_text(&line.detail)
        } else if line.ok {
            String::new()
        } else {
            "no answer".into()
        };
        if line.model.is_empty() {
            line.model = if model.is_empty() {
                catalog::model_for(&self.cfg(), name)
            } else {
                model.to_string()
            };
        }
        checks.span_finish(&span, line.row());
        line
    }

    // ---------- the daemon's start and doctor ----------

    /// `_prepare_codex` (DMN-510, L28): when a role runs on Codex or Codex is signed in for FERB, the catalog shipped
    /// with the pinned binary is dumped and the version checked once for the daemon's life; a failure is journaled.
    pub async fn prepare_codex(&self) -> Result<(), HandlerError> {
        let used = ROLES.iter().any(|r| self.runtime_for(r) == "codex");
        if !(used || ferb_runtimes::codex::codex_home(&self.host.home).is_some()) {
            return Ok(());
        }
        let programs = self.deps.programs.clone();
        tokio::task::spawn_blocking(move || programs.bundled_catalog())
            .await
            .map_err(join_fault)?;
        let prepared = match self.deps.programs.codex_binary() {
            Ok(binary) => self.deps.programs.prepare_codex(&binary).await,
            Err(e) => Err(e),
        };
        if let Err(e) = prepared {
            let detail = self.host.redact_text(&format!("{}: {e}", e.class()));
            self.journal("runtime.codex_problem", json!({"detail": detail}))?;
        }
        Ok(())
    }

    /// `_sweep_stale_runs` (review M1): Codex run folders a crash left in the temporary folder, removed at start;
    /// how many, journaled.
    pub async fn sweep_stale_runs(&self) -> Result<(), HandlerError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0.0, |d| d.as_secs_f64());
        let swept = tokio::task::spawn_blocking(move || {
            ferb_runtimes::codex::sweep_stale_runs(&std::env::temp_dir(), now)
        })
        .await
        .map_err(join_fault)?;
        if !swept.is_empty() {
            self.journal("runtime.stale_runs_swept", json!({"count": swept.len()}))?;
        }
        Ok(())
    }

    /// `_runtime_limits` (L28): each role's runtime and model with its documented limits, or the problem.
    pub fn runtime_limits(&self) -> Result<Map<String, Value>, FerbError> {
        let cfg = self.cfg();
        let mut out = Map::new();
        for role in ROLES {
            let name = self.runtime_for(role);
            let model = catalog::role_model(&cfg, &name, role);
            let model = (!model.is_empty()).then_some(model);
            let shown = model
                .clone()
                .unwrap_or_else(|| catalog::default_model(&name).to_string());
            let mut entry = Map::new();
            entry.insert("runtime".into(), name.clone().into());
            entry.insert("model".into(), shown.into());
            let limits = if catalog::entry(&name).is_none() {
                Err(FerbError::ModelLimitsUnknown(format!(
                    "unknown runtime {}",
                    repr_str(&name)
                )))
            } else {
                catalog::limits(&name, model.as_deref(), &self.host.home)
            };
            match limits {
                Ok(l) => {
                    entry.insert("model".into(), l.model.clone().into());
                    entry.insert("context_window".into(), l.context_window.into());
                    entry.insert("max_output".into(), l.max_output.into());
                    entry.insert("context_budget".into(), l.context_budget().into());
                    entry.insert("max_input_chars".into(), json!(l.max_input_chars));
                    entry.insert("source".into(), l.source.clone().into());
                }
                Err(FerbError::ModelLimitsUnknown(e)) => {
                    entry.insert("problem".into(), e.into());
                }
                Err(e) => return Err(e),
            }
            out.insert(role.into(), Value::Object(entry));
        }
        Ok(out)
    }

    /// `_runtime_status` (doctor): each role's configured runtime, the pinned Codex (or its problem), the pinned
    /// Claude Code's status, and the roles' limits.
    pub async fn runtime_status(&self) -> Result<Map<String, Value>, HandlerError> {
        let mut out = Map::new();
        out.insert(
            "orchestrator".into(),
            self.host
                .get_or("runtimes", "orchestrator", "codex".into())?,
        );
        out.insert(
            "worker".into(),
            self.host.get_or("runtimes", "worker", "codex".into())?,
        );
        let codex = match self.deps.programs.codex_binary() {
            Ok(binary) => match self.deps.programs.check_codex_version(&binary).await {
                Ok(()) => format!(
                    "{} ({})",
                    ferb_runtimes::codex::CODEX_VERSION,
                    binary.display()
                ),
                Err(e) => format!("problem: {e}"),
            },
            Err(e) => format!("problem: {e}"),
        };
        out.insert("codex".into(), codex.into());
        out.insert(
            "claude".into(),
            self.deps.programs.claude_pin_status().into(),
        );
        let limits = self
            .runtime_limits()
            .map_err(|e| fault(e.class(), e.to_string()))?;
        out.insert("limits".into(), Value::Object(limits));
        Ok(out)
    }
}

/// `_check_call`'s `answer()`: the run started, its events read until a final answer, an error or the end.
async fn answer(
    built: &Built,
    task: &Task,
    request: &ModelRequest,
    token: &str,
    schema: &Value,
    line: &mut Line,
    control: &mut Option<Arc<dyn RunControl>>,
) -> Result<(), RuntimeError> {
    let mut started = built.adapter.start(task, request, token).await?;
    *control = Some(started.control.clone());
    while let Some(item) = started.events.next().await {
        let ev = item?;
        match ev.kind {
            RuntimeEventKind::Usage => line.add_usage(&ev.data),
            RuntimeEventKind::Final => {
                let text = ferb_py::value::py_str(
                    ev.data.get("text").unwrap_or(&Value::String(String::new())),
                );
                match ferb_context::structured::check_final(&text, Some(schema)) {
                    Ok(_) => line.ok = true,
                    Err(e) => line.detail = format!("the answer didn't fit its form: {e}"),
                }
                return Ok(());
            }
            RuntimeEventKind::Error | RuntimeEventKind::Violation => {
                line.detail = match ev.data.get("message") {
                    Some(m) if ferb_py::value::truthy(m) => ferb_py::value::py_str(m),
                    _ => py_repr(&Value::Object(ev.data.clone())),
                };
                return Ok(());
            }
            RuntimeEventKind::Retrying => {
                // Codex retrying after an error: why, if it never answers
                line.retrying = Some(match ev.data.get("message") {
                    Some(m) if ferb_py::value::truthy(m) => ferb_py::value::py_str(m),
                    _ => String::new(),
                });
            }
            _ => {}
        }
    }
    Ok(())
}

/// models.py `effort_levels`, with Codex's models in its catalog's order (Python's dicts keep the catalog's order;
/// ferb-runtimes returns them sorted).
fn effort_levels(
    name: &str,
    home: &Path,
    shown: &[String],
) -> (Map<String, Value>, Map<String, Value>) {
    let (levels, defaults) = ferb_runtimes::models::effort_levels(name, home, shown);
    let order: Vec<String> = if name == "codex" {
        ferb_runtimes::codex::catalog(home)
            .and_then(|(d, _)| ferb_runtimes::codex::models(&d))
            .unwrap_or_default()
            .iter()
            .filter(|m| m.get("slug").is_some_and(ferb_py::value::truthy))
            .map(|m| ferb_py::value::py_str(&m["slug"]))
            .collect()
    } else {
        shown.to_vec()
    };
    let mut out_levels = Map::new();
    let mut out_defaults = Map::new();
    for slug in order {
        if let Some(l) = levels.get(&slug) {
            out_levels.entry(slug.clone()).or_insert_with(|| json!(l));
        }
        if let Some(d) = defaults.get(&slug) {
            out_defaults.entry(slug.clone()).or_insert_with(|| json!(d));
        }
    }
    (out_levels, out_defaults)
}
