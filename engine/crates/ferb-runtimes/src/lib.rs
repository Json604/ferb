//! FERB's model runtimes and the models' tool bridge (ferb/runtimes, ferb/cli/mcp.py): the pinned Codex app-server,
//! the bundled Claude Code driven over its stream-json protocol without the Python SDK (ENG-2), FERB's own tool
//! loop over the Anthropic, OpenAI and Gemini APIs, the runtime table, and the MCP stdio server the model programs
//! call FERB's tools through (AGENTS.md rule 12: every tool call goes through FERB's gateway).
//!
//! Checked against fixtures the Python adapters wrote while driving scripted fake programs and servers
//! (engine/fixtures/runtimes, ENG-3).
//!
//! The adapters' own shape is [`run::Started`]: an event stream the caller pulls (as Python's async generator is
//! pulled) and a control handle for steering and cancel that can be used while the stream is read. The
//! `ferb_core::interfaces::AgentRuntime` trait is implemented on top of it by [`run::CoreRuntime`].
//!
//! Mac-side (it starts programs and owns Unix sockets): the crate is empty on other targets.
#![cfg(unix)]

pub mod answer_rules;
pub mod api;
pub mod catalog;
pub mod claude;
pub mod codex;
pub mod compact;
pub mod error;
pub mod forms;
pub mod gen;
pub mod keep;
pub mod mcp_bridge;
pub mod models;
pub mod prices;
pub mod process;
pub mod pyjson;
pub mod run;
pub mod tooltrace;
pub mod usage;
pub mod worker;

pub use error::{Raised, RuntimeError};

/// Seconds on a clock that only goes forward, counted from its first use in this process: what every adapter's
/// call timing reads unless a test gives it another clock (TEST-508).
///
/// decision: RUN-419, RUN-420
pub fn monotonic_now() -> f64 {
    use std::sync::OnceLock;
    use std::time::Instant;
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f64()
}

fn codex_home_exists(home: &std::path::Path) -> bool {
    codex::codex_home(home).is_some()
}

fn claude_home_exists(home: &std::path::Path) -> bool {
    claude::claude_home(home).is_some()
}

fn codex_limits(
    home: &std::path::Path,
    model: &str,
) -> Result<ferb_core::types::ModelLimits, ferb_core::errors::FerbError> {
    codex::limits_for(home, model)
}

fn claude_limits(
    model: &str,
) -> Result<ferb_core::types::ModelLimits, ferb_core::errors::FerbError> {
    claude::limits_for(Some(model))
}
