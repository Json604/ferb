//! A real ferb-rpc server with the daemon's phone area on a temporary socket and store, an injected clock and nonces,
//! stub handlers for every other method of the table (they answer, they don't act), and a client that speaks as the
//! phone link's helper does: `auth.hello {role: "phone", device, session}`, `proof` beside `params`.
#![allow(dead_code)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use chrono::{FixedOffset, TimeZone};
use ferb_core::interfaces::{BoxFuture, Clock};
use ferb_daemon::phone::{
    proof::SPKI, Limits, MacPrompts, Phones, QuestionFacts, Questions, ScreenPrompt, Seams,
};
use ferb_rpc::server::{EventStream, HandlerResult};
use ferb_rpc::{Params, RunTokens, Server, Session};
use p256::ecdsa::signature::Signer;
use p256::ecdsa::{Signature, SigningKey};
use serde_json::{json, Map, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::UnixStream;

/// Wall time and monotonic seconds that move only when a test says.
pub struct FakeClock {
    pub secs: Mutex<f64>,
}

impl FakeClock {
    pub fn advance(&self, s: f64) {
        *self.secs.lock().unwrap() += s;
    }
}

impl Clock for FakeClock {
    fn now(&self) -> ferb_core::when::DateTime {
        let start = FixedOffset::east_opt(5 * 3600 + 1800)
            .unwrap()
            .with_ymd_and_hms(2026, 11, 16, 10, 0, 0)
            .unwrap();
        start + chrono::Duration::milliseconds((*self.secs.lock().unwrap() * 1000.0) as i64)
    }
    fn monotonic(&self) -> f64 {
        1000.0 + *self.secs.lock().unwrap()
    }
    fn sleep_until(&self, _t: ferb_core::when::DateTime) -> BoxFuture<'_, ()> {
        Box::pin(async {})
    }
}

/// The questions FERB has, as a test sets them.
#[derive(Default)]
pub struct StubQuestions {
    pub facts: Mutex<HashMap<String, QuestionFacts>>,
    pub asked: Mutex<Vec<(String, String)>>,
}

impl Questions for StubQuestions {
    fn facts(&self, id: &str) -> Option<QuestionFacts> {
        self.facts.lock().unwrap().get(id).cloned()
    }
    fn ask_check(&self, reason: &str, exact: &str, _task_id: &str) -> String {
        let id = format!("ap_prompt{}", self.asked.lock().unwrap().len());
        self.asked
            .lock()
            .unwrap()
            .push((reason.to_owned(), exact.to_owned()));
        self.facts.lock().unwrap().insert(
            id.clone(),
            QuestionFacts {
                pending: true,
                hidden: false,
                own_check: true,
                routine: false,
            },
        );
        id
    }
}

/// Prompts on the Mac's screen: each on-screen call leaves one; `clickable` decides whether the click is taken.
pub struct FakePrompts {
    pub clickable: bool,
    pub clicked: Mutex<Vec<String>>,
}

impl MacPrompts for FakePrompts {
    fn left_open(&self, method: &str, _params: &Params) -> Option<ScreenPrompt> {
        Some(ScreenPrompt {
            title: format!("FERB would like access ({method})"),
            button: "Allow".into(),
        })
    }
    fn click(&self, prompt: &ScreenPrompt) -> Result<(), String> {
        self.clicked.lock().unwrap().push(prompt.title.clone());
        if self.clickable {
            Ok(())
        } else {
            Err("the prompt is still there".into())
        }
    }
}

/// A phone's three keys (fixed scalars: the same bytes on every run).
pub struct Keys {
    pub lock: SigningKey,
    pub strong: SigningKey,
    pub device: SigningKey,
}

impl Keys {
    pub fn new(seed: u8) -> Keys {
        let k = |n: u8| {
            SigningKey::from_bytes(&[seed.wrapping_mul(3).wrapping_add(n); 32].into()).unwrap()
        };
        Keys {
            lock: k(1),
            strong: k(2),
            device: k(3),
        }
    }
}

pub fn spki(k: &SigningKey) -> String {
    let mut der = SPKI.to_vec();
    der.extend_from_slice(k.verifying_key().to_encoded_point(false).as_bytes());
    STANDARD.encode(der)
}

pub fn signed(k: &SigningKey, payload: &str) -> Value {
    let sig: Signature = k.sign(payload.as_bytes());
    json!({"payload": payload, "sig": STANDARD.encode(sig.to_der().as_bytes())})
}

/// The proof of one call: `{challenge, method, params}` signed.
pub fn call_proof(k: &SigningKey, challenge: &str, method: &str, params: &Value) -> Value {
    signed(
        k,
        &json!({"challenge": challenge, "method": method, "params": params}).to_string(),
    )
}

pub struct World {
    pub dir: tempfile::TempDir,
    pub server: Server,
    pub phones: Phones,
    pub clock: Arc<FakeClock>,
    pub questions: Arc<StubQuestions>,
    pub journal: Arc<Mutex<Vec<(String, Value)>>>,
    pub published: Arc<Mutex<Vec<(String, Value)>>>,
    pub db: Arc<Mutex<ferb_store::Db>>,
    /// What the stub handlers were called with (method, params).
    pub calls: Arc<Mutex<Vec<(String, Value)>>>,
}

impl World {
    pub fn socket(&self) -> PathBuf {
        self.server.path().to_path_buf()
    }
}

struct One(Option<HandlerResult>);
impl EventStream for One {
    fn next(&mut self) -> ferb_rpc::BoxFuture<'_, Option<HandlerResult>> {
        Box::pin(std::future::ready(self.0.take()))
    }
}

/// A world with the phone area and stub handlers, its server started; `prompts` as given.
pub async fn world(prompts: Option<Arc<dyn MacPrompts>>) -> World {
    let dir = tempfile::tempdir().unwrap();
    let db = ferb_store::Db::open(dir.path().join("ferb.db")).unwrap();
    db.migrate().unwrap();
    let db = Arc::new(Mutex::new(db));
    let clock = Arc::new(FakeClock {
        secs: Mutex::new(0.0),
    });
    let questions = Arc::new(StubQuestions::default());
    let journal = Arc::new(Mutex::new(Vec::new()));
    let published = Arc::new(Mutex::new(Vec::new()));
    let n = Arc::new(AtomicU64::new(1));
    let seams = Seams {
        db: db.clone(),
        clock: clock.clone(),
        nonce: Arc::new(move || {
            let mut b = [0u8; 16];
            b[8..].copy_from_slice(&n.fetch_add(1, Ordering::SeqCst).to_be_bytes());
            b
        }),
        redact: Arc::new(|v: &Value| v.clone()),
        questions: questions.clone(),
        prompts,
        journal: {
            let j = journal.clone();
            Arc::new(move |k: &str, v: Value| j.lock().unwrap().push((k.to_owned(), v)))
        },
        publish: {
            let p = published.clone();
            Arc::new(move |k: &str, v: Map<String, Value>| {
                p.lock().unwrap().push((k.to_owned(), Value::Object(v)))
            })
        },
    };
    let phones = Phones::new(seams, Limits::default());
    let tokens = Arc::new(RunTokens::new(Arc::new(ferb_rpc::SystemClock::default())));
    let server = Server::new(dir.path().join("run/d.sock"), tokens);
    // every peer of this test is the link's helper (the peer check itself: ferb-rpc tests/phone_entry.rs)
    server.set_is_link(|_| true);
    let calls = Arc::new(Mutex::new(Vec::new()));
    for m in ferb_rpc::auth::METHODS {
        let row = ferb_rpc::classes::phone_method(m.name);
        let (name, calls) = (m.name, calls.clone());
        if row.is_some_and(|r| r.stream) {
            server.register_stream(name, move |_s: Session, p: Params| {
                calls
                    .lock()
                    .unwrap()
                    .push((name.to_owned(), Value::Object(p.clone())));
                let mut ev = Map::new();
                ev.insert("method".into(), json!(name));
                ev.insert("params".into(), Value::Object(p));
                Box::new(One(Some(Ok(ev)))) as Box<dyn EventStream>
            });
        } else {
            let q = questions.clone();
            server.register_unary(name, move |_s: Session, p: Params| {
                calls
                    .lock()
                    .unwrap()
                    .push((name.to_owned(), Value::Object(p.clone())));
                let out = stub(name, &p, &q);
                async move { out }
            });
        }
    }
    phones.register(&server);
    server.start().await.unwrap();
    World {
        dir,
        server,
        phones,
        clock,
        questions,
        journal,
        published,
        db,
        calls,
    }
}

/// What a stub answers: chat.status lists the pending questions with their words; an answer resolves its question.
fn stub(name: &str, p: &Params, q: &StubQuestions) -> HandlerResult {
    let mut out = Map::new();
    match name {
        "chat.status" => {
            let approvals: Vec<Value> = q
                .facts
                .lock()
                .unwrap()
                .iter()
                .filter(|(_, f)| f.pending)
                .map(|(id, f)| {
                    let kind = if f.hidden { "owner_secret" } else { "owner_question" };
                    let cap = if f.own_check { "ferb.confirm" } else { "owner.ask" };
                    json!({"approval_id": id, "kind": kind, "capability": cap, "reason": format!("the words of {id}"),
                           "options": ["Yes", "No"], "task_id": "t1"})
                })
                .collect();
            out.insert("approvals".into(), Value::Array(approvals));
            out.insert(
                "results".into(),
                json!([{"task_id": "t0", "at": "a", "status": "done", "goal": "the goal's words", "answer": "the answer's words"}]),
            );
            out.insert(
                "running".into(),
                json!([{"task_id": "t1", "goal": "a running goal", "started_at": "s"}]),
            );
            out.insert("notices".into(), json!([{"text": "a notice's words"}]));
            out.insert("stopped".into(), json!(false));
        }
        "chat.approve" | "chat.answer" => {
            let id = p
                .get("approval_id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned();
            let granted = if name == "chat.approve" {
                p.get("granted") == Some(&json!(true))
            } else {
                !p.get("text")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .is_empty()
            };
            if let Some(f) = q.facts.lock().unwrap().get_mut(&id) {
                f.pending = false;
            }
            out.insert("approval_id".into(), json!(id));
            out.insert(
                "state".into(),
                json!(if granted { "granted" } else { "denied" }),
            );
        }
        _ => {
            out.insert("ok".into(), json!(true));
            out.insert("method".into(), json!(name));
        }
    }
    Ok(out)
}

/// A connection as the link's helper makes it.
pub struct Conn {
    reader: BufReader<OwnedReadHalf>,
    writer: OwnedWriteHalf,
    next: i64,
    /// Stream events of the last call.
    pub events: Vec<Value>,
}

pub type Reply = Result<Value, (i64, String)>;

impl Conn {
    pub async fn open(w: &World, hello: Value) -> Result<Conn, (i64, String)> {
        let (r, wr) = UnixStream::connect(w.socket()).await.unwrap().into_split();
        let mut c = Conn {
            reader: BufReader::new(r),
            writer: wr,
            next: 0,
            events: vec![],
        };
        c.call("auth.hello", hello, None).await.map(|_| c)
    }

    pub async fn phone(w: &World, device: &str, session: &str) -> Conn {
        Conn::open(
            w,
            json!({"role": "phone", "device": device, "session": session}),
        )
        .await
        .expect("a phone session")
    }

    pub async fn link(w: &World) -> Conn {
        Conn::open(w, json!({"role": "link"}))
            .await
            .expect("the link's session")
    }

    pub async fn call(&mut self, method: &str, params: Value, proof: Option<Value>) -> Reply {
        self.next += 1;
        let mut req =
            json!({"jsonrpc": "2.0", "id": self.next, "method": method, "params": params});
        if let Some(p) = proof {
            req["proof"] = p;
        }
        self.writer
            .write_all(format!("{req}\n").as_bytes())
            .await
            .unwrap();
        self.events.clear();
        loop {
            let mut line = String::new();
            let n = tokio::time::timeout(
                std::time::Duration::from_secs(10),
                self.reader.read_line(&mut line),
            )
            .await
            .expect("an answer in time")
            .unwrap();
            assert!(n > 0, "the connection closed during {method}");
            let v: Value = serde_json::from_str(&line).unwrap();
            if v.get("id") == Some(&json!(self.next)) {
                return match v.get("error") {
                    Some(e) => Err((
                        e["code"].as_i64().unwrap(),
                        e["message"].as_str().unwrap_or("").to_owned(),
                    )),
                    None => Ok(v["result"].clone()),
                };
            }
            self.events.push(v["params"].clone());
        }
    }

    /// Starts a call without waiting for its answer (a stream such as `link.events`).
    pub async fn start(&mut self, method: &str, params: Value) {
        self.next += 1;
        let req = json!({"jsonrpc": "2.0", "id": self.next, "method": method, "params": params});
        self.writer
            .write_all(format!("{req}\n").as_bytes())
            .await
            .unwrap();
    }

    /// The next line the daemon sends (a stream's event's params, or an answer), within a few seconds.
    pub async fn next_line(&mut self) -> Value {
        let mut line = String::new();
        let n = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            self.reader.read_line(&mut line),
        )
        .await
        .expect("a line in time")
        .unwrap();
        assert!(n > 0, "the connection closed");
        let v: Value = serde_json::from_str(&line).unwrap();
        v.get("params").cloned().unwrap_or(v)
    }

    pub async fn ok(&mut self, method: &str, params: Value) -> Value {
        self.call(method, params, None)
            .await
            .unwrap_or_else(|e| panic!("{method}: {e:?}"))
    }

    /// A fresh challenge of this session.
    pub async fn challenge(&mut self) -> String {
        self.ok("phone.challenge", json!({})).await["challenge"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    /// Unlocks this session with the lock key.
    pub async fn unlock(&mut self, keys: &Keys) {
        let c = self.challenge().await;
        let p = call_proof(&keys.lock, &c, "link.unlock", &json!({}));
        self.call("phone.unlock", json!({}), Some(p))
            .await
            .expect("unlocked");
    }

    /// Proves the device key on this session.
    pub async fn present(&mut self, keys: &Keys) -> Value {
        let c = self.challenge().await;
        let p = call_proof(&keys.device, &c, "link.present", &json!({}));
        self.call("phone.present", json!({}), Some(p))
            .await
            .unwrap()
    }

    /// A sensitive call with its proof by `key`.
    pub async fn signed_call(&mut self, key: &SigningKey, method: &str, params: Value) -> Reply {
        let c = self.challenge().await;
        let p = call_proof(key, &c, method, &params);
        self.call(method, params, Some(p)).await
    }
}

/// Pairs a phone through the link and the owner's Yes; `strong`/`device`: whether it offers those keys.
pub async fn pair(w: &World, endpoint: &str, keys: &Keys, strong: bool, device: bool) {
    let mut link = Conn::link(w).await;
    let mut params = json!({"endpoint": endpoint, "name": format!("Phone {endpoint}"), "app": "FERB Android",
                            "code": "123 456", "unlock_pub": spki(&keys.lock)});
    if strong {
        params["strong_pub"] = json!(spki(&keys.strong));
    }
    if device {
        params["device_pub"] = json!(spki(&keys.device));
    }
    let asked = link.ok("link.pair_request", params).await;
    let id = asked["request_id"].as_str().unwrap().to_owned();
    // the owner's Yes in FERB.app (its FERB.app check is ferb-rpc's; here the area is called as that call reaches it)
    let mut decided = Params::new();
    decided.insert("yes".into(), json!(true));
    decided.insert("request_id".into(), json!(id));
    w.phones.pair_decide(&decided).unwrap();
    let waited = link.ok("link.pair_wait", json!({"request_id": id})).await;
    assert_eq!(waited["paired"], true, "{waited}");
}

/// The session as FERB.app's own calls carry it.
pub fn app_session() -> Session {
    Session {
        role: ferb_rpc::Role::Cli,
        grant: None,
        pid: None,
        token: None,
        app: Some(true),
        phone: None,
    }
}

/// Every audit row as text, for a search.
pub fn audit_text(w: &World) -> String {
    let db = w.db.lock().unwrap();
    let mut st = db
        .conn()
        .prepare(
            "SELECT at, device, method, class, outcome, detail_json FROM phone_audit ORDER BY seq",
        )
        .unwrap();
    let rows: Vec<String> = st
        .query_map([], |r| {
            Ok(format!(
                "{} {} {} {} {} {}",
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?
            ))
        })
        .unwrap()
        .map(Result::unwrap)
        .collect();
    rows.join("\n")
}

pub fn ask(w: &World, id: &str, f: QuestionFacts) {
    w.questions.facts.lock().unwrap().insert(id.to_owned(), f);
}
