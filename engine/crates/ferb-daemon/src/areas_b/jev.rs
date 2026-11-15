//! Jev's shadow wiring (D23, D33: advice only). Port of `ferb/daemon/areas/jev.py`. Must not let Jev's answer
//! authorize or refuse an action.
//!
//! Jev is OFF (`[jev] on`, false by default; SENSE-2): while it is off nothing here reads a key, builds a decider or
//! calls Jev, at start or on `jev.reload` (`build` returns before any of that, jev.py:37-46). The decider itself
//! (`ShadowDecider`, ported in ferb-orchestrator's `decide`) and the Keychain read come through [`JevSeams`]; this
//! area only switches and reports.

use std::sync::{Arc, Mutex};

use ferb_rpc::{BoxFuture, HandlerResult, Server};
use serde_json::{json, Map, Value};

use super::core::{ok, refusal, Host};

/// A running decider (`self.jev`): what the RPCs read of it.
pub trait Decider: Send + Sync {
    /// `jev.stats()`.
    fn stats(&self) -> Result<Vec<Map<String, Value>>, String>;
    /// `jev.dropped`.
    fn dropped(&self) -> i64;
    /// `jev.flagged_refs(question)`.
    fn flagged_refs(&self, question: &str) -> Result<Vec<String>, String>;
    /// The last 20 decisions not skipped as "not sampled", newest first: (Jev's answer, error).
    fn recent(&self) -> Result<Vec<(Value, Option<String>)>, String>;
    /// The source of a flagged ref now: `trail:<id>` → "source: data_json", `span:<id>` → its input_json; "" gone.
    fn source_text(&self, kind: &str, id: &str) -> Result<String, String>;
    /// `jev.client.ask("FERB setup check", …, 20)`: Err with Jev's words when unavailable.
    fn ping(&self) -> BoxFuture<'static, Result<(), String>>;
    fn close(&self) {}
}

/// What only the composition can do: read the key, build the decider, wire it to the distiller and the trail.
pub trait JevSeams: Send + Sync {
    /// `_jev_key`: the Keychain (service "ferb", account jev/api_key); None when absent. Called only while on.
    fn key(&self) -> BoxFuture<'static, Option<String>>;
    /// A `ShadowDecider` over the store with that key.
    fn build(&self, key: String) -> Arc<dyn Decider>;
    /// `_wire_jev(jev)`.
    fn wire(&self, jev: Option<Arc<dyn Decider>>);
}

pub struct Deps {
    pub seams: Arc<dyn JevSeams>,
    /// `self.jev`.
    pub jev: Arc<Mutex<Option<Arc<dyn Decider>>>>,
}

/// ferb/decide/questions.py `QUESTIONS`: (id, live, shadow_only).
fn question(q: &str) -> Option<(bool, bool)> {
    ferb_orchestrator::decide::questions(&|_| String::new())
        .into_iter()
        .find(|x| x.id == q)
        .map(|x| (x.live, x.shadow_only))
}

fn question_ids() -> Vec<String> {
    ferb_orchestrator::decide::questions(&|_| String::new())
        .into_iter()
        .map(|x| x.id.to_string())
        .collect()
}

/// `_jev_on`: the one switch.
pub fn jev_on(h: &Host) -> Result<bool, ferb_rpc::HandlerError> {
    Ok(h.get("jev", "on")? == Some(Value::Bool(true)))
}

fn enabled_list(h: &Host) -> Result<Vec<String>, ferb_rpc::HandlerError> {
    Ok(match h.get("jev", "enabled")? {
        Some(Value::Array(a)) => a.iter().map(ferb_py::value::py_str).collect(),
        _ => vec![],
    })
}

/// `_build_jev`: Jev joins only with the switch on and its key in the Keychain; otherwise every hook is skipped.
pub async fn build(h: &Host, d: &Deps) -> Result<(), ferb_rpc::HandlerError> {
    let old = d.jev.lock().unwrap().take();
    if let Some(old) = old {
        old.close();
    }
    if !jev_on(h)? {
        d.seams.wire(None);
        return Ok(());
    }
    let Some(key) = d.seams.key().await else {
        d.seams.wire(None);
        return Ok(());
    };
    let jev = d.seams.build(key);
    *d.jev.lock().unwrap() = Some(jev.clone());
    d.seams.wire(Some(jev));
    Ok(())
}

pub fn register(server: &Server, host: &Host, deps: &Deps) {
    let (h, jev) = (host.clone(), deps.jev.clone());
    server.register_unary("jev.stats", move |_s, _p: Map<String, Value>| {
        let (h, jev) = (h.clone(), jev.lock().unwrap().clone());
        async move { stats(&h, jev) }
    });
    let (h, jev) = (host.clone(), deps.jev.clone());
    server.register_unary("jev.enable", move |_s, p: Map<String, Value>| {
        let (h, jev) = (h.clone(), jev.lock().unwrap().is_some());
        async move { enable(&h, jev, &p) }
    });
    let (h, seams, jev) = (host.clone(), deps.seams.clone(), deps.jev.clone());
    server.register_unary("jev.reload", move |_s, p: Map<String, Value>| {
        let d = Deps {
            seams: seams.clone(),
            jev: jev.clone(),
        };
        let h = h.clone();
        async move { reload(&h, &d, &p).await }
    });
}

fn fault(e: String) -> ferb_rpc::HandlerError {
    ferb_rpc::HandlerError::fault("DatabaseError", e)
}

/// `_jev_stats`.
fn stats(h: &Host, jev: Option<Arc<dyn Decider>>) -> HandlerResult {
    let Some(jev) = jev else {
        return ok(json!({"configured": false, "on": jev_on(h)?, "stats": []}));
    };
    let mut stats = jev.stats().map_err(fault)?;
    for st in stats.iter_mut() {
        if st.get("answered").is_some_and(ferb_py::value::truthy) {
            let q = st
                .get("question")
                .map(ferb_py::value::py_str)
                .unwrap_or_default();
            st.insert("examples".into(), json!(examples(h, jev.as_ref(), &q)?));
        }
    }
    let recent = jev.recent().map_err(fault)?;
    let answered = recent.iter().filter(|(j, _)| !j.is_null()).count();
    let last_error = recent
        .iter()
        .find_map(|(_, e)| e.clone().filter(|e| !e.is_empty()));
    let known = question_ids();
    let mut retired: Vec<String> = enabled_list(h)?
        .into_iter()
        .filter(|q| !known.contains(q))
        .collect();
    retired.sort();
    retired.dedup();
    ok(
        json!({"configured": true, "on": true, "dropped": jev.dropped(), "stats": stats,
              "recent_answered": answered, "recent_total": recent.len(), "last_error": last_error,
              "retired_enabled": retired}),
    )
}

/// `_jev_examples`: a few items Jev flagged, read from their source now (redacted).
fn examples(
    h: &Host,
    jev: &dyn Decider,
    question: &str,
) -> Result<Vec<String>, ferb_rpc::HandlerError> {
    let mut out = vec![];
    for r in jev.flagged_refs(question).map_err(fault)? {
        let (kind, id) = r.split_once(':').unwrap_or((&r, ""));
        let text = if kind == "trail" || kind == "span" {
            jev.source_text(kind, id).map_err(fault)?
        } else {
            r.clone()
        };
        if !text.is_empty() {
            out.push(h.redact_text(&text));
        }
    }
    Ok(out)
}

/// `_jev_enable`. ENG-6: a missing `question` was Python's KeyError (an internal error); it is refused here.
fn enable(h: &Host, have_jev: bool, p: &Map<String, Value>) -> HandlerResult {
    let Some(q) = p.get("question") else {
        return Err(refusal("name the question to switch on or off"));
    };
    let q = ferb_py::value::py_str(q);
    let Some((live, shadow_only)) = question(&q) else {
        return Err(refusal(format!("unknown question {q}")));
    };
    let on = p.get("on").is_none_or(ferb_py::value::truthy);
    if live {
        return Err(refusal(format!(
            "{q} is live by the plan of record (Q4); it has no switch to turn on or off"
        )));
    }
    if shadow_only && on {
        return Err(refusal(format!(
            "{q} is measured in shadow; it cannot act before owner-reviewed measurement"
        )));
    }
    let known = question_ids();
    let mut enabled: Vec<String> = enabled_list(h)?
        .into_iter()
        .filter(|x| known.contains(x))
        .collect();
    if on {
        if !jev_on(h)? {
            return Err(refusal(
                "Jev is switched off ([jev] on = false in config.toml)",
            ));
        }
        if !have_jev {
            return Err(refusal(
                "Jev isn't set up: there's no Jev key in the Keychain",
            ));
        }
        enabled.push(q.clone());
    } else {
        enabled.retain(|x| *x != q);
    }
    enabled.sort();
    enabled.dedup();
    h.set("jev", "enabled", &json!(enabled))?;
    h.journal(
        if on { "jev.enabled" } else { "jev.disabled" },
        json!({"question": q}),
        None,
    )?;
    ok(json!({"enabled": enabled}))
}

/// `_jev_reload`.
async fn reload(h: &Host, d: &Deps, p: &Map<String, Value>) -> HandlerResult {
    build(h, d).await?;
    let jev = d.jev.lock().unwrap().clone();
    let mut out = Map::new();
    out.insert("configured".into(), json!(jev.is_some()));
    if let Some(jev) = jev {
        if p.get("ping").is_some_and(ferb_py::value::truthy) {
            let answer = match jev.ping().await {
                Ok(()) => "ok".to_owned(),
                Err(e) => e,
            };
            out.insert("ping".into(), json!(answer));
        }
    }
    Ok(out)
}
