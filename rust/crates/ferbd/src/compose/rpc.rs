//! The RPC handlers no ported area owns (`areas/rpc.py`, `areas/chat.py`): `ping`, the tool bridge's `tools.list`
//! and `tools.call`, the global stop, `update.prepare`; and the plain refusal of the methods whose handlers are not
//! ported yet, so a caller learns that rather than an unknown method.

use std::collections::BTreeSet;
use std::sync::Arc;

use ferb_core::interfaces::Clock;
use ferb_gateway::gateway::Gateway;
use ferb_rpc::auth::RunGrant;
use ferb_rpc::{HandlerError, Params, Server, Session};
use serde_json::{json, Map, Value};

use crate::basics::{Events, Journal, Stop};

/// What these handlers use.
pub struct Parts {
    pub clock: Arc<dyn Clock>,
    pub stop: Arc<Stop>,
    pub gateway: Arc<Gateway>,
    pub tools: super::tools::Parts,
    pub journal: Arc<Journal>,
    pub events: Arc<Events>,
    pub tokens: Arc<ferb_rpc::RunTokens>,
    pub quit: Arc<tokio::sync::Notify>,
}

/// Methods of `auth::METHODS` whose Python handlers aren't ported to Rust yet (beyond the supervisor's): each
/// answers [`NOT_PORTED`].
pub const NOT_PORTED_UNARY: &[&str] = &[
    "doctor.check",
    "setup.permissions",
    "setup.permission_request",
    "setup.finish",
    "clipboard.request",
    "automation.request",
    "ui.click_through",
    "identity.check",
    "conversations.list",
    "conversations.change",
    "tasks.preview",
    "questions.picture",
    "reports.build",
    "reports.diagnostic",
    "reports.send",
    "recovery.list",
    "recovery.restore",
    "trace.get",
    "verdict.set",
];
pub const NOT_PORTED_STREAMS: &[&str] = &["logs.subscribe"];

pub const NOT_PORTED: &str =
    "not available in this build: this part of FERB's daemon isn't in the Rust build yet";

fn not_ported() -> HandlerError {
    HandlerError::refusal("FerbError", NOT_PORTED)
}

fn ferb_error(message: &str) -> HandlerError {
    HandlerError::refusal("FerbError", message)
}

/// The listing a grant is offered (`gateway.tools_for(grant)`), with the descriptions as they read now.
pub fn tools_for(gateway: &Gateway, tools: &super::tools::Parts, grant: &RunGrant) -> Vec<Value> {
    let live = tools.live();
    ferb_gateway::listing::tools_for(&gateway.registry(), grant, &live)
}

pub fn register(server: &Server, p: &Parts) {
    // ping (rpc.py:190): {pid, role, now, stopped}
    {
        let clock = p.clock.clone();
        let stop = p.stop.clone();
        server.register_unary(
            "ping",
            ferb_rpc::server::ping(
                move || ferb_core::when::isoformat(&clock.now()),
                move || stop.engaged(),
            ),
        );
    }
    // tools.list / tools.call (chat.py:181-189): the shim's run token is its grant
    {
        let gw = p.gateway.clone();
        let tools = p.tools.clone();
        server.register_unary("tools.list", move |session: Session, _params: Params| {
            let (gw, tools) = (gw.clone(), tools.clone());
            async move {
                let grant = session.grant.ok_or_else(|| ferb_error("no run token"))?;
                Ok(crate::basics::object(
                    json!({"tools": tools_for(&gw, &tools, &grant)}),
                ))
            }
        });
        let gw = p.gateway.clone();
        let parts = p.tools.clone();
        server.register_unary("tools.call", move |session: Session, params: Params| {
            let (gw, parts) = (gw.clone(), parts.clone());
            async move {
                let grant = session.grant.ok_or_else(|| ferb_error("no run token"))?;
                let name = match params.get("name") {
                    Some(Value::String(s)) => s.clone(),
                    Some(other) => ferb_py::value::py_str(other),
                    None => "None".into(),
                };
                let args: Map<String, Value> = match params.get("arguments") {
                    Some(Value::Object(m)) => m.clone(),
                    _ => Map::new(),
                };
                let left = ferb_rpc::server::caller_left_flag();
                let left: ferb_gateway::gateway::Left =
                    Arc::new(move || left.as_ref().is_some_and(|l| l.is_set()));
                match parts.call(gw, grant, name, args, left).await {
                    Ok(reply) => Ok(crate::basics::object(reply.to_json())),
                    Err(ferb_gateway::gateway::CallError::Cancelled) => {
                        Err(HandlerError::fault("CancelledError", ""))
                    }
                    Err(ferb_gateway::gateway::CallError::Internal(m)) => {
                        Err(HandlerError::fault("RuntimeError", m))
                    }
                }
            }
        });
    }
    // the global stop (rpc.py:376-386): every run token revoked, the gateway refuses, the marker survives a restart
    {
        let (stop, gw, j, ev, tokens) = (
            p.stop.clone(),
            p.gateway.clone(),
            p.journal.clone(),
            p.events.clone(),
            p.tokens.clone(),
        );
        server.register_unary("stop.engage", move |_s: Session, _p: Params| {
            let (stop, gw, j, ev, tokens) = (
                stop.clone(),
                gw.clone(),
                j.clone(),
                ev.clone(),
                tokens.clone(),
            );
            async move {
                // stop.py `engage`: block first (nothing new starts), then revoke, then the hooks (none in this
                // build: the supervisor's `runtimes` and the identity browser's are not wired here)
                stop.set(true)
                    .map_err(|e| HandlerError::fault("OSError", e.to_string()))?;
                gw.set_stopped(true);
                let revoked = tokens.revoke_all();
                let summary = json!({"tokens_revoked": revoked, "hooks": 0, "hook_errors": {}});
                j.note("stop.engaged", summary.clone());
                ev.publish("stop.engaged", crate::basics::object(summary.clone()));
                Ok(crate::basics::object(summary))
            }
        });
        let (stop, gw, j, ev) = (
            p.stop.clone(),
            p.gateway.clone(),
            p.journal.clone(),
            p.events.clone(),
        );
        server.register_unary("stop.lift", move |_s: Session, _p: Params| {
            let (stop, gw, j, ev) = (stop.clone(), gw.clone(), j.clone(), ev.clone());
            async move {
                stop.set(false)
                    .map_err(|e| HandlerError::fault("OSError", e.to_string()))?;
                gw.set_stopped(false);
                j.note("stop.lifted", json!({}));
                ev.publish("stop.lifted", Map::new());
                Ok(crate::basics::object(json!({"stopped": false})))
            }
        });
    }
    // update.prepare (rpc.py:112): nothing runs that a restart would cut in this build (no supervisor), so the
    // daemon is ready at once and shuts down
    {
        let (j, quit) = (p.journal.clone(), p.quit.clone());
        server.register_unary("update.prepare", move |_s: Session, params: Params| {
            let (j, quit) = (j.clone(), quit.clone());
            async move {
                let version: String = params
                    .get("version")
                    .map(ferb_py::value::py_str)
                    .unwrap_or_default()
                    .chars()
                    .take(40)
                    .collect();
                j.note(
                    "update.prepared",
                    json!({"version": version, "interrupted": Vec::<String>::new()}),
                );
                quit.notify_one();
                Ok(crate::basics::object(json!({"ready": true, "running": []})))
            }
        });
    }
    for m in NOT_PORTED_UNARY {
        server.register_unary(m, |_s, _p: Params| async { Err(not_ported()) });
    }
    for m in NOT_PORTED_STREAMS {
        server.register_stream(m, |_s, _p: Params| {
            let (tx, rx) = tokio::sync::mpsc::channel(1);
            let _ = tx.try_send(Err(not_ported()));
            Box::new(rx) as Box<dyn ferb_rpc::server::EventStream>
        });
    }
    let _ = BTreeSet::<String>::new();
}
