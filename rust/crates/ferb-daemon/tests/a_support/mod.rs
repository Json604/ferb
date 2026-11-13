//! Group A's test harness: the handlers on a real `ferb_rpc::Server` over temp stores, with recording stand-ins for
//! the seams and a raw line client, as rust/fixtures/daemon/README.md "How a Rust test replays a scenario" says.
#![allow(dead_code)]

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use ferb_clock::clock::FakeClock;
use ferb_clock::schedule_store::ClaimedRun;
use ferb_clock::schedules::Schedule;
use ferb_core::config::Config;
use ferb_core::interfaces::IdSource;
use ferb_daemon::areas_a::seams::{Ask, LoginError, RunFailed};
use ferb_daemon::areas_a::{self, Deps, LocalFuture, Logins, Owner, Questions, State, Tasks};
use ferb_daemon::shared::Local;
use ferb_rpc::{RunTokens, Server};
use ferb_sanitize::redact::Redactor;
use serde_json::{json, Map, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::UnixStream;

/// The repository (`FERB_REPO` when the crate is built from a copy elsewhere).
pub fn repo() -> PathBuf {
    match std::env::var("FERB_REPO") {
        Ok(r) => PathBuf::from(r),
        Err(_) => Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.."),
    }
}

pub fn fixture(rel: &str) -> Value {
    let text =
        std::fs::read_to_string(repo().join("rust/fixtures").join(rel)).expect("the fixture");
    serde_json::from_str(&text).expect("JSON")
}

/// `SequentialIds` whose next number can be set (a scenario's earlier steps used the ones before).
pub struct Seq {
    pub prefix: Mutex<String>,
    pub n: AtomicU64,
}

impl Seq {
    pub fn new(prefix: &str) -> Arc<Seq> {
        Arc::new(Seq {
            prefix: Mutex::new(prefix.into()),
            n: AtomicU64::new(0),
        })
    }

    /// The next id is `<prefix>-<next>`.
    pub fn set_next(&self, next: u64) {
        self.n.store(next - 1, Ordering::SeqCst);
    }

    pub fn bump(&self) {
        self.n.fetch_add(1, Ordering::SeqCst);
    }
}

impl IdSource for Seq {
    fn new_id(&self) -> String {
        let n = self.n.fetch_add(1, Ordering::SeqCst) + 1;
        format!("{}-{n:06}", self.prefix.lock().unwrap())
    }
}

/// Every seam call, in order, as JSON.
#[derive(Clone, Default)]
pub struct Calls(pub Arc<Mutex<Vec<Value>>>);

impl Calls {
    pub fn push(&self, v: Value) {
        self.0.lock().unwrap().push(v);
    }

    pub fn take(&self) -> Vec<Value> {
        std::mem::take(&mut self.0.lock().unwrap())
    }
}

/// The seams, recording; a scheduled run answers from `runs` (in order).
#[derive(Clone, Default)]
pub struct Stand {
    pub calls: Calls,
    pub runs: Arc<Mutex<Vec<Map<String, Value>>>>,
    pub running: Arc<Mutex<BTreeSet<String>>>,
    pub asks: Arc<AtomicU64>,
    /// How many `create_scheduled` calls fail next (a crash before the run's task was recorded).
    pub fail_create: Arc<AtomicU64>,
}

impl Tasks for Stand {
    fn schedule_running(&self, schedule_id: &str) -> bool {
        self.running.lock().unwrap().contains(schedule_id)
    }
    fn create_scheduled(
        &self,
        _conn: &rusqlite::Connection,
        run: &ClaimedRun,
        late: bool,
    ) -> Result<Value, String> {
        self.calls.push(
            json!({"create_scheduled": run.schedule.id, "timer": run.timer_id, "late": late,
                               "retry": run.retry}),
        );
        if self.fail_create.load(Ordering::SeqCst) > 0 {
            self.fail_create.fetch_sub(1, Ordering::SeqCst);
            return Err("the daemon stopped before the run's task was recorded".into());
        }
        Ok(json!({}))
    }
    fn run_scheduled(
        &self,
        schedule: Schedule,
        _created: Value,
        late: bool,
        overdue_s: f64,
        retry: bool,
    ) -> LocalFuture<Result<Map<String, Value>, RunFailed>> {
        self.calls.push(
            json!({"run_scheduled": schedule.id, "late": late, "overdue_s": overdue_s,
                               "retry": retry}),
        );
        let next = {
            let mut runs = self.runs.lock().unwrap();
            (!runs.is_empty()).then(|| runs.remove(0))
        };
        Box::pin(async move {
            match next {
                Some(m) if m.contains_key("raise") => Err(RunFailed {
                    described: ferb_py::value::py_str(&m["raise"]),
                    reason: ferb_py::value::py_str(&m["reason"]),
                }),
                Some(m) => Ok(m),
                None => Ok(Map::new()),
            }
        })
    }
    fn turn_running(&self) -> bool {
        false
    }
    fn running(&self) -> bool {
        false
    }
    fn mention_paused(&self) -> LocalFuture<()> {
        self.calls.push(json!({"mention_paused": true}));
        Box::pin(async {})
    }
    fn task_column(&self, _task_id: &str, _column: &str) -> Option<String> {
        None
    }
    fn set_task_status(
        &self,
        _task_id: &str,
        _status: &str,
        _outcome: &Value,
    ) -> Result<(), String> {
        Ok(())
    }
    fn forget_pending(&self, texts: &[String]) {
        self.calls.push(json!({"forget_pending": texts}));
    }
    fn forget_trail_copies(&self, trail: &BTreeSet<String>) {
        self.calls.push(json!({"forget_trail_copies": trail}));
    }
}

impl Owner for Stand {
    fn task_result(&self, event: Map<String, Value>, shown: bool, alert: bool) -> LocalFuture<()> {
        self.calls
            .push(json!({"task_result": event, "shown": shown, "alert": alert}));
        Box::pin(async {})
    }
    fn notify(
        &self,
        title: &str,
        text: &str,
        ident: &str,
        target: Value,
        card: Value,
    ) -> LocalFuture<Result<(), String>> {
        self.calls.push(
            json!({"notify": {"title": title, "text": text, "ident": ident, "target": target,
                                          "card": card}}),
        );
        Box::pin(async { Ok(()) })
    }
    fn tell_owner(
        &self,
        ident: &str,
        refs: Vec<String>,
        text: &str,
        origin: &str,
        cards: Option<Value>,
    ) -> LocalFuture<()> {
        self.calls.push(
            json!({"tell_owner": {"ident": ident, "refs": refs, "text": text, "origin": origin,
                                              "cards": cards}}),
        );
        Box::pin(async {})
    }
}

impl Questions for Stand {
    fn ask(&self, ask: Ask<'_>) -> Result<String, String> {
        let id = match ask.approval_id {
            Some(id) => id.to_owned(),
            None => format!("ap-a{:011}", self.asks.fetch_add(1, Ordering::SeqCst) + 1),
        };
        self.calls.push(
            json!({"ask": {"id": id, "kind": ask.kind, "reason": ask.reason,
                                       "summary": ask.summary, "detail": ask.detail,
                                       "labels": [ask.labels.0, ask.labels.1]}}),
        );
        Ok(id)
    }
    fn supersede(&self, approval_id: &str) -> bool {
        self.calls.push(json!({"supersede": approval_id}));
        true
    }
    fn waiting(&self, _approval_id: &str) -> bool {
        false
    }
}

impl Logins for Stand {
    fn clear_usernames(&self, _sources: &BTreeSet<String>) -> Result<Vec<String>, LoginError> {
        Ok(Vec::new())
    }
}

/// One daemon for a scenario: its folders, clock, ids, the server and group A's state.
pub struct Daemon {
    pub root: PathBuf,
    pub home: PathBuf,
    pub user: PathBuf,
    pub clock: FakeClock,
    pub ids: Arc<Seq>,
    pub stand: Stand,
    pub published: Arc<Mutex<Vec<Value>>>,
    pub server: Server,
    pub local: Local<State>,
    pub token: Arc<AtomicU64>,
}

pub struct RpcClock;

impl ferb_rpc::Clock for RpcClock {
    fn monotonic(&self) -> f64 {
        1000.0
    }
}

impl Daemon {
    /// A daemon on `root/<nn>` with ids under `prefix`.
    pub async fn start(
        root: &Path,
        nn: &str,
        prefix: &str,
        clock: FakeClock,
        ids: Arc<Seq>,
    ) -> Daemon {
        let base = root.join(nn);
        let (home, user) = (base.join("home"), base.join("user"));
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&user).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o700)).unwrap();
        let _ = prefix;
        let stand = Stand::default();
        let published: Arc<Mutex<Vec<Value>>> = Arc::default();
        let token = Arc::new(AtomicU64::new(0));
        let deps = {
            let p = published.clone();
            let t = token.clone();
            Deps {
                home: home.clone(),
                user_home: user.clone(),
                db_path: home.join("ferb.db"),
                loop_thread: None,
                prompts: repo().join("ferb"),
                clock: Arc::new(clock.clone()),
                ids: ids.clone(),
                config: Arc::new(Config::new(&home)),
                redactor: Arc::new(Redactor::new(&[], &[])),
                publish: Arc::new(move |topic: &str, data: Map<String, Value>| {
                    p.lock()
                        .unwrap()
                        .push(json!({"topic": topic, "data": data}));
                }),
                timer_tokens: Some(Arc::new(move || {
                    format!("t{:015}", t.fetch_add(1, Ordering::SeqCst) + 1)
                })),
                make_runtime: None,
                app_bundle: None,
                persona_scan: None,
                tasks: Arc::new(stand.clone()),
                owner: Arc::new(stand.clone()),
                questions: Arc::new(stand.clone()),
                logins: Arc::new(stand.clone()),
                purges: None,
            }
        };
        let mut server = Server::new(
            base.join("s.sock"),
            Arc::new(RunTokens::new(Arc::new(RpcClock))),
        );
        let local = areas_a::register(&mut server, deps).expect("group A's state");
        server.set_is_app(|_| true);
        server.start().await.expect("the server");
        Daemon {
            root: root.to_path_buf(),
            home,
            user,
            clock,
            ids,
            stand,
            published,
            server,
            local,
            token,
        }
    }

    /// Run `f` on the loop with the state.
    pub async fn on<R: Send + 'static>(
        &self,
        f: impl FnOnce(std::rc::Rc<State>) -> LocalFuture<R> + Send + 'static,
    ) -> R {
        self.local.call(f).await.expect("the loop")
    }

    pub async fn sql(&self, statements: Vec<(String, Vec<Value>)>) {
        self.on(move |st| {
            Box::pin(async move {
                for (q, args) in statements {
                    let params: Vec<rusqlite::types::Value> = args
                        .iter()
                        .map(|a| match a {
                            Value::Null => rusqlite::types::Value::Null,
                            Value::Number(n) if n.is_i64() => {
                                rusqlite::types::Value::Integer(n.as_i64().unwrap())
                            }
                            Value::Number(n) => rusqlite::types::Value::Real(n.as_f64().unwrap()),
                            Value::Bool(b) => rusqlite::types::Value::Integer(i64::from(*b)),
                            Value::String(s) => rusqlite::types::Value::Text(s.clone()),
                            other => rusqlite::types::Value::Text(other.to_string()),
                        })
                        .collect();
                    st.db()
                        .conn()
                        .execute(&q, rusqlite::params_from_iter(params))
                        .unwrap_or_else(|e| panic!("{q}: {e}"));
                }
            })
        })
        .await
    }

    /// A table's rows (rowid order) as the fixtures write them.
    pub async fn table(&self, name: &str) -> Vec<Vec<Value>> {
        let name = name.to_owned();
        self.on(move |st| {
            Box::pin(async move {
                let conn = st.db().conn();
                let mut stmt = conn.prepare(&format!("SELECT * FROM \"{name}\"")).unwrap();
                let n = stmt.column_count();
                let rows = stmt
                    .query_map([], |r| {
                        Ok((0..n)
                            .map(|i| match r.get_ref(i).unwrap() {
                                rusqlite::types::ValueRef::Null => Value::Null,
                                rusqlite::types::ValueRef::Integer(v) => json!(v),
                                rusqlite::types::ValueRef::Real(v) => json!(v),
                                rusqlite::types::ValueRef::Text(t) => {
                                    json!(String::from_utf8_lossy(t))
                                }
                                rusqlite::types::ValueRef::Blob(b) => {
                                    json!({"hex": hex::encode(b)})
                                }
                            })
                            .collect::<Vec<Value>>())
                    })
                    .unwrap()
                    .collect::<Result<Vec<_>, _>>()
                    .unwrap();
                rows
            })
        })
        .await
    }

    pub async fn close(self) {
        self.server.close().await;
    }
}

/// A raw connection: lines out, lines in.
pub struct Raw {
    pub reader: BufReader<OwnedReadHalf>,
    pub writer: OwnedWriteHalf,
}

impl Raw {
    pub async fn connect(path: &Path) -> Raw {
        let (reader, writer) = UnixStream::connect(path)
            .await
            .expect("connect")
            .into_split();
        Raw {
            reader: BufReader::new(reader),
            writer,
        }
    }

    pub async fn send(&mut self, line: &str) {
        self.writer.write_all(line.as_bytes()).await.unwrap();
        self.writer.write_all(b"\n").await.unwrap();
    }

    pub async fn recv(&mut self) -> String {
        let mut line = String::new();
        tokio::time::timeout(
            std::time::Duration::from_secs(20),
            self.reader.read_line(&mut line),
        )
        .await
        .expect("an answer in time")
        .unwrap();
        line.trim_end_matches('\n').to_owned()
    }
}

/// The fixture's `/FX` for this run's root.
pub fn fx(text: &str, root: &Path) -> String {
    text.replace(&root.to_string_lossy().to_string(), "/FX")
}

thread_local! {
    pub static NOTES: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}
