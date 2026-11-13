//! ferb/orchestrator/stop.py: the emergency stop. Order is the contract: block (nothing new starts), revoke every
//! run token, then run each registered hook; one failing hook never stops the rest.

/// A stop hook: `Err(type name)` when it failed (Python records `type(e).__name__`).
pub type StopHook = Box<dyn Fn() -> Result<(), String> + Send + Sync>;

/// stop.py:20 `StopControl`.
pub struct StopControl {
    pub revoke_all_tokens: Box<dyn Fn() -> i64 + Send + Sync>,
    pub set_blocked: Box<dyn Fn(bool) + Send + Sync>,
    pub hooks: Vec<(String, StopHook)>,
    pub engaged: bool,
}

/// What `engage` reports: `{"tokens_revoked", "hooks", "hook_errors"}`.
#[derive(Debug, Clone, PartialEq)]
pub struct Engaged {
    pub tokens_revoked: i64,
    pub hooks: usize,
    /// hook name -> the error's type name, in hook order.
    pub hook_errors: Vec<(String, String)>,
}

impl Engaged {
    pub fn to_json(&self) -> serde_json::Value {
        let errors: serde_json::Map<String, serde_json::Value> = self
            .hook_errors
            .iter()
            .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
            .collect();
        serde_json::json!({"tokens_revoked": self.tokens_revoked, "hooks": self.hooks, "hook_errors": errors})
    }
}

impl StopControl {
    pub fn new(
        revoke_all_tokens: impl Fn() -> i64 + Send + Sync + 'static,
        set_blocked: impl Fn(bool) + Send + Sync + 'static,
    ) -> StopControl {
        StopControl {
            revoke_all_tokens: Box::new(revoke_all_tokens),
            set_blocked: Box::new(set_blocked),
            hooks: Vec::new(),
            engaged: false,
        }
    }

    pub fn register(&mut self, name: &str, hook: StopHook) {
        self.hooks.push((name.to_string(), hook));
    }

    pub fn engage(&mut self) -> Engaged {
        (self.set_blocked)(true);
        self.engaged = true;
        let revoked = (self.revoke_all_tokens)();
        let mut errors: Vec<(String, String)> = Vec::new();
        for (name, hook) in &self.hooks {
            if let Err(kind) = hook() {
                // a dict: a later hook of the same name replaces the earlier entry in place
                if let Some(slot) = errors.iter_mut().find(|(n, _)| n == name) {
                    slot.1 = kind;
                } else {
                    errors.push((name.clone(), kind));
                }
            }
        }
        Engaged {
            tokens_revoked: revoked,
            hooks: self.hooks.len(),
            hook_errors: errors,
        }
    }

    pub fn lift(&mut self) {
        (self.set_blocked)(false);
        self.engaged = false;
    }
}
