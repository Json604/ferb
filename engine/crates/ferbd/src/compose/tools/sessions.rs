//! The owner's coding-agent sessions as FERB's task sees them (ORCH-687): `agent_session.list`, `.read`, `.send` and
//! `.answer` (catalog additions after `mac.clipboard`). A hand-over is the owner's chat message; the task that takes it
//! uses these. Waiting for the agent is code (`send`'s wait: no model call until its turn is over). Typing into a
//! terminal or an app's window asks the owner first, as any terminal input does (GATE-585: the tool's
//! `terminal_input`, one Yes per app per task). Code refuses what the task may never do: answer a permission prompt
//! (HAND-663), type while the agent asks something, type when the agent isn't its terminal's front program (HAND-658).

use std::sync::Arc;

use ferb_core::interfaces::BoxFuture;
use ferb_core::types::{ActionSpec, Target, TerminalInput, ToolContext, ToolResult};
use ferb_daemon::sessions::{Caller, Sessions};
use ferb_gateway::tool::{Args, Tool, ToolError, ToolOutput};
use serde_json::{json, Value};

use super::Parts;

/// How long `send` waits for the agent by default, in minutes (wait.for's own default, ORCH-632).
pub const WAIT_DEFAULT_MIN: f64 = ferb_clock::wait::DEFAULT_LIMIT_MIN;
/// How many items `read` gives by default.
pub const READ_DEFAULT: usize = 30;

fn s(a: &Args, k: &str) -> String {
    a.get(k).and_then(Value::as_str).unwrap_or("").to_owned()
}

fn result(v: Value) -> ToolOutput {
    ToolResult {
        content: crate::basics::object(v),
        // what the agents wrote is outside content (rule 9)
        read_sources: vec![("agent_session".into(), String::new())],
        ..ToolResult::default()
    }
    .into()
}

fn area(p: &Parts) -> Result<Sessions, ToolError> {
    let s = p.sessions().ok_or_else(|| {
        ToolError::refused("FERB doesn't see the owner's coding sessions in this build")
    })?;
    if !s.enabled() {
        return Err(ToolError::refused(
            "the owner hasn't turned on FERB's sight of his coding sessions (Settings > Privacy)",
        ));
    }
    Ok(s)
}

/// The task's tree (the hand-over shows on the session until it ends).
fn tree(p: &Parts, ctx: &ToolContext) -> String {
    p.supervisor()
        .and_then(|s| s.inner().tree_of(&ctx.task_id))
        .unwrap_or_else(|| ctx.task_id.clone())
}

fn session_value(s: &ferb_sessions::Session) -> Value {
    json!({
        "session": s.id, "agent": s.agent.name(), "folder": s.cwd, "title": s.title,
        "state": s.status.as_str(), "waiting": match s.waiting {
            ferb_sessions::Waiting::None => "none",
            ferb_sessions::Waiting::Permission => "permission",
            ferb_sessions::Waiting::Question => "question",
        },
        "app": s.host.as_ref().map(|h| h.name.clone()).unwrap_or_default(),
        "can_send": !s.send_paths.is_empty(), "last": s.last_text,
        "program": s.program, "window": s.window,
    })
}

/// One of the four tools.
pub struct SessionTool {
    name: &'static str,
    parts: Parts,
}

impl Tool for SessionTool {
    fn name(&self) -> &str {
        self.name
    }

    fn describe(&self, args: &Args) -> ActionSpec {
        match self.name {
            "agent_session.send" | "agent_session.answer" => {
                let mut spec = ActionSpec::new("mac_input");
                spec.target = Target {
                    item: Some(s(args, "session")),
                    ..Target::default()
                };
                spec.content = Some(s(
                    args,
                    if self.name.ends_with("send") {
                        "text"
                    } else {
                        "choice"
                    },
                ));
                spec
            }
            _ => ActionSpec::new("read"),
        }
    }

    /// GATE-585: a typed send is a terminal input (asked with the exact text; one Yes per app per task). The agent's
    /// own interface (Codex's daemon) types into nothing.
    fn terminal_input<'a>(
        &'a self,
        args: &'a Args,
        _task_id: &'a str,
    ) -> BoxFuture<'a, Option<TerminalInput>> {
        Box::pin(async move {
            if self.name != "agent_session.send" {
                return None;
            }
            let s = self.parts.sessions()?.session(&s(args, "session"))?;
            let first = *s.send_paths.first()?;
            if !first.types() {
                return None;
            }
            let host = s.host.clone().unwrap_or_default();
            Some(TerminalInput {
                app: if host.name.is_empty() {
                    s.agent.name().to_owned()
                } else {
                    host.name
                },
                bundle_id: host.bundle_id,
                // HAND-675: an app chat's own message box is said as the app's chat, not a terminal
                surface: if s.agent == ferb_sessions::Agent::AppChat {
                    "its message box".into()
                } else {
                    "terminal".into()
                },
                chat: s.agent == ferb_sessions::Agent::AppChat,
                text: Some(
                    args.get("text")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned(),
                ),
                enter: true,
                window: s.title.clone(),
                // HAND-692: a terminal is named by the program in front in it
                label: format!(
                    "{} in {}",
                    if s.agent == ferb_sessions::Agent::Terminal && !s.program.is_empty() {
                        s.program.as_str()
                    } else {
                        s.agent.name()
                    },
                    ferb_daemon::sessions::folder_name(&s.cwd)
                ),
                ..TerminalInput::default()
            })
        })
    }

    fn run<'a>(
        &'a self,
        args: &'a Args,
        ctx: &'a ToolContext,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let a = area(&self.parts)?;
            match self.name {
                "agent_session.list" => {
                    let list: Vec<Value> = a.list().iter().map(session_value).collect();
                    Ok(result(json!({"sessions": list})))
                }
                "agent_session.read" => {
                    let id = s(args, "session");
                    let after = args.get("after").and_then(Value::as_u64).unwrap_or(0);
                    let limit = args
                        .get("limit")
                        .and_then(Value::as_u64)
                        .map_or(READ_DEFAULT, |n| n as usize);
                    let a2 = a.clone();
                    let id2 = id.clone();
                    let (items, more, cards) =
                        tokio::task::spawn_blocking(move || a2.timeline(&id2, after, limit))
                            .await
                            .map_err(|e| ToolError::failed("RuntimeError", e.to_string()))?
                            .map_err(|r| ToolError::refused(r.message))?;
                    let s = a
                        .session(&id)
                        .map(|s| session_value(&s))
                        .unwrap_or(Value::Null);
                    Ok(result(
                        json!({"session": s, "items": items, "more": more, "asking": cards}),
                    ))
                }
                "agent_session.send" => {
                    let id = s(args, "session");
                    let text = s(args, "text");
                    let tree = tree(&self.parts, ctx);
                    let by = Caller::Task(tree.clone());
                    let before = a
                        .timeline(&id, 0, 1)
                        .map(|(i, _, _)| i.last().map_or(0, |x| x.seq))
                        .unwrap_or(0);
                    let sent = a
                        .send(&id, &text, &by)
                        .await
                        .map_err(|r| ToolError::refused(r.message))?;
                    a.task_touched(&id, &tree);
                    let wait = args.get("wait").and_then(Value::as_bool).unwrap_or(true);
                    let mut out = json!({"sent": true, "delivered": sent.delivered, "path": sent.path.as_str()});
                    if !sent.delivered {
                        out["note"] = json!(
                            "FERB couldn't see the message arrive in the agent's own record yet."
                        );
                    }
                    if wait {
                        let limit = args
                            .get("limit_minutes")
                            .and_then(Value::as_f64)
                            .unwrap_or(WAIT_DEFAULT_MIN)
                            .clamp(0.1, 24.0 * 60.0);
                        let now = a.wait_turn(&id, limit * 60.0).await;
                        let a2 = a.clone();
                        let id2 = id.clone();
                        let replies =
                            tokio::task::spawn_blocking(move || a2.timeline(&id2, before, 100))
                                .await
                                .ok()
                                .and_then(Result::ok);
                        out["session"] = now
                            .as_ref()
                            .map(session_value)
                            .unwrap_or(json!({"state": "ended"}));
                        if let Some((items, _, cards)) = replies {
                            out["new"] = json!(items);
                            out["asking"] = json!(cards);
                        }
                    }
                    Ok(result(out))
                }
                "agent_session.answer" => {
                    let tree = tree(&self.parts, ctx);
                    a.resolve(
                        &s(args, "session"),
                        &s(args, "card"),
                        &s(args, "choice"),
                        Some(&Caller::Task(tree)),
                    )
                    .map_err(|r| ToolError::refused(r.message))?;
                    Ok(result(json!({"answered": true})))
                }
                _ => Err(ToolError::refused("no such tool")),
            }
        })
    }
}

/// The four tools, in catalog order.
pub fn tools(p: &Parts) -> Vec<Arc<dyn Tool>> {
    [
        "agent_session.list",
        "agent_session.read",
        "agent_session.send",
        "agent_session.answer",
    ]
    .into_iter()
    .map(|name| {
        Arc::new(SessionTool {
            name,
            parts: p.clone(),
        }) as Arc<dyn Tool>
    })
    .collect()
}
