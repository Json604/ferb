//! `ferb/daemon/areas/runtimes.py` against the Python daemon: the signed-out `runtimes` scenario and the keys calls
//! of engine/fixtures/daemon (rpc_transcripts.json), and the six scenarios engine/fixtures/daemon-b/runtimes.py recorded
//! past them (signed in, keys stored and moved, checks passing and failing, a model and effort saved, a switch
//! answered, the Claude sign-in). Every answer line is compared byte for byte, and so are the journal rows, the
//! published events, every seam call FERB made (builds, starts, cancels, tokens, task holds, spans, gateway forgets,
//! model lists, offers), config.toml and the fake Keychain afterwards.
// decision: GATE-543, GATE-547, RUN-424, RUN-429, RUN-439, RUN-463, DMN-580, ORCH-315, APP-882

mod b_support;

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use b_support::{fixture, line_text, replay, scenario, Daemon, Home};
use ferb_core::errors::FerbError;
use ferb_core::interfaces::Clock as _;
use ferb_core::types::{Identity, ModelRequest, RunHandle, RuntimeEvent, RuntimeEventKind, Task};
use ferb_core::when::DateTime;
use ferb_daemon::areas_b::runtimes::check::CheckTool;
use ferb_daemon::areas_b::runtimes::keys::{HelperError, KeyHelper, Security};
use ferb_daemon::areas_b::runtimes::{
    self, Build, Built, CheckHost, Deps, Factory, JournalReads, ModelLists, Programs, Runtimes,
    State, Supervisor, ToolRegistry,
};
use ferb_identity::Secret;
use ferb_rpc::{BoxFuture, HandlerError};
use ferb_runtimes::error::{Raised, RuntimeError};
use ferb_runtimes::gen::EventStream;
use ferb_runtimes::run::{Adapter, RunControl, Started};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

// ---------- the fakes, each logging what Python's recorder logged ----------

#[derive(Default)]
struct World {
    log: Mutex<Vec<Value>>,
    script: Mutex<VecDeque<Value>>,
    tools: Mutex<HashMap<String, Arc<dyn CheckTool>>>,
    lists: Mutex<Map<String, Value>>,
    offers: Mutex<Map<String, Value>>,
    helper_items: Mutex<Map<String, Value>>,
    security_items: Mutex<Map<String, Value>>,
    reads: Mutex<Vec<String>>,
    delete_fails: Mutex<bool>,
    /// The Python fixture's stand-in: the helper is never found.
    no_helper: bool,
    ids: Mutex<u64>,
    claude_cli: Option<PathBuf>,
    home: Option<Arc<Home>>,
}

impl World {
    fn log(&self, entry: Value) {
        self.log.lock().unwrap().push(entry);
    }
}

#[derive(Clone)]
struct W(Arc<World>);

fn sha(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

fn exc(spec: &Value) -> RuntimeError {
    let (cls, words) = (
        spec[0].as_str().unwrap(),
        spec[1].as_str().unwrap().to_string(),
    );
    match cls {
        "Unsupported" => RuntimeError::Ferb(FerbError::Unsupported(words)),
        other => RuntimeError::Raised(Raised::new(other, words)),
    }
}

struct Control(W);

impl RunControl for Control {
    fn steer<'a>(
        &'a self,
        _message: &'a str,
    ) -> ferb_core::interfaces::BoxFuture<'a, Result<(), RuntimeError>> {
        Box::pin(async { Err(RuntimeError::unsupported("no steering in a check")) })
    }
    fn cancel(&self) -> ferb_core::interfaces::BoxFuture<'_, ()> {
        self.0 .0.log(json!(["cancel"]));
        Box::pin(async {})
    }
}

/// The scripted runtime, and whether its schema has ORCH-652's gaps (a worker report's or a check's).
struct Scripted(W, bool);

impl Adapter for Scripted {
    fn name(&self) -> &str {
        "scripted"
    }
    fn start<'a>(
        &'a self,
        task: &'a Task,
        request: &'a ModelRequest,
        run_token: &'a str,
    ) -> ferb_core::interfaces::BoxFuture<'a, Result<Started, RuntimeError>> {
        Box::pin(async move {
            let w = self.0.clone();
            let entry =
                w.0.script
                    .lock()
                    .unwrap()
                    .pop_front()
                    .expect("a script entry for this start");
            let images: Vec<Value> = request
                .images
                .iter()
                .map(|i| json!({"sha256": sha(&i.data), "png_hex": hex::encode(&i.data), "reference": i.reference,
                                "mime": i.mime}))
                .collect();
            w.0.log(json!(["start", {"task_id": task.id, "parent_id": task.parent_id, "goal": task.goal,
                "device_id": task.device_id, "identity": task.identity, "scopes": task.scopes, "role": task.role,
                "request_id": task.request_id, "system": request.system, "parts": request.parts,
                "cacheable": request.cacheable, "images": images, "token": !run_token.is_empty()}]));
            if let Some(spec) = entry.get("raise") {
                return Err(exc(spec));
            }
            let body = w.clone();
            let gaps = self.1;
            let events = EventStream::spawn(move |mut y| async move {
                for t in entry
                    .get("tools")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default()
                {
                    let tool = body
                        .0
                        .tools
                        .lock()
                        .unwrap()
                        .get(t[0].as_str().unwrap())
                        .cloned()
                        .unwrap();
                    tool.run(t[1].as_object().unwrap());
                }
                for e in entry
                    .get("events")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default()
                {
                    let kind: RuntimeEventKind = serde_json::from_value(e[0].clone()).unwrap();
                    let mut data = e[1].as_object().cloned().unwrap_or_default();
                    if gaps {
                        gaps_answered(&mut data);
                    }
                    if y.emit(RuntimeEvent { kind, data }).await.is_err() {
                        return Ok(());
                    }
                }
                match entry.get("raise_in_events") {
                    Some(spec) => Err(exc(spec)),
                    None => Ok(()),
                }
            });
            Ok(Started {
                handle: RunHandle {
                    id: format!("run-{}", task.id),
                    runtime: "scripted".into(),
                    task_id: task.id.clone(),
                    state: Map::new(),
                },
                events,
                control: Arc::new(Control(w)),
            })
        })
    }
}

impl Factory for W {
    fn build(&self, b: &Build) -> Result<Built, RuntimeError> {
        let mut entry = json!({"name": b.name, "role": b.role, "mode": b.mode, "tools": b.tool_names,
            // ORCH-652's gaps and HAND-580's task profiles (not Python's) taken out: the named changes
            "schema_sha256": sha(ferb_py::json::dumps(&ferb_context::schemas::as_recorded(&b.schema)).as_bytes()),
            "model": b.model, "effort": b.effort,
            "planner": b.planner, "limit_s": b.limit_s});
        if b.name == "codex" {
            entry["cache_role"] = json!(b.cache_role);
            entry["cache_scope"] = json!(b.cache_scope);
        }
        self.0.log(json!(["build", entry]));
        Ok(Built {
            adapter: Arc::new(Scripted(
                self.clone(),
                ferb_context::schemas::before_gaps(&b.schema) != b.schema,
            )),
            retried: Arc::new(Vec::new),
        })
    }
}

impl Supervisor for W {
    fn runtime_offer(&self, task_id: &str) -> Option<Value> {
        self.0.log(json!(["offer", task_id]));
        self.0.offers.lock().unwrap().get(task_id).cloned()
    }
    fn answer_runtime_switch(&self, task_id: &str, runtime: &str) -> Result<Value, HandlerError> {
        self.0.log(json!(["answer", task_id, runtime]));
        let role = self.0.offers.lock().unwrap()[task_id]["role"].clone();
        Ok(json!({"task_id": task_id, "runtime": runtime, "role": role}))
    }
}

impl Programs for W {
    fn codex_binary(&self) -> Result<PathBuf, FerbError> {
        Ok(PathBuf::from(
            "/REPO/node_modules/@openai/codex-darwin-arm64/vendor/aarch64-apple-darwin/bin/codex",
        ))
    }
    fn claude_cli(&self) -> Result<PathBuf, FerbError> {
        self.0.claude_cli.clone().ok_or_else(|| {
            FerbError::Unsupported("the claude program isn't available in the fixture".into())
        })
    }
    fn bundled_catalog(&self) {}
    fn prepare_codex<'a>(&'a self, _binary: &'a Path) -> BoxFuture<'a, Result<(), FerbError>> {
        Box::pin(async { Ok(()) })
    }
    fn check_codex_version<'a>(
        &'a self,
        _binary: &'a Path,
    ) -> BoxFuture<'a, Result<(), FerbError>> {
        Box::pin(async { Ok(()) })
    }
    fn claude_pin_status(&self) -> String {
        String::new()
    }
}

/// tests/test_api_keys.py FakeKeychain, the helper's side.
impl KeyHelper for W {
    fn store(&self, account: &str, value: &Secret) -> Result<(), HelperError> {
        if self.0.no_helper {
            return Err(HelperError::Refused(
                "the signed secret helper is unavailable".into(),
            ));
        }
        self.0
            .helper_items
            .lock()
            .unwrap()
            .insert(account.into(), value.expose().into());
        Ok(())
    }
    fn lookup(&self, account: &str) -> Result<Option<Secret>, HelperError> {
        if self.0.no_helper {
            return Err(HelperError::Refused(
                "the signed secret helper is unavailable".into(),
            ));
        }
        self.0
            .reads
            .lock()
            .unwrap()
            .push(format!("helper:{account}"));
        match self.0.helper_items.lock().unwrap().get(account) {
            Some(v) => Ok(Some(Secret::new(v.as_str().unwrap().into()))),
            None => Err(HelperError::Refused(
                "the Keychain didn't show this item".into(),
            )),
        }
    }
    fn delete(&self, account: &str) -> Result<bool, HelperError> {
        if self.0.no_helper {
            return Err(HelperError::Refused(
                "the signed secret helper is unavailable".into(),
            ));
        }
        Ok(self
            .0
            .helper_items
            .lock()
            .unwrap()
            .shift_remove(account)
            .is_some())
    }
}

/// FakeKeychain's `security`.
impl Security for W {
    fn run(&self, args: &[&str]) -> (i32, Secret) {
        let none = Secret::new(String::new());
        let verb = args[0];
        let acct = args[args.iter().position(|a| *a == "-a").unwrap() + 1];
        let mut items = self.0.security_items.lock().unwrap().clone();
        for (k, v) in self.0.helper_items.lock().unwrap().iter() {
            items.insert(k.clone(), v.clone());
        }
        match verb {
            "find-generic-password" => match items.get(acct) {
                None => (44, none),
                Some(v) if args.contains(&"-w") => {
                    self.0
                        .reads
                        .lock()
                        .unwrap()
                        .push(format!("security:{acct}"));
                    (0, Secret::new(format!("{}\n", v.as_str().unwrap())))
                }
                Some(_) => (0, none),
            },
            "delete-generic-password" => {
                if *self.0.delete_fails.lock().unwrap() {
                    return (1, none);
                }
                let gone = self
                    .0
                    .security_items
                    .lock()
                    .unwrap()
                    .shift_remove(acct)
                    .is_some();
                (if gone { 0 } else { 44 }, none)
            }
            other => panic!("security {other}"),
        }
    }
}

impl ModelLists for W {
    fn provider_models<'a>(
        &'a self,
        provider: &'a str,
        key: &'a Secret,
    ) -> BoxFuture<'a, Result<Vec<String>, String>> {
        self.0
            .log(json!(["models.list", {"provider": provider, "key": key.expose()}]));
        let found = self.0.lists.lock().unwrap().get(provider).cloned();
        Box::pin(async move {
            match found {
                Some(Value::String(words)) => Err(words),
                Some(Value::Array(ids)) => Ok(ids
                    .iter()
                    .map(|i| i.as_str().unwrap().to_string())
                    .collect()),
                _ => Ok(vec![]),
            }
        })
    }
}

impl ToolRegistry for W {
    fn add(&self, tool: Arc<dyn CheckTool>) {
        self.0
            .tools
            .lock()
            .unwrap()
            .insert(tool.name().into(), tool);
    }
    fn remove(&self, name: &str) {
        self.0.tools.lock().unwrap().remove(name);
    }
}

impl CheckHost for W {
    fn new_id(&self) -> String {
        let mut n = self.0.ids.lock().unwrap();
        *n += 1;
        format!("#{}", *n)
    }
    fn span_start(&self, kind: &str, input: Value) -> String {
        self.0
            .log(json!(["span.start", {"kind": kind, "task_id": null, "input": input}]));
        "span".into()
    }
    fn span_finish(&self, _span_id: &str, output: Value) {
        self.0.log(json!(["span.finish", {"output": output}]));
    }
    fn hold_task(&self, task: &Task, started_at: &DateTime, _span_id: &str) {
        self.0.log(
            json!(["tasks.hold", {"task_id": task.id, "goal": task.goal, "scopes": task.scopes,
                                          "at": ferb_core::when::isoformat(started_at)}]),
        );
    }
    fn remove_task(&self, task_id: &str) {
        self.0.log(json!(["tasks.remove", task_id]));
    }
    fn issue_token(
        &self,
        task_id: &str,
        holder: &str,
        scopes: &BTreeSet<String>,
        identity: Identity,
        ttl_s: f64,
        observe_only: bool,
        tools: &BTreeSet<String>,
        max_calls: u32,
    ) -> String {
        self.0.log(
            json!(["token.issue", {"task_id": task_id, "holder": holder, "scopes": scopes,
            "identity": identity, "ttl_s": ttl_s, "observe_only": observe_only, "tools": tools,
            "max_calls": max_calls}]),
        );
        "run-token".into()
    }
    fn revoke_token(&self, holder: &str) {
        self.0.log(json!(["token.revoke", holder]));
    }
    fn forget_calls(&self, holder: &str) {
        self.0.log(json!(["gateway.forget", holder]));
    }
}

/// The journal's reads over the rows this test's handlers wrote (`at` is the fake clock's now).
impl JournalReads for W {
    fn latest_kind(&self, kinds: &[&str], field: &str, equals: &str) -> Option<String> {
        let home = self.0.home.as_ref().unwrap();
        home.journal_rows()
            .iter()
            .rev()
            .find(|r| kinds.contains(&r[0].as_str().unwrap()) && r[1][field] == equals)
            .map(|r| r[0].as_str().unwrap().to_string())
    }
    fn of_kind(&self, kind: &str, field: &str, equals: &str) -> Vec<(String, Value)> {
        let home = self.0.home.as_ref().unwrap();
        let at = ferb_core::when::isoformat(&home.clock.now());
        home.journal_rows()
            .iter()
            .filter(|r| r[0] == kind && r[1][field] == equals)
            .map(|r| (at.clone(), r[1].clone()))
            .collect()
    }
}

struct Setup {
    home: Arc<Home>,
    world: W,
    deps: Deps,
}

fn setup(config: &str, s: Option<&Value>, no_helper: bool) -> Setup {
    let home = Arc::new(Home::new(config));
    let mut world = World {
        no_helper,
        home: Some(home.clone()),
        ..World::default()
    };
    if let Some(s) = s {
        for (rel, text) in s["files"].as_object().unwrap() {
            let path = home.path().join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, line_text(text)).unwrap();
        }
        let kc = &s["keychain"];
        *world.helper_items.get_mut().unwrap() =
            kc["helper"].as_object().cloned().unwrap_or_default();
        *world.security_items.get_mut().unwrap() =
            kc["security"].as_object().cloned().unwrap_or_default();
        *world.delete_fails.get_mut().unwrap() = kc["delete_fails"] == true;
        *world.lists.get_mut().unwrap() = s["lists"].as_object().cloned().unwrap();
        *world.offers.get_mut().unwrap() = s["offers"].as_object().cloned().unwrap();
        *world.script.get_mut().unwrap() = s["script"].as_array().cloned().unwrap().into();
        if let Some(sh) = s["claude_sh"].as_str() {
            use std::os::unix::fs::PermissionsExt;
            let cli = home.dir.path().join("bin/claude");
            std::fs::create_dir_all(cli.parent().unwrap()).unwrap();
            std::fs::write(&cli, sh).unwrap();
            std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o755)).unwrap();
            world.claude_cli = Some(cli);
        }
    }
    let w = W(Arc::new(world));
    let deps = Deps {
        supervisor: Arc::new(w.clone()),
        factory: Arc::new(w.clone()),
        programs: Arc::new(w.clone()),
        helper: Arc::new(w.clone()),
        security: Arc::new(w.clone()),
        tools: Arc::new(w.clone()),
        checks: Arc::new(w.clone()),
        journal_reads: Arc::new(w.clone()),
        model_lists: Arc::new(w.clone()),
        state: Arc::new(State::default()),
    };
    Setup {
        home,
        world: w,
        deps,
    }
}

/// Named difference from Python (ENG-6): a runtime check whose adapter's event stream raises answers ok false with
/// the error's class and words, where Python let the exception escape as an internal error (-32603).
fn stream_raise_fix(request: &str, expected: &str) -> Option<String> {
    if b_support::method_of(request) != "runtimes.check" || !expected.contains("\"code\": -32603") {
        return None;
    }
    let id = serde_json::from_str::<Value>(request).unwrap()["id"].clone();
    let class = serde_json::from_str::<Value>(expected).unwrap()["error"]["message"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(class, "ConnectionError");
    Some(format!(
        "{{\"jsonrpc\": \"2.0\", \"id\": {id}, \"result\": {{\"ok\": false, \"detail\": \"ConnectionError: invented network drop\"}}}}"
    ))
}

/// Named difference from Python (DMN-595, ORCH-663, SENSE-422): the check's planner call offers the planner's lookups as
/// the supervisor does, `about.diagnostics`, `task.trace` and `device.facts` (Rust additions) with Python's six, sorted. The recorded planner builds and their run
/// tokens are compared with it added; returns how many lists it named.
fn planner_lookups_added(log: &mut Value) -> usize {
    let python: Vec<&str> = {
        let mut six: Vec<&str> = ferb_orchestrator::lookups::PLANNER_TOOLS
            .iter()
            .copied()
            .filter(|t| !["about.diagnostics", "task.trace", "device.facts"].contains(t))
            .collect();
        six.sort();
        six
    };
    let mut rust = ferb_orchestrator::lookups::PLANNER_TOOLS.to_vec();
    rust.sort();
    let mut n = 0;
    for e in log.as_array_mut().unwrap() {
        let planner = (e[0] == "build" && e[1]["mode"] == "supervisor") || e[0] == "token.issue";
        if planner && e[1]["tools"] == json!(python) {
            e[1]["tools"] = json!(rust);
            n += 1;
        }
    }
    n
}

/// Named difference from Python (RUN-466, owner 2026-11-16): a Codex check resumes the checker's fixed thread for its
/// cache key (`cache_role` "checker", `[runtimes] codex_checker_cache` on by default), where Python's checker ran
/// an ephemeral thread (`cache_role` null). Returns how many recorded builds it named.
fn checker_cache_added(log: &mut Value) -> usize {
    let mut n = 0;
    for e in log.as_array_mut().unwrap() {
        let is_build = e[0] == "build";
        let Some(b) = e.get_mut(1) else { continue };
        let checker = b["role"]
            .as_str()
            .is_some_and(|r| r.split(':').next() == Some("checker"));
        if is_build
            && b["name"] == "codex"
            && b["mode"] == "worker"
            && checker
            && b["cache_role"].is_null()
        {
            b["cache_role"] = json!("checker");
            n += 1;
        }
    }
    n
}

fn renumber(log: &[Value]) -> Value {
    let text = serde_json::to_string(log).unwrap();
    let re = regex::Regex::new(r"check-#\d+").unwrap();
    let mut seen: Vec<String> = vec![];
    let out = re.replace_all(&text, |c: &regex::Captures| {
        let k = c[0].to_string();
        let n = seen.iter().position(|s| *s == k).unwrap_or_else(|| {
            seen.push(k.clone());
            seen.len() - 1
        });
        format!("check-#{}", n + 1)
    });
    serde_json::from_str(&out).unwrap()
}

/// `{"sha256"}` references in a recorded value put back.
fn expand(v: &Value) -> Value {
    match v {
        Value::Object(o) if o.len() == 1 && o.contains_key("sha256") => Value::String(line_text(v)),
        Value::Object(o) => Value::Object(o.iter().map(|(k, x)| (k.clone(), expand(x))).collect()),
        Value::Array(a) => Value::Array(a.iter().map(expand).collect()),
        other => other.clone(),
    }
}

fn diff(what: &str, python: &Value, rust: &Value, diffs: &mut Vec<String>) {
    if python != rust {
        let (p, r) = (python.as_array(), rust.as_array());
        if let (Some(p), Some(r)) = (p, r) {
            for i in 0..p.len().max(r.len()) {
                if p.get(i) != r.get(i) {
                    diffs.push(format!(
                        "{what}[{i}]:\n  python: {}\n  rust:   {}",
                        p.get(i).map_or("-".into(), |x| x.to_string()),
                        r.get(i).map_or("-".into(), |x| x.to_string())
                    ));
                    return;
                }
            }
        }
        diffs.push(format!("{what}:\n  python: {python}\n  rust:   {rust}"));
    }
}

async fn wait_sign_in(state: &State, want: &str, url: bool) {
    let until = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        let st = state.sign_in.status();
        if st["state"] == want && (st["url"] != "") == url {
            return;
        }
        assert!(
            std::time::Instant::now() < until,
            "the sign-in never became {want}: {st:?}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// (lines the same, answers named, planner builds named).
async fn play(name: &str) -> (usize, usize, usize) {
    let file = fixture("daemon-b/runtimes.json");
    let sc = scenario(&file, name);
    let s = &sc["setup"];
    let st = setup(&line_text(&s["config"]), Some(s), false);
    let rt = Runtimes::new(st.home.host.clone(), st.deps.clone());
    let deps = st.deps.clone();
    let host = st.home.host.clone();
    let daemon = Daemon::start(move |server| runtimes::register(server, &host, &deps)).await;
    let steps = sc["steps"].as_array().unwrap();
    let (mut same, mut named) = (0, 0);
    let mut segment: Vec<Value> = vec![];
    let mut all_diffs = vec![];
    for step in steps.iter().chain([json!({"do": "end"})].iter()) {
        let what = step["do"].as_str().unwrap();
        if !["act", "wait_sign_in", "keychain", "end"].contains(&what) {
            segment.push(step.clone());
            continue;
        }
        if !segment.is_empty() {
            let out = replay(&daemon, &segment, &|_| true, &[], &stream_raise_fix).await;
            same += out.same.values().sum::<usize>();
            named += out.named.values().sum::<usize>();
            all_diffs.extend(out.diffs);
            segment.clear();
        }
        match what {
            "act" => match step["what"].as_str().unwrap() {
                "runtime_account" => rt
                    .runtime_account(
                        step["args"][0].as_str().unwrap(),
                        step["args"][1].as_str().unwrap(),
                    )
                    .unwrap(),
                "forget_refusals" => st.deps.state.forget_refusals(),
                other => panic!("act {other}"),
            },
            "wait_sign_in" => {
                wait_sign_in(
                    &st.deps.state,
                    step["state"].as_str().unwrap(),
                    step["url"] == true,
                )
                .await
            }
            "keychain" => {
                *st.world.0.delete_fails.lock().unwrap() = step["delete_fails"] == true;
                for (k, v) in step["security"].as_object().unwrap() {
                    st.world
                        .0
                        .security_items
                        .lock()
                        .unwrap()
                        .insert(k.clone(), v.clone());
                }
            }
            _ => {}
        }
    }
    // a sign-in's end is told after its last state change: let it land before reading the journal
    tokio::time::sleep(Duration::from_millis(100)).await;
    let python_journal: Vec<Value> = sc["journal"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r[0] != "daemon.internal_error") // the stream-raise fix: Rust answers instead of crashing
        .cloned()
        .collect();
    diff(
        &format!("{name} journal"),
        &json!(python_journal),
        &json!(st.home.journal_rows()),
        &mut all_diffs,
    );
    diff(
        &format!("{name} published"),
        &sc["published"],
        &json!(st.home.events()),
        &mut all_diffs,
    );
    let rust_log = renumber(&st.world.0.log.lock().unwrap());
    let mut python_log = expand(&sc["log"]);
    let lookups = planner_lookups_added(&mut python_log);
    let checker_cache = checker_cache_added(&mut python_log);
    assert_eq!(
        checker_cache,
        usize::from(name == "checks"),
        "{name}: the Codex checker builds named (the runtime check's one checker call)"
    );
    diff(
        &format!("{name} log"),
        &python_log,
        &rust_log,
        &mut all_diffs,
    );
    let after = expand(&sc["after"]);
    let config = std::fs::read_to_string(st.home.path().join("config.toml")).unwrap();
    diff(
        &format!("{name} config.toml"),
        &after["config"],
        &json!(config),
        &mut all_diffs,
    );
    let keychain = json!({"helper": *st.world.0.helper_items.lock().unwrap(),
                          "security": *st.world.0.security_items.lock().unwrap(),
                          "reads": *st.world.0.reads.lock().unwrap()});
    diff(
        &format!("{name} keychain"),
        &sort_items(&after["keychain"]),
        &sort_items(&keychain),
        &mut all_diffs,
    );
    let signed = st.home.path().join("claude-home/.ferb-signed-in").is_file();
    diff(
        &format!("{name} signed in"),
        &after["claude_signed_in"],
        &json!(signed),
        &mut all_diffs,
    );
    assert!(
        st.world.0.script.lock().unwrap().is_empty(),
        "{name}: script entries left"
    );
    assert!(
        all_diffs.is_empty(),
        "{} difference(s):\n{}",
        all_diffs.len(),
        all_diffs.join("\n")
    );
    assert!(same > 0);
    (same, named, lookups)
}

fn sort_items(v: &Value) -> Value {
    let mut out = v.clone();
    for k in ["helper", "security"] {
        if let Some(Value::Object(o)) = out.get_mut(k) {
            o.sort_keys();
        }
    }
    out
}

#[tokio::test(flavor = "multi_thread")]
async fn status_and_choice() {
    let (same, _, lookups) = play("status_and_choice").await;
    assert_eq!(lookups, 0);
    eprintln!("status_and_choice: {same} lines");
}

#[tokio::test(flavor = "multi_thread")]
async fn keys() {
    let (same, _, lookups) = play("keys").await;
    assert_eq!(lookups, 0);
    eprintln!("keys: {same} lines");
}

// decision: DMN-595, ORCH-641, RUN-466, ORCH-675
#[tokio::test(flavor = "multi_thread")]
async fn checks() {
    let (same, named, lookups) = play("checks").await;
    assert_eq!(named, 1, "the one named difference: a stream that raises");
    assert_eq!(
        lookups, 6,
        "three planner calls' lookups, built and granted, about.diagnostics added"
    );
    eprintln!("checks: {same} lines, {named} named");
}

#[tokio::test(flavor = "multi_thread")]
async fn model_choice() {
    let (same, _, lookups) = play("model_choice").await;
    assert_eq!(lookups, 0);
    eprintln!("model_choice: {same} lines");
}

#[tokio::test(flavor = "multi_thread")]
async fn switch_task() {
    let (same, _, lookups) = play("switch_task").await;
    assert_eq!(lookups, 0);
    eprintln!("switch_task: {same} lines");
}

#[tokio::test(flavor = "multi_thread")]
async fn sign_in() {
    let (same, _, lookups) = play("sign_in").await;
    assert_eq!(lookups, 0);
    eprintln!("sign_in: {same} lines");
}

/// The Python daemon's own recording (signed out; helper never found; `security` finds nothing).
#[tokio::test(flavor = "multi_thread")]
async fn signed_out_transcripts() {
    let file = fixture("daemon/rpc_transcripts.json");
    let mut total = 0;
    for (name, keep) in [("runtimes", "runtimes."), ("settings_setup_about", "keys.")] {
        let st = setup("", None, true);
        let (deps, host) = (st.deps.clone(), st.home.host.clone());
        let daemon = Daemon::start(move |server| runtimes::register(server, &host, &deps)).await;
        let sc = scenario(&file, name);
        let out = replay(
            &daemon,
            sc["steps"].as_array().unwrap(),
            &|m: &str| m.starts_with(keep),
            &["pid"],
            &|_, _| None,
        )
        .await;
        out.assert_clean();
        let n: usize = out
            .same
            .iter()
            .filter(|(m, _)| m.starts_with(keep))
            .map(|x| x.1)
            .sum();
        assert!(n > 0, "{name}: nothing compared");
        total += n;
    }
    eprintln!("signed-out transcripts: {total} lines");
}

/// Per role: the cache role built, and whether the planner's developer-role refusal came with it.
type Roles = Vec<(String, Option<String>, bool)>;

/// What the daemon builds for each role on Codex, with and without `[runtimes] codex_checker_cache`.
fn cache_roles(config: &str) -> (Runtimes, Setup, Roles) {
    let st = setup(config, None, false);
    let auth = st.home.path().join("codex-home/auth.json");
    std::fs::create_dir_all(auth.parent().unwrap()).unwrap();
    std::fs::write(&auth, "{\"tokens\":{}}").unwrap();
    let rt = Runtimes::new(st.home.host.clone(), st.deps.clone());
    let planner = ferb_context::schemas::supervisor_schema();
    let mut out = vec![];
    for (role, mode, schema) in [
        ("orchestrator", "supervisor", planner.clone()),
        ("orchestrator", "supervisor", json!({"type": "object"})),
        ("checker", "worker", json!({"type": "object"})),
        ("checker:codex", "worker", json!({"type": "object"})),
        ("worker:default", "worker", json!({"type": "object"})),
        ("worker:codex", "worker", json!({"type": "object"})),
    ] {
        let (b, _) = rt
            .build_runtime("codex", role, mode, &[], &schema, None, "", "")
            .unwrap_or_else(|e| panic!("{role}: {e}"));
        let refusal = b.cache_hooks.as_ref().is_some_and(|h| h.refusal.is_some());
        assert_eq!(b.cache_hooks.is_some(), b.cache_role.is_some(), "{role}");
        out.push((role.to_string(), b.cache_role, refusal));
    }
    (rt, st, out)
}

// decision: RUN-401, RUN-416, RUN-466, RUN-467
#[tokio::test(flavor = "multi_thread")]
async fn codex_cache_roles_and_their_journal_lines() {
    let (rt, st, roles) = cache_roles("");
    let r = |s: &str| Some(s.to_string());
    assert_eq!(
        roles,
        vec![
            ("orchestrator".into(), r("planner"), true),
            ("orchestrator".into(), None, false), // a one-turn call that isn't the planner's chat turn
            ("checker".into(), r("checker"), false),
            ("checker:codex".into(), r("checker"), false), // the owner's one-tap switch (ORCH-315)
            ("worker:default".into(), None, false),        // workers keep their own thread
            ("worker:codex".into(), None, false),
        ]
    );
    // the hooks the factory hands Codex: a fallback journaled by role, a skipped planner turn as before
    let checker = rt.cache_hooks("checker");
    (checker.fallback)(
        "task-1.verify",
        "Codex could not complete thread/resume: no rollout",
    );
    let planner = rt.cache_hooks("planner");
    (planner.fallback)(
        "req-1",
        "the planner template could not be written (OSError)",
    );
    (planner.skipped)("req-1", "a scheduled run's turn");
    assert_eq!(
        st.home.journal_rows(),
        vec![
            json!(["runtime.cache_fallback", {"runtime": "codex", "role": "checker", "task_id": "task-1.verify",
                "why": "Codex could not complete thread/resume: no rollout"}, "ferb"]),
            json!(["runtime.cache_fallback", {"runtime": "codex", "role": "planner", "request_id": "req-1",
                "why": "the planner template could not be written (OSError)"}, "ferb"]),
            json!(["runtime.cache_skipped", {"runtime": "codex", "role": "planner", "request_id": "req-1",
                "why": "a scheduled run's turn"}, "ferb"]),
        ]
    );
    // each switch turns off only its own role
    let (_, _, off) = cache_roles("[runtimes]\ncodex_checker_cache = false\n");
    assert_eq!(off[0].1, r("planner"));
    assert_eq!(off[2].1, None);
    let (_, _, off) = cache_roles("[runtimes]\ncodex_planner_cache = false\n");
    assert_eq!(off[0].1, None);
    assert_eq!(off[2].1, r("checker"));
}

/// ORCH-652's, ORCH-679's and ORCH-738's named differences: a report or check recorded against Python's schema,
/// answered as a model given today's (whose `gaps`, and a check's `progress`, each result's `left` and its
/// `unconfirmed_short`, are required) answers it: with `"gaps": []`, `"progress": null`, `"left": null` and
/// `"unconfirmed_short": ""`.
// decision: ORCH-675, ORCH-679, ORCH-738
fn gaps_answered(data: &mut Map<String, Value>) {
    for v in data.values_mut() {
        let Some(text) = v.as_str() else { continue };
        let Ok(Value::Object(mut report)) = serde_json::from_str::<Value>(text) else {
            continue;
        };
        let check = report.contains_key("next");
        let answers = report.contains_key("how") || check;
        let mut changed = false;
        if answers && !report.contains_key("gaps") {
            report.insert("gaps".into(), json!([]));
            changed = true;
        }
        if check && !report.contains_key("progress") {
            report.insert("progress".into(), Value::Null);
            if let Some(Value::Array(results)) = report.get_mut("results") {
                for r in results.iter_mut().filter_map(Value::as_object_mut) {
                    r.entry("left").or_insert(Value::Null);
                }
            }
            changed = true;
        }
        if check && !report.contains_key("unconfirmed_short") {
            report.insert("unconfirmed_short".into(), json!(""));
            changed = true;
        }
        if changed {
            *v = Value::String(ferb_py::json::dumps(&Value::Object(report)));
        }
    }
}
