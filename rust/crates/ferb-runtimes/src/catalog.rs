//! The one table of model runtimes FERB can run (ferb/runtimes/catalog.py; RUN-404, RUN-423, RUN-429, RUN-460..463):
//! each runtime's name, default model, sign-in kind, billing and documented limits; the model and effort a role
//! runs with; the one overall limit on a call. Never chooses a runtime for a task.

use std::path::Path;

use ferb_core::errors::FerbError;
use ferb_core::types::ModelLimits;
use serde_json::Value;

pub const ROLES: [&str; 3] = ["orchestrator", "worker", "checker"];
/// ORCH-528: every call that isn't the planner's or a worker's has this overall limit beside the silence guard.
pub const CALL_LIMIT_S: f64 = 900.0;
/// One-turn calls that aren't the planner's, and the planner when the owner set nothing (owner 2026-11-10).
pub const ONE_TURN_EFFORT: &str = "low";

/// catalog.py `call_limit`.
pub fn call_limit(mode: &str, planner: bool) -> Option<f64> {
    if mode == "worker" || planner {
        None
    } else {
        Some(CALL_LIMIT_S)
    }
}

/// How the owner knows each runtime.
pub fn label(name: &str) -> &str {
    match name {
        "codex" => "Codex",
        "claude" => "Claude",
        "anthropic" => "the Anthropic API",
        "openai" => "the OpenAI API",
        "google" => "the Google API",
        other => other,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignIn {
    Codex,
    Claude,
    Key,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Runtime {
    pub name: &'static str,
    /// "" when the owner must choose one.
    pub default_model: &'static str,
    pub sign_in: SignIn,
    /// "subscription" | "api".
    pub billing: &'static str,
}

/// catalog.py CATALOG, in its order.
pub const CATALOG: [Runtime; 5] = [
    Runtime {
        name: "codex",
        default_model: "gpt-5.6-sol",
        sign_in: SignIn::Codex,
        billing: "subscription",
    },
    Runtime {
        name: "claude",
        default_model: "claude-opus-5-5",
        sign_in: SignIn::Claude,
        billing: "subscription",
    },
    Runtime {
        name: "anthropic",
        default_model: "claude-sonnet-5-5",
        sign_in: SignIn::Key,
        billing: "api",
    },
    Runtime {
        name: "openai",
        default_model: "gpt-6.1-sol",
        sign_in: SignIn::Key,
        billing: "api",
    },
    Runtime {
        name: "google",
        default_model: "gemini-3.8-flash",
        sign_in: SignIn::Key,
        billing: "api",
    },
];

pub fn entry(name: &str) -> Option<&'static Runtime> {
    CATALOG.iter().find(|r| r.name == name)
}

pub fn default_model(name: &str) -> &'static str {
    entry(name).map_or("", |r| r.default_model)
}

/// The runtimes that run on a provider API key.
pub fn api_providers() -> Vec<&'static str> {
    CATALOG
        .iter()
        .filter(|r| r.sign_in == SignIn::Key)
        .map(|r| r.name)
        .collect()
}

/// The owner's config.toml as the catalog reads it (ferb/config.py `Config.get`).
pub trait Config {
    fn get(&self, section: &str, key: &str) -> Option<Value>;
}

/// catalog.py `configured_model`: `[models] <name>`; Claude's older `[runtimes] claude_model` too.
pub fn configured_model(cfg: &dyn Config, name: &str) -> Option<String> {
    let mut model = cfg.get("models", name).filter(ferb_py::value::truthy);
    if model.is_none() && name == "claude" {
        model = cfg
            .get("runtimes", "claude_model")
            .filter(ferb_py::value::truthy);
    }
    model.map(|m| ferb_py::value::py_str(&m))
}

/// catalog.py `model_for`: the owner's choice, else the runtime's default ("" when neither).
pub fn model_for(cfg: &dyn Config, name: &str) -> String {
    configured_model(cfg, name).unwrap_or_else(|| default_model(name).to_string())
}

fn role_row(cfg: &dyn Config, name: &str, role: &str, field: &str) -> Option<String> {
    let chosen = cfg.get("role_models", role.split(':').next().unwrap_or_default())?;
    let Value::Object(row) = chosen else {
        return None;
    };
    if row.get("runtime").and_then(Value::as_str) != Some(name) {
        return None;
    }
    let value = ferb_py::value::py_str(row.get(field).unwrap_or(&Value::Null));
    let value = if row.get(field).is_some_and(ferb_py::value::truthy) {
        ferb_py::value::strip(&value).to_string()
    } else {
        String::new()
    };
    (!value.is_empty()).then_some(value)
}

/// catalog.py `role_model` (RUN-429).
pub fn role_model(cfg: &dyn Config, name: &str, role: &str) -> String {
    role_row(cfg, name, role, "model").unwrap_or_else(|| model_for(cfg, name))
}

/// catalog.py `configured_effort`.
pub fn configured_effort(cfg: &dyn Config, name: &str) -> String {
    match cfg.get("efforts", name) {
        Some(Value::String(s)) => ferb_py::value::strip(&s).to_string(),
        _ => String::new(),
    }
}

/// catalog.py `role_effort`.
pub fn role_effort(cfg: &dyn Config, name: &str, role: &str) -> String {
    role_row(cfg, name, role, "effort").unwrap_or_else(|| configured_effort(cfg, name))
}

/// catalog.py `run_effort` (RUN-460..462).
pub fn run_effort(
    cfg: &dyn Config,
    name: &str,
    role: &str,
    mode: &str,
    planner: bool,
    asked: Option<&str>,
) -> Option<String> {
    if mode == "supervisor" {
        if planner {
            let set = role_effort(cfg, name, "orchestrator");
            return Some(if set.is_empty() {
                ONE_TURN_EFFORT.into()
            } else {
                set
            });
        }
        return Some(ONE_TURN_EFFORT.into());
    }
    let kind = if role.split(':').next() == Some("checker") {
        "checker"
    } else {
        "worker"
    };
    let set = role_effort(cfg, name, kind);
    if !set.is_empty() {
        return Some(set);
    }
    asked.filter(|a| !a.is_empty()).map(str::to_string)
}

/// catalog.py `effort_problem` (RUN-463): argument shape only.
pub fn effort_problem(name: &str, effort: &str) -> String {
    let mut chars = effort.chars();
    let first_ok = chars.next().is_some_and(|c| c.is_ascii_alphanumeric());
    let rest_ok = effort
        .chars()
        .skip(1)
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if !first_ok || !rest_ok || effort.chars().count() > 32 {
        return "an effort level is one short word (letters, digits, - or _), at most 32 characters"
            .into();
    }
    if name == "codex" && effort.to_lowercase() == "ultra" {
        return "Codex's ultra level hands work to sub-agents, which FERB keeps off; pick another level"
            .into();
    }
    String::new()
}

/// catalog.py `sign_in_fix`.
pub fn sign_in_fix(name: &str) -> String {
    match name {
        "codex" => "sign in to ChatGPT again in Settings → AI".into(),
        "claude" => "sign in to Claude again in Settings → AI".into(),
        other => {
            let l = label(other);
            format!(
                "replace your {} key in Settings → AI",
                l.strip_prefix("the ").unwrap_or(l)
            )
        }
    }
}

/// What a runtime's sign-in check needs from the host: presence only, never a value.
pub trait SignInFacts {
    fn codex_binary(&self) -> bool;
    fn has_api_key(&self, provider: &str) -> bool;
}

/// catalog.py `Runtime.signed_in`.
pub fn signed_in(rt: &Runtime, home: &Path, facts: &dyn SignInFacts) -> (bool, String) {
    match rt.sign_in {
        SignIn::Codex => {
            if !facts.codex_binary() {
                return (
                    false,
                    "FERB's pinned Codex isn't installed: set FERB up again".into(),
                );
            }
            if crate::codex_home_exists(home) {
                (true, String::new())
            } else {
                (
                    false,
                    "Codex isn't signed in for FERB: sign in to ChatGPT in Settings → AI".into(),
                )
            }
        }
        SignIn::Claude => {
            if crate::claude_home_exists(home) {
                (true, String::new())
            } else {
                (
                    false,
                    "Claude isn't signed in for FERB: sign in to Claude in Settings → AI".into(),
                )
            }
        }
        SignIn::Key => {
            if facts.has_api_key(rt.name) {
                (true, String::new())
            } else {
                (
                    false,
                    format!("FERB has no {} API key: add one in Settings → AI", rt.name),
                )
            }
        }
    }
}

/// catalog.py `Runtime.available`.
pub fn available(
    rt: &Runtime,
    home: &Path,
    cfg: &dyn Config,
    facts: &dyn SignInFacts,
    model: Option<&str>,
) -> (bool, String) {
    let (ok, fix) = signed_in(rt, home, facts);
    if !ok {
        return (false, fix);
    }
    let chosen = match model {
        Some(m) => m.to_string(),
        None => model_for(cfg, rt.name),
    };
    if chosen.is_empty() {
        return (false, format!("no model is chosen for {}", rt.name));
    }
    (true, String::new())
}

/// catalog.py `Runtime.limits`: the documented limits of `model` (None: the default).
pub fn limits(name: &str, model: Option<&str>, home: &Path) -> Result<ModelLimits, FerbError> {
    let chosen = model
        .filter(|m| !m.is_empty())
        .unwrap_or(default_model(name));
    if chosen.is_empty() {
        return Err(FerbError::ModelLimitsUnknown(format!(
            "no model is chosen for {name}, so FERB doesn't know its context window"
        )));
    }
    match name {
        "codex" => crate::codex_limits(home, chosen),
        "claude" => crate::claude_limits(chosen),
        "anthropic" => Ok(crate::api::anthropic::limits_for(chosen)),
        "openai" => Ok(crate::api::openai::limits_for(chosen)),
        "google" => Ok(crate::api::google::limits_for(chosen)),
        other => Err(FerbError::ModelLimitsUnknown(format!(
            "no runtime named {other}"
        ))),
    }
}
