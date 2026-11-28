//! FERB.app's control of the phone link (stage 3 S4, kb T-1204): Settings → Phone reaches the link's helper only
//! through the daemon (kb PHN-441: the helper has no socket of its own; one peer check, FERB.app's, guards the owner's
//! side). The daemon answers what it holds itself (whether the phone feature is on, the paired phones' limit) and asks
//! the helper the rest over the helper's own `link.events` stream: an `ask` event with an id, answered by the helper's
//! `link.answer`. What only the helper knows: the pairing code (the QR's one-time secret and the Mac's endpoint), each
//! connection's path, the relay setting and its test.
//!
//! The process itself is ferbd's (`LinkControl`): it runs the helper while the phone feature is on, which the owner's
//! first action on the link (a pairing, a relay) turns on and only `phone.enable {on: false}` turns off. The relay
//! token is a secret (rule 7): it goes from FERB.app to the helper inside the ask, comes back to the daemon alone (the
//! helper's answer names it under `keys`), is kept in the link's one Keychain item through `LinkControl`
//! (kb PHN-143), and is never in a reply, a log, the journal or the database.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use ferb_rpc::phone_gate;
use ferb_rpc::server::{HandlerError, HandlerResult};
use ferb_rpc::{Params, Server, Session};
use serde_json::{json, Map, Value};
use tokio::sync::oneshot;

use super::{lock, Phones};

/// The helper process as ferbd runs it.
pub trait LinkControl: Send + Sync {
    /// The phone feature is on: the helper should run.
    fn enabled(&self) -> bool;
    /// Turns the phone feature on or off (ferbd starts or stops the helper; the setting is kept).
    fn set_enabled(&self, on: bool) -> Result<(), String>;
    /// Keeps the owner's relay token in the link's Keychain item (`None`: removes it). Never logged.
    fn keep_relay_token(&self, token: Option<&[u8]>) -> Result<(), String>;
}

/// How long an ask waits for the helper: the relay test dials the relay (its own timeouts are under this).
const ASK_WAIT: Duration = Duration::from_secs(40);
/// How long a pairing's start waits for a helper that was just turned on to connect.
const START_WAIT: Duration = Duration::from_secs(20);

#[derive(Default)]
pub(crate) struct Asks {
    next: u64,
    waiting: HashMap<String, oneshot::Sender<Value>>,
}

fn plain(message: impl Into<String>) -> HandlerError {
    HandlerError::coded(ferb_rpc::wire::FAILED, message)
}

/// The helper's refusal as FERB.app reads it: plain words with what to do, never the code in front (the code stays
/// apart, as the JSON-RPC number where it has one).
fn from_link(e: &Value) -> HandlerError {
    let mut words = e["message"]
        .as_str()
        .unwrap_or("The phone link refused that")
        .trim()
        .to_owned();
    if !words.ends_with(['.', '!', '?']) {
        words.push('.');
    }
    if let Some(allow) = e["allow"].as_str().map(str::trim).filter(|a| !a.is_empty()) {
        words.push(' ');
        words.push_str(allow.trim_end_matches('.'));
        words.push('.');
    }
    let code = match e["code"].as_str() {
        Some("device_limit") => phone_gate::DEVICE_LIMIT,
        Some("bad_request") => phone_gate::BAD_REQUEST,
        _ => ferb_rpc::wire::FAILED,
    };
    HandlerError::coded(code, words)
}

impl Phones {
    /// Puts ferbd's control of the helper in place (none in a daemon that runs no helper: the app's link calls say so).
    pub fn set_link(&self, control: Arc<dyn LinkControl>) {
        *lock(&self.inner.link) = Some(control);
    }

    fn control(&self) -> Option<Arc<dyn LinkControl>> {
        lock(&self.inner.link).clone()
    }

    /// A helper listens on `link.events` now.
    pub fn helper_connected(&self) -> bool {
        self.inner.events.receiver_count() > 0
    }

    /// Asks the helper; its answer (`result`) or its refusal.
    pub async fn ask(&self, what: &str, params: Value) -> Result<Value, HandlerError> {
        if !self.helper_connected() {
            return Err(plain(
                "The phone link isn't running. Turn on Phone in Settings, or wait a moment while it starts.",
            ));
        }
        let (tx, rx) = oneshot::channel();
        let id = {
            let mut asks = lock(&self.inner.asks);
            asks.next += 1;
            let id = format!("ask-{}", asks.next);
            asks.waiting.insert(id.clone(), tx);
            id
        };
        let _ = self
            .inner
            .events
            .send(json!({"event": "ask", "ask_id": id, "what": what, "params": params}));
        let answer = tokio::time::timeout(ASK_WAIT, rx).await;
        lock(&self.inner.asks).waiting.remove(&id);
        match answer {
            Ok(Ok(v)) => match v.get("error").filter(|e| !e.is_null()) {
                Some(e) => Err(from_link(e)),
                None => Ok(v.get("result").cloned().unwrap_or(Value::Null)),
            },
            _ => Err(plain(
                "The phone link didn't answer. Try again in a moment.",
            )),
        }
    }

    /// `link.answer {ask_id, result | error}`: the helper's answer to an ask.
    pub fn answer(&self, params: &Params) -> HandlerResult {
        let id = params
            .get("ask_id")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let found = lock(&self.inner.asks).waiting.remove(id);
        let taken = match found {
            Some(tx) => tx.send(Value::Object(params.clone())).is_ok(),
            None => false,
        };
        let mut out = Params::new();
        out.insert("taken".into(), json!(taken));
        Ok(out)
    }

    /// The feature on (the owner acted on the link), then a helper connected, within `START_WAIT`.
    async fn running(&self) -> Result<(), HandlerError> {
        let Some(control) = self.control() else {
            return Err(plain("This FERB has no phone link."));
        };
        if !control.enabled() {
            control.set_enabled(true).map_err(plain)?;
        }
        let end = tokio::time::Instant::now() + START_WAIT;
        while !self.helper_connected() {
            if tokio::time::Instant::now() >= end {
                return Err(plain(
                    "The phone link didn't start. Try again in a moment; if it keeps happening, FERB's log says why.",
                ));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        Ok(())
    }

    /// `phone.enable {on}`: the phone feature on or off. Off stops the helper: paired phones stay paired and can't
    /// connect until it is on again.
    pub fn enable(&self, params: &Params) -> HandlerResult {
        let on = params.get("on").and_then(Value::as_bool) == Some(true);
        let control = self
            .control()
            .ok_or_else(|| plain("This FERB has no phone link."))?;
        control.set_enabled(on).map_err(plain)?;
        (self.inner.seams.journal)("phone.link_enabled", json!({"on": on}));
        let mut out = Params::new();
        out.insert("enabled".into(), json!(on));
        Ok(out)
    }

    /// `phone.link_status`: whether the feature is on and the helper runs; with it, the relay in use, the owner's
    /// relay (its address; `has_token`, never the token), Always use my relay, and how each phone is connected now.
    pub async fn link_status(&self) -> HandlerResult {
        let enabled = self.control().is_some_and(|c| c.enabled());
        let running = self.helper_connected();
        let mut out = Params::new();
        out.insert("enabled".into(), json!(enabled));
        out.insert("running".into(), json!(running));
        if !running {
            out.insert("relay".into(), json!({"state": "off", "connected": false}));
            out.insert("own_relay".into(), Value::Null);
            out.insert("own_relay_always".into(), json!(false));
            out.insert("devices".into(), json!([]));
            return Ok(out);
        }
        let st = self.ask("status", json!({})).await?;
        out.insert("relay".into(), st["relay"]["state"].clone());
        let own = st["own_relay"].get("url").map(
            |url| json!({"url": url, "has_token": st["own_relay"]["has_token"] == json!(true)}),
        );
        out.insert("own_relay".into(), own.unwrap_or(Value::Null));
        out.insert(
            "own_relay_always".into(),
            json!(st["own_relay_always"] == json!(true)),
        );
        let devices: Vec<Value> = st["devices"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|d| {
                let online = d["online"] == json!(true);
                json!({"id": d["id"], "online": online,
                       "tier": if online { d["tier"].clone() } else { json!("offline") }})
            })
            .collect();
        out.insert("devices".into(), Value::Array(devices));
        out.insert("pairing".into(), st["pairing"].clone());
        Ok(out)
    }

    /// `phone.pair_start {update?}`: a new pairing code (the QR's text, the text to type, the seconds it lasts), refused
    /// at the daemon's limit before a code is made. Turns the phone feature on. With `update` (a paired phone's id,
    /// PHN-849) the code only lets that phone replace its fingerprint key: no limit applies, and it pairs no phone.
    pub async fn code_start(&self) -> HandlerResult {
        self.code_start_for(&Params::new()).await
    }
    /// [`Self::code_start`] with the call's params (`update`).
    pub async fn code_start_for(&self, params: &Params) -> HandlerResult {
        let update = params
            .get("update")
            .and_then(Value::as_str)
            .filter(|u| !u.is_empty());
        let live = {
            let db = lock(&self.inner.seams.db);
            ferb_store::phones::live(db.conn())
                .map_err(|e| HandlerError::fault("OperationalError", e.to_string()))?
        };
        if let Some(id) = update {
            match live.iter().find(|p| p.endpoint == id) {
                None => {
                    return Err(HandlerError::coded(
                        phone_gate::NOT_FOUND,
                        "That phone is not paired with this Mac.",
                    ))
                }
                Some(p) if p.strong_pub.is_none() => {
                    return Err(HandlerError::coded(
                        phone_gate::BAD_REQUEST,
                        "That phone has no fingerprint key to update.",
                    ))
                }
                Some(_) => {}
            }
            self.running().await?;
            let v = self.ask("pair_start", json!({"update": id})).await?;
            let mut out = Params::new();
            for k in ["code", "url", "qr", "seconds"] {
                out.insert(k.into(), v[k].clone());
            }
            return Ok(out);
        }
        let live = live.len();
        let max = self.inner.limits.devices_max;
        if live >= max {
            return Err(HandlerError::coded(
                phone_gate::DEVICE_LIMIT,
                format!(
                    "FERB already has {max} paired phones, the most it keeps. Remove one first."
                ),
            ));
        }
        self.running().await?;
        let v = self.ask("pair_start", json!({})).await?;
        let mut out = Params::new();
        for k in ["code", "url", "qr", "seconds"] {
            out.insert(k.into(), v[k].clone());
        }
        Ok(out)
    }

    /// `phone.pair_cancel`: the code is withdrawn (and a phone waiting for the owner's Yes is told No).
    pub async fn code_cancel(&self) -> HandlerResult {
        if self.helper_connected() {
            self.ask("pair_cancel", json!({})).await?;
        }
        let mut out = Params::new();
        out.insert("ok".into(), json!(true));
        Ok(out)
    }

    /// `phone.relay_test {url, token?, ca?}`: what Test says (each check that passed, each note), or why it failed in
    /// plain words. Saves nothing.
    pub async fn relay_test(&self, params: &Params) -> HandlerResult {
        self.running().await?;
        let mut v = self.ask("relay_test", relay_params(params, false)?).await?;
        v["reachable"] = json!(true);
        Ok(object(v))
    }

    /// `phone.relay_set {url, token?, ca?} | {off: true}`: tested by the helper first and used only when the test
    /// passes (PHN-205); the token is kept in the link's Keychain item and never returned (`has_token` says it is).
    pub async fn relay_set(&self, params: &Params) -> HandlerResult {
        self.running().await?;
        let off = params.get("off").and_then(Value::as_bool) == Some(true);
        let mut v = self.ask("relay_set", relay_params(params, off)?).await?;
        let token = v
            .get_mut("keys")
            .and_then(Value::as_object_mut)
            .and_then(|k| k.remove("relay-token"));
        if let Some(obj) = v.as_object_mut() {
            obj.remove("keys");
        }
        let bytes = token.as_ref().and_then(Value::as_str).and_then(|t| {
            use base64::Engine;
            base64::engine::general_purpose::STANDARD.decode(t).ok()
        });
        if let Some(control) = self.control() {
            control
                .keep_relay_token(bytes.as_deref())
                .map_err(|e| plain(format!("Your relay is in use, but its token couldn't be kept for the next start ({e}).")))?;
        }
        v["has_token"] = json!(bytes.is_some());
        (self.inner.seams.journal)("phone.relay_set", json!({"on": !off}));
        Ok(object(v))
    }

    /// `phone.relay_always {on}`: Always use my relay (PHN-219, PHN-453): on, the link drops direct paths while the
    /// owner's relay is in use.
    pub async fn relay_always(&self, params: &Params) -> HandlerResult {
        self.running().await?;
        let on = params.get("on").and_then(Value::as_bool) == Some(true);
        let v = self.ask("always", json!({"on": on})).await?;
        Ok(object(v))
    }

    pub(crate) fn register_link(&self, server: &Server) {
        let me = self.clone();
        server.register_unary("link.answer", move |_s: Session, a: Params| {
            let me = me.clone();
            async move { me.answer(&a) }
        });
        let me = self.clone();
        server.register_unary("phone.enable", move |_s: Session, a: Params| {
            let me = me.clone();
            async move { me.enable(&a) }
        });
        let me = self.clone();
        server.register_unary("phone.link_status", move |_s: Session, _a: Params| {
            let me = me.clone();
            async move { me.link_status().await }
        });
        let me = self.clone();
        server.register_unary("phone.pair_start", move |_s: Session, a: Params| {
            let me = me.clone();
            async move { me.code_start_for(&a).await }
        });
        let me = self.clone();
        server.register_unary("phone.pair_cancel", move |_s: Session, _a: Params| {
            let me = me.clone();
            async move { me.code_cancel().await }
        });
        let me = self.clone();
        server.register_unary("phone.relay_test", move |_s: Session, a: Params| {
            let me = me.clone();
            async move { me.relay_test(&a).await }
        });
        let me = self.clone();
        server.register_unary("phone.relay_set", move |_s: Session, a: Params| {
            let me = me.clone();
            async move { me.relay_set(&a).await }
        });
        let me = self.clone();
        server.register_unary("phone.relay_always", move |_s: Session, a: Params| {
            let me = me.clone();
            async move { me.relay_always(&a).await }
        });
    }
}

/// The relay fields FERB.app may send, and nothing else.
fn relay_params(params: &Params, off: bool) -> Result<Value, HandlerError> {
    if off {
        return Ok(json!({"off": true}));
    }
    let text = |k: &str| params.get(k).and_then(Value::as_str).map(str::trim);
    let Some(url) = text("url").filter(|u| !u.is_empty()) else {
        return Err(HandlerError::coded(
            phone_gate::BAD_REQUEST,
            "Paste the line your relay's setup printed, or its address and token.",
        ));
    };
    let mut p = Map::new();
    p.insert("url".into(), json!(url));
    for k in ["token", "ca"] {
        if let Some(v) = text(k).filter(|v| !v.is_empty()) {
            p.insert(k.into(), json!(v));
        }
    }
    Ok(Value::Object(p))
}

fn object(v: Value) -> Params {
    match v {
        Value::Object(m) => m,
        other => {
            let mut m = Params::new();
            m.insert("result".into(), other);
            m
        }
    }
}
