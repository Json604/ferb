//! What the fixture tests share: the fixture files, path normalization, and the scripted fake programs that answer
//! each stdin frame from a scenario's script, as rust/fixtures/runtimes/make.py's fakes do.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use base64::Engine;
use ferb_core::types::{ModelImage, ModelRequest, RuntimeEvent};
use ferb_runtimes::forms::{Forms, SchemaTooComplex, Shape};
use ferb_runtimes::process::{BoxFuture, Child, ProcControl, Spawn, Spawner};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream};
use tokio::sync::watch;

pub fn fixture(name: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/runtimes")
        .join(name);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).expect("fixture JSON")
}

pub struct Paths {
    pub root: PathBuf,
    pub home: PathBuf,
    pub tmp: PathBuf,
}

impl Paths {
    pub fn normalize(&self, value: &Value) -> Value {
        match value {
            Value::String(s) => Value::String(self.text(s)),
            Value::Array(items) => Value::Array(items.iter().map(|v| self.normalize(v)).collect()),
            Value::Object(m) => Value::Object(
                m.iter()
                    .map(|(k, v)| (self.text(k), self.normalize(v)))
                    .collect(),
            ),
            other => other.clone(),
        }
    }

    pub fn text(&self, s: &str) -> String {
        let s = s
            .replace(&self.home.to_string_lossy().to_string(), "$FERB")
            .replace(&self.tmp.to_string_lossy().to_string(), "$TMP")
            .replace(&self.root.to_string_lossy().to_string(), "$ROOT");
        static RX: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
        let rx = RX.get_or_init(|| {
            regex::Regex::new(r"ferb-(codex|claude)-run-[A-Za-z0-9_]+").expect("pattern")
        });
        rx.replace_all(&s, "ferb-${1}-run-X").into_owned()
    }
}

pub fn event_text(ev: &RuntimeEvent) -> String {
    ferb_py::json::dumps(&json!({"kind": ev.kind.as_str(), "data": Value::Object(ev.data.clone())}))
}

pub fn request_of(spec: &Value) -> ModelRequest {
    let mut parts: Vec<String> = spec["parts"]
        .as_array()
        .map(|p| {
            p.iter()
                .map(|v| v.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default();
    if let Some(n) = spec.get("huge").and_then(Value::as_u64) {
        parts.push("a".repeat(n as usize));
    }
    let images = spec
        .get("images")
        .and_then(Value::as_array)
        .map(|imgs| {
            imgs.iter()
                .map(|i| {
                    let data = base64::engine::general_purpose::STANDARD
                        .decode(i["data_b64"].as_str().unwrap_or_default())
                        .expect("base64");
                    let mut img = ModelImage::new(data, i["ref"].as_str().unwrap_or_default());
                    if let Some(m) = i.get("mime").and_then(Value::as_str) {
                        img.mime = m.to_string();
                    }
                    img
                })
                .collect()
        })
        .unwrap_or_default();
    ModelRequest {
        system: spec
            .get("system")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        parts,
        cacheable: spec.get("cacheable").and_then(Value::as_u64).unwrap_or(0) as usize,
        images,
    }
}

pub fn raised(e: &ferb_runtimes::RuntimeError) -> Value {
    json!([e.class(), e.to_string()])
}

/// make.py `substitute`.
fn substitute(value: &Value, msg: Option<&Value>) -> Value {
    match value {
        Value::String(s) if s == "$ID" => match msg {
            Some(m) if m.get("id").is_some() => m["id"].clone(),
            Some(m) => m.get("request_id").cloned().unwrap_or(Value::Null),
            None => value.clone(),
        },
        Value::String(s) if s == "$REQ" => msg
            .and_then(|m| m.get("request_id").cloned())
            .unwrap_or(Value::Null),
        Value::String(s) if s == "$TEXT" => msg
            .and_then(|m| m.get("message").and_then(|x| x.get("content")).cloned())
            .unwrap_or(Value::Null),
        Value::String(s) if s == "$UUID" => msg
            .and_then(|m| m.get("uuid").cloned())
            .unwrap_or(Value::Null),
        Value::Array(items) => Value::Array(items.iter().map(|v| substitute(v, msg)).collect()),
        Value::Object(m) => Value::Object(
            m.iter()
                .map(|(k, v)| (k.clone(), substitute(v, msg)))
                .collect(),
        ),
        other => other.clone(),
    }
}

struct FakeControl {
    status: watch::Sender<Option<i32>>,
    app: Arc<FakeApp>,
}

impl ProcControl for FakeControl {
    fn kill(&self) {
        self.app.kill();
    }
    fn terminate(&self) {
        self.app.terminate();
    }
    fn returncode(&self) -> Option<i32> {
        *self.status.borrow()
    }
    fn wait(&self) -> BoxFuture<'_, i32> {
        let mut rx = self.status.subscribe();
        Box::pin(async move {
            loop {
                if let Some(c) = *rx.borrow_and_update() {
                    return c;
                }
                if rx.changed().await.is_err() {
                    return -1;
                }
            }
        })
    }
}

/// Which stdin frames pick a reaction: Codex's `method`, Claude Code's frame kinds.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Keying {
    Codex,
    Claude,
}

/// A scripted program: each stdin line's key picks the next reaction from `script.on[key]`.
pub struct FakeApp {
    pub lines: Mutex<Vec<String>>,
    counts: Mutex<BTreeMap<String, usize>>,
    script: Value,
    env: BTreeMap<String, String>,
    stdout: tokio::sync::Mutex<Option<DuplexStream>>,
    stderr: tokio::sync::Mutex<Option<DuplexStream>>,
    status: watch::Sender<Option<i32>>,
    keying: Keying,
    /// Claude Code ends on stdin EOF; this many writes after it are ignored.
    exit_on_eof: bool,
    dead: std::sync::atomic::AtomicBool,
}

impl FakeApp {
    fn key(&self, msg: &Value) -> String {
        match self.keying {
            Keying::Codex => ferb_py::value::py_str(msg.get("method").unwrap_or(&Value::Null)),
            Keying::Claude => match msg.get("type").and_then(Value::as_str) {
                Some("control_request") => format!(
                    "control:{}",
                    ferb_py::value::py_str(
                        msg.get("request")
                            .and_then(|r| r.get("subtype"))
                            .unwrap_or(&Value::Null)
                    )
                ),
                Some(t) => t.to_string(),
                None => "None".into(),
            },
        }
    }

    pub async fn receive(&self, line: String) {
        self.lines.lock().unwrap().push(line.clone());
        let Ok(msg) = serde_json::from_str::<Value>(&line) else {
            return;
        };
        let key = self.key(&msg);
        let n = {
            let mut c = self.counts.lock().unwrap();
            let n = *c.get(&key).unwrap_or(&0);
            c.insert(key.clone(), n + 1);
            n
        };
        let reaction = self.script["on"]
            .get(&key)
            .and_then(Value::as_array)
            .and_then(|r| r.get(n))
            .cloned();
        if let Some(Value::Array(actions)) = reaction {
            self.apply(&actions, Some(&msg)).await;
        }
    }

    pub async fn apply(&self, actions: &[Value], msg: Option<&Value>) {
        for a in actions {
            if let Some(out) = a.get("out") {
                let line = format!("{}\n", ferb_py::json::dumps(&substitute(out, msg)));
                self.write_out(line.as_bytes()).await;
            } else if let Some(raw) = a.get("raw").and_then(Value::as_str) {
                self.write_out(raw.as_bytes()).await;
            } else if let Some(err) = a.get("err").and_then(Value::as_str) {
                let mut e = self.stderr.lock().await;
                if self.dead.load(std::sync::atomic::Ordering::SeqCst) {
                    e.take();
                }
                if let Some(w) = e.as_mut() {
                    let _ = w.write_all(err.as_bytes()).await;
                }
            } else if let Some(auth) = a.get("auth").and_then(Value::as_str) {
                let home = self.env.get("CODEX_HOME").cloned().unwrap_or_default();
                std::fs::write(Path::new(&home).join("auth.json"), auth).expect("auth copy");
            } else if let Some(code) = a.get("exit") {
                match code.as_i64() {
                    Some(c) => self.end(c as i32),
                    None => self.kill(),
                }
            }
        }
    }

    async fn write_out(&self, bytes: &[u8]) {
        let mut out = self.stdout.lock().await;
        if self.dead.load(std::sync::atomic::Ordering::SeqCst) {
            out.take();
            return;
        }
        if let Some(w) = out.as_mut() {
            let _ = w.write_all(bytes).await;
        }
    }

    pub fn kill(&self) {
        self.end(-9);
    }

    pub fn terminate(&self) {
        self.end(-15);
    }

    fn end(&self, code: i32) {
        if self.status.borrow().is_some() {
            return;
        }
        self.dead.store(true, std::sync::atomic::Ordering::SeqCst);
        // a writer holding a pipe closes it at its next write
        if let Ok(mut o) = self.stdout.try_lock() {
            o.take();
        }
        if let Ok(mut e) = self.stderr.try_lock() {
            e.take();
        }
        self.status.send_replace(Some(code));
    }

    fn exit(&self, code: i32) {
        self.end(code);
    }
}

/// The fake programs one scenario started, in order, and what each was started with.
#[derive(Default)]
pub struct Spawned {
    pub specs: Mutex<Vec<Spawn>>,
    pub apps: Mutex<Vec<Arc<FakeApp>>>,
}

pub struct FakeSpawner {
    pub script: Value,
    pub version: String,
    pub keying: Keying,
    pub spawned: Arc<Spawned>,
}

impl Spawner for FakeSpawner {
    fn spawn(&self, spec: Spawn) -> std::io::Result<Child> {
        self.spawned.specs.lock().unwrap().push(spec.clone());
        let version_probe = spec
            .argv
            .get(1)
            .is_some_and(|a| a == "--version" || a == "-v");
        if version_probe {
            let (tx, _) = watch::channel(Some(0));
            let app = Arc::new(FakeApp {
                lines: Mutex::new(Vec::new()),
                counts: Mutex::new(BTreeMap::new()),
                script: json!({}),
                env: BTreeMap::new(),
                stdout: tokio::sync::Mutex::new(None),
                stderr: tokio::sync::Mutex::new(None),
                status: tx.clone(),
                keying: self.keying,
                exit_on_eof: false,
                dead: Default::default(),
            });
            return Ok(Child {
                stdin: None,
                stdout: Some(Box::new(std::io::Cursor::new(
                    self.version.clone().into_bytes(),
                ))),
                stderr: None,
                control: Arc::new(FakeControl { status: tx, app }),
            });
        }
        let (stdin_w, stdin_r) = tokio::io::duplex(16 << 20);
        let (stdout_w, stdout_r) = tokio::io::duplex(16 << 20);
        let (stderr_w, stderr_r) = tokio::io::duplex(16 << 20);
        let (tx, _) = watch::channel(None);
        let script = match self.script.get("apps").and_then(Value::as_array) {
            Some(apps) => {
                let n = self.spawned.apps.lock().unwrap().len();
                apps[n.min(apps.len() - 1)].clone()
            }
            None => self.script.clone(),
        };
        let app = Arc::new(FakeApp {
            lines: Mutex::new(Vec::new()),
            counts: Mutex::new(BTreeMap::new()),
            script,
            env: spec.env.clone(),
            stdout: tokio::sync::Mutex::new(Some(stdout_w)),
            stderr: tokio::sync::Mutex::new(spec.stderr.then_some(stderr_w)),
            status: tx.clone(),
            keying: self.keying,
            exit_on_eof: self.keying == Keying::Claude,
            dead: Default::default(),
        });
        self.spawned.apps.lock().unwrap().push(app.clone());
        let reader = app.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdin_r);
            loop {
                let mut buf = Vec::new();
                match lines.read_until(b'\n', &mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        reader
                            .receive(String::from_utf8_lossy(&buf).into_owned())
                            .await
                    }
                }
            }
            if reader.exit_on_eof {
                reader.exit(0);
            }
        });
        Ok(Child {
            stdin: Some(Box::new(stdin_w)),
            stdout: Some(Box::new(stdout_r)),
            stderr: spec.stderr.then(|| Box::new(stderr_r) as _),
            control: Arc::new(FakeControl { status: tx, app }),
        })
    }
}

/// A sequential `secrets.token_hex`.
pub fn counter() -> Arc<dyn Fn(usize) -> String + Send + Sync> {
    let n = Arc::new(Mutex::new(0u64));
    Arc::new(move |bytes: usize| {
        let mut g = n.lock().unwrap();
        *g += 1;
        format!("{:0width$x}", *g, width = 2 * bytes)
    })
}

pub fn env_of(v: &Value) -> BTreeMap<String, String> {
    v.as_object()
        .map(|m| {
            m.iter()
                .map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_string()))
                .collect()
        })
        .unwrap_or_default()
}

pub fn write_files(home: &Path, files: &Value) {
    if let Some(m) = files.as_object() {
        for (rel, text) in m {
            let p = home.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, text.as_str().unwrap()).unwrap();
        }
    }
}

/// The first difference between two values, as a path, for a readable failure.
pub fn first_diff(a: &Value, b: &Value, path: &str) -> Option<String> {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            for k in x.keys().chain(y.keys()) {
                let p = format!("{path}.{k}");
                match (x.get(k), y.get(k)) {
                    (Some(u), Some(v)) => {
                        if let Some(d) = first_diff(u, v, &p) {
                            return Some(d);
                        }
                    }
                    (u, v) => return Some(format!("{p}: {u:?} != {v:?}")),
                }
            }
            None
        }
        (Value::Array(x), Value::Array(y)) => {
            for i in 0..x.len().max(y.len()) {
                let p = format!("{path}[{i}]");
                match (x.get(i), y.get(i)) {
                    (Some(u), Some(v)) => {
                        if let Some(d) = first_diff(u, v, &p) {
                            return Some(d);
                        }
                    }
                    (u, v) => return Some(format!("{p}: {u:?} != {v:?}")),
                }
            }
            None
        }
        _ if a == b => None,
        _ => Some(format!("{path}: {a} != {b}")),
    }
}

pub struct RecordedForms(pub Value);

impl Forms for RecordedForms {
    fn anthropic(&self, schema: &Value) -> Result<Shape, SchemaTooComplex> {
        let entry = self.0["anthropic"]
            .as_array()
            .and_then(|a| a.iter().find(|e| &e[0] == schema))
            .unwrap_or_else(|| panic!("no recorded Anthropic form for {schema}"));
        let mut shape = Shape::same(schema);
        shape.wire = entry[1].clone();
        Ok(shape)
    }
    fn gemini(&self, schema: &Value) -> Result<Shape, SchemaTooComplex> {
        let entry = self.0["gemini"]
            .as_array()
            .and_then(|a| a.iter().find(|e| &e[0] == schema))
            .unwrap_or_else(|| panic!("no recorded Gemini form for {schema}"));
        let mut shape = Shape::same(schema);
        shape.wire = entry[1].clone();
        Ok(shape)
    }
    fn check_final(&self, text: &str, schema: &Value) -> Result<(), String> {
        let entry = self.0["checks"]
            .as_array()
            .and_then(|a| a.iter().find(|e| e[0] == text && &e[1] == schema))
            .unwrap_or_else(|| panic!("Python never checked {text:?}: a drift"));
        match &entry[2] {
            Value::Null => Ok(()),
            v => Err(v.as_str().unwrap().to_string()),
        }
    }
}
