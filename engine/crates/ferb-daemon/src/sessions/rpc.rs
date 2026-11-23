//! The `sessions.*` calls (kb PHN-797): FERB.app's own, a phone's by their classes (ferb-rpc `classes`), and the hook
//! program's `sessions.hook`.

use ferb_rpc::{HandlerError, Params, Server, Session as RpcSession};
use ferb_sessions::model::Agent;
use serde_json::{json, Map, Value};

use super::{list_json, session_json, Caller, NeedsYes, Refused, Sent, Sessions};
use crate::shared::text;

fn err(r: Refused) -> HandlerError {
    HandlerError::coded(r.code, r.message)
}

fn caller(s: &RpcSession) -> Caller {
    match &s.phone {
        Some(p) => Caller::Phone(p.device.clone()),
        None => Caller::Mac,
    }
}

fn obj(v: Value) -> Map<String, Value> {
    match v {
        Value::Object(m) => m,
        _ => Map::new(),
    }
}

fn sent_json(s: &Sent) -> Map<String, Value> {
    obj(
        json!({"sent": true, "path": s.path.as_str(), "delivered": s.delivered, "seconds": s.seconds}),
    )
}

fn needs_json(n: &NeedsYes) -> Map<String, Value> {
    obj(json!({"needs_yes": {"question": n.question, "detail": n.detail, "covers": n.covers}}))
}

fn bad(msg: &str) -> HandlerError {
    HandlerError::coded(ferb_rpc::phone_gate::BAD_REQUEST, msg)
}

/// A new terminal's command: `command` (HAND-695); an older phone's `agent` and first words become the agent's
/// program with them (that phone showed exactly that).
fn command_of(p: &Params) -> Result<String, HandlerError> {
    let agent = text(p, "agent");
    if agent.is_empty() || agent == "terminal" {
        return Ok(text(p, "command"));
    }
    Agent::parse(&agent)
        .and_then(|a| Sessions::legacy_command(a, &text(p, "text")))
        .ok_or_else(|| bad("agent must be terminal"))
}

fn num(p: &Params, k: &str) -> u64 {
    p.get(k).and_then(Value::as_u64).unwrap_or(0)
}

/// Registers the calls.
pub fn register(me: &Sessions, server: &Server) {
    let m = me.clone();
    server.register_unary("sessions.list", move |s: RpcSession, _p: Params| {
        let m = m.clone();
        async move { Ok(list_json(&m, &caller(&s))) }
    });

    let m = me.clone();
    server.register_unary("sessions.timeline", move |s: RpcSession, p: Params| {
        let m = m.clone();
        async move {
            let by = caller(&s);
            let id = text(&p, "session");
            let limit = p.get("limit").and_then(Value::as_u64).unwrap_or(50) as usize;
            let before = num(&p, "before");
            let after = num(&p, "after");
            let out = tokio::task::spawn_blocking(move || {
                if before > 0 {
                    m.earlier(&id, before, limit).map(|(items, more)| {
                        obj(json!({"session": m.session(&id).map(|x| session_json(&x, &by)), "items": items, "more": more, "cards": []}))
                    })
                } else {
                    m.timeline(&id, after, limit).map(|(items, more, cards)| {
                        obj(json!({"session": m.session(&id).map(|x| session_json(&x, &by)), "items": items, "more": more, "cards": cards}))
                    })
                }
            })
            .await
            .map_err(|e| HandlerError::fault("RuntimeError", e.to_string()))?;
            out.map_err(err)
        }
    });

    // the owner's words into a session: his first typed send asks (GATE-622)
    let m = me.clone();
    server.register_unary("sessions.send", move |s: RpcSession, p: Params| {
        let m = m.clone();
        async move {
            let id = text(&p, "session");
            let words = text(&p, "text");
            let by = caller(&s);
            m.prime(&id).await;
            if let Some(n) = m.needs_yes(&id, &words, &by).map_err(err)? {
                return Ok(needs_json(&n));
            }
            m.send(&id, &words, &by)
                .await
                .map(|x| sent_json(&x))
                .map_err(err)
        }
    });

    // the owner's Yes: control of this session from here, and the text he was asked about
    let m = me.clone();
    server.register_unary("sessions.control", move |s: RpcSession, p: Params| {
        let m = m.clone();
        async move {
            let id = text(&p, "session");
            let words = text(&p, "text");
            let by = caller(&s);
            m.take_control(&id, &by).map_err(err)?;
            // HAND-683: the terminal he chose for it, signed with his Yes (one the session offered)
            let terminal = text(&p, "terminal");
            if !terminal.is_empty() {
                m.choose_terminal(&id, &terminal).map_err(err)?;
            }
            if words.trim().is_empty() {
                return Ok(obj(json!({"control": by.key()})));
            }
            let mut out = m
                .send(&id, &words, &by)
                .await
                .map(|x| sent_json(&x))
                .map_err(err)?;
            out.insert("control".into(), json!(by.key()));
            Ok(out)
        }
    });

    let m = me.clone();
    server.register_unary("sessions.release", move |s: RpcSession, p: Params| {
        let m = m.clone();
        async move {
            let id = text(&p, "session");
            let n = m.release_control((!id.is_empty()).then_some(id.as_str()), &caller(&s));
            Ok(obj(json!({"released": n})))
        }
    });

    let m = me.clone();
    server.register_unary("sessions.answer", move |s: RpcSession, p: Params| {
        let m = m.clone();
        async move {
            let by = caller(&s);
            m.resolve(
                &text(&p, "session"),
                &text(&p, "card"),
                &text(&p, "choice"),
                Some(&by),
            )
            .map_err(err)?;
            Ok(obj(json!({"answered": true})))
        }
    });

    let m = me.clone();
    server.register_unary("sessions.interrupt", move |s: RpcSession, p: Params| {
        let m = m.clone();
        async move {
            m.interrupt(&text(&p, "session"), &caller(&s))
                .await
                .map_err(err)?;
            Ok(obj(json!({"interrupted": true})))
        }
    });

    // decision: HAND-695 — a new session is a terminal: the folders to choose from and the app it opens in; with a
    // folder and a command, the exact command and its question (GATE-585). No command: nothing is typed, nothing asked.
    let m = me.clone();
    server.register_unary("sessions.prepare", move |_s, p: Params| {
        let m = m.clone();
        async move {
            let folder = text(&p, "folder");
            let (_, app) = m.start_host();
            let mut out = obj(json!({"folders": m.folders(), "host": app}));
            if !folder.is_empty() {
                let command = command_of(&p)?;
                if let Some(n) = m.start_question(&folder, &command).map_err(err)? {
                    out.insert(
                        "needs_yes".into(),
                        json!({"question": n.question, "detail": n.detail, "covers": n.covers}),
                    );
                }
            }
            Ok(out)
        }
    });

    let m = me.clone();
    server.register_unary("sessions.start", move |s: RpcSession, p: Params| {
        let m = m.clone();
        async move {
            let command = command_of(&p)?;
            let (id, app) = m
                .start_terminal(&text(&p, "folder"), &command, &caller(&s))
                .await
                .map_err(err)?;
            Ok(obj(
                json!({"started": true, "command": command, "session": id, "host": app}),
            ))
        }
    });

    let m = me.clone();
    server.register_unary("sessions.delegate", move |s: RpcSession, p: Params| {
        let m = m.clone();
        async move {
            let client = match &s.phone {
                Some(ph) => Some(format!("phone:{}", ph.device)),
                None => p
                    .get("client_id")
                    .and_then(Value::as_str)
                    .filter(|c| !c.is_empty())
                    .map(str::to_owned),
            };
            let words = m
                .delegate(&text(&p, "session"), &text(&p, "goal"), client)
                .map_err(err)?;
            Ok(obj(json!({"handed_over": true, "message": words})))
        }
    });

    let m = me.clone();
    server.register_unary("sessions.remove_hooks", move |_s, _p: Params| {
        let m = m.clone();
        async move {
            let removed = tokio::task::spawn_blocking(move || m.remove_hooks())
                .await
                .map_err(|e| HandlerError::fault("RuntimeError", e.to_string()))?
                .map_err(crate::shared::refusal)?;
            Ok(obj(json!({"removed": removed})))
        }
    });

    // decision: HAND-662 — Claude Code's hook: answered at once unless it holds a card for the owner; the call ends with
    // the hook's process (sessions.hook is in CANCEL_WHEN_LEFT), and Claude Code's own dialog then decides
    let m = me.clone();
    server.register_unary("sessions.hook", move |_s, p: Params| {
        let m = m.clone();
        async move {
            let payload = p.get("event").cloned().unwrap_or(Value::Null);
            let ancestors: Vec<i32> = p
                .get("ancestors")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_i64)
                        .map(|x| x as i32)
                        .collect()
                })
                .unwrap_or_default();
            let reply = m.hook(&payload, &ancestors).await;
            Ok(obj(json!({"reply": reply})))
        }
    });
}
