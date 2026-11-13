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
use ferb_daemon::areas_b::runtimes::{Build, Built, Factory, Programs};
use ferb_gateway::gateway::Gateway;
use ferb_identity::Secret;
use ferb_runtimes::error::RuntimeError;
use ferb_runtimes::forms::Refusals;
use ferb_sanitize::redact::Redactor;
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
                    Arc::new(ferb_runtimes::forms::ContextForms),
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
                    Arc::new(ferb_runtimes::forms::ContextForms),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tool_bridge_is_this_program_again() {
        let cmd = mcp_shim_command();
        assert_eq!(cmd.last().map(String::as_str), Some("mcp"));
        assert_eq!(cmd.len(), 2);
    }
}
