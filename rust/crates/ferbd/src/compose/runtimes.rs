//! The runtime factory (`ferb/runtimes/catalog.py` RuntimeDeps, `areas/runtimes.py` `_make_runtime`): FERB's
//! runtime table's adapters (Codex, Claude through the `claude` program, the three APIs) built for one call, with
//! the daemon's tool bridge, redactor and keys, and the glue from those adapters to ferb-worker's `ModelRuntime`.
//!
//! The MCP tool bridge a model program starts is this program again (`ferbd mcp`), as Python re-enters its host
//! through `sys.executable -I -m ferb.cli.mcp` ([`mcp_shim_command`]).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ferb_core::errors::FerbError;
use ferb_core::interfaces::BoxFuture;
use ferb_core::types::{ModelLimits, ModelRequest, RuntimeEvent, Task};
use ferb_daemon::areas_b::runtimes::{Build, Built, Factory, Programs, Runtimes};
use ferb_gateway::gateway::Gateway;
use ferb_identity::Secret;
use ferb_runtimes::error::RuntimeError;
use ferb_runtimes::forms::{Forms, Refusals, SchemaTooComplex, Shape};
use ferb_runtimes::run::Adapter;
use ferb_sanitize::redact::Redactor;
use ferb_worker::errors::Error as WorkerError;
use ferb_worker::ports::{EventSource, ModelRuntime, RunControl, StartedRun};
use serde_json::Value;

/// How a model program starts FERB's tool bridge: this program, `mcp` (`mcp_shim_command`).
pub fn mcp_shim_command() -> Vec<String> {
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("ferbd"));
    vec![exe.to_string_lossy().into_owned(), "mcp".into()]
}

// ---------------------------------------------------------------------------------------------------------------
// the programs FERB runs for its runtimes
// ---------------------------------------------------------------------------------------------------------------

/// codex_exec.py `CODEX_VERSION`: the pinned Codex.
pub const CODEX_VERSION: &str = "0.166.0";

/// The Mac's programs for the runtimes (the pinned Codex, the pinned Claude Code).
pub struct MacPrograms {
    pub home: PathBuf,
    pub app_bundle: Option<PathBuf>,
}

fn machine() -> &'static str {
    if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else {
        "arm64"
    }
}

impl Programs for MacPrograms {
    fn codex_binary(&self) -> Result<PathBuf, FerbError> {
        // FERB.app carries its own node folder; a checkout's Codex is in FERB's home
        let repo = self
            .app_bundle
            .as_ref()
            .map(|a| a.join("Contents/Resources"));
        ferb_runtimes::codex::codex_binary(&self.home, repo.as_deref(), machine())
    }

    /// The pinned Claude Code (ENG-2: driven directly, no Python SDK). Where the Rust build carries it is a packaging
    /// decision not made yet (rust/PLAN.md part 13): inside FERB.app it is looked for at `Contents/Resources/claude`.
    fn claude_cli(&self) -> Result<PathBuf, FerbError> {
        let found = self
            .app_bundle
            .as_ref()
            .map(|a| a.join("Contents/Resources/claude"))
            .filter(|p| p.is_file());
        found.ok_or_else(|| {
            FerbError::Unsupported("FERB's pinned Claude Code isn't part of this build yet".into())
        })
    }

    fn bundled_catalog(&self) {
        if let Ok(binary) = self.codex_binary() {
            let environ: BTreeMap<String, String> = std::env::vars().collect();
            let _ = ferb_runtimes::codex::bundled_catalog(&self.home, &binary, &environ);
        }
    }

    fn prepare_codex<'a>(&'a self, binary: &'a Path) -> BoxFuture<'a, Result<(), FerbError>> {
        Box::pin(async move {
            self.check_codex_version(binary).await?;
            let home = self.home.clone();
            let _ = tokio::task::spawn_blocking(move || {
                ferb_runtimes::codex::write_locked_catalog(&home)
            })
            .await;
            Ok(())
        })
    }

    fn check_codex_version<'a>(&'a self, binary: &'a Path) -> BoxFuture<'a, Result<(), FerbError>> {
        Box::pin(async move {
            let out = tokio::process::Command::new(binary)
                .arg("--version")
                .stdin(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .output()
                .await
                .map_err(|e| FerbError::Unsupported(format!("Codex didn't run: {e}")))?;
            let text = String::from_utf8_lossy(&out.stdout).into_owned();
            if text.split_whitespace().last() != Some(CODEX_VERSION) {
                return Err(FerbError::Unsupported(format!(
                    "Codex {} is not the pinned {CODEX_VERSION}; rerun spike 1's tests before changing the pin",
                    text.trim()
                )));
            }
            Ok(())
        })
    }

    fn claude_pin_status(&self) -> String {
        match self.claude_cli() {
            Ok(p) => format!("Claude Code at {}", p.display()),
            Err(e) => format!("problem: {e}"),
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// the answer forms (schema_shape.py, ported in ferb-context) as the adapters ask for them
// ---------------------------------------------------------------------------------------------------------------

/// ferb-context's reshaping behind ferb-runtimes' `Forms` seam (two `Shape` types: see the mismatch list).
pub struct ContextForms;

fn reshape(
    r: Result<ferb_context::shape::Shape, ferb_context::shape::SchemaTooComplex>,
) -> Result<Shape, SchemaTooComplex> {
    match r {
        Ok(s) if s.mapped => {
            let held = s.clone();
            Ok(Shape::mapped(
                s.core,
                s.wire,
                Arc::new(move |answer: &Value| {
                    let py = ferb_context::pyvalue::Py::from_value(answer);
                    held.value(py).to_value()
                }),
            ))
        }
        Ok(s) => Ok(Shape::same(&s.core)),
        Err(e) => Err(SchemaTooComplex(e.0)),
    }
}

impl Forms for ContextForms {
    fn anthropic(&self, schema: &Value) -> Result<Shape, SchemaTooComplex> {
        reshape(ferb_context::shape::for_anthropic(schema))
    }
    fn gemini(&self, schema: &Value) -> Result<Shape, SchemaTooComplex> {
        reshape(ferb_context::shape::for_gemini(schema))
    }
    fn check_final(&self, text: &str, schema: &Value) -> Result<(), String> {
        ferb_context::structured::check_final(text, Some(schema))
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}

// ---------------------------------------------------------------------------------------------------------------
// the factory
// ---------------------------------------------------------------------------------------------------------------

/// The gateway in-process, as the API loop's tool bridge (`ferb/runtimes/api.py` `GatewayBridge`).
pub struct GatewayBridge {
    pub gateway: Arc<Gateway>,
    pub tokens: Arc<ferb_rpc::RunTokens>,
    pub tools: crate::compose::tools::Parts,
}

impl ferb_runtimes::api::Bridge for GatewayBridge {
    fn tools(&self, run_token: &str) -> Vec<Value> {
        match self.tokens.validate(run_token) {
            Ok(grant) => crate::compose::rpc::tools_for(&self.gateway, &self.tools, &grant),
            Err(_) => Vec::new(),
        }
    }

    fn call<'a>(
        &'a self,
        run_token: &'a str,
        name: &'a str,
        arguments: &'a Value,
    ) -> BoxFuture<'a, Result<Value, FerbError>> {
        Box::pin(async move {
            let grant = self
                .tokens
                .validate(run_token)
                .map_err(|e| FerbError::Ferb(e.to_string()))?;
            let args = arguments.as_object().cloned().unwrap_or_default();
            let call = self.tools.call(
                self.gateway.clone(),
                grant,
                name.to_string(),
                args,
                Arc::new(|| false),
            );
            match call.await {
                Ok(reply) => Ok(reply.to_json()),
                Err(e) => Err(FerbError::Ferb(format!("{e:?}"))),
            }
        })
    }
}

/// `RuntimeDeps`'s factories: an adapter for one [`Build`].
pub struct RealFactory {
    pub redactor: Arc<Redactor>,
    pub bridge: Arc<GatewayBridge>,
    pub keys: Arc<dyn Fn(String) -> BoxFuture<'static, Option<Secret>> + Send + Sync>,
    pub refusals: Arc<Refusals>,
    pub claude_cli: Option<PathBuf>,
    pub proxy: Option<Arc<dyn ferb_runtimes::claude::ToolProxy>>,
}

impl Factory for RealFactory {
    fn build(&self, b: &Build) -> Result<Built, RuntimeError> {
        let r = self.redactor.clone();
        let redact: ferb_runtimes::codex::Redact = Arc::new(move |s: &str| r.text(s));
        let none = || -> Arc<dyn Fn() -> Vec<Value> + Send + Sync> { Arc::new(Vec::new) };
        let output_schema = (!b.schema.is_null()).then(|| b.schema.clone());
        match b.name.as_str() {
            "codex" => {
                let binary = b.codex_binary.clone().ok_or_else(|| {
                    RuntimeError::unsupported("FERB's pinned Codex isn't installed")
                })?;
                let mode = if b.mode == "supervisor" {
                    ferb_runtimes::codex::Mode::Supervisor
                } else {
                    ferb_runtimes::codex::Mode::Worker
                };
                let mut rt = ferb_runtimes::codex::CodexRuntime::new(
                    binary,
                    mode,
                    b.home.clone(),
                    &b.model,
                    &b.tool_names,
                    ferb_runtimes::codex::Host::system(),
                );
                rt.shim_cmd = Some(mcp_shim_command());
                rt.output_schema = output_schema;
                rt.timeout_s = b.limit_s;
                rt.redact = redact;
                rt.cache_role = b.cache_role.clone();
                rt.cache_scope = b.cache_scope.clone();
                rt.effort = b.effort.clone();
                Ok(Built {
                    adapter: Arc::new(rt),
                    retried: none(),
                })
            }
            "claude" => {
                let proxy = self.proxy.clone().ok_or_else(|| {
                    RuntimeError::unsupported("the Claude runtime's tool-list relay isn't running")
                })?;
                let mode = if b.mode == "supervisor" {
                    ferb_runtimes::claude::Mode::Supervisor
                } else {
                    ferb_runtimes::claude::Mode::Worker
                };
                let mut rt = ferb_runtimes::claude::ClaudeRuntime::new(
                    mode,
                    proxy,
                    b.home.clone(),
                    &b.tool_names,
                    output_schema,
                    (!b.model.is_empty()).then(|| b.model.clone()),
                    Arc::new(ContextForms),
                    self.refusals.clone(),
                    ferb_runtimes::claude::Host::system(),
                );
                rt.shim_cmd = Some(mcp_shim_command());
                rt.effort = b.effort.clone();
                rt.limit_s = b.limit_s;
                rt.cli = self.claude_cli.clone();
                Ok(Built {
                    adapter: Arc::new(rt),
                    retried: none(),
                })
            }
            provider => {
                let model = (!b.model.is_empty()).then_some(b.model.as_str());
                let p: Arc<dyn ferb_runtimes::api::Provider> = match provider {
                    "anthropic" => {
                        Arc::new(ferb_runtimes::api::anthropic::AnthropicProvider::new(model))
                    }
                    "openai" => Arc::new(ferb_runtimes::api::openai::OpenAIProvider::new(model)),
                    "google" => Arc::new(ferb_runtimes::api::google::GoogleProvider::new(model)),
                    other => {
                        return Err(RuntimeError::unsupported(format!(
                            "no adapter for runtime {other}"
                        )))
                    }
                };
                let mut rt = ferb_runtimes::api::ApiRuntime::new(
                    p,
                    b.mode == "supervisor",
                    &b.role,
                    &b.tool_names,
                    output_schema,
                    Some(self.bridge.clone() as Arc<dyn ferb_runtimes::api::Bridge>),
                    self.keys.clone(),
                    Arc::new(ContextForms),
                    self.refusals.clone(),
                )
                .map_err(|e| RuntimeError::unsupported(e.0))?;
                rt.effort = b.effort.clone();
                rt.limit_s = b.limit_s;
                rt.planner = b.planner;
                // journal 24: the daemon runs its redactor on a provider's words too (rule 7)
                rt.redact = Some(redact);
                let retried = rt.retried.clone();
                Ok(Built {
                    adapter: Arc::new(rt),
                    retried: Arc::new(move || {
                        retried.lock().map(|v| v.clone()).unwrap_or_default()
                    }),
                })
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// the glue to ferb-worker's ModelRuntime
// ---------------------------------------------------------------------------------------------------------------

/// A runtime error as the worker reads it, class by class (journal 37: errors arrive as the stream's items).
pub fn worker_error(e: RuntimeError) -> WorkerError {
    match e {
        RuntimeError::Ferb(f) => WorkerError::Ferb(f),
        RuntimeError::SignedOut { message, runtime } => WorkerError::SignedOut { message, runtime },
        RuntimeError::Account { .. } => WorkerError::Unavailable {
            message: e.to_string(),
            account: e.account().to_string(),
            reason: match &e {
                RuntimeError::Account { reason, .. } => reason.clone(),
                _ => String::new(),
            },
        },
        RuntimeError::Protocol { message, .. } => WorkerError::other("ProtocolError", message),
        RuntimeError::Provider { message, .. } => WorkerError::other("ProviderError", message),
        RuntimeError::Raised(r) => WorkerError::other(&r.class, r.message),
    }
}

struct Events(ferb_runtimes::gen::EventStream);

impl EventSource for Events {
    fn next(
        &mut self,
    ) -> ferb_worker::ports::BoxFuture<'_, Option<Result<RuntimeEvent, WorkerError>>> {
        Box::pin(async move { self.0.next().await.map(|i| i.map_err(worker_error)) })
    }
}

struct Control(Arc<dyn ferb_runtimes::run::RunControl>);

impl RunControl for Control {
    fn steer<'a>(
        &'a self,
        message: &'a str,
    ) -> ferb_worker::ports::BoxFuture<'a, Result<(), ferb_orchestrator::steering::ContinueError>>
    {
        Box::pin(async move {
            self.0.steer(message).await.map_err(|e| match e {
                RuntimeError::Ferb(FerbError::Unsupported(_)) => {
                    ferb_orchestrator::steering::ContinueError::Unsupported
                }
                other => ferb_orchestrator::steering::ContinueError::Failed(other.class()),
            })
        })
    }

    fn cancel(&self) -> ferb_worker::ports::BoxFuture<'_, ()> {
        self.0.cancel()
    }
}

/// One built adapter as ferb-worker's `ModelRuntime`.
pub struct WorkerRuntime {
    pub build: Build,
    pub adapter: Arc<dyn Adapter>,
}

impl ModelRuntime for WorkerRuntime {
    fn name(&self) -> String {
        self.adapter.name().to_string()
    }

    fn limits(&self) -> Result<ModelLimits, FerbError> {
        ferb_runtimes::catalog::limits(&self.build.name, Some(&self.build.model), &self.build.home)
    }

    fn output_schema(&self) -> Option<Value> {
        (!self.build.schema.is_null()).then(|| self.build.schema.clone())
    }

    fn start<'a>(
        &'a self,
        task: &'a Task,
        request: &'a ModelRequest,
        run_token: &'a str,
    ) -> ferb_worker::ports::BoxFuture<'a, Result<StartedRun, WorkerError>> {
        Box::pin(async move {
            let started = self
                .adapter
                .start(task, request, run_token)
                .await
                .map_err(worker_error)?;
            Ok(StartedRun {
                handle: started.handle,
                events: Box::new(Events(started.events)),
                control: Arc::new(Control(started.control)),
            })
        })
    }
}

/// `make_runtime(role, mode=, tool_names=, schema=, effort=)` as the worker and the supervisor call it.
pub fn make_runtime(
    rt: &Runtimes,
    role: &str,
    mode: &str,
    tool_names: &[String],
    schema: &Value,
    effort: Option<&str>,
) -> Result<Arc<dyn ModelRuntime>, WorkerError> {
    let (build, built) = rt
        .make_runtime(role, mode, tool_names, schema, effort)
        .map_err(worker_error)?;
    Ok(Arc::new(WorkerRuntime {
        build,
        adapter: built.adapter,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_runtime_error_reaches_the_worker_by_its_python_class() {
        let e = worker_error(RuntimeError::account_error("auth_required", Some(7)));
        assert_eq!(e.class(), "RuntimeUnavailable");
        assert!(matches!(&e, WorkerError::Unavailable { account, .. } if account == "signed_out"));
        let e = worker_error(RuntimeError::SignedOut {
            message: "Claude isn't signed in".into(),
            runtime: "claude".into(),
        });
        assert_eq!(e.class(), "SignedOut");
        let e = worker_error(RuntimeError::protocol("turn failed"));
        assert_eq!(
            (e.class().as_str(), e.is_value_error()),
            ("ProtocolError", true)
        );
        assert_eq!(e.to_string(), "turn failed");
        let e = worker_error(RuntimeError::provider("rate limited"));
        assert_eq!(e.class(), "ProviderError");
        let e = worker_error(RuntimeError::unsupported("no"));
        assert_eq!(e.class(), "Unsupported");
    }

    #[test]
    fn the_tool_bridge_is_this_program_again() {
        let cmd = mcp_shim_command();
        assert_eq!(cmd.last().map(String::as_str), Some("mcp"));
        assert_eq!(cmd.len(), 2);
    }
}
