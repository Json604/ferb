//! Emergency stop (`ferb/orchestrator/stop.py`): revoke every run token, block new actions at the gateway, cancel
//! what is in flight, and run every registered stop hook (workers, debuggers, task-owned processes register as they
//! are built). Must not infer intent or choose a target.
//!
//! Order is the contract (`stop.py:22-34`): block first (nothing new starts), then revoke, then the hooks. One
//! failing hook never stops the rest. A stop of named tasks is `Gateway::cancel_tasks` with their ids; an empty list
//! there stops nothing (it never means "everything": that is only this global stop, DMN-1).

use serde_json::{Map, Value};

/// A registered hook: its name, and the work (an error's type name when it fails).
pub type Hook = Box<dyn Fn() -> Result<(), String> + Send + Sync>;

/// `StopControl`.
pub struct StopControl {
    revoke_all_tokens: Box<dyn Fn() -> usize + Send + Sync>,
    set_blocked: Box<dyn Fn(bool) + Send + Sync>,
    hooks: Vec<(String, Hook)>,
    pub engaged: bool,
}

impl StopControl {
    pub fn new(
        revoke_all_tokens: impl Fn() -> usize + Send + Sync + 'static,
        set_blocked: impl Fn(bool) + Send + Sync + 'static,
    ) -> StopControl {
        StopControl {
            revoke_all_tokens: Box::new(revoke_all_tokens),
            set_blocked: Box::new(set_blocked),
            hooks: Vec::new(),
            engaged: false,
        }
    }

    pub fn register(&mut self, name: &str, hook: Hook) {
        self.hooks.push((name.to_string(), hook));
    }

    /// `engage()`: `{"tokens_revoked", "hooks", "hook_errors"}`.
    pub fn engage(&mut self) -> Map<String, Value> {
        (self.set_blocked)(true);
        self.engaged = true;
        let revoked = (self.revoke_all_tokens)();
        let mut errors = Map::new();
        for (name, hook) in &self.hooks {
            if let Err(class) = hook() {
                errors.insert(name.clone(), Value::String(class));
            }
        }
        let mut out = Map::new();
        out.insert(
            "tokens_revoked".into(),
            Value::Number((revoked as u64).into()),
        );
        out.insert(
            "hooks".into(),
            Value::Number((self.hooks.len() as u64).into()),
        );
        out.insert("hook_errors".into(), Value::Object(errors));
        out
    }

    /// `lift()`.
    pub fn lift(&mut self) {
        (self.set_blocked)(false);
        self.engaged = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    // decision: DMN-1
    #[test]
    fn block_then_revoke_then_hooks_and_a_failing_hook_never_stops_the_rest() {
        let log = Arc::new(Mutex::new(Vec::<String>::new()));
        let (a, b) = (log.clone(), log.clone());
        let mut s = StopControl::new(
            move || {
                a.lock().unwrap().push("revoke".into());
                3
            },
            move |on| b.lock().unwrap().push(format!("blocked {on}")),
        );
        let (c, d) = (log.clone(), log.clone());
        s.register(
            "one",
            Box::new(move || {
                c.lock().unwrap().push("one".into());
                Err("RuntimeError".into())
            }),
        );
        s.register(
            "two",
            Box::new(move || {
                d.lock().unwrap().push("two".into());
                Ok(())
            }),
        );
        let out = s.engage();
        assert_eq!(
            *log.lock().unwrap(),
            vec!["blocked true", "revoke", "one", "two"]
        );
        assert_eq!(
            ferb_py::json::dumps(&out),
            r#"{"tokens_revoked": 3, "hooks": 2, "hook_errors": {"one": "RuntimeError"}}"#
        );
        assert!(s.engaged);
        s.lift();
        assert!(!s.engaged);
        assert_eq!(log.lock().unwrap().last().unwrap(), "blocked false");
    }
}
