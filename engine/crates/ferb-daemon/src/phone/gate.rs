//! The phone gate: sessions, one-time challenges, the unlock window, the checks every class needs, the answer rules,
//! and what a phone gets back. A port of the phone lab's `standin/phone.py` `Gate` and the class dispatch of its
//! `standin/daemon.py` `connection` (ferb-phone-link@57b3f34, read, not run), on the engine's class table
//! (`ferb_rpc::classes`), with the owner's later decisions: routine questions answered from the notification without
//! the fingerprint (PHN-417), and the six on-screen methods (PHN-117, PHN-120).

use std::collections::HashMap;
use std::sync::Arc;

use ferb_rpc::auth::{PHONE_FILES, PHONE_SESSION};
use ferb_rpc::classes::{self, Class, Locked};
use ferb_rpc::phone_gate::{self, PhoneGate, PhoneSession, Refusal};
use ferb_rpc::server::HandlerResult;
use ferb_rpc::{wire, Params};
use ferb_store::phones::Phone;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use super::{lock, proof, refused, Inner, Limits, ScreenPrompt};

/// What a locked connection is told of a question: that it is there and which kind, never its words (`KIND_WORDS`).
const HIDDEN_WORDS: &str = "FERB needs a private answer";
const CHECK_WORDS: &str = "FERB asks before it goes on";
const QUESTION_WORDS: &str = "FERB has a question";
/// The lab keeps a session's last 16 challenges (`[-15:]` before the new one).
const CHALLENGES_KEPT: usize = 16;
const ANSWERS: [&str; 3] = ["chat.approve", "chat.answer", "chat.correct"];

/// One phone's one link connection.
#[derive(Default)]
pub(crate) struct Sess {
    challenges: Vec<(String, f64)>,
    /// Approval id → the one-time challenge its answer must be signed with (given with the question).
    for_question: HashMap<String, String>,
    unlocked: Option<(f64, f64)>,
    /// This connection proved the phone's own device key (PHN-126).
    present: bool,
}

/// Every session's state, and each phone's unlock (carried to a reconnect that proved the device key).
#[derive(Default)]
pub(crate) struct State {
    sessions: HashMap<(String, String), Sess>,
    windows: HashMap<String, (f64, f64)>,
    /// The prompts on the Mac's screen asked about, by their question's id.
    prompts: HashMap<String, ScreenPrompt>,
    /// When each phone's last use was written (written at most once a minute).
    seen_written: HashMap<String, f64>,
    /// GATE-615: a session's "until I'm back at my Mac" given with the answer under way (by its question's id), taken
    /// back when that answer fails.
    until_back: HashMap<(String, String), String>,
    /// PHN-649: each phone's unlock statements already taken (nonce → when), kept for `unlock_max_s`.
    statement_nonces: HashMap<String, HashMap<String, f64>>,
    /// PHN-649: when each phone's app last locked (`phone.lock`): a statement made before it is refused.
    locked_at: HashMap<String, f64>,
}

fn invalid(why: &str, done: &str) -> Refusal {
    refused(
        phone_gate::UNLOCK_INVALID,
        format!("The phone's unlock proof was not accepted ({why}); nothing was {done}."),
    )
}

fn locked_refusal() -> Refusal {
    refused(
        phone_gate::LOCKED,
        "FERB on your Mac opens for this phone only after its fingerprint, face or PIN (again after a few idle minutes).",
    )
}

/// The code's name, for the audit.
pub(crate) fn code_name(code: i64) -> String {
    phone_gate::CODES
        .iter()
        .find(|(_, c)| *c == code)
        .map_or_else(
            || {
                if code == wire::UNAUTHORIZED {
                    "method_not_allowed".to_owned()
                } else {
                    format!("error {code}")
                }
            },
            |(n, _)| (*n).to_owned(),
        )
}

impl State {
    fn sess(&mut self, device: &str, session: &str) -> &mut Sess {
        self.sessions
            .entry((device.to_owned(), session.to_owned()))
            .or_default()
    }

    pub(crate) fn sessions_of(&self, device: &str) -> usize {
        self.sessions.keys().filter(|(d, _)| d == device).count()
    }

    pub(crate) fn device_unlocked(&self, device: &str, now: f64, limits: &Limits) -> bool {
        self.windows
            .get(device)
            .is_some_and(|(since, until)| now < *until && now - since <= limits.unlock_max_s)
    }

    /// A removed phone: its sessions, its unlock and what it was asked end now.
    /// The phone's FERB opens as after a fingerprint, for its sessions that prove the device key (PHN-647: the
    /// owner's Yes to its pairing).
    pub(crate) fn open_device_window(&mut self, device: &str, now: f64, limits: &Limits) {
        self.windows
            .insert(device.to_owned(), (now, now + limits.unlock_idle_s));
    }

    pub(crate) fn forget_device(&mut self, device: &str) {
        self.sessions.retain(|(d, _), _| d != device);
        self.windows.remove(device);
        self.statement_nonces.remove(device);
        self.locked_at.remove(device);
        self.seen_written.remove(device);
    }

    /// `_signed`: the signed text of a proof whose signature is this phone's. `strong`: a sensitive call, a key place, a
    /// Yes to one of FERB's own checks: only the fingerprint-only key counts when the phone registered one (PHN-129,
    /// PHN-133, PHN-137). For everything else either key will do.
    fn signed(
        &self,
        dev: &Phone,
        proof: Option<&Map<String, Value>>,
        what: &str,
        done: &str,
        strong: bool,
    ) -> Result<Map<String, Value>, Refusal> {
        let lock_key = dev
            .unlock_pub
            .clone()
            .filter(|k| proof::key(k).is_some())
            .ok_or_else(|| {
                refused(
                    phone_gate::UNLOCK_REQUIRED,
                    format!("{what} needs the phone's screen lock, and this phone was paired without one."),
                )
            })?;
        let strong_key = dev.strong_pub.clone();
        let keys: Vec<String> = match (&strong_key, strong) {
            (Some(k), true) => vec![k.clone()],
            _ => [Some(lock_key.clone()), strong_key.clone()]
                .into_iter()
                .flatten()
                .collect(),
        };
        let Some(proof) = proof else {
            return Err(refused(
                phone_gate::UNLOCK_REQUIRED,
                "Unlock the phone (fingerprint, face or PIN) to do this.",
            ));
        };
        let (Some(payload), Some(sig)) = (
            proof.get("payload").and_then(Value::as_str),
            proof.get("sig").and_then(Value::as_str),
        ) else {
            return Err(invalid("unreadable signature", done));
        };
        if !keys
            .iter()
            .any(|k| proof::verify(k, payload.as_bytes(), sig))
        {
            let pin =
                strong && strong_key.is_some() && proof::verify(&lock_key, payload.as_bytes(), sig);
            return Err(invalid(
                if pin {
                    "it must be confirmed with your fingerprint, not the PIN"
                } else {
                    "wrong signature"
                },
                done,
            ));
        }
        match serde_json::from_str::<Value>(payload) {
            Ok(Value::Object(signed)) => Ok(signed),
            _ => Err(invalid("unreadable payload", done)),
        }
    }

    /// `verify_call`: the phone was unlocked for exactly this call: its key signed `{challenge, method, params}` with a
    /// challenge this daemon gave this session, unused and fresh. The challenge is used up.
    pub(crate) fn verify_call(
        &mut self,
        inner: &Inner,
        dev: &Phone,
        phone: &PhoneSession,
        method: &str,
        params: &Value,
        strong: bool,
    ) -> Result<(), Refusal> {
        let signed = self.signed(dev, phone.proof.as_ref(), "This", "done", strong)?;
        if signed.get("method") != Some(&json!(method)) || signed.get("params") != Some(params) {
            return Err(invalid("it was made for a different call", "done"));
        }
        let (ttl, now) = (inner.limits.challenge_ttl_s, inner.mono());
        let s = self.sess(&phone.device, &phone.session);
        match s
            .challenges
            .iter()
            .position(|(c, t)| signed.get("challenge") == Some(&json!(c)) && now - t < ttl)
        {
            Some(i) => {
                s.challenges.remove(i); // one use
                Ok(())
            }
            None => Err(invalid("it was used before, or is too old", "done")),
        }
    }

    /// `_open_window`: the unlock this session may use: its own, or (for a session that proved the phone's device key)
    /// the one this very phone was given on a session before, so a reconnect does not ask again. `None`: locked.
    fn open_window(
        &mut self,
        phone: &PhoneSession,
        now: f64,
        limits: &Limits,
    ) -> Option<(f64, f64)> {
        let device_window = self.windows.get(&phone.device).copied();
        let s = self.sess(&phone.device, &phone.session);
        let open = |(since, until): (f64, f64)| now < until && now - since <= limits.unlock_max_s;
        if let Some(w) = s.unlocked.filter(|w| open(*w)) {
            return Some(w);
        }
        if s.present {
            if let Some(w) = device_window.filter(|w| open(*w)) {
                return Some(w);
            }
        }
        None
    }

    /// This session proved the phone's own device key (PHN-126).
    pub(crate) fn proved_present(&self, phone: &PhoneSession) -> bool {
        self.sessions
            .get(&(phone.device.clone(), phone.session.clone()))
            .is_some_and(|s| s.present)
    }

    pub(crate) fn is_unlocked(&mut self, phone: &PhoneSession, now: f64, limits: &Limits) -> bool {
        self.open_window(phone, now, limits).is_some()
    }

    /// `require_unlocked`: the window is open (and use moves its idle end), else `locked`.
    fn require_unlocked(
        &mut self,
        phone: &PhoneSession,
        now: f64,
        limits: &Limits,
    ) -> Result<(), Refusal> {
        match self.open_window(phone, now, limits) {
            Some((since, _)) => {
                let w = (since, now + limits.unlock_idle_s);
                self.sess(&phone.device, &phone.session).unlocked = Some(w);
                self.windows.insert(phone.device.clone(), w);
                Ok(())
            }
            None => {
                self.sess(&phone.device, &phone.session).unlocked = None;
                Err(locked_refusal())
            }
        }
    }

    /// `answer_needs_no_window`: an answer that does not rest on the unlock window: one that carries its own proof (a
    /// hidden input's, or a Yes to one of FERB's own checks that is not routine: `check_answer`), a plain No, one to a
    /// question no longer waiting (refused next), or a routine answer from the paired phone's notification by a
    /// session that proved the device key (PHN-415, PHN-417). A correction always needs the window.
    fn answer_needs_no_window(
        &mut self,
        inner: &Inner,
        phone: &PhoneSession,
        method: &str,
        params: &Params,
    ) -> bool {
        if !ANSWERS.contains(&method) {
            return false;
        }
        let id = params
            .get("approval_id")
            .and_then(Value::as_str)
            .unwrap_or("");
        match inner.questions().facts(id) {
            Some(f) if f.pending => {
                if f.hidden || (f.own_check && !f.routine) {
                    return true;
                }
            }
            _ => return true,
        }
        if plain_no(method, params) {
            return true;
        }
        self.sess(&phone.device, &phone.session).present && method != "chat.correct"
    }

    /// `check_answer`: the question must still be waiting; a hidden input is answered only by `chat.answer` (or
    /// declined); anything but a plain No to a hidden input or a non-routine own check needs the phone's proof signed
    /// over this exact answer and the question's one-time challenge, a hidden answer as its SHA-256 only (PHN-110). A
    /// Yes to a check takes the fingerprint itself where the phone has one (PHN-137); a private answer takes either key.
    fn check_answer(
        &mut self,
        inner: &Inner,
        dev: &Phone,
        phone: &PhoneSession,
        method: &str,
        params: &Params,
    ) -> Result<(), Refusal> {
        let id = params
            .get("approval_id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let facts = match inner.questions().facts(&id) {
            Some(f) if f.pending => f,
            _ => {
                return Err(refused(
                    phone_gate::QUESTION_NOT_WAITING,
                    "That question isn't waiting any more, so nothing was sent.",
                ))
            }
        };
        let no = plain_no(method, params);
        if facts.hidden && !no && method != "chat.answer" {
            return Err(refused(
                phone_gate::BAD_REQUEST,
                "A private answer is typed into its hidden field, or declined.",
            ));
        }
        if no || !(facts.hidden || (facts.own_check && !facts.routine)) {
            return Ok(());
        }
        let what = if facts.hidden {
            "This private answer"
        } else {
            "This answer"
        };
        if phone.proof.is_none() {
            // a phone without a lock key says so first
            if dev.unlock_pub.as_deref().and_then(proof::key).is_none() {
                return Err(refused(
                    phone_gate::UNLOCK_REQUIRED,
                    format!("{what} needs the phone's screen lock, and this phone was paired without one."),
                ));
            }
            return Err(refused(
                phone_gate::UNLOCK_REQUIRED,
                "Unlock the phone (fingerprint, face or PIN) to give this answer.",
            ));
        }
        let signed = self.signed(dev, phone.proof.as_ref(), what, "sent", !facts.hidden)?;
        let s = self.sess(&phone.device, &phone.session);
        let Some(challenge) = s.for_question.get(&id).cloned() else {
            return Err(invalid("the question was asked again since", "sent"));
        };
        let same = if facts.hidden {
            let text = params.get("text").and_then(Value::as_str).unwrap_or("");
            signed.get("sealed") == Some(&json!(proof::b64u(&Sha256::digest(text.as_bytes()))))
                && !signed.contains_key("params")
        } else {
            signed.get("params") == Some(&Value::Object(params.clone()))
        };
        if signed.get("approval_id") != Some(&json!(id))
            || signed.get("challenge") != Some(&json!(challenge))
            || signed.get("method") != Some(&json!(method))
            || !same
        {
            return Err(invalid("it was made for a different answer", "sent"));
        }
        s.for_question.remove(&id); // one use
        Ok(())
    }

    /// `decorate`: what the phone needs to answer each question.
    fn decorate(&mut self, inner: &Inner, dev: &Phone, phone: &PhoneSession, status: &mut Params) {
        let has_key = dev.unlock_pub.as_deref().and_then(proof::key).is_some();
        let strong = dev.strong_pub.is_some();
        let mut seen = Vec::new();
        let mut parts: Vec<(usize, Value)> = Vec::new();
        if let Some(Value::Array(aps)) = status.get("approvals") {
            for (i, ap) in aps.iter().enumerate() {
                let id = ap
                    .get("approval_id")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned();
                let f = inner.questions().facts(&id).unwrap_or_default();
                seen.push(id.clone());
                let part = if f.hidden && !has_key {
                    json!({"can_answer": false, "hidden": true, "error": {"code": "unlock_required",
                           "message": "This private answer needs the phone's screen lock, and this phone was paired without one.",
                           "allow": "Answer on the Mac, or set a screen lock on the phone and pair it again"}})
                } else if f.hidden || (f.own_check && !f.routine) {
                    let s = self.sess(&phone.device, &phone.session);
                    let c = s
                        .for_question
                        .entry(id)
                        .or_insert_with(|| inner.nonce())
                        .clone();
                    json!({"can_answer": true, "needs_unlock": true, "hidden": f.hidden, "challenge": c,
                           "fingerprint": !f.hidden && strong})
                } else if inner.questions().until_back_offered(&id) {
                    // decision: GATE-615 — the screen-use question may be answered "until I'm back at my Mac"
                    json!({"can_answer": true, "needs_unlock": false, "until_back": true})
                } else {
                    json!({"can_answer": true, "needs_unlock": false})
                };
                parts.push((i, part));
            }
        }
        if let Some(Value::Array(aps)) = status.get_mut("approvals") {
            for (i, part) in parts {
                if let Some(Value::Object(ap)) = aps.get_mut(i) {
                    ap.insert("phone".into(), part);
                }
            }
        }
        self.sess(&phone.device, &phone.session)
            .for_question
            .retain(|k, _| seen.contains(k));
    }

    /// `status_for`: what FERB has waiting, as this session may see it. Unlocked: everything. Locked: with its words only
    /// for a session that proved the phone's device key, so the paired phone's notifications say what FERB asks and
    /// answered, with no time limit (PHN-126); any other holder of the link key learns only that something waits and of
    /// which kind. A hidden input's question is never worded for a locked session (PHN-422).
    fn status_for(
        &mut self,
        inner: &Inner,
        dev: &Phone,
        phone: &PhoneSession,
        mut status: Params,
    ) -> Params {
        self.decorate(inner, dev, phone, &mut status);
        if self.is_unlocked(phone, inner.mono(), &inner.limits) {
            return status;
        }
        let words = self.sess(&phone.device, &phone.session).present;
        let keep = |v: &Value, keys: &[&str]| -> Value {
            let mut m = Map::new();
            for k in keys {
                if let Some(x) = v.get(*k) {
                    m.insert((*k).to_owned(), x.clone());
                }
            }
            m.insert("held".into(), json!(true));
            Value::Object(m)
        };
        let list = |status: &Params, key: &str| -> Vec<Value> {
            status
                .get(key)
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
        };
        let approvals: Vec<Value> = list(&status, "approvals")
            .into_iter()
            .map(|ap| {
                let part = ap.get("phone").cloned().unwrap_or(Value::Null);
                let hidden = part.get("hidden") == Some(&json!(true));
                let kind = if hidden {
                    HIDDEN_WORDS
                } else if part.get("needs_unlock") == Some(&json!(true)) {
                    CHECK_WORDS
                } else {
                    QUESTION_WORDS
                };
                if words && !hidden {
                    let mut a = ap.clone();
                    if let Value::Object(m) = &mut a {
                        m.insert("held".into(), json!(false));
                    }
                    a
                } else {
                    json!({"approval_id": ap.get("approval_id"), "kind": ap.get("kind"), "capability": ap.get("capability"),
                           "task_id": ap.get("task_id"), "options": [], "reason": kind, "held": true, "phone": part})
                }
            })
            .collect();
        status.insert("approvals".into(), Value::Array(approvals));
        if !words {
            let held: [(&str, &[&str]); 5] = [
                ("results", &["task_id", "request_id", "at", "status"]),
                ("running", &["task_id", "started_at"]),
                ("notices", &[]),
                ("delegated_conversations", &[]),
                ("loops", &[]),
            ];
            for (key, kept) in held {
                if status.contains_key(key) {
                    let items: Vec<Value> =
                        list(&status, key).iter().map(|v| keep(v, kept)).collect();
                    status.insert(key.into(), Value::Array(items));
                }
            }
        }
        status.insert("locked".into(), json!(true));
        status.insert("words".into(), json!(words));
        status
    }
}

fn plain_no(method: &str, params: &Params) -> bool {
    (method == "chat.approve" && params.get("granted") == Some(&json!(false)))
        || (method == "chat.answer"
            && params
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or("")
                .is_empty())
}

// ---- the phone session's own calls (`auth::PHONE_SESSION`) ---------------------------------------------------------

/// `phone.challenge`: a one-time challenge for one sensitive call, one unlock or one device-key proof.
pub(crate) fn challenge(
    inner: &Inner,
    st: &mut State,
    _dev: &Phone,
    phone: &PhoneSession,
) -> Result<Params, Refusal> {
    let (ttl, now) = (inner.limits.challenge_ttl_s, inner.mono());
    let c = inner.nonce();
    let s = st.sess(&phone.device, &phone.session);
    s.challenges.retain(|(_, t)| now - t < ttl);
    let drop = s.challenges.len().saturating_sub(CHALLENGES_KEPT - 1);
    s.challenges.drain(..drop);
    s.challenges.push((c.clone(), now));
    let mut out = Params::new();
    out.insert("challenge".into(), json!(c));
    out.insert("ttl_s".into(), json!(ttl as u64));
    Ok(out)
}

/// `phone.unlock`: a proof signed for `link.unlock {}` with a fresh challenge, by either key, opens FERB for this
/// phone for the idle time.
pub(crate) fn unlock(
    inner: &Inner,
    st: &mut State,
    dev: &Phone,
    phone: &PhoneSession,
) -> Result<Params, Refusal> {
    let checked = st.verify_call(inner, dev, phone, "link.unlock", &json!({}), false);
    let outcome = checked
        .as_ref()
        .map_or_else(|r| code_name(r.code), |_| "ok".to_owned());
    inner.audit(&phone.device, "phone.unlock", "link", &outcome, json!({}));
    checked?;
    let now = inner.mono();
    let w = (now, now + inner.limits.unlock_idle_s);
    st.sess(&phone.device, &phone.session).unlocked = Some(w);
    st.windows.insert(phone.device.clone(), w);
    let mut out = Params::new();
    out.insert("unlocked".into(), json!(true));
    out.insert("idle_s".into(), json!(inner.limits.unlock_idle_s as u64));
    out.insert("max_s".into(), json!(inner.limits.unlock_max_s as u64));
    Ok(out)
}

/// The domain an unlock statement's signed text starts with (PHN-649); nothing else the phone signs carries it.
pub const STATEMENT_DOMAIN: &str = "ferb.unlock-statement.v1";

/// `phone.unlock_statement {age_ms}` (the phone's `link.unlock_statement`, PHN-649): the unlock the phone's lock key
/// signed in the app's opening prompt before the link was up, `{domain, nonce, made}` made by the phone itself. Taken
/// once, as `phone.unlock`'s challenge-signed proof would be, when: the signature is this phone's (its lock key or its
/// fingerprint-only key, registered at its pairing: no other phone's key verifies it); the text is a statement; the
/// nonce was never taken from this phone; the phone's steady clock says it was made at most `statement_age_s` ago; it
/// wasn't made before the app last locked; and this connection proved the phone's device key. `made` (the phone's
/// wall clock) is only reported: a phone's wrong clock neither refuses nor admits anything.
pub(crate) fn unlock_statement(
    inner: &Inner,
    st: &mut State,
    dev: &Phone,
    phone: &PhoneSession,
    params: &Params,
) -> Result<Params, Refusal> {
    let checked = statement_ok(inner, st, dev, phone, params);
    let outcome = checked
        .as_ref()
        .map_or_else(|r| code_name(r.code), |_| "ok".to_owned());
    inner.audit(
        &phone.device,
        "phone.unlock",
        "link",
        &outcome,
        json!({"by": "statement"}),
    );
    checked?;
    let now = inner.mono();
    let w = (now, now + inner.limits.unlock_idle_s);
    st.sess(&phone.device, &phone.session).unlocked = Some(w);
    st.windows.insert(phone.device.clone(), w);
    let mut out = Params::new();
    out.insert("unlocked".into(), json!(true));
    out.insert("idle_s".into(), json!(inner.limits.unlock_idle_s as u64));
    out.insert("max_s".into(), json!(inner.limits.unlock_max_s as u64));
    Ok(out)
}

fn statement_ok(
    inner: &Inner,
    st: &mut State,
    dev: &Phone,
    phone: &PhoneSession,
    params: &Params,
) -> Result<(), Refusal> {
    if !st.proved_present(phone) {
        // the device key's proof is the link's own, on its own thread: the phone tries again in a moment
        return Err(refused(
            phone_gate::LOCKED,
            "This connection hasn't proved it is the paired phone yet.",
        ));
    }
    let signed = st.signed(dev, phone.proof.as_ref(), "Opening FERB", "opened", false)?;
    if signed.get("domain") != Some(&json!(STATEMENT_DOMAIN)) {
        return Err(invalid("it is not an unlock statement", "opened"));
    }
    let nonce = signed
        .get("nonce")
        .and_then(Value::as_str)
        .filter(|n| n.len() >= 16 && n.len() <= 128)
        .ok_or_else(|| invalid("it has no nonce", "opened"))?
        .to_owned();
    let age_s = params
        .get("age_ms")
        .and_then(Value::as_u64)
        .map(|ms| ms as f64 / 1000.0)
        .ok_or_else(|| invalid("it doesn't say how old it is", "opened"))?;
    let now = inner.mono();
    if age_s > inner.limits.statement_age_s {
        return Err(refused(
            phone_gate::UNLOCK_REQUIRED,
            "The phone's unlock was made too long ago. Unlock the phone again.",
        ));
    }
    if st
        .locked_at
        .get(&phone.device)
        .is_some_and(|t| now - age_s <= *t)
    {
        return Err(invalid("it was made before the app locked", "opened"));
    }
    let max = inner.limits.unlock_max_s;
    let used = st.statement_nonces.entry(phone.device.clone()).or_default();
    used.retain(|_, t| now - *t <= max);
    if used.contains_key(&nonce) {
        return Err(invalid("it was used before", "opened"));
    }
    used.insert(nonce, now);
    Ok(())
}

/// `phone.lock`: this phone's unlock ends (the app locked).
pub(crate) fn lock_phone(
    inner: &Inner,
    st: &mut State,
    _dev: &Phone,
    phone: &PhoneSession,
) -> Result<Params, Refusal> {
    st.sess(&phone.device, &phone.session).unlocked = None;
    st.windows.remove(&phone.device);
    st.locked_at.insert(phone.device.clone(), inner.mono());
    inner.audit(&phone.device, "phone.lock", "link", "ok", json!({}));
    let mut out = Params::new();
    out.insert("unlocked".into(), json!(false));
    Ok(out)
}

/// `phone.present`: a proof by the phone's device key, signed for `link.present {}` with a fresh challenge: this session
/// comes from the paired phone itself (PHN-126). A wrong proof changes nothing.
pub(crate) fn present(
    inner: &Inner,
    st: &mut State,
    dev: &Phone,
    phone: &PhoneSession,
) -> Result<Params, Refusal> {
    let strong = dev.strong_pub.is_some();
    let answer = |present: bool, enrolled: bool| {
        let mut out = Params::new();
        out.insert("present".into(), json!(present));
        out.insert("enrolled".into(), json!(enrolled));
        out.insert("strong".into(), json!(strong));
        out
    };
    let (Some(key), Some(p)) = (dev.device_pub.clone(), phone.proof.as_ref()) else {
        let present = st.sess(&phone.device, &phone.session).present;
        return Ok(answer(present, dev.device_pub.is_some()));
    };
    let (payload, sig) = (
        p.get("payload").and_then(Value::as_str).unwrap_or(""),
        p.get("sig").and_then(Value::as_str).unwrap_or(""),
    );
    let mut ok = false;
    let mut active = false;
    if proof::verify(&key, payload.as_bytes(), sig) {
        let signed: Value = serde_json::from_str(payload).unwrap_or(Value::Null);
        // `{"active": true}`: the app says it is unlocked and in front (PHN-648), signed with the rest
        active = signed.get("params") == Some(&json!({"active": true}));
        if signed.get("method") == Some(&json!("link.present"))
            && (signed.get("params") == Some(&json!({})) || active)
        {
            let (ttl, now) = (inner.limits.challenge_ttl_s, inner.mono());
            let s = st.sess(&phone.device, &phone.session);
            if let Some(i) = s
                .challenges
                .iter()
                .position(|(c, t)| signed.get("challenge") == Some(&json!(c)) && now - t < ttl)
            {
                s.challenges.remove(i);
                ok = true;
            }
        }
    }
    let s = st.sess(&phone.device, &phone.session);
    s.present = s.present || ok;
    let present = s.present;
    if ok && active {
        // decision: PHN-648 — the device key proves the phone and the app's own lock proves the owner: while the app
        // is unlocked and in front its presence moves the idle end of a window a fingerprint or the pairing Yes opened.
        // Never past the 12 h limit (`open_window` checks it from the window's start), never a window `phone.lock`
        // ended or one that already lapsed.
        let now = inner.mono();
        let limits = &inner.limits;
        if let Some((since, until)) = st.windows.get(&phone.device).copied() {
            if now < until && now - since <= limits.unlock_max_s {
                let w = (since, now + limits.unlock_idle_s);
                st.windows.insert(phone.device.clone(), w);
                st.sess(&phone.device, &phone.session).unlocked = Some(w);
            }
        }
    }
    inner.audit(
        &phone.device,
        "phone.present",
        "link",
        if ok { "ok" } else { "unlock_invalid" },
        json!({}),
    );
    Ok(answer(present, true))
}

/// `phone.session_end`: the link connection ended: its challenges and its own unlock go; the phone's unlock stays for
/// the session it moves to (the lab's `end`, NET-25).
pub(crate) fn session_end(
    inner: &Inner,
    st: &mut State,
    _dev: &Phone,
    phone: &PhoneSession,
) -> Result<Params, Refusal> {
    st.sessions
        .remove(&(phone.device.clone(), phone.session.clone()));
    inner.device_disconnected(&phone.device, st.sessions_of(&phone.device));
    // its uploads under way pause, kept for the same phone to go on with (PHN-127)
    super::files::session_ended(inner, phone);
    let mut out = Params::new();
    out.insert("ok".into(), json!(true));
    Ok(out)
}

// ---- the server's questions --------------------------------------------------------------------------------------

/// The phone area as the server's [`PhoneGate`].
#[derive(Clone)]
pub struct Gate(pub(crate) Arc<Inner>);

impl Gate {
    fn audited(row_class: Class, method: &str) -> bool {
        // decision: GATE-615 — a phone ending "until I'm back at my Mac" is audited too
        matches!(row_class, Class::Private | Class::Sensitive)
            || ANSWERS.contains(&method)
            || method == "screen_use.end"
    }

    fn admit_inner(
        &self,
        phone: &PhoneSession,
        method: &str,
        params: &mut Params,
    ) -> Result<(), Refusal> {
        let inner = &self.0;
        let dev = inner.live(&phone.device)?;
        self.touch(&dev);
        if PHONE_SESSION.contains(&method) {
            return Ok(()); // each checks its own proof
        }
        // decision: PHN-444
        if let Some(r) = super::files::shared_relay_refusal(phone, method) {
            return Err(r);
        }
        let Some(row) = classes::phone_method(method) else {
            return Err(refused(
                wire::UNAUTHORIZED,
                format!("{method} not allowed for phone"),
            ));
        };
        for name in row.refused_params {
            if params.contains_key(*name) {
                return Err(refused(
                    wire::UNAUTHORIZED,
                    format!("{method}: {name} not allowed for phone"),
                ));
            }
        }
        let now = inner.mono();
        let mut st = lock(&inner.state);
        // decision: GATE-615 — "until I'm back at my Mac" goes only with a Yes to the screen-use question, from this
        // paired phone with its unlock window open (no fingerprint: the owner, 2026-11-17)
        let mut until_back: Option<String> = None;
        if method == "chat.approve" && params.contains_key("until_back") {
            let wanted = params.remove("until_back") == Some(json!(true));
            if wanted {
                if params.get("granted") != Some(&json!(true)) {
                    return Err(refused(
                        phone_gate::BAD_REQUEST,
                        "\"Until I'm back at my Mac\" goes with a Yes.",
                    ));
                }
                st.require_unlocked(phone, now, &inner.limits)?;
                let id = params
                    .get("approval_id")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned();
                if !inner.questions().until_back_offered(&id) {
                    return Err(refused(
                        phone_gate::BAD_REQUEST,
                        "This question can't be answered \"until I'm back at my Mac\" now. Answer Yes or No.",
                    ));
                }
                until_back = Some(id);
            }
        }
        match row.class {
            Class::Plain | Class::Private => {
                let free = match row.locked {
                    Locked::Allowed => true,
                    Locked::Answer => st.answer_needs_no_window(inner, phone, method, params),
                    Locked::Refused => false,
                };
                if !free {
                    st.require_unlocked(phone, now, &inner.limits)?;
                }
                if ANSWERS.contains(&method) {
                    st.check_answer(inner, &dev, phone, method, params)?;
                }
            }
            Class::Sensitive => {
                if row.on_mac_screen && inner.prompts().is_none() {
                    // decision: PHN-120 — nothing in FERB clicks a prompt on the Mac's screen yet: said before it opens
                    return Err(refused(
                        phone_gate::NEEDS_THE_MAC,
                        "This opens something on your Mac's screen that needs a click there, and FERB can't click it \
                         for you yet. Do it at the Mac.",
                    ));
                }
                st.verify_call(
                    inner,
                    &dev,
                    phone,
                    method,
                    &Value::Object(params.clone()),
                    true,
                )?;
            }
            Class::Never => {
                return Err(refused(
                    wire::UNAUTHORIZED,
                    format!("{method} cannot be done from a phone ({})", row.why),
                ))
            }
        }
        drop(st);
        if let Some(id) = until_back {
            inner
                .questions()
                .grant_until_back(&id, &phone.device, &dev.name)
                .map_err(|why| refused(phone_gate::BAD_REQUEST, why))?;
            lock(&inner.state)
                .until_back
                .insert((phone.device.clone(), phone.session.clone()), id);
        }
        // a message's attachments are this phone's own finished uploads, by id, as the paths chat.send takes (PHN-127)
        if method == "chat.send" {
            if let Some(ids) = params.remove("attachments") {
                let paths = super::files::attachments(inner, &phone.device, &ids)?;
                params.insert("input_paths".into(), json!(paths));
            }
        }
        // the phone is the client of what it sends and marks (APP-916): never another client's id
        if method == "chat.send" || method == "chat.entries.mark" {
            params.insert("client_id".into(), json!(format!("phone:{}", phone.device)));
        }
        Ok(())
    }

    /// The phone's last use, written at most once a minute.
    fn touch(&self, dev: &Phone) {
        let inner = &self.0;
        let now = inner.mono();
        let due = {
            let mut st = lock(&inner.state);
            let last = st.seen_written.get(&dev.endpoint).copied();
            let due = last.is_none_or(|t| now - t >= 60.0);
            if due {
                st.seen_written.insert(dev.endpoint.clone(), now);
            }
            due
        };
        if due {
            let db = lock(&inner.seams.db);
            let _ = ferb_store::phones::seen(db.conn(), &dev.endpoint, &inner.now_iso());
        }
    }

    fn detail(method: &str, params: &Params) -> Value {
        // structural facts only: which question, which phone; never a text, a value or a path's content (rule 7)
        let mut d = Map::new();
        for key in ["approval_id", "task_id"] {
            if let Some(v) = params.get(key).filter(|v| v.is_string()) {
                d.insert(key.into(), v.clone());
            }
        }
        if method == "phone.remove" {
            if let Some(v) = params.get("id") {
                d.insert("removed".into(), v.clone());
            }
        }
        // a file call refused before the file area: the path as sent (the owner's own audit; never a content)
        if PHONE_FILES.contains(&method) || method == "phone.search" {
            for key in ["path", "name", "size"] {
                if let Some(v) = params.get(key).filter(|v| v.is_string() || v.is_u64()) {
                    d.insert(key.into(), v.clone());
                }
            }
        }
        Value::Object(d)
    }
}

impl PhoneGate for Gate {
    fn open(&self, device: &str, session: &str) -> Result<(), Refusal> {
        let inner = &self.0;
        let dev = inner.live(device)?;
        lock(&inner.state).sess(device, session);
        self.touch(&dev);
        Ok(())
    }

    fn admit(
        &self,
        phone: &PhoneSession,
        method: &str,
        params: &mut Params,
    ) -> Result<(), Refusal> {
        let class = classes::phone_method(method).map_or(Class::Never, |r| r.class);
        let detail = Self::detail(method, params);
        let admitted = self.admit_inner(phone, method, params);
        let class_name = if PHONE_SESSION.contains(&method) {
            "link"
        } else {
            class.as_str()
        };
        match &admitted {
            Err(r) if !PHONE_SESSION.contains(&method) => self.0.audit(
                &phone.device,
                method,
                class_name,
                &code_name(r.code),
                detail,
            ),
            // a stream is audited as it starts; a unary call with its outcome (`reply`)
            // (a file call is audited by the file area, with its path and bytes)
            Ok(())
                if Self::audited(class, method)
                    && !PHONE_FILES.contains(&method)
                    && classes::phone_method(method).is_some_and(|r| r.stream) =>
            {
                self.0
                    .audit(&phone.device, method, class_name, "ok", detail)
            }
            _ => {}
        }
        admitted
    }

    fn reply(&self, phone: &PhoneSession, method: &str, result: HandlerResult) -> HandlerResult {
        let inner = &self.0;
        let class = classes::phone_method(method).map_or(Class::Never, |r| r.class);
        let mut result = result;
        // decision: GATE-615 — the "until I'm back" given with an answer that was not taken is taken back, unsaid
        if method == "chat.approve" {
            let given = lock(&inner.state)
                .until_back
                .remove(&(phone.device.clone(), phone.session.clone()));
            if given.is_some() && result.is_err() {
                inner
                    .questions()
                    .end_until_back(super::UNTIL_BACK_ANSWER_FAILED, Some(&phone.device));
            }
        }
        if let Ok(out) = &mut result {
            if method == "chat.status" {
                // decision: PHN-761 — the planner's question that waits is one more of the phone's questions (read
                // before the gate's state is held: the store is never locked inside it)
                let asking = super::asking::waiting(&lock(&inner.seams.db));
                if let Some(q) = asking {
                    if let Some(Value::Array(aps)) = out.get_mut("approvals") {
                        aps.push(q);
                    } else {
                        out.insert("approvals".into(), Value::Array(vec![q]));
                    }
                }
                if let Ok(dev) = inner.live(&phone.device) {
                    let mut st = lock(&inner.state);
                    *out = st.status_for(inner, &dev, phone, std::mem::take(out));
                }
                // decision: GATE-615 — the phone shows "FERB may use your Mac's screen until you're back", with its end
                out.insert("screen_use".into(), inner.questions().screen_use_status());
                // decision: APP-983 — insomnia mode's state, warnings and "someone's at your Mac" notice ride the pushed
                // status, so the phone notifies even while locked (its words are for the paired phone's own
                // notifications, as a question's are)
                if let Some(status) = inner.insomnia.get() {
                    out.insert("insomnia".into(), Value::Object(status()));
                }
            }
            if classes::phone_method(method).is_some_and(|r| r.on_mac_screen) {
                self.prompt_opened(method, out);
            }
            if ANSWERS.contains(&method) {
                self.prompt_answered(out);
            }
        }
        if Self::audited(class, method)
            && !PHONE_SESSION.contains(&method)
            && !PHONE_FILES.contains(&method)
        {
            let outcome = match &result {
                Ok(_) => "ok".to_owned(),
                Err(e) => match e.failure {
                    ferb_rpc::Failure::Coded(c) => code_name(c),
                    _ => "ferb_error".to_owned(),
                },
            };
            let mut detail = Map::new();
            if let Ok(out) = &result {
                if let Some(id) = out.get("approval_id").filter(|v| v.is_string()) {
                    detail.insert("approval_id".into(), id.clone());
                }
                if let Some(id) = out.get("removed").filter(|v| v.is_string()) {
                    detail.insert("removed".into(), id.clone());
                }
            }
            inner.audit(
                &phone.device,
                method,
                class.as_str(),
                &outcome,
                Value::Object(detail),
            );
        }
        result
    }
}

impl Gate {
    /// One of the six on-screen methods ran from the phone: the prompt it left open is asked about there, as one of
    /// FERB's own checks (its Yes signed with the fingerprint: PHN-120, PHN-137).
    fn prompt_opened(&self, method: &str, out: &mut Params) {
        let inner = &self.0;
        let Some(prompts) = inner.prompts() else {
            return;
        };
        let Some(prompt) = prompts.left_open(method, out) else {
            out.insert("mac_prompt".into(), json!({"open": false}));
            return;
        };
        let reason = format!(
            "A prompt is open on your Mac: \"{}\". Shall FERB click \"{}\" for you?",
            prompt.title, prompt.button
        );
        let exact = format!("{}\n[ {} ]", prompt.title, prompt.button);
        let id = inner.questions().ask_check(&reason, &exact, "");
        out.insert(
            "mac_prompt".into(),
            json!({"open": true, "approval_id": id, "title": prompt.title}),
        );
        lock(&inner.state).prompts.insert(id, prompt);
    }

    /// The answer to a prompt's question: on Yes FERB clicks; when the system does not take the click, the phone is
    /// told it must be done at the Mac (PHN-120).
    fn prompt_answered(&self, out: &mut Params) {
        let inner = &self.0;
        let Some(id) = out
            .get("approval_id")
            .and_then(Value::as_str)
            .map(str::to_owned)
        else {
            return;
        };
        let Some(prompt) = lock(&inner.state).prompts.remove(&id) else {
            return;
        };
        let granted = out.get("state") == Some(&json!("granted"));
        let said = if !granted {
            json!({"clicked": false, "message": format!("FERB left the prompt \"{}\" on your Mac's screen untouched.", prompt.title)})
        } else {
            match inner.prompts().map(|p| p.click(&prompt)) {
                Some(Ok(())) => {
                    json!({"clicked": true, "message": format!("FERB clicked \"{}\" on your Mac.", prompt.button)})
                }
                Some(Err(why)) => {
                    json!({"clicked": false, "needs_the_mac": true, "message": format!(
                    "macOS did not take a click from software on \"{}\" ({why}), so \"{}\" has to be clicked at the Mac itself.",
                    prompt.title, prompt.button)})
                }
                None => {
                    json!({"clicked": false, "needs_the_mac": true, "message": "FERB can't click prompts on your Mac's screen."})
                }
            }
        };
        out.insert("mac_prompt".into(), said);
    }
}
