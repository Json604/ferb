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
                    // a stop cancelled the call: Python's CancelledError closes the bridge's connection, no line
                    Err(ferb_gateway::gateway::CallError::Cancelled) => {
                        Err(HandlerError::cancelled())
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
        let (stop, gw, j, ev, tokens, sup, tools) = (
            p.stop.clone(),
            p.gateway.clone(),
            p.journal.clone(),
            p.events.clone(),
            p.tokens.clone(),
            p.supervisor.clone(),
            p.tools.clone(),
        );
        server.register_unary("stop.engage", move |_s: Session, _p: Params| {
            let (stop, gw, j, ev, tokens, sup, tools) = (
                stop.clone(),
                gw.clone(),
                j.clone(),
                ev.clone(),
                tokens.clone(),
                sup.clone(),
                tools.clone(),
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
                // compose.py:190 `_stop_identity_browser` (browsers.py:93): hearing, recordings, both browser
                // drivers and the shell's task-owned processes stop, in the background
                {
                    let t = tools.clone();
                    control.register(
                        "identity_browser",
                        Box::new(move || {
                            stop_everything(&t);
                            Ok(())
                        }),
                    );
                }
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
                let redactor = tools.redactor();
                let running = update_blockers(
                    sup.update_blockers(),
                    &tools.shells().map(|s| s.list()).unwrap_or_default(),
                    tools.mac().is_some_and(|m| m.transcriber.busy()),
                    &|t: &str| redactor.text(t),
                );
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
                // DMN-401: FERB's hearing runs from the app the update replaces
                if let Some(m) = tools.mac() {
                    let (transcriber, folder) = (m.transcriber.clone(), m.hearing.folder.clone());
                    let _ = tokio::task::spawn_blocking(move || {
                        ferb_daemon::areas_b::voice::quit_voice_app(
                            &transcriber,
                            &folder,
                            &ferb_sense::transcribe::hearing_pids_now,
                            &|pid| {
                                // SAFETY: kill takes a pid and a signal number, no pointers.
                                unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
                            },
                        )
                    })
                    .await;
                }
                quit.notify_one();
                Ok(crate::basics::object(
                    json!({"ready": true, "running": running}),
                ))
            }
        });
    }
    let _ = BTreeSet::<String>::new();
}

/// browsers.py:93 `_stop_identity_browser` (the global stop's hook): hearing and recordings end, both browser drivers
/// let their tabs go, and the shell's task-owned processes are killed, each in the background.
pub fn stop_everything(tools: &super::tools::Parts) {
    let Ok(rt) = tokio::runtime::Handle::try_current() else {
        return;
    };
    if let Some(m) = tools.mac() {
        let (h, r) = (m.hearing.clone(), m.recordings.clone());
        rt.spawn(async move { h.shutdown().await });
        rt.spawn(async move { r.shutdown().await });
    }
    for d in [tools.ferb_driver(), tools.owner_driver()]
        .into_iter()
        .flatten()
    {
        rt.spawn(async move { d.shutdown().await });
    }
    if let Some(s) = tools.shells() {
        rt.spawn(async move {
            s.kill_all().await;
        });
    }
}

/// rpc.py:97 `_update_blockers`: what an update must wait for: the supervisor's running trees, each FERB process still
/// running whose tree isn't listed already (a long-lived server outlives its task; a restart would kill it), and a
/// transcription in flight that no listed task covers (a setup or doctor probe, DMN-401).
pub fn update_blockers(
    trees: Vec<Value>,
    processes: &[ferb_shell::manager::ProcInfo],
    transcribing: bool,
    redact: &dyn Fn(&str) -> String,
) -> Vec<Value> {
    let mut running = trees;
    let listed: BTreeSet<String> = running
        .iter()
        .map(|r| {
            r.get("task_id")
                .map(ferb_py::value::py_str)
                .unwrap_or_default()
        })
        .collect();
    for p in processes {
        let root = p.owner_task.split('.').next().unwrap_or("");
        if p.running && !listed.contains(root) {
            let said: String = redact(&p.command).chars().take(120).collect();
            running.push(json!({"task_id": p.owner_task, "goal": format!("process: {said}")}));
        }
    }
    if running.is_empty() && transcribing {
        running.push(json!({"task_id": "", "goal": "turning captured sound into text"}));
    }
    running
}

#[cfg(test)]
mod update_tests {
    use super::update_blockers;
    use serde_json::json;

    fn proc(owner: &str, command: &str, running: bool) -> ferb_shell::manager::ProcInfo {
        ferb_shell::manager::ProcInfo {
            id: "p1".into(),
            owner_task: owner.into(),
            command: command.into(),
            cwd: "/tmp".into(),
            started: 0.0,
            running,
            exit_code: None,
            long_lived: true,
            network: false,
            capture_error: None,
            sandboxed: true,
            lifted: None,
            read_path: None,
            sandbox_ms: None,
            direct: Vec::new(),
            stopped: None,
        }
    }

    /// rpc.py:97-110 (DMN-401): a FERB process still running holds an update unless its tree is listed already, by
    /// its redacted command cut at 120 characters; a finished one doesn't; a transcription holds it only when nothing
    /// else does.
    // decision: DMN-401
    #[test]
    fn a_running_process_and_a_transcription_hold_an_update() {
        let up = |s: &str| s.to_uppercase();
        let long = "x".repeat(200);
        let got = update_blockers(
            vec![json!({"task_id": "t1", "goal": "a tree"})],
            &[
                proc("t1.s2", "serve the tree's site", true),
                proc("t9.s1", &long, true),
                proc("t8", "done already", false),
            ],
            true,
            &up,
        );
        assert_eq!(
            got,
            vec![
                json!({"task_id": "t1", "goal": "a tree"}),
                json!({"task_id": "t9.s1", "goal": format!("process: {}", "X".repeat(120))}),
            ]
        );
        assert_eq!(
            update_blockers(vec![], &[], true, &up),
            vec![json!({"task_id": "", "goal": "turning captured sound into text"})]
        );
        assert!(update_blockers(vec![], &[proc("t8", "x", false)], false, &up).is_empty());
    }
}
