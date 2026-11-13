//! Test-only (feature `stand-ins`): the ferb-daemon test suite's stand-ins put into the real program, and a control
//! socket the tests drive it with. A build without the feature has none of this code: `FERBD_STAND_INS` is never
//! read, so a release build can't be made to honor it.
//!
//! `FERBD_STAND_INS` names a JSON file:
//!
//! - `native` (false): the Mac's own parts (TCC reads, the running-app list, watchers, presence) stay off.
//! - `app_check` (true): every peer is FERB.app, or none is (`false`), as the recorder's `is_app`.
//! - `user_home`, `browser_support`: folders in the test's temporary root.
//! - `fake_clock` (true): ferb-clock's `FakeClock` at 2026-01-01T09:00:00Z, moved only through the control socket.
//! - `ids` ("id"): sequential ids `<prefix>-000001`, whose next number the control socket sets.
//! - `group_a` (true): group A's seams are the test suite's recording stand-ins (`tests/a_support`'s `Stand`).
//! - `group_b`: `"small"` gives group B's areas the stand-ins `tests/b_small.rs` replays with (`perception`,
//!   `answers` from the same file).
//! - `script`: a recorded chat scenario's scripted model and reads (src/standins_chat.rs); with it the questions take
//!   the recorder's counter ids, the planner counts tokens by the byte ceiling, the untrusted-data wrapper has the
//!   recorder's fixed id and a new chat the recorder's id. `fs_identity` (true): file results carry the recorder's
//!   invented file facts.
//!
//! The Keychain is always off (every helper call refused, `security` finds nothing), retention and the secrets'
//! preload don't run, and no browser is started.
//!
//! The control socket is `<home>/run/stand-ins.sock`: one JSON request per line, one `{"ok": ...}` or
//! `{"error": ...}` per line.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use ferb_clock::clock::FakeClock;
use ferb_clock::schedule_store::ClaimedRun;
use ferb_clock::schedules::Schedule;
use ferb_core::interfaces::IdSource;
use ferb_daemon::areas_a::seams::{Ask, LoginError, RunFailed};
use ferb_daemon::areas_a::{self, LocalFuture, Logins, Owner, Questions, Tasks};
use ferb_daemon::areas_b::{self, about, intro, sensing};
use serde_json::{json, Map, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::compose::{Daemon, Keychain, Options, Overrides};

/// `SequentialIds` whose next number can be set.
pub struct Seq {
    prefix: String,
    n: AtomicU64,
}

impl IdSource for Seq {
    fn new_id(&self) -> String {
        let n = self.n.fetch_add(1, Ordering::SeqCst) + 1;
        format!("{}-{n:06}", self.prefix)
    }
}

/// tests/a_support's `Stand`: every seam call recorded; a scheduled run answers `{}`.
#[derive(Clone, Default)]
pub struct Stand {
    pub calls: Arc<Mutex<Vec<Value>>>,
    pub asks: Arc<AtomicU64>,
}

impl Stand {
    fn push(&self, v: Value) {
        self.calls.lock().unwrap_or_else(|e| e.into_inner()).push(v);
    }
}

impl Tasks for Stand {
    fn schedule_running(&self, _schedule_id: &str) -> bool {
        false
    }
    fn create_scheduled(
        &self,
        _conn: &rusqlite::Connection,
        run: &ClaimedRun,
        late: bool,
    ) -> Result<Value, String> {
        self.push(
            json!({"create_scheduled": run.schedule.id, "timer": run.timer_id, "late": late,
                         "retry": run.retry}),
        );
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
        self.push(
            json!({"run_scheduled": schedule.id, "late": late, "overdue_s": overdue_s,
                         "retry": retry}),
        );
        Box::pin(async { Ok(Map::new()) })
    }
    fn turn_running(&self) -> bool {
        false
    }
    fn running(&self) -> bool {
        false
    }
    fn mention_paused(&self) -> LocalFuture<()> {
        self.push(json!({"mention_paused": true}));
        Box::pin(async {})
    }
    fn task_column(&self, _task_id: &str, _column: &str) -> Option<String> {
        None
    }
    fn set_task_status(&self, _t: &str, _s: &str, _o: &Value) -> Result<(), String> {
        Ok(())
    }
    fn forget_pending(&self, texts: &[String]) {
        self.push(json!({"forget_pending": texts}));
    }
    fn forget_trail_copies(&self, trail: &BTreeSet<String>) {
        self.push(json!({"forget_trail_copies": trail}));
    }
}

impl Owner for Stand {
    fn task_result(&self, event: Map<String, Value>, shown: bool, alert: bool) -> LocalFuture<()> {
        self.push(json!({"task_result": event, "shown": shown, "alert": alert}));
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
        self.push(
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
        self.push(
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
        self.push(
            json!({"ask": {"id": id, "kind": ask.kind, "reason": ask.reason,
                                 "summary": ask.summary, "detail": ask.detail,
                                 "labels": [ask.labels.0, ask.labels.1]}}),
        );
        Ok(id)
    }
    fn supersede(&self, approval_id: &str) -> bool {
        self.push(json!({"supersede": approval_id}));
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

/// The perception watcher the recorder's daemon ran (`jev_batches`): runs until stopped.
struct Batches;

impl ferb_sense::manager::Watcher for Batches {
    fn source(&self) -> &str {
        "jev_batches"
    }
    fn run(
        self: Arc<Self>,
        _emit: ferb_sense::manager::Emit,
        stop: ferb_sense::manager::Stop,
    ) -> ferb_core::interfaces::BoxFuture<'static, Result<(), String>> {
        Box::pin(async move {
            stop.wait().await;
            Ok(())
        })
    }
}

struct NoAccount;
impl about::Account for NoAccount {
    fn profiles(&self) -> Vec<ferb_browser::profiles::BrowserProfile> {
        vec![]
    }
    fn identity_paired(&self) -> bool {
        false
    }
}

/// b_runtimes.rs `World`'s programs: the pinned Codex is installed (never run), no `claude` program.
struct RecorderPrograms;

impl areas_b::runtimes::Programs for RecorderPrograms {
    fn codex_binary(&self) -> Result<PathBuf, ferb_core::errors::FerbError> {
        Ok(PathBuf::from(
            "/REPO/node_modules/@openai/codex-darwin-arm64/vendor/aarch64-apple-darwin/bin/codex",
        ))
    }
    fn claude_cli(&self) -> Result<PathBuf, ferb_core::errors::FerbError> {
        Err(ferb_core::errors::FerbError::Unsupported(
            "the claude program isn't available in the fixture".into(),
        ))
    }
    fn bundled_catalog(&self) {}
    fn prepare_codex<'a>(
        &'a self,
        _binary: &'a std::path::Path,
    ) -> ferb_rpc::BoxFuture<'a, Result<(), ferb_core::errors::FerbError>> {
        Box::pin(async { Ok(()) })
    }
    fn check_codex_version<'a>(
        &'a self,
        _binary: &'a std::path::Path,
    ) -> ferb_rpc::BoxFuture<'a, Result<(), ferb_core::errors::FerbError>> {
        Box::pin(async { Ok(()) })
    }
    fn claude_pin_status(&self) -> String {
        String::new()
    }
}

/// What a test configured.
pub struct StandIns {
    pub script: Option<Arc<crate::standins_chat::Script>>,
    pub config: Value,
    pub clock: Option<FakeClock>,
    pub ids: Option<Arc<Seq>>,
    pub tokens: Arc<AtomicU64>,
    pub stand: Stand,
}

struct Seams {
    config: Value,
    stand: Stand,
    group_a: bool,
    script: Option<Arc<crate::standins_chat::Script>>,
}

impl Overrides for Seams {
    fn supervisor_reads(
        &self,
        real: Arc<dyn crate::compose::supervisor::Reads>,
    ) -> Arc<dyn crate::compose::supervisor::Reads> {
        match &self.script {
            Some(s) => Arc::new(crate::standins_chat::ScriptReads(s.clone())),
            None => real,
        }
    }

    fn supervisor_build(&self, p: &mut crate::compose::supervisor::BuildParts) {
        if self.script.is_some() {
            p.tokenizer = ferb_context::tokens::Tokenizer::Utf8Bytes;
            p.wrap_id = Some("0123456789ab".into());
            p.new_chat = Some(Arc::new(|| "c00000000001".into()));
        }
    }

    fn started(&self, d: &Arc<Daemon>) {
        let Some(s) = &self.script else { return };
        let _ = s.db.set(d.db.clone());
        let _ = s.ids.set(d.opts.ids.clone());
        let _ = s.socket.set(ferb_core::paths::socket_path(&d.opts.home));
        if self.config["fs_identity"] == true {
            let mut reg = d.gateway.registry();
            for name in crate::compose::tools::wrap::THREADED {
                // around the registry's tool (its file work in a thread already), the recorder's facts
                if let Some(t) = reg.get(name).cloned() {
                    reg.set(Arc::new(crate::compose::tools::wrap::Wrapped {
                        inner: t,
                        around: crate::standins_chat::RecordedFacts { script: s.clone() },
                    }));
                }
            }
        }
    }

    fn group_a(&self, d: &mut areas_a::Deps) {
        if self.group_a {
            d.tasks = Arc::new(self.stand.clone());
            d.owner = Arc::new(self.stand.clone());
            d.questions = Arc::new(self.stand.clone());
            d.logins = Arc::new(self.stand.clone());
        }
    }

    fn group_c(&self, d: &mut crate::areas_c::Deps) {
        crate::standins_c::group_c(&self.config, d);
    }

    fn group_b(&self, d: &mut areas_b::Deps) {
        if self.config["programs"] == "recorder" {
            if let Some(r) = d.runtimes.as_mut() {
                r.programs = Arc::new(RecorderPrograms);
            }
        }
        if self.config["group_b"] != "small" {
            return;
        }
        let host = d.host.clone();
        // b_small's `deps`: the recorder's stand-ins (make.py GRANTS, its containment, no account)
        if let Some(old) = d.about.take() {
            d.about = Some(Arc::new(about::Deps {
                version: "0.27.3".into(),
                guide: Arc::new(Mutex::new(None)),
                grants: Arc::new(|| {
                    crate::basics::object(
                        json!({"accessibility": "granted", "screen_recording": "granted",
                        "input_monitoring": "granted", "full_disk_access": "denied"}),
                    )
                }),
                containment: Arc::new(|| {
                    crate::basics::object(json!({"profile_ok": true, "shell": true, "proxy": true}))
                }),
                account: Arc::new(NoAccount),
                store: old.store.clone(),
                store_facts: Arc::new(|| (0, false)),
                permissions_in_force: old.permissions_in_force.clone(),
                saved_logins: old.saved_logins.clone(),
                owner_secrets: old.owner_secrets.clone(),
                evidence: Some(Arc::new(about::Evidence {
                    root: host.home.join("evidence"),
                })),
                tree_of: Arc::new(|_| None),
            }));
        }
        let perception = self.config["perception"].clone();
        let h = host.clone();
        let sense = Arc::new(ferb_sense::manager::Sense::new(
            host.home.clone(),
            vec![Arc::new(Batches)],
            Arc::new(|_| {}),
            None,
            None,
            ferb_sense::manager::Hooks {
                on_pause: Arc::new(move |paused| {
                    let _ = sensing::on_pause(&h, paused, &|| {});
                }),
                ..ferb_sense::manager::Hooks::default()
            },
        ));
        {
            let s = sense.clone();
            tokio::spawn(async move { s.start().await });
        }
        d.sensing = Some(sensing::Deps {
            sense: Some(sense),
            owner_input_running: Arc::new(|| false),
            watch_problems: Arc::new(Mutex::new(vec![])),
            perception_status: Some(Arc::new(move || Ok(perception.clone()))),
        });
        let scan = self
            .config
            .get("scan")
            .cloned()
            .unwrap_or(json!({"state": "not_started"}));
        let answers: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(
            self.config["answers"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(String::from)
                        .collect()
                })
                .unwrap_or_default(),
        ));
        d.intro = Some(Arc::new(intro::Deps {
            scan: Arc::new(move || scan.clone()),
            ask: Arc::new(move |_system, _ctx| {
                let next = {
                    let mut a = answers.lock().unwrap_or_else(|e| e.into_inner());
                    (!a.is_empty()).then(|| a.remove(0))
                };
                Box::pin(async move {
                    next.ok_or_else(|| intro::CallFailed("no recorded answer".into()))
                })
            }),
            system: Arc::new(|| Ok("intro instructions".into())),
            limit: std::time::Duration::from_secs(55),
            shutting_down: Arc::new(|| false),
            preview: None,
        }));
    }
}

/// Reads `FERBD_STAND_INS` and changes `opts` as it says; None when it isn't set.
pub fn from_env(opts: &mut Options) -> Option<StandIns> {
    let path = std::env::var_os("FERBD_STAND_INS")?;
    let config: Value = std::fs::read(&path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or(json!({}));
    let flag = |k: &str, d: bool| config.get(k).and_then(Value::as_bool).unwrap_or(d);
    opts.native = flag("native", false);
    opts.mac = flag("mac", false);
    opts.keychain = Keychain::Refused;
    opts.retention = false;
    opts.memory_loops = flag("memory_loops", false);
    opts.resources = flag("resources", false);
    opts.preload_secrets = false;
    opts.launch_browsers = false;
    opts.app_check = match config.get("app_check") {
        Some(Value::Bool(b)) => Some(*b),
        _ => Some(true),
    };
    if let Some(u) = config.get("user_home").and_then(Value::as_str) {
        opts.user_home = PathBuf::from(u);
    }
    if let Some(b) = config.get("browser_support").and_then(Value::as_str) {
        opts.browser_support = PathBuf::from(b);
    } else {
        opts.browser_support = opts.home.join("no-browsers");
    }
    let clock = flag("fake_clock", true).then(|| FakeClock::new(None));
    if let Some(c) = &clock {
        opts.clock = Arc::new(c.clone());
        opts.timer_loop = false;
    }
    let ids = config.get("ids").and_then(Value::as_str).map(|p| {
        Arc::new(Seq {
            prefix: p.to_string(),
            n: AtomicU64::new(0),
        })
    });
    if let Some(i) = &ids {
        opts.ids = i.clone();
    }
    let tokens = Arc::new(AtomicU64::new(0));
    {
        let t = tokens.clone();
        opts.timer_tokens = Some(Arc::new(move || {
            format!("t{:015}", t.fetch_add(1, Ordering::SeqCst) + 1)
        }));
    }
    let stand = Stand::default();
    let script = config
        .get("script")
        .and_then(Value::as_str)
        .and_then(crate::standins_chat::Script::load);
    if let Some(s) = &script {
        opts.question_ids = crate::standins_chat::question_counter(s.asked.clone());
    }
    opts.overrides = Some(Arc::new(Seams {
        config: config.clone(),
        stand: stand.clone(),
        group_a: flag("group_a", true),
        script: script.clone(),
    }));
    Some(StandIns {
        script,
        config,
        clock,
        ids,
        tokens,
        stand,
    })
}

/// The control socket, served for the daemon's life.
pub async fn attach(d: &Arc<Daemon>, s: StandIns) {
    let path = d.opts.home.join("run/stand-ins.sock");
    let _ = std::fs::remove_file(&path);
    let Ok(listener) = tokio::net::UnixListener::bind(&path) else {
        return;
    };
    let s = Arc::new(s);
    let d = d.clone();
    let job = tokio::spawn(async move {
        while let Ok((conn, _)) = listener.accept().await {
            let (d, s) = (d.clone(), s.clone());
            tokio::spawn(async move {
                let (r, mut w) = conn.into_split();
                let mut lines = BufReader::new(r).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let req: Value = serde_json::from_str(&line).unwrap_or(Value::Null);
                    let out = match control(&d, &s, &req).await {
                        Ok(v) => json!({"ok": v}),
                        Err(e) => json!({"error": e}),
                    };
                    let mut text = out.to_string();
                    text.push('\n');
                    if w.write_all(text.as_bytes()).await.is_err() {
                        return;
                    }
                }
            });
        }
    });
    d_keep(&job);
    std::mem::forget(job);
}

fn d_keep(_job: &tokio::task::JoinHandle<()>) {}

fn sql_value(a: &Value) -> rusqlite::types::Value {
    match a {
        Value::Null => rusqlite::types::Value::Null,
        Value::Number(n) if n.is_i64() => rusqlite::types::Value::Integer(n.as_i64().unwrap_or(0)),
        Value::Number(n) => rusqlite::types::Value::Real(n.as_f64().unwrap_or(0.0)),
        Value::Bool(b) => rusqlite::types::Value::Integer(i64::from(*b)),
        Value::String(s) => rusqlite::types::Value::Text(s.clone()),
        other => rusqlite::types::Value::Text(other.to_string()),
    }
}

async fn control(d: &Arc<Daemon>, s: &StandIns, req: &Value) -> Result<Value, String> {
    match req["do"].as_str().unwrap_or("") {
        "clock" => {
            let c = s.clock.as_ref().ok_or("no fake clock")?;
            c.advance(req["advance_s"].as_f64().unwrap_or(0.0));
            Ok(Value::Null)
        }
        "ids_next" => {
            let i = s.ids.as_ref().ok_or("no sequential ids")?;
            i.n.store(
                req["n"].as_u64().unwrap_or(1).saturating_sub(1),
                Ordering::SeqCst,
            );
            Ok(Value::Null)
        }
        "ids_bump" => {
            let i = s.ids.as_ref().ok_or("no sequential ids")?;
            i.n.fetch_add(1, Ordering::SeqCst);
            Ok(Value::Null)
        }
        "tokens" => {
            s.tokens
                .store(req["n"].as_u64().unwrap_or(0), Ordering::SeqCst);
            Ok(Value::Null)
        }
        "is_app" => {
            let answer = req["answer"].as_bool().unwrap_or(true);
            d.server.set_is_app(move |_| answer);
            Ok(Value::Null)
        }
        "memory_start" => {
            d.a.call(|st| async move { areas_a::memory::start(&st).map_err(|e| e.to_string()) })
                .await
                .map_err(|e| e.to_string())?
                .map(|_| Value::Null)
        }
        "sql" => {
            let statements: Vec<(String, Vec<Value>)> =
                serde_json::from_value(req["statements"].clone()).map_err(|e| e.to_string())?;
            d.a.call(move |st| async move {
                for (q, args) in statements {
                    let params: Vec<rusqlite::types::Value> = args.iter().map(sql_value).collect();
                    st.db()
                        .conn()
                        .execute(&q, rusqlite::params_from_iter(params))
                        .map_err(|e| format!("{q}: {e}"))?;
                }
                Ok::<_, String>(Value::Null)
            })
            .await
            .map_err(|e| e.to_string())?
        }
        "table" => {
            let name = req["name"].as_str().unwrap_or("").to_string();
            d.a.call(move |st| async move {
                let conn = st.db().conn();
                let mut stmt = conn
                    .prepare(&format!("SELECT * FROM \"{name}\""))
                    .map_err(|e| e.to_string())?;
                let n = stmt.column_count();
                let rows = stmt
                    .query_map([], |r| {
                        Ok((0..n)
                            .map(|i| match r.get_ref(i) {
                                Ok(rusqlite::types::ValueRef::Integer(v)) => json!(v),
                                Ok(rusqlite::types::ValueRef::Real(v)) => json!(v),
                                Ok(rusqlite::types::ValueRef::Text(t)) => {
                                    json!(String::from_utf8_lossy(t))
                                }
                                Ok(rusqlite::types::ValueRef::Blob(b)) => {
                                    json!({"hex": hex::encode(b)})
                                }
                                _ => Value::Null,
                            })
                            .collect::<Vec<Value>>())
                    })
                    .map_err(|e| e.to_string())?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|e| e.to_string())?;
                Ok::<_, String>(json!(rows))
            })
            .await
            .map_err(|e| e.to_string())?
        }
        "timers_tick" => {
            d.a.call(|st| async move {
                let fired = areas_a::schedules::tick(&st).await.map_err(|e| e.message)?;
                areas_a::schedules::settle_runs(&st).await;
                Ok::<_, String>(json!(fired))
            })
            .await
            .map_err(|e| e.to_string())?
        }
        "calls" => Ok(json!(std::mem::take(
            &mut *s.stand.calls.lock().unwrap_or_else(|e| e.into_inner())
        ))),
        "issue" => {
            let identity = if req["identity"] == "ferb" {
                ferb_rpc::auth::Identity::Ferb
            } else {
                ferb_rpc::auth::Identity::Owner
            };
            let scopes: BTreeSet<String> = req["scopes"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(String::from)
                        .collect()
                })
                .unwrap_or_default();
            let tools: BTreeSet<String> = req["tools"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(String::from)
                        .collect()
                })
                .unwrap_or_default();
            let token = d.tokens.issue(
                req["task_id"].as_str().unwrap_or("task-1"),
                req["worker_id"].as_str().unwrap_or("w-1"),
                ferb_rpc::auth::GrantSpec {
                    scopes,
                    identity,
                    ttl_s: req["ttl_s"].as_f64().unwrap_or(600.0),
                    observe_only: req["observe_only"].as_bool().unwrap_or(false),
                    tools,
                    max_calls: req["max_calls"].as_u64().unwrap_or(0),
                },
            );
            Ok(json!(token))
        }
        "hold_task" => {
            // a live task record for the gateway (what the supervisor's `tasks.hold` gives it)
            let task_id = req["task_id"].as_str().unwrap_or("task-1").to_string();
            let span_id = {
                let db = crate::basics::lock(&d.db);
                let r = d.redactor.clone();
                let clock: Arc<dyn ferb_core::interfaces::Clock> = d.opts.clock.clone();
                let spans = ferb_store::spans::SpanStore::new(
                    &db,
                    Box::new(ferb_gateway::gateway::StoreTime(clock)),
                    Box::new(ferb_gateway::gateway::StoreIds(d.opts.ids.clone())),
                    Box::new(move |v: &Value| r.value(v)),
                    Box::new(|_k: &str| false),
                );
                spans
                    .start(
                        "task",
                        &json!({"goal": "a test's task"}),
                        Some(&task_id),
                        None,
                        None,
                    )
                    .map_err(|e| e.to_string())?
                    .id
            };
            let started = ferb_core::interfaces::Clock::now(d.opts.clock.as_ref());
            d.tools.hold_task(
                &d.gateway,
                ferb_gateway::gateway::TaskRecord {
                    task_id,
                    request_id: req["request_id"].as_str().unwrap_or("req-1").to_string(),
                    started_at: started,
                    deadline: req["deadline_s"]
                        .as_f64()
                        .map(|s| started + chrono::Duration::milliseconds((s * 1000.0) as i64)),
                    span_id,
                    tree_id: req["tree_id"].as_str().map(String::from),
                },
            );
            Ok(Value::Null)
        }
        "attach_identity" => {
            // `use_identity_browser`: FERB's own profile's tools join the registry (the catalog's last 21)
            let mut reg = d.gateway.registry();
            for t in d.b_tools().ferb_browser_tools() {
                reg.set(t);
            }
            Ok(Value::Null)
        }
        "listing" => {
            // the gateway's listing for a grant, straight from the registry (the catalog test)
            let identity = if req["identity"] == "ferb" {
                ferb_rpc::auth::Identity::Ferb
            } else {
                ferb_rpc::auth::Identity::Owner
            };
            let set = |k: &str| -> BTreeSet<String> {
                req[k]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(Value::as_str)
                            .map(String::from)
                            .collect()
                    })
                    .unwrap_or_default()
            };
            let grant = ferb_rpc::auth::RunGrant {
                task_id: "t".into(),
                worker_id: "w".into(),
                scopes: set("scopes"),
                identity,
                expires_at: f64::MAX,
                observe_only: req["observe_only"].as_bool().unwrap_or(false),
                tools: set("tools"),
                max_calls: req["max_calls"].as_u64().unwrap_or(0),
            };
            let tools = crate::compose::rpc::tools_for(&d.gateway, &d.b_tools(), &grant);
            Ok(json!(tools))
        }
        // the tools built as refusing "not available in this build" (the catalog test: none)
        "refusing" => Ok(json!(crate::compose::tools::refusing())),
        // a chat scenario's (standins_chat.rs), then areas_c's and the services' controls (standins_c.rs)
        other => match crate::standins_chat::control(d, s.script.as_ref(), req, other) {
            Some(r) => r,
            None => crate::standins_c::control(d, req, other),
        },
    }
}
