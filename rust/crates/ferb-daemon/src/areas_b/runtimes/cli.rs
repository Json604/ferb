//! ferb/cli/runtimes.py's `roles`, `status`, `check_model` and `assign` (D32): FERB's runtime table, what each role
//! uses, and pointing roles at a runtime (and its model). They read and write config.toml directly; the handlers
//! call them. (`show`, `use` and `pins` are the developer toolbox's printing and are not ported here.)

use std::path::Path;

use ferb_core::config::Config as CoreConfig;
use ferb_core::errors::FerbError;
use ferb_runtimes::catalog::{self, Config, SignInFacts, CATALOG, ROLES};
use serde_json::{json, Map, Value};

/// config.toml as the runtime table reads it. A config FERB can't read counts as empty here.
pub struct View<'a>(pub &'a CoreConfig);

impl Config for View<'_> {
    fn get(&self, section: &str, key: &str) -> Option<Value> {
        self.0.get(section, key).ok().flatten()
    }
}

/// `str(cfg.get(section, key, default))`.
pub fn text_or(cfg: &dyn Config, section: &str, key: &str, default: &str) -> String {
    cfg.get(section, key)
        .map_or_else(|| default.to_string(), |v| ferb_py::value::py_str(&v))
}

/// runtimes.py `roles`: which runtime each role uses; the checker follows the worker unless set.
pub fn roles(cfg: &dyn Config) -> Map<String, Value> {
    let worker = text_or(cfg, "runtimes", "worker", "codex");
    let mut out = Map::new();
    out.insert(
        "orchestrator".into(),
        text_or(cfg, "runtimes", "orchestrator", "codex").into(),
    );
    out.insert("worker".into(), worker.clone().into());
    out.insert(
        "checker".into(),
        text_or(cfg, "runtimes", "checker", &worker).into(),
    );
    out
}

/// runtimes.py `status`: each runtime, whether it can run here (and the fix when not), and each role's runtime.
/// Presence checks only: no key is read.
pub fn status(home: &Path, cfg: &dyn Config, facts: &dyn SignInFacts) -> Map<String, Value> {
    let rows: Vec<Value> = CATALOG
        .iter()
        .map(|rt| {
            let model = catalog::model_for(cfg, rt.name);
            let (signed, _) = catalog::signed_in(rt, home, facts);
            let (ok, fix) = catalog::available(rt, home, cfg, facts, Some(&model));
            json!({"name": rt.name, "usable": ok, "fix": fix, "signed_in": signed, "model": model})
        })
        .collect();
    let mut out = Map::new();
    out.insert("roles".into(), Value::Object(roles(cfg)));
    out.insert("runtimes".into(), Value::Array(rows));
    out
}

/// Why `assign` refused (Python's ValueError, said as it is), or an error it let through.
#[derive(Debug)]
pub enum AssignError {
    Value(String),
    Other(FerbError),
}

/// runtimes.py `check_model`: refused when FERB knows no documented limits for `model` on `runtime` (L28).
/// `bundled_catalog` dumps Codex's catalog once per pinned version (it runs the codex program).
pub fn check_model(
    home: &Path,
    runtime: &str,
    model: &str,
    bundled_catalog: &dyn Fn(),
) -> Result<(), AssignError> {
    if runtime == "codex" {
        bundled_catalog();
    }
    match catalog::limits(runtime, Some(model), home) {
        Ok(_) => Ok(()),
        Err(FerbError::ModelLimitsUnknown(e)) => Err(AssignError::Value(format!(
            "FERB can't use {} with {runtime}: {e}",
            ferb_runtimes::pyjson::repr_str(model)
        ))),
        Err(other) => Err(AssignError::Other(other)),
    }
}

/// runtimes.py `assign`: points the chosen roles at `runtime` (and its model, when given) from their next call.
/// Nothing is written when a role, the runtime or the model is refused.
pub fn assign(
    home: &Path,
    config: &CoreConfig,
    runtime: &str,
    chosen: &[String],
    model: Option<&str>,
    bundled_catalog: &dyn Fn(),
) -> Result<Map<String, Value>, AssignError> {
    let all = ROLES.join(", ");
    if chosen.is_empty() {
        return Err(AssignError::Value(format!(
            "Name at least one role; roles: {all}."
        )));
    }
    if let Some(unknown) = chosen.iter().find(|r| !ROLES.contains(&r.as_str())) {
        return Err(AssignError::Value(format!(
            "Unknown role {}; roles: {all}.",
            ferb_runtimes::pyjson::repr_str(unknown)
        )));
    }
    if catalog::entry(runtime).is_none() {
        let names: Vec<&str> = CATALOG.iter().map(|r| r.name).collect();
        return Err(AssignError::Value(format!(
            "Unknown runtime {}; runtimes: {}.",
            ferb_runtimes::pyjson::repr_str(runtime),
            names.join(", ")
        )));
    }
    let model = model.filter(|m| !m.is_empty());
    if let Some(m) = model {
        check_model(home, runtime, m, bundled_catalog)?;
    }
    for role in chosen {
        config
            .set("runtimes", role, &Value::String(runtime.to_string()))
            .map_err(|e| AssignError::Other(FerbError::Ferb(e.to_string())))?;
    }
    if let Some(m) = model {
        config
            .set("models", runtime, &Value::String(m.to_string()))
            .map_err(|e| AssignError::Other(FerbError::Ferb(e.to_string())))?;
    }
    Ok(roles(&View(config)))
}
