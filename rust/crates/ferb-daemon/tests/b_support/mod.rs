//! Group B's replay harness: a `ferb_rpc::Server` on a temporary socket with only the handlers a test registers,
//! driven by the raw lines `rust/fixtures/daemon/make.py` and `rust/fixtures/daemon-b/*.py` recorded from the
//! Python daemon (a request line goes out; each answer line must come back byte for byte).
#![allow(dead_code)]

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use ferb_core::config::Config;
use ferb_core::interfaces::Clock;
use ferb_core::when::DateTime;
use ferb_daemon::areas_b::core::{Bus, Host, Journal};
use ferb_rpc::{RunTokens, Server};
use ferb_sanitize::redact::Redactor;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::UnixStream;

pub const WAIT: Duration = Duration::from_secs(20);

/// `rust/fixtures` (a scratch build of these tests outside the crate sets FERB_FIXTURES).
pub fn fixtures() -> PathBuf {
    match std::env::var_os("FERB_FIXTURES") {
        Some(p) => PathBuf::from(p),
        None => PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures"),
    }
}

/// A fixture file, keys in their recorded order.
pub fn fixture(rel: &str) -> Value {
    let path = fixtures().join(rel);
    serde_json::from_slice(
        &std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display())),
    )
    .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// Every text stored once by its SHA-256 (`bodies.json` of both fixture folders).
pub fn bodies() -> &'static HashMap<String, String> {
    static BODIES: OnceLock<HashMap<String, String>> = OnceLock::new();
    BODIES.get_or_init(|| {
        let mut out = HashMap::new();
        let mut files = vec!["daemon/bodies.json".to_owned()];
        if let Ok(dir) = std::fs::read_dir(fixtures().join("daemon-b")) {
            for e in dir.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if name.starts_with("bodies-") && name.ends_with(".json") {
                    files.push(format!("daemon-b/{name}"));
                }
            }
        }
        for rel in &files {
            if fixtures().join(rel).exists() {
                if let Value::Object(map) = fixture(rel) {
                    for (k, v) in map {
                        out.insert(k, v.as_str().unwrap_or_default().to_owned());
                    }
                }
            }
        }
        out
    })
}

/// A recorded line: the text itself, or `{"sha256"}` standing for a long one.
pub fn line_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Object(o) if o.contains_key("sha256") => {
            let h = o["sha256"].as_str().unwrap();
            bodies()
                .get(h)
                .cloned()
                .unwrap_or_else(|| panic!("no body {h}"))
        }
        other => panic!("not a line: {other}"),
    }
}

/// `make.py`'s masks: the keys whose values a run can't fix are `"<volatile>"` at any depth.
pub fn scrub(line: &str, keys: &[&str]) -> String {
    if keys.is_empty() {
        return line.to_owned();
    }
    let Ok(mut v) = serde_json::from_str::<Value>(line) else {
        return line.to_owned();
    };
    fn walk(v: &mut Value, keys: &[&str]) {
        match v {
            Value::Object(o) => {
                for (k, x) in o.iter_mut() {
                    if keys.contains(&k.as_str()) {
                        *x = Value::String("<volatile>".into());
                    } else {
                        walk(x, keys);
                    }
                }
            }
            Value::Array(a) => a.iter_mut().for_each(|x| walk(x, keys)),
            _ => {}
        }
    }
    walk(&mut v, keys);
    ferb_py::json::dumps(&v)
}

/// The journal rows a test's handlers wrote, as Python's `Journal.append` would store them: redacted, with the
/// origin worked out when none is given.
pub struct RecJournal {
    pub redactor: Arc<Redactor>,
    pub rows: Mutex<Vec<Value>>,
}

impl Journal for RecJournal {
    fn append(&self, kind: &str, data: Value, origin: Option<&str>) -> Result<(), String> {
        let body = self.redactor.value(&data);
        let origin = match origin {
            Some(o) if !o.is_empty() => o.to_owned(),
            _ => ferb_learning::journal::journal_origin(kind, &body),
        };
        self.rows
            .lock()
            .unwrap()
            .push(serde_json::json!([kind, body, origin]));
        Ok(())
    }
}

/// Published events, redacted once (events.py).
pub struct RecBus {
    pub redactor: Arc<Redactor>,
    pub events: Mutex<Vec<Value>>,
}

impl Bus for RecBus {
    fn publish(&self, topic: &str, data: Value) {
        let body = self.redactor.value(&data);
        self.events
            .lock()
            .unwrap()
            .push(serde_json::json!([topic, body]));
    }
}

/// ferb.clock.clock.FakeClock at 2026-01-01T09:00:00Z.
pub struct FakeClock {
    pub now: Mutex<DateTime>,
    pub mono: Mutex<f64>,
}

impl FakeClock {
    pub fn new() -> Arc<FakeClock> {
        Arc::new(FakeClock {
            now: Mutex::new(ferb_core::when::parse("2026-01-01T09:00:00+00:00").unwrap()),
            mono: Mutex::new(0.0),
        })
    }
}

impl Clock for FakeClock {
    fn now(&self) -> DateTime {
        *self.now.lock().unwrap()
    }
    fn monotonic(&self) -> f64 {
        *self.mono.lock().unwrap()
    }
    fn sleep_until(&self, _t: DateTime) -> ferb_core::interfaces::BoxFuture<'_, ()> {
        Box::pin(async {})
    }
}

struct MonoClock;
impl ferb_rpc::Clock for MonoClock {
    fn monotonic(&self) -> f64 {
        0.0
    }
}

/// A temporary FERB home with config.toml (0600, folder 0700) and the host every handler reads.
pub struct Home {
    pub dir: tempfile::TempDir,
    pub host: Host,
    pub journal: Arc<RecJournal>,
    pub bus: Arc<RecBus>,
    pub clock: Arc<FakeClock>,
}

impl Home {
    pub fn new(config_toml: &str) -> Home {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o700)).unwrap();
        let cfg = home.join("config.toml");
        std::fs::write(&cfg, config_toml).unwrap();
        std::fs::set_permissions(&cfg, std::fs::Permissions::from_mode(0o600)).unwrap();
        let redactor = Arc::new(Redactor::new(&[], &[]));
        let journal = Arc::new(RecJournal {
            redactor: redactor.clone(),
            rows: Mutex::new(vec![]),
        });
        let bus = Arc::new(RecBus {
            redactor: redactor.clone(),
            events: Mutex::new(vec![]),
        });
        let clock = FakeClock::new();
        let host = Host {
            home: home.clone(),
            config: Arc::new(Config::new(&home)),
            redactor,
            journal: journal.clone(),
            bus: bus.clone(),
            clock: clock.clone(),
        };
        Home {
            dir,
            host,
            journal,
            bus,
            clock,
        }
    }

    pub fn path(&self) -> &Path {
        &self.host.home
    }

    pub fn journal_rows(&self) -> Vec<Value> {
        self.journal.rows.lock().unwrap().clone()
    }

    pub fn events(&self) -> Vec<Value> {
        self.bus.events.lock().unwrap().clone()
    }
}

/// The server with the handlers a test registered; FERB.app's methods are answered (as the recorder's
/// `is_app = lambda token: True`).
pub struct Daemon {
    pub server: Server,
    _dir: tempfile::TempDir,
}

impl Daemon {
    pub async fn start(register: impl FnOnce(&mut Server)) -> Daemon {
        let dir = tempfile::Builder::new()
            .prefix("fxb")
            .tempdir_in("/tmp")
            .unwrap();
        let mut server = Server::new(
            dir.path().join("s/ferb.sock"),
            Arc::new(RunTokens::new(Arc::new(MonoClock))),
        );
        server.set_is_app(|_| true);
        register(&mut server);
        server.start().await.unwrap();
        Daemon { server, _dir: dir }
    }

    pub async fn connect(&self) -> Conn {
        let stream = UnixStream::connect(self.server.path()).await.unwrap();
        let (r, w) = stream.into_split();
        Conn {
            reader: BufReader::new(r),
            writer: w,
        }
    }
}

pub struct Conn {
    reader: BufReader<OwnedReadHalf>,
    writer: OwnedWriteHalf,
}

impl Conn {
    pub async fn send(&mut self, line: &str) {
        self.writer.write_all(line.as_bytes()).await.unwrap();
        self.writer.write_all(b"\n").await.unwrap();
    }

    /// The next line, without its newline; None when the daemon closed the connection.
    pub async fn recv(&mut self) -> Option<String> {
        let mut buf = Vec::new();
        let n = tokio::time::timeout(WAIT, self.reader.read_until(b'\n', &mut buf))
            .await
            .expect("no answer in time")
            .unwrap();
        if n == 0 {
            return None;
        }
        if buf.ends_with(b"\n") {
            buf.pop();
        }
        Some(String::from_utf8(buf).unwrap())
    }
}

/// A method of a request line ("" for none).
pub fn method_of(line: &str) -> String {
    serde_json::from_str::<Value>(line)
        .ok()
        .and_then(|v| v.get("method").and_then(Value::as_str).map(str::to_owned))
        .unwrap_or_default()
}

/// What a replay compared.
#[derive(Default, Debug)]
pub struct Outcome {
    /// method -> lines compared equal
    pub same: BTreeMap<String, usize>,
    /// method -> lines that differ by a named fix (the fix's expected line matched)
    pub named: BTreeMap<String, usize>,
    pub diffs: Vec<String>,
}

impl Outcome {
    pub fn assert_clean(&self) {
        assert!(
            self.diffs.is_empty(),
            "{} difference(s):\n{}",
            self.diffs.len(),
            self.diffs.join("\n")
        );
        assert!(
            !self.same.is_empty() || !self.named.is_empty(),
            "nothing compared"
        );
    }

    fn record(&mut self, method: &str, expected: &str, got: Option<&str>, named: bool) {
        match got {
            Some(g) if g == expected => {
                *if named {
                    &mut self.named
                } else {
                    &mut self.same
                }
                .entry(method.to_owned())
                .or_default() += 1;
            }
            _ => self.diffs.push(format!(
                "{method}:\n  python: {expected}\n  rust:   {}",
                got.unwrap_or("(closed)")
            )),
        }
    }
}

/// A named difference from Python (ENG-6): for this request line, the line Rust answers instead of Python's.
pub type Fixes<'a> = &'a dyn Fn(&str, &str) -> Option<String>;

/// Plays a scenario's `steps`: connections, the hello, and every call and stream of a method `keep` accepts, in
/// order, comparing each answer line (after `volatile` masks). Calls of other methods are skipped.
pub async fn replay(
    d: &Daemon,
    steps: &[Value],
    keep: &dyn Fn(&str) -> bool,
    volatile: &[&str],
    fixes: Fixes<'_>,
) -> Outcome {
    let mut out = Outcome::default();
    let mut conns: HashMap<String, Conn> = HashMap::new();
    let mut streams: HashMap<String, (String, String)> = HashMap::new();
    for st in steps {
        let label = st["conn"].as_str().unwrap_or("").to_owned();
        match st["do"].as_str().unwrap_or("") {
            "connect" => {
                conns.insert(label, d.connect().await);
            }
            "call" => {
                let lines = st["lines"].as_array().unwrap();
                let Some(first) = lines.first().and_then(|l| l.get("send")) else {
                    continue;
                };
                let request = line_text(first);
                let method = method_of(&request);
                if method != "auth.hello" && !keep(&method) {
                    continue;
                }
                let c = conns.get_mut(&label).expect("connection");
                c.send(&request).await;
                for l in &lines[1..] {
                    if let Some(r) = l.get("recv") {
                        let expected = scrub(&line_text(r), volatile);
                        let (expected, named) = match fixes(&request, &expected) {
                            Some(fixed) => (fixed, true),
                            None => (expected, false),
                        };
                        let got = c.recv().await.map(|g| scrub(&g, volatile));
                        out.record(&method, &expected, got.as_deref(), named);
                    }
                }
            }
            "stream" => {
                let request = line_text(&st["lines"][0]["send"]);
                let method = method_of(&request);
                if !keep(&method) {
                    continue;
                }
                conns.get_mut(&label).unwrap().send(&request).await;
                streams.insert(label, (method, request));
            }
            "stream_end" | "stream_cut" => {
                let Some((method, request)) = streams.remove(&label) else {
                    continue;
                };
                let lines = st["lines"].as_array().unwrap();
                if !lines.iter().any(|l| l.get("recv").is_some()) {
                    continue;
                }
                let c = conns.get_mut(&label).expect("connection");
                for l in lines {
                    if let Some(r) = l.get("recv") {
                        let expected = scrub(&line_text(r), volatile);
                        let (expected, named) = match fixes(&request, &expected) {
                            Some(fixed) => (fixed, true),
                            None => (expected, false),
                        };
                        let got = c.recv().await.map(|g| scrub(&g, volatile));
                        out.record(&method, &expected, got.as_deref(), named);
                    }
                }
            }
            "close" => {
                conns.remove(&label);
            }
            _ => {}
        }
    }
    out
}

/// One scenario of a fixture file by name.
pub fn scenario<'a>(file: &'a Value, name: &str) -> &'a Value {
    file["scenarios"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == name)
        .unwrap_or_else(|| panic!("no scenario {name}"))
}
