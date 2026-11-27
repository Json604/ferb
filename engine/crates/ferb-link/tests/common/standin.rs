//! A STAND-IN for FERB's daemon, for these tests only: its answers are fake. It is a Rust port of the minimum of the
//! phone lab's Python stand-in (ferb-phone-link@57b3f34 `standin/daemon.py`, `standin/phone.py`) that the link's
//! tests need: the same socket framing (newline-delimited JSON-RPC 2.0, `auth.hello {role}` first, streams as
//! `<method>.event` notifications, the same error codes) and the phone gate the bridge relies on (one-time challenges,
//! every unlock proof verified for the exact call, the unlock window, the answer rules, file reads and uploads).
//!
//! Since S4 (kb T-1204) it speaks the helper's protocol with the real daemon (ferb-daemon `phone`): role `link`,
//! `link.devices`, `link.events` (the device list's changes and the daemon's asks), `link.pair_request`,
//! `link.pair_wait`, `link.pair_cancel`, `link.answer`; the owner's Yes, removal and the daemon's own numbers are the
//! test's (`pair_decide`, `remove`, `configure`). The real daemon's phone area now exists and tests/real_daemon.rs
//! runs the bridge against it in-process; this stand-in stays for what the real one does not serve yet: the files
//! both ways (S3b, kb T-1203: `phone.files`, `phone.tree`, `phone.read`, `phone.put_*`) and FERB's own chat answers
//! (ferbd's handlers: `chat.send`, `chat.status`, the questions). Left out here (they test the daemon, not the link):
//! FERB's protection lists, the prompts on the Mac's screen, the quarantine mark on uploads.
//!
//! Chat texts that make it do something (anything else gets a short reply): `ask <question>`, `confirm <thing>`,
//! `secret`, `code`, `long <seconds>`, `file <bytes>`, `big <chars>`.
#![allow(dead_code)]
use base64::{
    Engine,
    engine::general_purpose::{STANDARD as B64S, URL_SAFE_NO_PAD as B64},
};
use ferb_link::role;
use p256::ecdsa::{Signature, VerifyingKey, signature::Verifier};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::UnixListener,
    sync::mpsc,
};

const UNAUTHORIZED: i64 = -32001;
const FAILED: i64 = -32004;
const METHOD_NOT_FOUND: i64 = -32601;
const PARSE_ERROR: i64 = -32700;
const READ_PIECE: usize = 48 * 1024;
const SECRET_KINDS: &[&str] = &["owner_login_code", "owner_admin_password", "owner_secret"];
const OWN_CHECKS: &[&str] = &[
    "ferb.confirm",
    "ferb.retry",
    "conversation.approve",
    "memory.forget",
    "downloads.delete",
    "browsing.choose",
];
const LOCKED_OK: &[&str] = &["ping", "chat.status", "stop.engage", "task.stop"];
const LINK_METHODS: &[&str] = &[
    "phone.challenge",
    "phone.unlock",
    "phone.lock",
    "phone.present",
    "phone.keys",
    "phone.files",
    "phone.tree",
    "phone.read",
    "phone.put_group",
    "phone.put_begin",
    "phone.put_chunk",
    "phone.put_end",
    "phone.session_end",
];

/// A refusal: (code name, words). The code is answered as the daemon's JSON-RPC number (`daemon::REFUSALS`).
struct Refused(&'static str, String);
fn code_of(name: &str) -> i64 {
    match name {
        "locked" => -32011,
        "unlock_required" => -32012,
        "unlock_invalid" => -32013,
        "fresh_unlock_required" => -32014,
        "protected_folder" => -32015,
        "file_not_allowed" => -32016,
        "not_a_file" => -32017,
        "not_found" => -32018,
        "question_not_waiting" => -32019,
        "unknown_phone" => -32020,
        "mac_disk_full" => -32021,
        "needs_the_mac" => -32022,
        "device_limit" => -32023,
        "bad_request" => -32600,
        _ => FAILED,
    }
}
fn refused(code: &'static str, words: impl Into<String>) -> Refused {
    Refused(code, words.into())
}

fn b64_any(s: &str) -> Option<Vec<u8>> {
    B64.decode(s.trim_end_matches('='))
        .ok()
        .or_else(|| B64S.decode(s).ok())
}
fn key(b64: &str) -> Option<VerifyingKey> {
    use p256::pkcs8::DecodePublicKey;
    let raw = b64_any(b64)?;
    VerifyingKey::from_public_key_der(&raw)
        .ok()
        .or_else(|| VerifyingKey::from_sec1_bytes(&raw).ok())
}
fn verify(key_b64: &str, payload: &[u8], sig_b64: &str) -> bool {
    let (Some(k), Some(raw)) = (key(key_b64), b64_any(sig_b64)) else {
        return false;
    };
    let Ok(sig) = Signature::from_der(&raw).or_else(|_| Signature::from_slice(&raw)) else {
        return false;
    };
    k.verify(payload, &sig).is_ok()
}
fn token(n: usize) -> String {
    let mut b = vec![0u8; n];
    rand::RngCore::fill_bytes(&mut rand::rng(), &mut b);
    B64.encode(b)
}
fn hex(n: usize) -> String {
    let mut b = vec![0u8; n];
    rand::RngCore::fill_bytes(&mut rand::rng(), &mut b);
    b.iter().map(|x| format!("{x:02x}")).collect()
}
fn now_text() -> String {
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    format!("t{t}")
}
fn clean_name(name: &str, fallback: &str) -> String {
    let out: String = name
        .chars()
        .filter(|c| c.is_alphanumeric() || ".-_ ".contains(*c))
        .take(80)
        .collect();
    let out = out.trim().trim_matches('.').to_string();
    if out.is_empty() {
        fallback.to_string()
    } else {
        out
    }
}

#[derive(Default, Clone)]
struct Device {
    name: String,
    app: String,
    paired_at: String,
    unlock_pub: Option<String>,
    device_pub: Option<String>,
    strong_pub: Option<String>,
    window: Option<(Instant, Instant)>,
}

#[derive(Default)]
struct Sess {
    challenges: Vec<(String, Instant)>,
    for_question: HashMap<String, String>,
    unlocked: Option<(Instant, Instant)>,
    present: bool,
}

#[derive(Clone)]
struct Approval {
    id: String,
    kind: String,
    capability: String,
    reason: String,
    exact: String,
    options: Vec<String>,
    state: String,
    answer: Value,
    task_id: String,
}

struct Task {
    id: String,
    goal: String,
    started_at: String,
    stop: bool,
    files: Vec<PathBuf>,
    done: bool,
    status: String,
    answer: String,
}

struct Upload {
    device: String,
    path: PathBuf,
    size: u64,
    got: u64,
    hasher: Sha256,
    open: bool,
    done: bool,
}

struct Config {
    unlock_idle_s: u64,
    unlock_max_s: u64,
    challenge_ttl_s: u64,
    unlock_before_yes: bool,
    list_max: u64,
    upload_free_floor: u64,
}

/// A pairing the helper asked about, waiting for the test's Yes (the owner's in FERB.app).
#[derive(Clone)]
struct PendingPair {
    request_id: String,
    endpoint: String,
    device: Device,
    code: String,
    decided: Option<bool>,
}

struct St {
    devices: HashMap<String, Device>,
    /// `link.events`: the device list's changes and the asks.
    link_events: tokio::sync::broadcast::Sender<Value>,
    pending: Option<PendingPair>,
    devices_max: usize,
    /// The helper's answers to the asks (`link.answer`), by ask id.
    answers: HashMap<String, Value>,
    changed: Arc<tokio::sync::Notify>,
    /// How many helpers listen on `link.events` now.
    listening: usize,
    /// Per phone: its recent file reads, and whether this burst was told (the daemon's heavy-fetch notice, S3b
    /// PHN-485: `link.events heavy_fetch`, once per burst); the numbers as `configure` sets them.
    fetches: HashMap<String, (std::collections::VecDeque<Instant>, bool)>,
    fetch_notice_files: usize,
    fetch_notice_window_s: u64,
    /// Streams whose caller left before they ended (the bridge dropped them: a phone cancelled or went).
    streams_left: u64,
    config: Config,
    sessions: HashMap<(String, String), Sess>,
    approvals: Vec<Approval>,
    /// The planner's question that waits (ferb-daemon phone/asking.rs): its id and words, until the next message.
    clarify: Option<(String, String)>,
    tasks: Vec<Task>,
    results: Vec<Value>,
    messages: Vec<Value>,
    calls: Vec<Value>,
    stopped: bool,
    preview_size: usize,
    preview_seq: u64,
    fills: HashMap<String, String>,
    uploads: HashMap<String, Upload>,
    groups: HashMap<String, (String, PathBuf)>,
    inbox: PathBuf,
    files: PathBuf,
    never: Vec<PathBuf>,
    /// What the free disk is said to be (tests of the free-disk floor); None: the real one is not asked, enough.
    free_disk: Option<u64>,
    /// How long the stand-in takes to answer a phone's presence proof (`phone.present`): a slow or busy Mac.
    present_delay: Duration,
}

type Shared = Arc<Mutex<St>>;

impl St {
    fn ask(
        &mut self,
        kind: &str,
        capability: &str,
        reason: &str,
        exact: &str,
        options: Vec<String>,
        task_id: &str,
    ) -> String {
        let id = format!("ap_{}", hex(4));
        self.approvals.push(Approval {
            id: id.clone(),
            kind: kind.into(),
            capability: capability.into(),
            reason: reason.into(),
            exact: exact.into(),
            options,
            state: "pending".into(),
            answer: Value::Null,
            task_id: task_id.into(),
        });
        id
    }
    fn approval(&mut self, id: &str) -> Option<&mut Approval> {
        self.approvals.iter_mut().find(|a| a.id == id)
    }
    fn resolve(
        &mut self,
        id: &str,
        granted: bool,
        choice: Value,
        text: Option<String>,
        by: &str,
    ) -> Result<Value, Refused> {
        let Some(ap) = self.approvals.iter_mut().find(|a| a.id == id) else {
            return Err(refused(
                "ferb_error",
                "That question isn't waiting any more, so nothing was sent.",
            ));
        };
        if ap.state != "pending" {
            return Err(refused(
                "ferb_error",
                "That question isn't waiting any more, so nothing was sent.",
            ));
        }
        let sealed = SECRET_KINDS.contains(&ap.kind.as_str()) && text.is_some() && granted;
        let mut text = text;
        if sealed {
            self.fills.insert(id.to_string(), text.take().unwrap());
        }
        let ap = self.approvals.iter_mut().find(|a| a.id == id).unwrap();
        ap.state = if granted { "granted" } else { "denied" }.into();
        ap.answer = json!({"granted": granted, "choice": choice, "text": text, "by": by});
        if sealed {
            ap.answer["sealed"] = json!(true);
        }
        Ok(json!({"approval_id": id, "state": ap.state}))
    }
    fn status(&self) -> Value {
        let pending: Vec<Value> = self
            .approvals
            .iter()
            .filter(|a| a.state == "pending")
            .map(|a| {
                let mut v = json!({"approval_id": a.id, "kind": a.kind, "capability": a.capability, "reason": a.reason, "options": a.options, "task_id": a.task_id});
                if !a.exact.is_empty() {
                    v["exact"] = json!(a.exact);
                }
                v
            })
            .collect();
        let mut pending = pending;
        if let Some((id, words)) = &self.clarify {
            pending.push(json!({"approval_id": id, "kind": "clarify", "reason": words, "options": [], "task_id": null}));
        }
        let running: Vec<Value> = self.tasks.iter().filter(|t| !t.done).map(|t| json!({"task_id": t.id, "goal": t.goal, "name": t.goal.chars().take(40).collect::<String>(), "started_at": t.started_at})).collect();
        let results: Vec<Value> = self
            .results
            .iter()
            .filter(|r| r["shown"] != json!(true))
            .cloned()
            .collect();
        json!({"results": results, "running": running, "approvals": pending, "persona": {"state": "ready"}, "notices": [], "delegated_conversations": [], "loops": [], "paused": false, "stopped": self.stopped})
    }

    /// A phone that takes unusually much is said so once per burst, as the daemon does (S3b): on `link.events`.
    fn note_fetch(&mut self, device: &str) {
        let window = Duration::from_secs(self.fetch_notice_window_s.max(1));
        let max = self.fetch_notice_files;
        let (q, told) = self.fetches.entry(device.to_string()).or_default();
        while q.front().is_some_and(|t| t.elapsed() > window) {
            q.pop_front();
        }
        if q.is_empty() {
            *told = false;
        }
        q.push_back(Instant::now());
        let files = q.len();
        if files > max && !*told {
            *told = true;
            let name = self
                .devices
                .get(device)
                .map(|d| d.name.clone())
                .unwrap_or_default();
            let _ = self.link_events.send(json!({"event": "heavy_fetch", "id": device, "name": name, "files": files, "bytes": 0,
                "error": {"code": "heavy_fetch", "message": format!("{name} took {files} files from your Mac in the last few minutes."),
                          "allow": "If that was not you, remove this phone in FERB on the Mac"}}));
        }
    }

    // ---- the phone gate (standin/phone.py `Gate`) ---------------------------------------------------------------
    fn session(&mut self, device: &str, session: &str) -> Result<(), Refused> {
        if !self.devices.contains_key(device) {
            return Err(refused(
                "unknown_phone",
                "This phone is not one of FERB's paired phones.",
            ));
        }
        self.sessions
            .entry((device.into(), session.into()))
            .or_default();
        Ok(())
    }
    fn sess(&mut self, device: &str, session: &str) -> &mut Sess {
        self.sessions
            .entry((device.into(), session.into()))
            .or_default()
    }
    fn challenge(&mut self, device: &str, session: &str) -> Value {
        let ttl = Duration::from_secs(self.config.challenge_ttl_s);
        let s = self.sess(device, session);
        s.challenges.retain(|(_, t)| t.elapsed() < ttl);
        let c = token(16);
        s.challenges.push((c.clone(), Instant::now()));
        json!({"challenge": c, "ttl_s": ttl.as_secs()})
    }
    fn lock_key(&self, device: &str, what: &str) -> Result<String, Refused> {
        match self
            .devices
            .get(device)
            .and_then(|d| d.unlock_pub.clone())
            .filter(|k| key(k).is_some())
        {
            Some(k) => Ok(k),
            None => Err(refused(
                "unlock_required",
                format!(
                    "{what} needs the phone's screen lock, and this phone was paired without one."
                ),
            )),
        }
    }
    /// The signed text of a proof made by this phone's key (`strong`: the fingerprint-only key where the phone has one).
    fn signed(
        &self,
        device: &str,
        proof: &Value,
        what: &str,
        done: &str,
        strong: bool,
    ) -> Result<Value, Refused> {
        let lock = self.lock_key(device, what)?;
        let strong_key = self.devices.get(device).and_then(|d| d.strong_pub.clone());
        let keys: Vec<String> = match (&strong_key, strong) {
            (Some(k), true) => vec![k.clone()],
            _ => [Some(lock.clone()), strong_key.clone()]
                .into_iter()
                .flatten()
                .collect(),
        };
        if !proof.is_object() {
            return Err(refused(
                "unlock_required",
                "Unlock the phone (fingerprint, face or PIN) to do this.",
            ));
        }
        let invalid = |why: &str| {
            refused(
                "unlock_invalid",
                format!("The phone's unlock proof was not accepted ({why}); nothing was {done}."),
            )
        };
        let (Some(payload), Some(sig)) = (proof["payload"].as_str(), proof["sig"].as_str()) else {
            return Err(invalid("unreadable signature"));
        };
        if !keys.iter().any(|k| verify(k, payload.as_bytes(), sig)) {
            let pin = strong && strong_key.is_some() && verify(&lock, payload.as_bytes(), sig);
            return Err(invalid(if pin {
                "it must be confirmed with your fingerprint, not the PIN"
            } else {
                "wrong signature"
            }));
        }
        match serde_json::from_str::<Value>(payload) {
            Ok(v) if v.is_object() => Ok(v),
            _ => Err(invalid("unreadable payload")),
        }
    }
    fn verify_call(
        &mut self,
        device: &str,
        session: &str,
        proof: &Value,
        method: &str,
        params: &Value,
        strong: bool,
    ) -> Result<(), Refused> {
        let signed = self.signed(device, proof, "This", "done", strong)?;
        let invalid = |why: &str| {
            refused(
                "unlock_invalid",
                format!("The phone's unlock proof was not accepted ({why}); nothing was done."),
            )
        };
        if signed["method"] != json!(method) || &signed["params"] != params {
            return Err(invalid("it was made for a different call"));
        }
        let ttl = Duration::from_secs(self.config.challenge_ttl_s);
        let s = self.sess(device, session);
        match s
            .challenges
            .iter()
            .position(|(c, t)| signed["challenge"] == json!(c) && t.elapsed() < ttl)
        {
            Some(i) => {
                s.challenges.remove(i); // one use
                Ok(())
            }
            None => Err(invalid("it was used before, or is too old")),
        }
    }
    fn unlock(&mut self, device: &str, session: &str, proof: &Value) -> Result<Value, Refused> {
        self.verify_call(device, session, proof, "link.unlock", &json!({}), false)?;
        let now = Instant::now();
        let w = (now, now + Duration::from_secs(self.config.unlock_idle_s));
        self.sess(device, session).unlocked = Some(w);
        self.devices.get_mut(device).unwrap().window = Some(w);
        Ok(
            json!({"unlocked": true, "idle_s": self.config.unlock_idle_s, "max_s": self.config.unlock_max_s}),
        )
    }
    fn lock(&mut self, device: &str, session: &str) {
        self.sess(device, session).unlocked = None;
        if let Some(d) = self.devices.get_mut(device) {
            d.window = None;
        }
    }
    fn open_window(&mut self, device: &str, session: &str) -> Option<(Instant, Instant)> {
        let top = Duration::from_secs(self.config.unlock_max_s);
        let now = Instant::now();
        let dev_window = self.devices.get(device).and_then(|d| d.window);
        let s = self.sess(device, session);
        if let Some((since, until)) = s.unlocked
            && now < until
            && now - since <= top
        {
            return Some((since, until));
        }
        if let Some((since, until)) = dev_window
            && s.present
            && now < until
            && now - since <= top
        {
            return Some((since, until));
        }
        None
    }
    fn require_unlocked(&mut self, device: &str, session: &str) -> Result<(), Refused> {
        let idle = Duration::from_secs(self.config.unlock_idle_s);
        match self.open_window(device, session) {
            Some((since, _)) => {
                let w = (since, Instant::now() + idle);
                self.sess(device, session).unlocked = Some(w);
                if let Some(d) = self.devices.get_mut(device) {
                    d.window = Some(w);
                }
                Ok(())
            }
            None => {
                self.sess(device, session).unlocked = None;
                Err(refused(
                    "locked",
                    "FERB on your Mac opens for this phone only after its fingerprint, face or PIN (again after a few idle minutes).",
                ))
            }
        }
    }
    fn present(&mut self, device: &str, session: &str, proof: &Value) -> Value {
        let ttl = Duration::from_secs(self.config.challenge_ttl_s);
        let dev = self.devices.get(device).cloned().unwrap_or_default();
        let Some(k) = dev.device_pub.clone().filter(|_| proof.is_object()) else {
            return json!({"present": false, "enrolled": dev.device_pub.is_some(), "strong": dev.strong_pub.is_some()});
        };
        let (payload, sig) = (
            proof["payload"].as_str().unwrap_or_default(),
            proof["sig"].as_str().unwrap_or_default(),
        );
        let mut ok = verify(&k, payload.as_bytes(), sig);
        if ok {
            let signed: Value = serde_json::from_str(payload).unwrap_or(Value::Null);
            ok = false;
            if signed["method"] == "link.present" && signed["params"] == json!({}) {
                let s = self.sess(device, session);
                if let Some(i) = s
                    .challenges
                    .iter()
                    .position(|(c, t)| signed["challenge"] == json!(c) && t.elapsed() < ttl)
                {
                    s.challenges.remove(i);
                    ok = true;
                }
            }
        }
        let s = self.sess(device, session);
        s.present = s.present || ok;
        json!({"present": s.present, "enrolled": true, "strong": dev.strong_pub.is_some()})
    }
    fn decorate(&mut self, device: &str, session: &str, mut status: Value) -> Value {
        let dev = self.devices.get(device).cloned().unwrap_or_default();
        let has_key = dev.unlock_pub.as_deref().and_then(key).is_some();
        let before_yes = self.config.unlock_before_yes;
        let s = self.sess(device, session);
        let mut seen = vec![];
        for ap in status["approvals"].as_array_mut().into_iter().flatten() {
            let id = ap["approval_id"].as_str().unwrap_or_default().to_string();
            let hidden = SECRET_KINDS.contains(&ap["kind"].as_str().unwrap_or_default());
            seen.push(id.clone());
            ap["phone"] = if hidden && !has_key {
                json!({"can_answer": false, "hidden": true, "error": {"code": "unlock_required", "message": "This private answer needs the phone's screen lock, and this phone was paired without one.", "allow": "Answer on the Mac, or set a screen lock on the phone and pair it again"}})
            } else if hidden
                || (before_yes
                    && OWN_CHECKS.contains(&ap["capability"].as_str().unwrap_or_default()))
            {
                let c = s
                    .for_question
                    .entry(id)
                    .or_insert_with(|| token(16))
                    .clone();
                json!({"can_answer": true, "needs_unlock": true, "hidden": hidden, "challenge": c, "fingerprint": !hidden && dev.strong_pub.is_some()})
            } else {
                json!({"can_answer": true, "needs_unlock": false})
            };
        }
        s.for_question.retain(|k, _| seen.contains(k));
        status
    }
    fn status_for(&mut self, device: &str, session: &str) -> Value {
        let status = self.status();
        let status = self.decorate(device, session, status);
        if self.open_window(device, session).is_some() {
            return status;
        }
        let words = self.sess(device, session).present;
        let mut out = status.clone();
        let words_for = |kind: &str| match kind {
            "hidden" => "FERB needs a private answer",
            "check" => "FERB asks before it goes on",
            _ => "FERB has a question",
        };
        out["approvals"] = json!(status["approvals"].as_array().unwrap().iter().map(|ap| {
            let kind = if ap["phone"]["hidden"] == json!(true) { "hidden" } else if ap["phone"]["needs_unlock"] == json!(true) { "check" } else { "question" };
            if words && kind != "hidden" {
                let mut a = ap.clone();
                a["held"] = json!(false);
                a
            } else {
                json!({"approval_id": ap["approval_id"], "kind": ap["kind"], "capability": ap["capability"], "task_id": ap["task_id"], "options": [], "reason": words_for(kind), "held": true, "phone": ap["phone"]})
            }
        }).collect::<Vec<_>>());
        out["results"] = json!(status["results"].as_array().unwrap().iter().map(|r| if words { r.clone() } else { json!({"task_id": r["task_id"], "request_id": r["request_id"], "at": r["at"], "status": r["status"], "held": true}) }).collect::<Vec<_>>());
        out["running"] = json!(
            status["running"]
                .as_array()
                .unwrap()
                .iter()
                .map(|t| if words {
                    t.clone()
                } else {
                    json!({"task_id": t["task_id"], "started_at": t["started_at"], "held": true})
                })
                .collect::<Vec<_>>()
        );
        out["locked"] = json!(true);
        out["words"] = json!(words);
        out
    }
    fn check_answer(
        &mut self,
        device: &str,
        session: &str,
        proof: &Value,
        method: &str,
        params: &Value,
    ) -> Result<(), Refused> {
        let id = params["approval_id"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let ap = match self.approvals.iter().find(|a| a.id == id) {
            Some(a) if a.state == "pending" => a.clone(),
            _ => {
                return Err(refused(
                    "question_not_waiting",
                    "That question isn't waiting any more, so nothing was sent.",
                ));
            }
        };
        let hidden = SECRET_KINDS.contains(&ap.kind.as_str());
        let plain_no = (method == "chat.approve" && params["granted"] == json!(false))
            || (method == "chat.answer" && params["text"].as_str().unwrap_or_default().is_empty());
        if hidden && !plain_no && method != "chat.answer" {
            return Err(refused(
                "bad_request",
                "A private answer is typed into its hidden field, or declined.",
            ));
        }
        if plain_no
            || !(hidden
                || (self.config.unlock_before_yes && OWN_CHECKS.contains(&ap.capability.as_str())))
        {
            return Ok(());
        }
        let what = if hidden {
            "This private answer"
        } else {
            "This answer"
        };
        self.lock_key(device, what)?;
        if !proof.is_object() {
            return Err(refused(
                "unlock_required",
                "Unlock the phone (fingerprint, face or PIN) to give this answer.",
            ));
        }
        let signed = self.signed(device, proof, what, "sent", !hidden)?;
        let invalid = |why: &str| {
            refused(
                "unlock_invalid",
                format!("The phone's unlock proof was not accepted ({why}); nothing was sent."),
            )
        };
        let s = self.sess(device, session);
        let Some(challenge) = s.for_question.get(&id).cloned() else {
            return Err(invalid("the question was asked again since"));
        };
        let same = if hidden {
            signed["sealed"]
                == json!(B64.encode(Sha256::digest(
                    params["text"].as_str().unwrap_or_default().as_bytes()
                )))
                && signed.get("params").is_none()
        } else {
            signed["params"] == *params
        };
        if signed["approval_id"] != json!(id)
            || signed["challenge"] != json!(challenge)
            || signed["method"] != json!(method)
            || !same
        {
            return Err(invalid("it was made for a different answer"));
        }
        s.for_question.remove(&id);
        Ok(())
    }
    fn answer_needs_no_window(
        &mut self,
        device: &str,
        session: &str,
        method: &str,
        params: &Value,
    ) -> bool {
        if !matches!(method, "chat.approve" | "chat.answer" | "chat.correct") {
            return false;
        }
        let id = params["approval_id"].as_str().unwrap_or_default();
        match self.approvals.iter().find(|a| a.id == id) {
            Some(a) if a.state == "pending" => {
                if SECRET_KINDS.contains(&a.kind.as_str())
                    || OWN_CHECKS.contains(&a.capability.as_str())
                {
                    return true;
                }
            }
            _ => return true,
        }
        self.sess(device, session).present && method != "chat.correct"
    }
    /// A path a phone asks for, opened the plain way: the link's own state folder is never shown (FERB's own data);
    /// FERB's protection lists are S3b's (kb T-1203) and not played here.
    fn open_path(&mut self, device: &str, session: &str, path: &str) -> Result<PathBuf, Refused> {
        self.require_unlocked(device, session)?;
        let p = std::fs::canonicalize(path)
            .map_err(|_| refused("not_found", "There is no such file or folder."))?;
        if self.never.iter().any(|n| p.starts_with(n)) {
            return Err(refused(
                "file_not_allowed",
                "FERB does not show this to a phone: it is FERB's own data",
            ));
        }
        Ok(p)
    }
}

pub struct Standin {
    pub socket: PathBuf,
    st: Shared,
    server: tokio::task::JoinHandle<()>,
}
impl Drop for Standin {
    fn drop(&mut self) {
        self.server.abort();
    }
}

impl Standin {
    /// The stand-in on a socket in `dir`; uploads land in `dir/mac/inbox`; `dir/mac` (the bridge's state) is never shown.
    pub async fn start(dir: &Path) -> Self {
        let socket = dir.join("d.sock");
        let files = dir.join("standin-files");
        std::fs::create_dir_all(&files).unwrap();
        let inbox = dir.join("mac/inbox");
        std::fs::create_dir_all(&inbox).unwrap();
        let st = Arc::new(Mutex::new(St {
            devices: HashMap::new(),
            link_events: tokio::sync::broadcast::channel(64).0,
            pending: None,
            devices_max: 5,
            answers: HashMap::new(),
            changed: Arc::new(tokio::sync::Notify::new()),
            listening: 0,
            fetches: HashMap::new(),
            fetch_notice_files: 40,
            fetch_notice_window_s: 600,
            streams_left: 0,
            config: Config {
                unlock_idle_s: 300,
                unlock_max_s: 12 * 3600,
                challenge_ttl_s: 120,
                unlock_before_yes: true,
                list_max: 500,
                upload_free_floor: 1024 * 1024 * 1024,
            },
            sessions: HashMap::new(),
            approvals: vec![],
            clarify: None,
            tasks: vec![],
            results: vec![],
            messages: vec![],
            calls: vec![],
            stopped: false,
            preview_size: 60_000,
            preview_seq: 0,
            fills: HashMap::new(),
            uploads: HashMap::new(),
            groups: HashMap::new(),
            inbox,
            files,
            never: vec![std::fs::canonicalize(dir).unwrap().join("mac")],
            free_disk: None,
            present_delay: Duration::ZERO,
        }));
        let _ = std::fs::remove_file(&socket);
        let listener = UnixListener::bind(&socket).unwrap();
        let st2 = st.clone();
        let server = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                tokio::spawn(connection(st2.clone(), stream));
            }
        });
        Self { socket, st, server }
    }
    /// The free disk the stand-in reports to the upload floor check (None: plenty).
    pub fn set_free_disk(&self, bytes: Option<u64>) {
        self.st.lock().unwrap().free_disk = bytes;
    }
    /// How long the stand-in takes to answer a phone's presence proof (a slow or busy Mac).
    pub fn set_present_delay(&self, d: Duration) {
        self.st.lock().unwrap().present_delay = d;
    }
    /// A call as role `role` straight to the stand-in (the owner's side of a test, or an attack on the daemon).
    pub async fn call_as(&self, role: &str, method: &str, params: Value) -> Result<Value, Value> {
        self.call_from(role, "test", method, params, None).await
    }
    /// The same as a named phone (an attack on the daemon that goes around the bridge), with or without a proof.
    pub async fn call_from(
        &self,
        role: &str,
        device: &str,
        method: &str,
        params: Value,
        proof: Option<Value>,
    ) -> Result<Value, Value> {
        let s = tokio::net::UnixStream::connect(&self.socket).await.unwrap();
        let (rx, mut tx) = s.into_split();
        let mut lines = BufReader::new(rx).lines();
        let hello = json!({"jsonrpc": "2.0", "id": 0, "method": "auth.hello", "params": {"role": role, "device": device, "session": "around-the-bridge"}});
        tx.write_all((hello.to_string() + "\n").as_bytes())
            .await
            .unwrap();
        let hi: Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        if let Some(e) = hi.get("error") {
            return Err(e.clone());
        }
        let mut req = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
        if let Some(p) = proof {
            req["proof"] = p;
        }
        tx.write_all((req.to_string() + "\n").as_bytes())
            .await
            .unwrap();
        let v: Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        match v.get("error") {
            Some(e) => Err(e.clone()),
            None => Ok(v["result"].clone()),
        }
    }
    pub async fn cli(&self, method: &str, params: Value) -> Value {
        self.call_as("cli", method, params).await.unwrap()
    }
    /// The daemon's own numbers the lab's bridge used to hand it (`phone.setup`'s limits): unlock_idle_s,
    /// unlock_max_s, challenge_ttl_s, unlock_before_yes, list_max, upload_free_floor, devices_max, and the upload
    /// folder (`upload_dir`).
    pub fn configure(&self, l: &Value) {
        let mut s = self.st.lock().unwrap();
        if let Some(v) = l["unlock_idle_s"].as_u64() {
            s.config.unlock_idle_s = v;
        }
        if let Some(v) = l["unlock_max_s"].as_u64() {
            s.config.unlock_max_s = v;
        }
        if let Some(v) = l["challenge_ttl_s"].as_u64() {
            s.config.challenge_ttl_s = v;
        }
        if let Some(v) = l["unlock_before_yes"].as_bool() {
            s.config.unlock_before_yes = v;
        }
        if let Some(v) = l["list_max"].as_u64() {
            s.config.list_max = v;
        }
        if let Some(v) = l["upload_free_floor"].as_u64() {
            s.config.upload_free_floor = v;
        }
        if let Some(v) = l["fetch_notice_files"].as_u64() {
            s.fetch_notice_files = v as usize;
        }
        if let Some(v) = l["fetch_notice_window_s"].as_u64() {
            s.fetch_notice_window_s = v;
        }
        if let Some(v) = l["devices_max"].as_u64() {
            s.devices_max = v as usize;
            // the helper reads the list again (and with it the limit) on any change
            let _ = s.link_events.send(json!({"event": "resync"}));
        }
        if let Some(d) = l["upload_dir"].as_str().filter(|d| !d.is_empty()) {
            s.inbox = PathBuf::from(d);
        }
    }
    /// The six digits of the pairing waiting for the owner, if one is.
    pub fn pairing_code(&self) -> Option<String> {
        let s = self.st.lock().unwrap();
        s.pending
            .as_ref()
            .filter(|p| p.decided.is_none())
            .map(|p| p.code.clone())
    }
    /// The owner's answer to the pairing waiting (FERB.app's `phone.pair_decide`).
    pub fn pair_decide(&self, yes: bool) -> Option<bool> {
        let mut s = self.st.lock().unwrap();
        let p = s.pending.clone().filter(|p| p.decided.is_none())?;
        let full = s.devices.len() >= s.devices_max;
        let paired = yes && !full;
        if paired {
            let mut d = p.device.clone();
            d.paired_at = now_text();
            s.devices.insert(p.endpoint.clone(), d);
            let _ = s
                .link_events
                .send(json!({"event": "device_added", "id": p.endpoint}));
        }
        s.pending.as_mut().unwrap().decided = Some(paired);
        s.changed.notify_waiters();
        Some(paired)
    }
    /// The owner removes a phone (FERB.app's `phone.remove`): its sessions end and the helper is told.
    pub fn remove(&self, id: &str) -> bool {
        let mut s = self.st.lock().unwrap();
        let gone = s.devices.remove(id).is_some();
        s.sessions.retain(|(d, _), _| d != id);
        if gone {
            let _ = s
                .link_events
                .send(json!({"event": "device_removed", "id": id}));
        }
        gone
    }
    /// Renames a phone (FERB.app's `phone.rename`).
    pub fn rename(&self, id: &str, name: &str) -> bool {
        let mut s = self.st.lock().unwrap();
        // the real daemon's clean_name: control characters out, 40 characters, trimmed
        let clean: String = name.chars().filter(|c| !c.is_control()).take(40).collect();
        let name = clean.trim().to_string();
        let found = s.devices.contains_key(id);
        if let Some(d) = s.devices.get_mut(id) {
            d.name = name.clone();
            let _ = s
                .link_events
                .send(json!({"event": "device_renamed", "id": id, "name": name}));
        }
        found
    }
    /// Asks the helper something as the daemon does for Settings → Phone (an `ask` on `link.events`), and waits for its
    /// `link.answer`.
    pub async fn ask(&self, what: &str, params: Value) -> Value {
        let id = format!("ask-{}", hex(4));
        // (an ask nobody listens to is lost, as the real daemon's would be: wait for the helper's stream)
        let end = Instant::now() + Duration::from_secs(20);
        while self.helpers() == 0 && Instant::now() < end {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let changed = {
            let s = self.st.lock().unwrap();
            let _ = s
                .link_events
                .send(json!({"event": "ask", "ask_id": id, "what": what, "params": params}));
            s.changed.clone()
        };
        let end = Instant::now() + Duration::from_secs(30);
        loop {
            let wait = changed.notified();
            let answer = self.st.lock().unwrap().answers.remove(&id);
            if let Some(v) = answer {
                return v;
            }
            if Instant::now() > end {
                panic!("the helper never answered {what}");
            }
            let _ = tokio::time::timeout(Duration::from_millis(200), wait).await;
        }
    }
    /// How many helpers listen on `link.events` now.
    pub fn helpers(&self) -> usize {
        self.st.lock().unwrap().listening
    }
}

fn result(id: &Value, v: Value) -> String {
    json!({"jsonrpc": "2.0", "id": id, "result": v}).to_string() + "\n"
}
fn error(id: &Value, code: i64, message: &str) -> String {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}).to_string()
        + "\n"
}

struct Session {
    role: String,
    device: String,
    session: String,
}

async fn connection(st: Shared, stream: tokio::net::UnixStream) {
    let (rx, mut tx) = stream.into_split();
    let mut reader = BufReader::new(rx);
    let (out, mut out_rx) = mpsc::unbounded_channel::<String>();
    let writer = tokio::spawn(async move {
        while let Some(line) = out_rx.recv().await {
            if tx.write_all(line.as_bytes()).await.is_err() {
                return;
            }
        }
    });
    let mut session: Option<Session> = None;
    loop {
        let mut raw = Vec::new();
        match (&mut reader)
            .take(1024 * 1024 + 1)
            .read_until(b'\n', &mut raw)
            .await
        {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let Ok(req) = serde_json::from_slice::<Value>(&raw) else {
            let _ = out.send(error(&Value::Null, PARSE_ERROR, "invalid JSON"));
            continue;
        };
        let (method, id, params) = (
            req["method"].as_str().unwrap_or_default().to_string(),
            req["id"].clone(),
            if req["params"].is_object() {
                req["params"].clone()
            } else {
                json!({})
            },
        );
        let Some(sess) = session.as_ref() else {
            if method != "auth.hello" {
                let _ = out.send(error(&id, UNAUTHORIZED, "authenticate first (auth.hello)"));
                break;
            }
            let role = params["role"].as_str().unwrap_or_default().to_string();
            if role != "cli" && role != "phone" && role != "link" {
                let _ = out.send(error(
                    &id,
                    UNAUTHORIZED,
                    "unknown role, or not the owner's uid",
                ));
                break;
            }
            let s = Session {
                role: role.clone(),
                device: params["device"].as_str().unwrap_or_default().into(),
                session: params["session"].as_str().unwrap_or_default().into(),
            };
            if role == "phone"
                && let Err(Refused(c, w)) = st.lock().unwrap().session(&s.device, &s.session)
            {
                let _ = out.send(error(&id, code_of(c), &w));
                break;
            }
            session = Some(s);
            let _ = out.send(result(&id, json!({"role": role})));
            continue;
        };
        let proof = if req["proof"].is_object() {
            req["proof"].clone()
        } else {
            Value::Null
        };
        if sess.role == "phone" {
            let checked = (|| -> Result<(), (i64, String)> {
                let mut s = st.lock().unwrap();
                s.session(&sess.device, &sess.session)
                    .map_err(|Refused(c, w)| (code_of(c), w))?;
                if LINK_METHODS.contains(&method.as_str()) {
                    return Ok(());
                }
                let class = match role::class(&method) {
                    None => return Err((UNAUTHORIZED, format!("{method} not allowed for phone"))),
                    Some(role::Class::Never) => {
                        return Err((
                            UNAUTHORIZED,
                            format!(
                                "{method} cannot be done from a phone: it {}",
                                role::why(&method)
                            ),
                        ));
                    }
                    Some(c) => c,
                };
                if method == "chat.send"
                    && (params.get("input_paths").is_some() || params.get("test_run").is_some())
                {
                    return Err((UNAUTHORIZED, format!("{method}: not allowed for phone")));
                }
                match class {
                    role::Class::Plain | role::Class::Private => {
                        if !LOCKED_OK.contains(&method.as_str())
                            && !s.answer_needs_no_window(
                                &sess.device,
                                &sess.session,
                                &method,
                                &params,
                            )
                        {
                            s.require_unlocked(&sess.device, &sess.session)
                                .map_err(|Refused(c, w)| (code_of(c), w))?;
                        }
                    }
                    role::Class::Sensitive => s
                        .verify_call(&sess.device, &sess.session, &proof, &method, &params, true)
                        .map_err(|Refused(c, w)| (code_of(c), w))?,
                    role::Class::Never => {}
                }
                Ok(())
            })();
            if let Err((code, words)) = checked {
                let _ = out.send(error(&id, code, &words));
                continue;
            }
        }
        st.lock()
            .unwrap()
            .calls
            .push(json!([sess.role, sess.device, method]));
        if sess.role == "link" && !method.starts_with("link.") {
            let _ = out.send(error(
                &id,
                UNAUTHORIZED,
                &format!("{method} not allowed for link"),
            ));
            continue;
        }
        if method.starts_with("link.") {
            if sess.role != "link" {
                let _ = out.send(error(
                    &id,
                    UNAUTHORIZED,
                    &format!("{method} not allowed for {}", sess.role),
                ));
                continue;
            }
            match method.as_str() {
                "link.events" => {
                    let mut rx = {
                        let mut s = st.lock().unwrap();
                        s.listening += 1;
                        s.link_events.subscribe()
                    };
                    let pump = {
                        let out = out.clone();
                        tokio::spawn(async move {
                            while let Ok(ev) = rx.recv().await {
                                if out.send(json!({"jsonrpc": "2.0", "method": "link.events.event", "params": ev}).to_string() + "\n").is_err() {
                                    return;
                                }
                            }
                        })
                    };
                    let mut one = [0u8; 1];
                    let _ = reader.read(&mut one).await; // the helper left
                    pump.abort();
                    st.lock().unwrap().listening -= 1;
                    break;
                }
                "link.pair_wait" => {
                    let rid = params["request_id"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string();
                    let end = Instant::now() + Duration::from_secs(120);
                    let paired = loop {
                        let changed = st.lock().unwrap().changed.clone();
                        let wait = changed.notified();
                        let decided = {
                            let s = st.lock().unwrap();
                            match s.pending.as_ref().filter(|p| p.request_id == rid) {
                                None => Some(false),
                                Some(p) => p.decided,
                            }
                        };
                        if let Some(d) = decided {
                            break d;
                        }
                        if Instant::now() > end {
                            break false;
                        }
                        let _ = tokio::time::timeout(Duration::from_millis(200), wait).await;
                    };
                    let _ = out.send(result(&id, json!({"paired": paired})));
                    continue;
                }
                _ => {}
            }
        }
        if method == "phone.read" {
            st.lock().unwrap().note_fetch(&sess.device);
        }
        if matches!(method.as_str(), "chat.send" | "phone.read") {
            let (ev_tx, mut ev_rx) = mpsc::unbounded_channel::<Value>();
            let work = {
                let (st, method, params, proof) =
                    (st.clone(), method.clone(), params.clone(), proof.clone());
                let (role, device, sid) =
                    (sess.role.clone(), sess.device.clone(), sess.session.clone());
                tokio::spawn(async move {
                    if method == "chat.send" {
                        chat_send(st, &role, &device, params, ev_tx).await
                    } else {
                        phone_read(st, &device, &sid, params, proof, ev_tx).await
                    }
                })
            };
            let pump = {
                let (out, method) = (out.clone(), method.clone());
                tokio::spawn(async move {
                    while let Some(ev) = ev_rx.recv().await {
                        let _ = out.send(json!({"jsonrpc": "2.0", "method": format!("{method}.event"), "params": ev}).to_string() + "\n");
                    }
                })
            };
            let mut one = [0u8; 1];
            tokio::select! {
                r = work => {
                    let _ = pump.await;
                    let _ = out.send(match r {
                        Ok(Ok(())) => result(&id, json!({"ended": true})),
                        Ok(Err(Refused(c, w))) => error(&id, code_of(c), &w),
                        Err(_) => error(&id, FAILED, "the stream failed"),
                    });
                }
                _ = reader.read(&mut one) => {
                    // the caller left; the task goes on, as the daemon's does
                    st.lock().unwrap().streams_left += 1;
                    break;
                }
            }
            continue;
        }
        if method == "phone.present" {
            let d = st.lock().unwrap().present_delay;
            tokio::time::sleep(d).await;
        }
        let reply = unary(&st, sess, &method, &params, &proof).await;
        let _ = out.send(match reply {
            Ok(v) => result(&id, v),
            Err(Some(Refused(c, w))) => error(&id, code_of(c), &w),
            Err(None) => error(&id, METHOD_NOT_FOUND, &format!("no method {method}")),
        });
    }
    drop(out);
    let _ = writer.await;
}

async fn unary(
    st: &Shared,
    sess: &Session,
    method: &str,
    p: &Value,
    proof: &Value,
) -> Result<Value, Option<Refused>> {
    let (dev, sid) = (sess.device.as_str(), sess.session.as_str());
    let phone = sess.role == "phone";
    let mut s = st.lock().unwrap();
    let r = |x: Result<Value, Refused>| x.map_err(Some);
    Ok(match method {
        "ping" => json!({"pid": std::process::id(), "role": sess.role, "now": now_text()}),
        "chat.status" if phone => s.status_for(dev, sid),
        "chat.status" => s.status(),
        "chat.attach" => {
            json!({"ok": true, "session_id": "default", "capabilities": ["input_paths"]})
        }
        "chat.approve" | "chat.answer" | "chat.correct" => {
            if phone {
                r(s.check_answer(dev, sid, proof, method, p)
                    .map(|_| Value::Null))?;
            }
            let id = p["approval_id"].as_str().unwrap_or_default().to_string();
            match method {
                "chat.approve" => r(s.resolve(
                    &id,
                    p["granted"] == json!(true),
                    p["choice"].clone(),
                    None,
                    &sess.role,
                ))?,
                "chat.answer" => {
                    let text = p["text"].as_str().unwrap_or_default().to_string();
                    r(s.resolve(
                        &id,
                        !text.is_empty(),
                        Value::Null,
                        Some(text).filter(|t| !t.is_empty()),
                        &sess.role,
                    ))?
                }
                _ => match s.approval(&id) {
                    Some(a) if a.state == "pending" => {
                        json!({"approval_id": a.id, "state": "pending", "task_id": a.task_id, "message": p["text"], "reply": "Told the task."})
                    }
                    _ => {
                        return Err(Some(refused(
                            "ferb_error",
                            "That question isn't waiting any more, so nothing was sent to the task.",
                        )));
                    }
                },
            }
        }
        "results.ack" => {
            let ids = p["task_ids"].as_array().cloned().unwrap_or_default();
            for res in s.results.iter_mut() {
                if ids.contains(&res["task_id"]) {
                    res["shown"] = json!(true);
                }
            }
            json!({"ok": true})
        }
        "attention.ack" => json!({"ok": true}),
        "task.stop" => {
            let ids: Vec<String> = p["task_ids"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect();
            if ids.is_empty() {
                return Err(Some(refused(
                    "ferb_error",
                    "task.stop needs the ids of the tasks to stop; stopping everything is stop.engage",
                )));
            }
            let mut stopped = vec![];
            for t in s
                .tasks
                .iter_mut()
                .filter(|t| ids.contains(&t.id) && !t.done)
            {
                t.stop = true;
                stopped.push(t.id.clone());
            }
            json!({"stopped": stopped})
        }
        "stop.engage" => {
            s.stopped = true;
            s.tasks.iter_mut().for_each(|t| t.stop = true);
            json!({"stopped": true})
        }
        "stop.lift" => {
            s.stopped = false;
            json!({"stopped": false})
        }
        "tasks.list" => {
            let tasks: Vec<Value> = s.tasks.iter().rev().map(|t| json!({"task_id": t.id, "goal": t.goal, "status": if t.done { t.status.as_str() } else { "running" }, "answer": t.answer})).collect();
            json!({"tasks": tasks, "more": false})
        }
        "tasks.preview" => {
            let tid = p["task_id"].as_str().unwrap_or_default().to_string();
            if !s.tasks.iter().any(|t| t.id == tid && !t.done) {
                json!({"state": "none"})
            } else {
                s.preview_seq += 1;
                let mut img = format!("\u{ff}FAKE-JPEG seq={} ", s.preview_seq).into_bytes();
                img.resize(s.preview_size.max(img.len()), 0x5a);
                img.truncate(s.preview_size);
                json!({"state": "working", "line": "FERB is working", "seq": s.preview_seq, "age_ms": 0, "fresh": true, "image": B64S.encode(img)})
            }
        }
        "trace.get" => {
            json!({"task_id": p["task_id"], "trace": "step ".repeat(p["steps"].as_u64().unwrap_or(40_000) as usize)})
        }
        // stage 2's pages (kb ENG-14), fake: `limit` entries carrying their position, `more` when older ones exist
        "chat.entries.page" => {
            let limit = p["limit"].as_u64().unwrap_or(50);
            let entries: Vec<Value> = (0..limit).map(|i| json!({"pos": 1000 - i, "kind": "message", "body": {"text": format!("entry {i}")}})).collect();
            json!({"conversation": "c", "epoch": 1, "head": 1000, "entries": entries, "more": true, "before_pos": 1000 - limit, "asked": p})
        }
        "chat.history.page" => json!({"entries": [], "asked": p}),
        "questions.picture" | "seen.image" => json!({"image": B64S.encode([0u8; 64])}),
        // ---- the phone link's own methods ----
        // ---- the helper's own calls (role link; the connection refuses them to any other role) ----
        "link.devices" => {
            let devices: Vec<Value> = s.devices.iter().map(|(id, d)| json!({"id": id, "name": d.name, "app": d.app, "paired_at": d.paired_at,
                "last_seen": d.paired_at, "unlock": d.unlock_pub.is_some(), "device_key": d.device_pub.is_some(), "fingerprint_key": d.strong_pub.is_some()})).collect();
            json!({"devices": devices, "max": s.devices_max})
        }
        "link.pair_request" => {
            if s.devices.len() >= s.devices_max {
                return Err(Some(refused(
                    "device_limit",
                    format!(
                        "FERB already has {} paired phones, the most it keeps. Remove one on the Mac first.",
                        s.devices_max
                    ),
                )));
            }
            let endpoint = p["endpoint"].as_str().unwrap_or_default().to_string();
            let text = |k: &str| {
                p[k].as_str()
                    .filter(|v| key(v).is_some())
                    .map(str::to_string)
            };
            let rid = token(12);
            s.pending = Some(PendingPair {
                request_id: rid.clone(),
                endpoint,
                code: p["code"].as_str().unwrap_or_default().to_string(),
                device: Device {
                    name: p["name"].as_str().unwrap_or("Phone").to_string(),
                    app: p["app"].as_str().unwrap_or_default().to_string(),
                    unlock_pub: text("unlock_pub"),
                    device_pub: text("device_pub"),
                    strong_pub: text("strong_pub"),
                    ..Default::default()
                },
                decided: None,
            });
            s.changed.notify_waiters();
            json!({"request_id": rid, "seconds": 120})
        }
        "link.pair_cancel" => {
            let rid = p["request_id"].as_str().unwrap_or_default();
            let cancelled = s
                .pending
                .as_ref()
                .is_some_and(|x| x.request_id == rid && x.decided.is_none());
            if cancelled {
                s.pending = None;
            }
            s.changed.notify_waiters();
            json!({"cancelled": cancelled})
        }
        "link.answer" => {
            let id = p["ask_id"].as_str().unwrap_or_default().to_string();
            s.answers.insert(id, p.clone());
            s.changed.notify_waiters();
            json!({"ok": true})
        }
        "phone.challenge" => s.challenge(dev, sid),
        "phone.unlock" => r(s.unlock(dev, sid, proof))?,
        "phone.lock" => {
            s.lock(dev, sid);
            json!({"unlocked": false})
        }
        "phone.present" => s.present(dev, sid, proof),
        "phone.keys" => {
            let mut want = serde_json::Map::new();
            for k in ["device_pub", "strong_pub"] {
                if let Some(v) = p.get(k).filter(|v| !v.is_null()) {
                    want.insert(k.into(), v.clone());
                }
            }
            if want.is_empty() || want.values().any(|v| v.as_str().and_then(key).is_none()) {
                return Err(Some(refused("bad_request", "Those are not keys.")));
            }
            r(s.verify_call(dev, sid, proof, "link.keys", p, true)
                .map(|_| Value::Null))?;
            let d = s.devices.get_mut(dev).unwrap();
            if d.device_pub.is_none() {
                d.device_pub = want
                    .get("device_pub")
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
            }
            if d.strong_pub.is_none() {
                d.strong_pub = want
                    .get("strong_pub")
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
            }
            json!({"device_pub": d.device_pub, "strong_pub": d.strong_pub})
        }
        "phone.session_end" => {
            s.sessions.remove(&(dev.to_string(), sid.to_string()));
            json!({"ok": true})
        }
        "phone.files" => {
            let path = p["path"]
                .as_str()
                .unwrap_or("~")
                .replace('~', &std::env::var("HOME").unwrap_or_default());
            let real = r(s.open_path(dev, sid, &path).map(|p| json!(p)))?;
            let real = PathBuf::from(real.as_str().unwrap());
            // pages of list_max by name, the next one asked by the Mac's marker (as the daemon's, kb PHN-589: bound
            // to its folder; another folder's is a bad request)
            let key = format!("files|{}|", real.display());
            let from = match p["after"].as_str() {
                None => 0,
                Some(m) => match m.strip_prefix(&key).and_then(|n| n.parse::<usize>().ok()) {
                    Some(n) => n,
                    None => {
                        return Err(Some(refused(
                            "bad_request",
                            "That page marker is not this folder's.",
                        )));
                    }
                },
            };
            let mut all: Vec<_> = std::fs::read_dir(&real)
                .into_iter()
                .flatten()
                .flatten()
                .collect();
            all.sort_by_key(|e| e.file_name());
            let total = all.len();
            let mut entries = vec![];
            for e in all.iter().skip(from).take(s.config.list_max as usize) {
                let m = e.metadata().ok();
                entries.push(json!({"name": e.file_name().to_string_lossy(), "dir": m.as_ref().is_some_and(|m| m.is_dir()), "size": m.as_ref().map(|m| m.len()).unwrap_or(0)}));
            }
            let more = from + entries.len() < total;
            let next = if more {
                json!(format!("{key}{}", from + entries.len()))
            } else {
                Value::Null
            };
            json!({"path": real, "entries": entries, "total": total, "from": from, "cut": more, "next": next})
        }
        "phone.tree" => {
            let real = r(s
                .open_path(dev, sid, p["path"].as_str().unwrap_or_default())
                .map(|p| json!(p)))?;
            let real = PathBuf::from(real.as_str().unwrap());
            let mut files = vec![];
            fn walk(base: &Path, dir: &Path, out: &mut Vec<Value>) {
                let mut all: Vec<_> = std::fs::read_dir(dir)
                    .into_iter()
                    .flatten()
                    .flatten()
                    .collect();
                all.sort_by_key(|e| e.file_name());
                for e in all {
                    let p = e.path();
                    if p.is_dir() {
                        walk(base, &p, out);
                    } else if p.is_file() {
                        out.push(json!({"rel": p.strip_prefix(base).unwrap().to_string_lossy(), "size": e.metadata().map(|m| m.len()).unwrap_or(0)}));
                    }
                }
            }
            walk(&real, &real, &mut files);
            json!({"path": real, "files": files, "skipped": 0, "cut": false})
        }
        "phone.put_group" => {
            r(s.require_unlocked(dev, sid).map(|_| Value::Null))?;
            let gid = token(9);
            let folder = s.inbox.join(format!(
                "{gid}-{}",
                clean_name(p["name"].as_str().unwrap_or_default(), "folder")
            ));
            std::fs::create_dir_all(&folder).unwrap();
            s.groups
                .insert(gid.clone(), (dev.to_string(), folder.clone()));
            json!({"group": gid, "saved": folder})
        }
        "phone.put_begin" => {
            let Some(size) = p["size"].as_u64() else {
                return Err(Some(refused("bad_request", "link.put needs the size")));
            };
            r(s.require_unlocked(dev, sid).map(|_| Value::Null))?;
            if let Some(resume) = p["resume"].as_str() {
                let Some(up) = s
                    .uploads
                    .get_mut(resume)
                    .filter(|u| u.device == dev && !u.done && u.size == size && !u.open)
                else {
                    return Err(Some(refused(
                        "not_found",
                        "That upload cannot be continued; send the file again.",
                    )));
                };
                up.open = true;
                return Ok(json!({"upload_id": resume, "offset": up.got}));
            }
            let free = s.free_disk.unwrap_or(1 << 50); // (a petabyte unless a test says otherwise)
            if free.saturating_sub(size) < s.config.upload_free_floor {
                return Err(Some(refused(
                    "mac_disk_full",
                    format!(
                        "Your Mac has {free} bytes free and FERB keeps {} free, so this file was not taken.",
                        s.config.upload_free_floor
                    ),
                )));
            }
            let id = token(12);
            let name = clean_name(p["name"].as_str().unwrap_or("file"), "file").replace(' ', "");
            let path = match p["group"].as_str() {
                Some(g) => {
                    let Some((owner, folder)) = s.groups.get(g).cloned() else {
                        return Err(Some(refused(
                            "not_found",
                            "That folder upload was not started in this session.",
                        )));
                    };
                    if owner != dev {
                        return Err(Some(refused(
                            "not_found",
                            "That folder upload was not started in this session.",
                        )));
                    }
                    let parts: Vec<String> = p["rel"]
                        .as_str()
                        .unwrap_or(&name)
                        .split('/')
                        .map(|x| clean_name(x, ""))
                        .collect();
                    if parts.iter().any(|x| x.is_empty() || x == "." || x == "..") {
                        return Err(Some(refused(
                            "bad_request",
                            "That file's place inside the folder cannot be used.",
                        )));
                    }
                    let mut where_ = folder.clone();
                    for x in &parts[..parts.len() - 1] {
                        where_ = where_.join(x);
                    }
                    std::fs::create_dir_all(&where_).unwrap();
                    where_.join(parts.last().unwrap())
                }
                None => s.inbox.join(format!("{id}-{name}")),
            };
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            if std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .is_err()
            {
                return Err(Some(refused(
                    "bad_request",
                    "A file of that name was already sent into this folder.",
                )));
            }
            s.uploads.insert(
                id.clone(),
                Upload {
                    device: dev.into(),
                    path,
                    size,
                    got: 0,
                    hasher: Sha256::new(),
                    open: true,
                    done: false,
                },
            );
            json!({"upload_id": id, "offset": 0})
        }
        "phone.put_chunk" => {
            let id = p["upload_id"].as_str().unwrap_or_default();
            let Some(up) = s.uploads.get_mut(id).filter(|u| u.device == dev && u.open) else {
                return Err(Some(refused("not_found", "No such upload is under way.")));
            };
            let data = B64S
                .decode(p["d"].as_str().unwrap_or_default())
                .unwrap_or_default();
            if up.got + data.len() as u64 > up.size {
                return Err(Some(refused(
                    "bad_request",
                    "More bytes arrived than the transfer announced.",
                )));
            }
            std::fs::OpenOptions::new()
                .append(true)
                .open(&up.path)
                .unwrap()
                .write_all(&data)
                .unwrap();
            up.hasher.update(&data);
            up.got += data.len() as u64;
            json!({"got": up.got})
        }
        "phone.put_end" => {
            let id = p["upload_id"].as_str().unwrap_or_default().to_string();
            let Some(up) = s.uploads.get_mut(&id).filter(|u| u.device == dev && u.open) else {
                return Err(Some(refused("not_found", "No such upload is under way.")));
            };
            up.open = false;
            if p["abort"] == json!(true) || up.got != up.size {
                let got = up.got;
                if p["drop"] == json!(true) || got == 0 {
                    let _ = std::fs::remove_file(&up.path);
                    s.uploads.remove(&id);
                }
                json!({"upload_id": id, "kept": s.uploads.contains_key(&id), "got": got})
            } else {
                up.done = true;
                json!({"upload_id": id, "saved": up.path, "size": up.size, "sha256": B64.encode(up.hasher.clone().finalize())})
            }
        }
        // ---- the test's own controls (cli) ----
        "standin.ask" => {
            let options = p["options"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_else(|| vec!["Yes".into(), "No".into()]);
            let id = s.ask(
                p["kind"].as_str().unwrap_or("owner_question"),
                p["capability"].as_str().unwrap_or("owner.ask"),
                p["reason"].as_str().unwrap_or("?"),
                p["exact"].as_str().unwrap_or(""),
                options,
                "",
            );
            json!({"approval_id": id})
        }
        "standin.cancel" => {
            if let Some(a) = s.approval(p["approval_id"].as_str().unwrap_or_default())
                && a.state == "pending"
            {
                a.state = "superseded".into();
            }
            json!({"ok": true})
        }
        "standin.fill_matches" => {
            json!({"matches": s.fills.get(p["approval_id"].as_str().unwrap_or_default()).map(String::as_str) == p["value"].as_str()})
        }
        "standin.preview_size" => {
            s.preview_size = p["bytes"].as_u64().unwrap_or(60_000) as usize;
            json!({"ok": true})
        }
        "standin.state" => json!({
            "approvals": s.approvals.iter().map(|a| json!({"approval_id": a.id, "kind": a.kind, "capability": a.capability, "state": a.state, "answer": a.answer})).collect::<Vec<_>>(),
            "messages": s.messages, "calls": s.calls, "stopped": s.stopped, "streams_left": s.streams_left,
        }),
        m if role::class(m).is_some() || m.starts_with("settings.") => {
            json!({"ok": true, "note": "stand-in", "method": m})
        }
        _ => return Err(None),
    })
}

fn ev(request_id: &str, mut v: Value) -> Value {
    v["request_id"] = json!(request_id);
    v["at"] = json!(now_text());
    v
}

async fn chat_send(
    st: Shared,
    role: &str,
    device: &str,
    params: Value,
    out: mpsc::UnboundedSender<Value>,
) -> Result<(), Refused> {
    let text = params["text"].as_str().unwrap_or_default().to_string();
    let request_id = format!("rq_{}", hex(4));
    let paths: Vec<Value> = if role == "phone" {
        let s = st.lock().unwrap();
        let mut v = vec![];
        for i in params["attachments"].as_array().into_iter().flatten() {
            let i = i.as_str().unwrap_or_default();
            match (s.uploads.get(i), s.groups.get(i)) {
                (Some(up), _) if up.device == device && up.done => v.push(json!(up.path)),
                (_, Some((d, folder))) if d == device => v.push(json!(folder)),
                _ => {
                    return Err(refused(
                        "not_found",
                        "An attachment of this message was not uploaded from this phone.",
                    ));
                }
            }
        }
        v
    } else {
        params["input_paths"]
            .as_array()
            .cloned()
            .unwrap_or_default()
    };
    st.lock()
        .unwrap()
        .messages
        .push(json!({"text": text, "input_paths": paths, "role": role, "device": device}));
    let _ = out.send(ev(&request_id, json!({"type": "ack", "text": text})));
    let (word, rest) = text.split_once(' ').unwrap_or((&text, ""));
    // any message answers the planner's question that waited; `clarify <words>` asks one (the turn ends with it)
    st.lock().unwrap().clarify = None;
    if word == "clarify" {
        st.lock().unwrap().clarify = Some((format!("clarify:{request_id}"), rest.to_string()));
        let _ = out.send(ev(
            &request_id,
            json!({"type": "clarify", "text": rest, "status": "done"}),
        ));
        return Ok(());
    }
    if st.lock().unwrap().stopped {
        let _ = out.send(ev(&request_id, json!({"type": "reply", "text": "FERB is stopped. Lift the stop on the Mac first.", "status": "failed"})));
        return Ok(());
    }
    let new_task = |st: &Shared, goal: &str| {
        let id = format!("t_{}", hex(3));
        st.lock().unwrap().tasks.push(Task {
            id: id.clone(),
            goal: goal.into(),
            started_at: now_text(),
            stop: false,
            files: vec![],
            done: false,
            status: "running".into(),
            answer: String::new(),
        });
        id
    };
    let stopped = |st: &Shared, id: &str| {
        st.lock()
            .unwrap()
            .tasks
            .iter()
            .any(|t| t.id == id && t.stop)
    };
    let finish = |st: &Shared, id: &str, status: &str, answer: &str, files: Vec<PathBuf>| {
        let mut s = st.lock().unwrap();
        if let Some(t) = s.tasks.iter_mut().find(|t| t.id == id) {
            t.done = true;
            t.status = status.into();
            t.answer = answer.into();
            t.files = files.clone();
        }
        let artifacts: Vec<Value> = files.iter().map(|f| json!(f)).collect();
        s.results.push(json!({"task_id": id, "request_id": request_id, "at": now_text(), "status": "done", "goal": text, "answer": answer, "name": text.chars().take(40).collect::<String>(), "artifacts": artifacts}));
    };
    match word {
        "ask" | "confirm" | "secret" | "code" => {
            let tid = new_task(&st, &text);
            let _ = out.send(ev(
                &request_id,
                json!({"type": "plan", "task_id": tid, "text": format!("Working on: {text}")}),
            ));
            let aid = {
                let mut s = st.lock().unwrap();
                match word {
                    "ask" => s.ask(
                        "owner_question",
                        "owner.ask",
                        if rest.is_empty() { "Which one?" } else { rest },
                        "",
                        vec![],
                        &tid,
                    ),
                    "confirm" => s.ask(
                        "owner_question",
                        "ferb.confirm",
                        &format!("FERB is about to: {rest}"),
                        rest,
                        vec!["Yes".into(), "No".into()],
                        &tid,
                    ),
                    "code" => s.ask(
                        "owner_login_code",
                        "login.code",
                        "The site sent a login code. What is it?",
                        "",
                        vec![],
                        &tid,
                    ),
                    _ => s.ask(
                        "owner_secret",
                        "secret.fill",
                        "The page asks for a password",
                        "",
                        vec![],
                        &tid,
                    ),
                }
            };
            let reason = st.lock().unwrap().approval(&aid).unwrap().reason.clone();
            let _ = out.send(ev(&request_id, json!({"type": "approval.requested", "task_id": tid, "approval_id": aid, "reason": reason})));
            let ap = loop {
                let ap = st.lock().unwrap().approval(&aid).unwrap().clone();
                if ap.state != "pending" || stopped(&st, &tid) {
                    break ap;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            };
            let answer = if ap.state == "pending" {
                st.lock().unwrap().approval(&aid).unwrap().state = "superseded".into();
                "Stopped.".to_string()
            } else if ap.state == "granted" && ap.answer["sealed"] == json!(true) {
                "Filled the hidden field with what you typed.".to_string()
            } else if ap.state == "granted" {
                format!(
                    "Done after your answer: {}",
                    ap.answer["text"].as_str().unwrap_or("Yes")
                )
            } else {
                "Not done: you said No.".to_string()
            };
            finish(&st, &tid, "done", &answer, vec![]);
            let _ = out.send(ev(
                &request_id,
                json!({"type": "outcome", "task_id": tid, "text": answer, "status": "done"}),
            ));
        }
        "long" | "file" => {
            let tid = new_task(&st, &text);
            let _ = out.send(ev(
                &request_id,
                json!({"type": "plan", "task_id": tid, "text": format!("Working on: {text}")}),
            ));
            let mut files = vec![];
            if word == "file" {
                let path = st.lock().unwrap().files.join(format!("{tid}.bin"));
                let n: usize = rest.parse().unwrap_or(1000);
                let mut data = vec![0u8; n];
                rand::RngCore::fill_bytes(&mut rand::rng(), &mut data);
                std::fs::write(&path, data).unwrap();
                files.push(path);
            } else {
                let end = Instant::now() + Duration::from_secs_f64(rest.parse().unwrap_or(5.0));
                let mut n = 0;
                while Instant::now() < end && !stopped(&st, &tid) {
                    n += 1;
                    let _ = out.send(ev(
                        &request_id,
                        json!({"type": "tool.called", "task_id": tid, "text": format!("step {n}")}),
                    ));
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
            }
            let stop = stopped(&st, &tid);
            let answer = if stop { "Stopped by you." } else { "Finished." };
            finish(
                &st,
                &tid,
                if stop { "stopped" } else { "done" },
                answer,
                files,
            );
            let _ = out.send(ev(&request_id, json!({"type": "outcome", "task_id": tid, "text": answer, "status": if stop { "stopped" } else { "done" }})));
        }
        "big" => {
            let _ = out.send(ev(&request_id, json!({"type": "reply", "text": "x".repeat(rest.parse().unwrap_or(1000)), "status": "done"})));
        }
        _ => {
            let _ = out.send(ev(&request_id, json!({"type": "reply", "text": format!("FERB (stand-in) got: {text}"), "status": "done"})));
        }
    }
    Ok(())
}

/// `phone.read`: {meta}, {sha256}, then {d: base64} pieces from `offset` on (standin/phone.py `read`).
async fn phone_read(
    st: Shared,
    device: &str,
    session: &str,
    p: Value,
    _proof: Value,
    out: mpsc::UnboundedSender<Value>,
) -> Result<(), Refused> {
    let path = {
        let mut s = st.lock().unwrap();
        if p["artifact"].is_object() {
            s.require_unlocked(device, session)?;
            let tid = p["artifact"]["task_id"].as_str().unwrap_or_default();
            let index = p["artifact"]["index"].as_u64().unwrap_or(0) as usize;
            match s
                .tasks
                .iter()
                .find(|t| t.id == tid)
                .and_then(|t| t.files.get(index))
            {
                Some(f) => f.clone(),
                None => return Err(refused("not_found", "FERB has no such file.")),
            }
        } else {
            s.open_path(device, session, p["path"].as_str().unwrap_or_default())?
        }
    };
    if path.is_dir() {
        return Err(refused("not_a_file", "That is a folder."));
    }
    let data =
        std::fs::read(&path).map_err(|_| refused("not_found", "FERB no longer has that file."))?;
    let total = data.len() as u64;
    let offset = p["offset"].as_u64().unwrap_or(0).min(total) as usize;
    let rest = &data[offset..];
    let _ = out.send(json!({"meta": {"size": rest.len(), "total": total, "offset": offset, "name": path.file_name().unwrap().to_string_lossy(), "real": path, "high_value": false}}));
    let _ = out.send(json!({"sha256": B64.encode(Sha256::digest(rest))}));
    for piece in rest.chunks(READ_PIECE) {
        if out.send(json!({"d": B64S.encode(piece)})).is_err() {
            break;
        }
        tokio::task::yield_now().await;
    }
    Ok(())
}
