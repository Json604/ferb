//! The RPC handlers no ported area owns (`areas/rpc.py`, `areas/chat.py`): `ping`, the tool bridge's `tools.list`
//! and `tools.call`, the global stop, `update.prepare`. The rest of `areas/rpc.py`'s handlers are `crate::areas_c`.

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
    pub supervisor: ferb_supervisor::flow::Supervisor,
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
        let (stop, gw, j, ev, tokens, sup) = (
            p.stop.clone(),
            p.gateway.clone(),
            p.journal.clone(),
            p.events.clone(),
            p.tokens.clone(),
            p.supervisor.clone(),
        );
        server.register_unary("stop.engage", move |_s: Session, _p: Params| {
            let (stop, gw, j, ev, tokens, sup) = (
                stop.clone(),
                gw.clone(),
                j.clone(),
                ev.clone(),
                tokens.clone(),
                sup.clone(),
            );
            async move {
                // stop.py `engage`: block first (nothing new starts), then revoke, then the hooks compose.py:189-190
                // registers: the supervisor's `cancel_all` and the identity browser's (FERB's own browser holds no
                // run of its own in this build)
                stop.set(true)
                    .map_err(|e| HandlerError::fault("OSError", e.to_string()))?;
                let mut control = ferb_gateway::stop::StopControl::new(
                    move || tokens.revoke_all(),
                    move |on| gw.set_stopped(on),
                );
                control.register(
                    "runtimes",
                    Box::new(move || {
                        sup.cancel_all();
                        Ok(())
                    }),
                );
                control.register("identity_browser", Box::new(|| Ok(())));
                let summary = Value::Object(control.engage());
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
    // update.prepare (rpc.py:112): ready only when nothing runs that a restart would cut, unless the owner chose Quit
    // and Install (`even_if_busy`); when ready, nothing new starts and the daemon shuts down
    {
        let (j, quit, sup, tools) = (
            p.journal.clone(),
            p.quit.clone(),
            p.supervisor.clone(),
            p.tools.clone(),
        );
        server.register_unary("update.prepare", move |_s: Session, params: Params| {
            let (j, quit, sup, tools) = (j.clone(), quit.clone(), sup.clone(), tools.clone());
            async move {
                let running = sup.update_blockers();
                if !running.is_empty() && params.get("even_if_busy") != Some(&Value::Bool(true)) {
                    return Ok(crate::basics::object(
                        json!({"ready": false, "running": running}),
                    ));
                }
                tools.set_shutting_down();
                sup.hold_new_work();
                let version: String = params
                    .get("version")
                    .map(ferb_py::value::py_str)
                    .unwrap_or_default()
                    .chars()
                    .take(40)
                    .collect();
                let interrupted: Vec<Value> = running
                    .iter()
                    .map(|r| r.get("task_id").cloned().unwrap_or(Value::Null))
                    .collect();
                j.note(
                    "update.prepared",
                    json!({"version": version, "interrupted": interrupted}),
                );
                quit.notify_one();
                Ok(crate::basics::object(
                    json!({"ready": true, "running": running}),
                ))
            }
        });
    }
    let _ = BTreeSet::<String>::new();
}
