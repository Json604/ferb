//! Test-only (feature `stand-ins`): `areas_c`'s stand-ins, as `rust/fixtures/daemon/make.py` gave its Python
//! daemon. Always: no report reaches the developer's deployed relay (conftest's `no_report_relay`). With
//! `"group_c": "recorder"` in `FERBD_STAND_INS`: the recorder's invented Mac (make.py `GRANTS`, a denied prompt,
//! the pasteboard at `default`, Automation unknown, no self-test) and its versions (FERB 0.27.3 on macOS 26.1).

use std::sync::Arc;

use ferb_core::interfaces::BoxFuture;
use serde_json::{json, Map, Value};

use crate::areas_c::{Deps, Mac};

/// make.py's invented Mac.
pub struct RecorderMac;

impl Mac for RecorderMac {
    fn grants(&self) -> Map<String, Value> {
        crate::basics::object(
            json!({"accessibility": "granted", "screen_recording": "granted", "input_monitoring": "granted",
                   "full_disk_access": "denied"}),
        )
    }
    fn request(&self, _permission: &str) -> String {
        "denied".into()
    }
    fn clipboard_access(&self) -> Option<String> {
        Some("default".into())
    }
    fn clipboard_request(&self) -> String {
        "default".into()
    }
    fn automation(&self, _bundle_id: &str, _ask: bool) -> String {
        "unknown".into()
    }
    fn selftest(&self) -> Vec<Value> {
        Vec::new()
    }
    fn fresh_grants(&self) -> BoxFuture<'static, Map<String, Value>> {
        Box::pin(async { Map::new() })
    }
    fn running(&self) -> Vec<String> {
        Vec::new()
    }
    fn click_through(&self, _pid: i32, _windows: &[i64]) -> bool {
        false
    }
}

/// The controls: `claude_relay` registers a run with the Claude tool-list relay and answers its base URL.
pub fn control(d: &Arc<crate::compose::Daemon>, req: &Value, what: &str) -> Result<Value, String> {
    match what {
        "claude_relay" => {
            let proxy = d.claude_proxy.as_ref().ok_or("the relay isn't running")?;
            Ok(json!(
                proxy.base_url(req["run_id"].as_str().unwrap_or("run-1"))
            ))
        }
        other => Err(format!("unknown control {other:?}")),
    }
}

pub fn group_c(config: &Value, d: &mut Deps) {
    d.relay_default = String::new();
    if config["group_c"] == "recorder" {
        d.mac = Arc::new(RecorderMac);
        d.facts.version = "0.27.3".into();
        d.facts.macos = "26.1".into();
    }
}
