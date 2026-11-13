//! ext_bridge.Hub and Bridge against rust/fixtures/browser/transcripts.json: the real Python hub was
//! driven by the Python tests' scripted fake extension; here the extension's recorded lines are
//! replayed (each after the daemon lines before it), and the Rust hub must write the same daemon
//! lines, publish the same events, record the same refusals, call the same hooks and return the
//! same results. Time is tokio's paused clock: a timeout passes without waiting.
#![cfg(target_os = "macos")]

mod common;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ferb_browser::bridge::{Bridge, Hooks, Hub, Ledger};
use ferb_browser::driver::{Driver, Kind, OwnerInput, SecretAsk, Secrets, Tasks};
use ferb_browser::pairing::Random;
use ferb_browser::pairing::{sqlite::SqliteInstalls, Pairings};
use ferb_browser::rules::{BrowserApps, BrowserInfo};
use ferb_core::interfaces::{BoxFuture, Clock};
use ferb_core::types::Identity;
use ferb_core::types::{ElementRef, TabRef};
use ferb_core::when::DateTime;
use ferb_identity::secrets::{SecretRef, SecretSpec};
use ferb_identity::Secret;
use serde_json::{json, Map, Value};
use std::collections::HashSet;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, Lines};
use tokio::task::JoinHandle;

use common::{fixture, Counter};

#[path = "ops/mod.rs"]
mod ops;

struct FakeClock(Mutex<(DateTime, f64)>);

impl Clock for FakeClock {
    fn now(&self) -> DateTime {
        self.0.lock().unwrap().0
    }
    fn monotonic(&self) -> f64 {
        self.0.lock().unwrap().1
    }
    fn sleep_until(&self, _: DateTime) -> BoxFuture<'_, ()> {
        Box::pin(async {})
    }
}

type Log = Arc<Mutex<Vec<Value>>>;

struct TestLedger {
    states: Mutex<HashMap<String, String>>,
    log: Log,
}

impl Ledger for TestLedger {
    fn state(&self, id: &str) -> Option<String> {
        self.states.lock().unwrap().get(id).cloned()
    }
    fn reconcile(&self, id: &str, observed: &str, evidence: Vec<String>) {
        self.states
            .lock()
            .unwrap()
            .insert(id.into(), observed.into());
        self.log
            .lock()
            .unwrap()
            .push(json!({"rec": [id, observed, evidence]}));
    }
}

struct Apps(Value);

impl BrowserApps for Apps {
    fn known(&self, bundle_id: &str) -> bool {
        self.0["known"][bundle_id]
            .as_bool()
            .unwrap_or_else(|| panic!("the fixture's known() of {bundle_id}"))
    }
    fn browser(&self, key: &str) -> Option<BrowserInfo> {
        let b = self.0["browsers"].get(key)?;
        Some(BrowserInfo {
            bundle_id: b["bundle_id"].as_str()?.into(),
            label: b["label"].as_str()?.into(),
        })
    }
}

/// The Python harness's `{"$repeat": c, "n": n}` back to the text.
fn expand(v: &Value) -> Value {
    match v {
        Value::Object(m) if m.contains_key("$repeat") => Value::String(
            m["$repeat"]
                .as_str()
                .unwrap()
                .repeat(m["n"].as_u64().unwrap() as usize),
        ),
        Value::Object(m) => Value::Object(m.iter().map(|(k, x)| (k.clone(), expand(x))).collect()),
        Value::Array(a) => Value::Array(a.iter().map(expand).collect()),
        other => other.clone(),
    }
}

struct Conn {
    to_daemon: Option<DuplexStream>,
    from_daemon: Lines<BufReader<DuplexStream>>,
}

struct Run {
    hub: Arc<Hub>,
    pairings: Arc<Pairings>,
    clock: Arc<FakeClock>,
    ledger: Arc<TestLedger>,
    log: Log,
    apps: Arc<Mutex<HashMap<i32, Option<String>>>>,
    paused: Arc<Mutex<bool>>,
    conns: Vec<Conn>,
    calls: HashMap<String, JoinHandle<Value>>,
    drivers: Option<(Arc<Driver>, Arc<Driver>)>,
    running: Arc<Mutex<HashSet<String>>>,
    keychain: Arc<Mutex<(Option<String>, i64)>>,
    /// The latest inbox inventory's tokens.
    tokens: Vec<String>,
    /// The driver calls (the harness records no step times for their errors).
    driver_calls: HashSet<String>,
    /// The real downloads, wired to the hub and both drivers (tools_more.json).
    downloads: Option<Arc<ferb_browser::downloads::Downloads>>,
    _dir: tempfile::TempDir,
}

const SECRET_KEY: &str = "ferb/site/password";

struct TestSecrets(Arc<Mutex<(Option<String>, i64)>>);

impl Secrets for TestSecrets {
    fn specs(&self) -> Vec<SecretSpec> {
        vec![]
    }
    fn has(&self, s: &SecretRef) -> bool {
        s.key == SECRET_KEY
    }
    fn spec(&self, s: &SecretRef) -> Option<SecretSpec> {
        Some(SecretSpec {
            secret: s.clone(),
            allowed_origins: vec!["https://a.example".into()],
            account_hint: "ferb@example.test".into(),
        })
    }
    fn reveal(&self, _: &SecretRef) -> BoxFuture<'_, Result<Secret, String>> {
        let mut k = self.0.lock().unwrap();
        k.1 += 1;
        let r = match &k.0 {
            Some(reason) => Err(reason.clone()),
            None => Ok(Secret::new("invented-Pa55word".into())),
        };
        Box::pin(async move { r })
    }
}

struct Asker(Log);

impl OwnerInput for Asker {
    fn ask_code(&self, _task: &str, _intent: &str, origin: &str) -> BoxFuture<'_, Option<Secret>> {
        self.0
            .lock()
            .unwrap()
            .push(json!({"asked": ["code", origin]}));
        Box::pin(async { Some(Secret::new("424242".into())) })
    }
    fn ask_secret(
        &self,
        _task: &str,
        _intent: &str,
        origin: &str,
        ask: SecretAsk,
    ) -> BoxFuture<'_, Option<Secret>> {
        self.0.lock().unwrap().push(json!({"asked": ["secret", origin, {"handle": ask.handle, "remember": ask.remember,
                                                                      "field": ask.field, "kind": ask.kind}]}));
        let declined = ask.handle.as_deref().is_some_and(|h| h.contains("decline"));
        Box::pin(async move { (!declined).then(|| Secret::new("owner-invented-pw".into())) })
    }
}

fn make_drivers(run: &mut Run, random: Arc<Counter>) {
    let running = run.running.clone();
    let tasks = Tasks {
        running: Arc::new(move |t| running.lock().unwrap().contains(t)),
        title: Arc::new(|_| "Book a table for four".into()),
        max_tabs: Arc::new(ops::max_tabs),
        redact: Arc::new(|t| t.replace("SECRET", "[redacted]")),
        ..Tasks::default()
    };
    let choose = Arc::new(|profile: Option<&str>| match profile {
        None | Some("Work") => Ok(("chrome".to_string(), "Profile 1".to_string())),
        Some("FERB") => Ok(("chrome".to_string(), "Profile 2".to_string())),
        Some(other) => Err(ferb_browser::Error::refused(format!(
            "no profile called {}",
            ferb_identity::py::repr(other)
        ))),
    });
    let router = Driver::new(
        run.hub.clone(),
        Kind::Owner {
            choose,
            input: Some(Arc::new(Asker(run.log.clone()))),
        },
        tasks.clone(),
        None,
        random.clone(),
    );
    let ferb = Driver::new(
        run.hub.clone(),
        Kind::Ferb {
            profile: Arc::new(|| Some(("chrome".into(), "Profile 2".into()))),
            secrets: Arc::new(TestSecrets(run.keychain.clone())),
        },
        tasks,
        None,
        random.clone(),
    );
    for d in [&router, &ferb] {
        let r = random.clone();
        d.set_sealer(Arc::new(move |key, value, cid, origin| {
            let nonce: [u8; 12] = r.bytes(12).try_into().unwrap();
            ferb_browser::seal::seal_with_nonce(
                &ferb_browser::seal::seal_key(key)[..],
                &nonce,
                value.expose(),
                cid,
                origin,
            )
        }));
    }
    run.drivers = Some((router, ferb));
}

fn shaped<T: Shape>(r: Result<T, ferb_browser::Error>) -> Value {
    match r {
        Ok(v) => json!({"ok": v.shape()}),
        Err(e) => outcome(Err(e)),
    }
}

trait Shape {
    fn shape(self) -> Value;
}

impl Shape for TabRef {
    fn shape(self) -> Value {
        json!({"tab_ref": {"device_id": self.device_id, "browser": self.browser, "profile": self.profile,
                           "generation": self.generation, "tab_id": self.tab_id}})
    }
}

impl Shape for ferb_core::types::PageSnapshot {
    fn shape(self) -> Value {
        json!({"snapshot": ferb_browser::snapshot::for_model(&self, true, false), "generation": self.generation})
    }
}

impl Shape for ferb_core::types::ActionResult {
    fn shape(self) -> Value {
        json!({"action": {"ok": self.ok, "detail": self.detail, "facts": self.facts}})
    }
}

impl Shape for Map<String, Value> {
    fn shape(self) -> Value {
        Value::Object(self)
    }
}

impl Shape for Vec<Map<String, Value>> {
    fn shape(self) -> Value {
        Value::Array(self.into_iter().map(Value::Object).collect())
    }
}

impl Shape for Value {
    fn shape(self) -> Value {
        self
    }
}

impl Shape for () {
    fn shape(self) -> Value {
        Value::Null
    }
}

impl Shape for String {
    fn shape(self) -> Value {
        Value::String(self)
    }
}

fn start_driver_call(run: &Run, v: &Value) -> JoinHandle<Value> {
    let (router, ferb) = run.drivers.clone().expect("drivers");
    let d = if v["driver"] == "ferb" { ferb } else { router };
    let a = v["args"].clone();
    let method = v["method"].as_str().unwrap().to_string();
    let tab_of = |d: &Driver, t: &Value| -> TabRef {
        let tab = d
            .tab(t["alias"].as_str().unwrap())
            .expect("a tab the driver knows")
            .tab;
        TabRef {
            generation: t
                .get("generation")
                .and_then(Value::as_i64)
                .unwrap_or(tab.generation),
            ..tab
        }
    };
    let tab = a.get("tab").map(|t| tab_of(&d, t));
    let el = a.get("el").map(|e| ElementRef {
        tab: tab_of(&d, e),
        frame_id: String::new(),
        snapshot_gen: e["snap"].as_i64().unwrap(),
        element_id: e["id"].as_str().unwrap().into(),
    });
    tokio::spawn(async move {
        let s = |k: &str| a.get(k).and_then(Value::as_str).map(String::from);
        let intent = s("intent").unwrap_or_default();
        match method.as_str() {
            "open_for" => shaped(
                d.open_for(
                    &s("url").unwrap(),
                    &s("owner").unwrap(),
                    s("profile").as_deref(),
                    None,
                    false,
                )
                .await,
            ),
            "snapshot" => shaped(d.snapshot(&tab.unwrap()).await),
            "read" => shaped(d.read(
                &tab.unwrap(),
                a["offset"].as_u64().unwrap() as usize,
                a["max_chars"].as_u64().unwrap() as usize,
            )),
            "navigate" => shaped(d.navigate(&tab.unwrap(), &s("url").unwrap(), None).await),
            "dialog" => shaped(d.dialog(&tab.unwrap(), None, None).await),
            "keep_open" => shaped(d.keep_open(&tab.unwrap()).await),
            "attach" => shaped(d.attach(&s("tab_ref").unwrap(), &s("owner").unwrap()).await),
            "owned_by" => shaped(d.owned_by(&s("tab_id").unwrap(), &s("owner").unwrap(), true)),
            "tab_chain" => shaped::<String>(Ok(d.tab_chain(&s("tab_id").unwrap()))),
            "task_tabs" => shaped(Ok(d.task_tabs(&s("owner").unwrap()))),
            "release" => shaped(d.release(&s("owner").unwrap()).await),
            "fill_secret" => shaped(
                d.fill_secret(
                    &el.unwrap(),
                    &SecretRef {
                        key: s("secret").unwrap(),
                    },
                    &intent,
                )
                .await,
            ),
            "owner_code" => shaped(d.owner_code(&el.unwrap(), "task-a", &intent).await),
            "owner_secret" => {
                let split: Option<Vec<String>> = a
                    .get("split_elements")
                    .map(|x| serde_json::from_value(x.clone()).unwrap());
                let ask = SecretAsk {
                    handle: s("handle"),
                    ..SecretAsk::default()
                };
                shaped(
                    d.owner_secret(&el.unwrap(), "task-a", &intent, ask, split.as_deref())
                        .await,
                )
            }
            _ => ops::call(&d, &method, &a, tab, el, &intent).await,
        }
    })
}

pub(crate) fn outcome(r: Result<Value, ferb_browser::Error>) -> Value {
    match r {
        Ok(v) => json!({"ok": v}),
        Err(e) => json!({"error": e.class(), "message": e.message(),
                         "facts": e.facts().map_or(Value::Null, |f| Value::Object(f.clone())),
                         "timing": e.timing.clone().unwrap_or(Value::Null)}),
    }
}

impl Run {
    fn new(f: &Value, with_drivers: bool) -> Run {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ferb.db");
        let db = ferb_store::Db::open(&path).unwrap();
        db.migrate().unwrap();
        db.close().unwrap();
        let start = ferb_core::when::parse(f["clock_start"].as_str().unwrap()).unwrap();
        let clock = Arc::new(FakeClock(Mutex::new((
            start,
            f["monotonic_start"].as_f64().unwrap(),
        ))));
        let random = Arc::new(Counter::new());
        let random_handle = random.clone();
        let store = Arc::new(SqliteInstalls::new(
            rusqlite::Connection::open(&path).unwrap(),
        ));
        let pairings = Arc::new(Pairings::new(store, clock.clone(), random.clone()));
        let log: Log = Arc::default();
        let ledger = Arc::new(TestLedger {
            states: Mutex::default(),
            log: log.clone(),
        });
        let apps: Arc<Mutex<HashMap<i32, Option<String>>>> = Arc::default();
        let a = apps.clone();
        let l = log.clone();
        let device = f["device_id"].as_str().unwrap().to_string();
        let hub = Hub::new(
            pairings.clone(),
            ledger.clone(),
            Arc::new(move |topic: &str, data: Map<String, Value>| {
                l.lock().unwrap().push(json!({"pub": [topic, data]}))
            }),
            Arc::new(move || device.clone()),
            random,
            Arc::new(Apps(f.clone())),
            Arc::new(move |pid| a.lock().unwrap().get(&pid.unwrap()).cloned().flatten()),
        );
        let paused: Arc<Mutex<bool>> = Arc::default();
        let (l1, l2, l3, l4, l5, p) = (
            log.clone(),
            log.clone(),
            log.clone(),
            log.clone(),
            log.clone(),
            paused.clone(),
        );
        hub.set_hooks(Hooks {
            on_unpaired: Arc::new(move || l1.lock().unwrap().push(json!({"hook": ["unpaired"]}))),
            on_tab: Arc::new(move |br: &Arc<Bridge>, e| {
                l2.lock()
                    .unwrap()
                    .push(json!({"hook": ["tab", br.name(), e.dump()]}))
            }),
            on_sound: Arc::new(move |br: &Arc<Bridge>, e| {
                l3.lock()
                    .unwrap()
                    .push(json!({"hook": ["sound", br.name(), e.dump()]}))
            }),
            on_tab_closed: Arc::new(move |br: &Arc<Bridge>, tab| {
                l4.lock()
                    .unwrap()
                    .push(json!({"hook": ["tab_closed", br.name(), tab]}));
                None
            }),
            record_refusal: Arc::new(move |facts| l5.lock().unwrap().push(json!({"ref": facts}))),
            paused: Arc::new(move || *p.lock().unwrap()),
            ..Hooks::default()
        });
        let random_for_drivers = random_handle.clone();
        let mut run = Run {
            hub,
            pairings,
            clock,
            ledger,
            log,
            apps,
            paused,
            conns: vec![],
            calls: HashMap::new(),
            drivers: None,
            running: Arc::new(Mutex::new(HashSet::from([
                "task-a".to_string(),
                "task-b".to_string(),
            ]))),
            keychain: Arc::default(),
            tokens: vec![],
            driver_calls: HashSet::new(),
            downloads: None,
            _dir: dir,
        };
        if let Some(more) = f.get("more").and_then(Value::as_str) {
            run.downloads = Some(real_downloads(&run, &path, more, random_handle.clone()));
            // the daemon's part of the tools, logging into this run's log
            ops::set_host(Some(Arc::new(MoreHost {
                log: run.log.clone(),
            })));
        }
        if with_drivers {
            make_drivers(&mut run, random_for_drivers);
            if f.get("uploads").is_some() {
                ops::seams(&run);
            }
            if let Some(dl) = &run.downloads {
                let (router, ferb) = run.drivers.clone().unwrap();
                for d in [router, ferb] {
                    let mut seams = d.seams_now();
                    seams.downloads = Some(dl.clone());
                    d.set_seams(seams);
                }
            }
        }
        run
    }

    fn state(&self, profile: &str) -> Value {
        let (b, f) = profile.split_once('/').unwrap();
        let Some(br) = self.hub.existing(b, f) else {
            return json!({});
        };
        let facts = br.facts();
        json!({"gen": facts.gen, "connected": facts.connected, "version": facts.version,
               "file_access": facts.file_access, "lost": facts.lost,
               "tabs": facts.tabs.iter().map(|(k, (h, a))| (k.to_string(), json!([h, a]))).collect::<Map<_, _>>(),
               "navs": facts.navs.iter().map(|(k, v)| (k.to_string(), json!(v))).collect::<Map<_, _>>(),
               "detached": facts.detached.iter().map(|(k, v)| (k.to_string(), json!(v))).collect::<Map<_, _>>(),
               "hellos": facts.hellos,
               "refused": self.hub.refusals().into_iter().map(|(k, v)| (k, Value::from(v))).collect::<Map<_, _>>(),
               "unpaired": self.hub.unpaired().into_iter().map(|(k, v)| (k, Value::from(v))).collect::<Map<_, _>>(),
               "unpaired_copies": self.hub.unpaired_copies()})
    }
}

/// downloads.Downloads as the daemon wires it (areas/browsers.py): the hub's `on_download`, the hub's
/// `task_tab`, its records in the run's store.
fn real_downloads(
    run: &Run,
    db: &std::path::Path,
    more: &str,
    random: Arc<Counter>,
) -> Arc<ferb_browser::downloads::Downloads> {
    use ferb_browser::downloads::{sqlite::SqliteRecords, DlHooks, Downloads, Peer};
    let log = run.log.clone();
    let dl = Downloads::new(
        std::path::PathBuf::from(more).join("downloads"),
        Arc::new(SqliteRecords::new(rusqlite::Connection::open(db).unwrap())),
        run.clock.clone(),
        Arc::new(|_: &str, _: &str| Ok(None)),
        Arc::new(|t: &str| t.replace("SECRET", "[redacted]")),
        Arc::new(move |topic: &str, data: Map<String, Value>| {
            log.lock().unwrap().push(json!({"pub": [topic, data]}))
        }),
        random,
    );
    let hub = Arc::downgrade(&run.hub);
    dl.set_hooks(DlHooks {
        task_tab: Arc::new(move |peer: &dyn Peer, tab: i64| {
            let hub = hub.upgrade()?;
            hub.task_tab(&peer.bridge()?, tab)
        }),
        task_root: Arc::new(|task: &str| task.split('.').next().unwrap().to_string()),
        free_bytes: Arc::new(|| Ok(1_000_000_000_000_000)),
        on_over_cap: Arc::new(|| {}),
    });
    let mut hooks = run.hub.hooks();
    hooks.on_download = Some(dl.on_download());
    run.hub.set_hooks(hooks);
    dl
}

/// The one deliberate difference from Python in these transcripts (fixed against Python): when a
/// connection ends, only the commands sent on it fail; Python also fails the ones the profile's newer
/// connection carries ("held-on-new"), and then ignores their results.
const FIXED: &[&str] = &["held-on-new"];

/// Fixed against Python: when an older connection of a profile ends while its newer one is up, Python
/// published `browser.disconnected`; the profile is still connected, so Rust doesn't.
const FIXED_DISCONNECTED: &[(&str, usize)] = &[("drops_and_reconnects", 24)];

/// Fixed against Python: a paired hello whose version Python's int() refuses ("0.19.²") ended the
/// connection (Python's ValueError); Rust refuses it as too old and keeps the connection.
async fn odd_version(run: &mut Run, at: &str, step: &Value) -> usize {
    let sent = step["log"][0]["e"].as_str().unwrap();
    run.conns[0]
        .to_daemon
        .as_mut()
        .unwrap()
        .write_all(format!("{sent}\n").as_bytes())
        .await
        .unwrap();
    let line = tokio::time::timeout(
        Duration::from_secs(3600),
        run.conns[0].from_daemon.next_line(),
    )
    .await
    .unwrap()
    .unwrap()
    .unwrap();
    let refused: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(refused["type"], "refused", "{at}");
    assert!(
        refused["reason"]
            .as_str()
            .unwrap()
            .contains("too old (version 0.19.²"),
        "{at}: {refused}"
    );
    for _ in 0..50 {
        tokio::task::yield_now().await;
    }
    let log = std::mem::take(&mut *run.log.lock().unwrap());
    assert_eq!(log.len(), 1, "{at}: {log:?}");
    assert_eq!(log[0]["ref"]["reason"], "extension_too_old", "{at}");
    assert!(
        run.state("chrome/Profile 1")["connected"] == true,
        "{at}: still connected"
    );
    3
}

async fn replay(f: &Value, scenario: &Value) -> usize {
    let name = scenario["name"].as_str().unwrap();
    let mut run = Run::new(f, name.ends_with("_tabs"));
    let mut compared = 0;
    let mut deferred: Vec<String> = vec![];
    for (n, step) in scenario["actions"].as_array().unwrap().iter().enumerate() {
        let action = step["action"].as_object().unwrap();
        let (kind, arg) = action.iter().next().unwrap();
        let at = format!("{name} action {n} {kind}");
        let mut expected_rets = vec![];
        let mut expected_log = vec![];
        if name == "odd_version_tabs" && (arg == "hello-odd-version" || kind == "state") {
            if kind == "ext" {
                compared += odd_version(&mut run, &at, step).await;
            }
            continue;
        }
        match kind.as_str() {
            "connect" => {
                let pid = 1000 + run.conns.len() as i32 + 1;
                run.apps
                    .lock()
                    .unwrap()
                    .insert(pid, arg["app"].as_str().map(String::from));
                let (ext_in, daemon_in) = tokio::io::duplex(8 << 20);
                let (daemon_out, ext_out) = tokio::io::duplex(8 << 20);
                tokio::spawn(run.hub.clone().serve(
                    BufReader::new(daemon_in),
                    daemon_out,
                    Some(pid),
                ));
                run.conns.push(Conn {
                    to_daemon: Some(ext_in),
                    from_daemon: BufReader::new(ext_out).lines(),
                });
            }
            "issue_token" => {
                let a = arg.as_array().unwrap();
                let who = if a[2] == "ferb" {
                    Identity::Ferb
                } else {
                    Identity::Owner
                };
                run.pairings
                    .issue_token(a[0].as_str().unwrap(), a[1].as_str().unwrap(), who);
            }
            "advance" => {
                let mut c = run.clock.0.lock().unwrap();
                c.0 += chrono::Duration::seconds(arg.as_i64().unwrap());
                c.1 += arg.as_f64().unwrap();
            }
            "close" => {
                run.conns[arg.as_u64().unwrap() as usize].to_daemon = None;
            }
            "ledger" => {
                for (k, v) in arg.as_object().unwrap() {
                    run.ledger
                        .states
                        .lock()
                        .unwrap()
                        .insert(k.clone(), v.as_str().unwrap().into());
                }
            }
            "paused" => *run.paused.lock().unwrap() = true,
            "keychain" => {
                run.keychain.lock().unwrap().0 = match arg.as_str().unwrap() {
                    "fails" => Some("the Keychain is locked".into()),
                    "fails-silently" => Some(String::new()),
                    _ => None,
                }
            }
            "running" => {
                *run.running.lock().unwrap() = serde_json::from_value(arg.clone()).unwrap();
            }
            "forget" => run.hub.forget_owner_tabs(),
            "keepalive" => tokio::time::advance(Duration::from_secs(15)).await,
            "tokens" => ops::tokens(&mut run),
            "budget" => ops::set_budget(arg.as_i64().unwrap()),
            "max_tabs" => ops::set_max_tabs(arg.as_i64().unwrap()),
            "staging" => ops::no_staging(&run),
            _ => {}
        }
        for entry in step["log"].as_array().unwrap() {
            let (k, v) = entry.as_object().unwrap().iter().next().unwrap();
            match k.as_str() {
                "start" if v.get("driver").is_some() => {
                    let mut v = v.clone();
                    ops::candidate(&run.tokens, &mut v);
                    run.driver_calls
                        .insert(v["name"].as_str().unwrap().to_string());
                    let handle = start_driver_call(&run, &v);
                    run.calls.insert(v["name"].as_str().unwrap().into(), handle);
                }
                "start" => {
                    let (b, fo) = v["profile"].as_str().unwrap().split_once('/').unwrap();
                    let br = run.hub.existing(b, fo).expect("the call's bridge");
                    let call = br.call(
                        v["op"].as_str().unwrap(),
                        expand(&v["args"]),
                        v["timeout"].as_f64().unwrap(),
                        v["cid"].as_str().map(String::from),
                    );
                    run.calls.insert(
                        v["name"].as_str().unwrap().into(),
                        tokio::spawn(async move { outcome(call.await) }),
                    );
                }
                "e" => {
                    if let Some(c) = entry["conn"].as_u64() {
                        let w = run.conns[c as usize]
                            .to_daemon
                            .as_mut()
                            .expect("an open connection");
                        w.write_all(format!("{}\n", v.as_str().unwrap()).as_bytes())
                            .await
                            .unwrap();
                    }
                }
                "d" => {
                    let c = entry["conn"].as_u64().unwrap() as usize;
                    let got = tokio::time::timeout(
                        Duration::from_secs(3600),
                        run.conns[c].from_daemon.next_line(),
                    )
                    .await
                    .unwrap_or_else(|_| panic!("{at}: no daemon line, expected {v}"))
                    .unwrap()
                    .unwrap_or_else(|| panic!("{at}: connection closed, expected {v}"));
                    assert_eq!(got, v.as_str().unwrap(), "{at}");
                    compared += 1;
                }
                "ret" => expected_rets.push(v.clone()),
                "pub"
                    if FIXED_DISCONNECTED.contains(&(name, n))
                        && entry["pub"][0] == "browser.disconnected" => {}
                "pub" | "ref" | "hook" | "rec" | "asked" | "seam" => {
                    expected_log.push(entry.clone())
                }
                "revealed" => {
                    assert_eq!(
                        json!(run.keychain.lock().unwrap().1),
                        *v,
                        "{at}: the Keychain was read only after the checks"
                    );
                    compared += 1;
                }
                "state" => {
                    let profile = arg.as_str().unwrap_or("chrome/Profile 1");
                    assert_eq!(run.state(profile), *v, "{at}");
                    if let Some(copies) = entry.get("copies") {
                        assert_eq!(json!(run.hub.unpaired_copies()), *copies, "{at}");
                    }
                    compared += 1;
                }
                "version" => {
                    let b = run
                        .pairings
                        .for_profile("chrome", "Profile 1")
                        .unwrap()
                        .unwrap();
                    assert_eq!(
                        json!(run.pairings.version(&b.install_id).unwrap()),
                        *v,
                        "{at}"
                    );
                    assert_eq!(json!(run.hub.problem(&b)), entry["problem"], "{at}");
                    compared += 2;
                }
                "assembled" => {}
                other => panic!("{at}: unknown log entry {other}"),
            }
        }
        for ret in expected_rets {
            let call = ret[0].as_str().unwrap();
            if FIXED.contains(&call) {
                // Python: OutcomeUnknown when the old connection closed. Rust: still in flight; its answer comes
                // with a later action, checked at the end.
                assert_eq!(ret[1]["error"], "OutcomeUnknown", "{at}");
                deferred.push(call.to_string());
                continue;
            }
            let got = run
                .calls
                .remove(call)
                .expect("a started call")
                .await
                .unwrap();
            if FIXED.contains(&call) {
                // Python: OutcomeUnknown when the old connection closed. Rust: still in flight, answered below.
                assert_eq!(ret[1]["error"], "OutcomeUnknown", "{at}");
                assert_eq!(
                    got,
                    json!({"ok": {"ok": true, "played": null}})
                        .as_object()
                        .map(|_| got.clone())
                        .unwrap()
                );
                assert_eq!(got["ok"]["ok"], true, "{at}: {got}");
                continue;
            }
            let got = if run.driver_calls.contains(call) {
                ops::normalize(&mut run, got, &ret[1])
            } else {
                got
            };
            assert_eq!(got, ret[1], "{at}: {call}");
            compared += 1;
        }
        // let spawned jobs and hooks finish, then compare what was published, refused, hooked and reconciled
        for _ in 0..50 {
            tokio::task::yield_now().await;
        }
        let mut got: Vec<Value> = std::mem::take(&mut *run.log.lock().unwrap());
        // Python iterates its `lost` set in hash order; Rust in sorted order: reconciliations compare as a set
        let order = |v: &mut Vec<Value>| {
            v.sort_by_key(|e| {
                if e.get("rec").is_some() {
                    e.to_string()
                } else {
                    String::new()
                }
            })
        };
        let mut want = expected_log;
        if want.iter().any(|e| e.get("rec").is_some()) {
            order(&mut got);
            order(&mut want);
        }
        assert_eq!(got, want, "{at}");
        compared += want.len();
    }
    for call in deferred {
        let got = run.calls.remove(&call).unwrap().await.unwrap();
        // answered by the extension on the newer connection (the fake has no tab 4: its KeyError's text), never
        // "unknown" because an older connection closed
        assert_eq!(
            got,
            json!({"error": "DriverError", "message": "4", "facts": {}, "timing": null}),
            "{name}: {call}"
        );
        compared += 1;
    }
    compared
}

// decisions: HAND-222, HAND-223, HAND-224, HAND-406, HAND-435, HAND-438, HAND-492, HAND-511
#[tokio::test(start_paused = true)]
async fn the_hub_replays_python_transcripts() {
    let f = fixture("transcripts.json");
    let mut total = 0;
    for scenario in f["scenarios"].as_array().unwrap() {
        let n = replay(&f, scenario).await;
        assert!(n > 5, "{}", scenario["name"]);
        total += n;
    }
    assert!(total > 200, "{total}");
}

// decisions: GATE-202, GATE-578, HAND-218, HAND-234, HAND-419, HAND-435, HAND-442, HAND-443, HAND-447, HAND-484,
// HAND-488, HAND-493, HAND-496, HAND-497, HAND-500, HAND-501, HAND-502, HAND-510, HAND-511, L13, L31, L33
/// The fixed folders the driver transcripts use (their frames name them): one test at a time.
static FIXED_FOLDERS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[tokio::test(start_paused = true)]
async fn the_drivers_page_operations_replay_python_transcripts() {
    let _one = FIXED_FOLDERS.lock().await;
    let f = fixture("ops.json");
    let _files = ops::upload_files(&f);
    ops::policy(&f);
    let mut total = 0;
    for scenario in f["scenarios"].as_array().unwrap() {
        let n = replay(&f, scenario).await;
        assert!(n > 5, "{}", scenario["name"]);
        total += n;
    }
    assert!(total > 500, "{total}");
}

// ---------- tools_more.json: the tool runs entry 28 left unverified ----------

/// The daemon's part of the tools, as tools_more.py's recorders answer: pairing and the handoff (their success paths
/// and a No), the display a rect is on, the recordings manager and hearing (keyed by the record call's output path,
/// as the Python script changed them between calls), and real files under the fixture's folder.
struct MoreHost {
    log: Log,
}

struct MoreRecordings {
    log: Log,
}

fn record_path() -> String {
    ops::TOOL_ARGS.lock().unwrap()["path"]
        .as_str()
        .unwrap_or("")
        .to_string()
}

impl ferb_browser::tools::Recordings for MoreRecordings {
    fn hearing(&self) -> bool {
        !record_path().ends_with("rec6.mp4")
    }
    fn excluded_host(&self, host: &str) -> bool {
        self.log
            .lock()
            .unwrap()
            .push(json!({"seam": ["excluded_host", host]}));
        record_path().ends_with("rec3.mp4") && host == "a.example"
    }
    fn resolve_audio(&self, target: &str) -> (String, Value) {
        self.log
            .lock()
            .unwrap()
            .push(json!({"seam": ["resolve_audio", target]}));
        let bundle = if record_path().ends_with("rec4.mp4") {
            "com.brave.Browser"
        } else {
            "com.google.Chrome"
        };
        (bundle.into(), json!({"target": target, "pid": 4242}))
    }
    fn start(
        &self,
        task: &str,
        target: Map<String, Value>,
        path: String,
        _capture: ferb_browser::capture::Recording,
        fps: f64,
        audio: Option<(String, Value)>,
        language: Option<String>,
        sources: Vec<(String, String)>,
        ctx: &ferb_core::types::ToolContext,
    ) -> BoxFuture<'_, ferb_browser::Result<Map<String, Value>>> {
        self.log.lock().unwrap().push(json!({"seam": ["record_start", task, target, path, true, fps,
            audio.map(|(b, v)| json!([b, v])), language,
            sources.iter().map(|(k, v)| json!([k, v])).collect::<Vec<_>>(), ctx.path_closed.is_some()]}));
        let out: Map<String, Value> = json!({"recording": "rec-1", "path": path, "fps": fps})
            .as_object()
            .unwrap()
            .clone();
        Box::pin(async move { Ok(out) })
    }
}

impl ferb_browser::tools::Host for MoreHost {
    fn pair(
        &self,
        browser: &str,
        folder: &str,
    ) -> Option<BoxFuture<'_, ferb_browser::Result<Map<String, Value>>>> {
        // the daemon's pair_profile runs (and is recorded) when the tool awaits it
        let (log, browser, folder) = (self.log.clone(), browser.to_string(), folder.to_string());
        Some(Box::pin(async move {
            log.lock()
                .unwrap()
                .push(json!({"seam": ["pair", browser, folder]}));
            let out = if folder == "Profile 8" {
                json!({"paired": false, "reason": "the owner said No"})
            } else {
                json!({"paired": true, "profile": format!("{browser}/{folder}"), "name": "Home"})
            };
            Ok(out.as_object().unwrap().clone())
        }))
    }
    fn handoff(
        &self,
        tab: TabRef,
        message: String,
        ctx: ferb_core::types::ToolContext,
    ) -> Option<BoxFuture<'_, ferb_browser::Result<Map<String, Value>>>> {
        let log = self.log.clone();
        Some(Box::pin(async move {
            log.lock().unwrap().push(json!({"seam": ["handoff",
                {"device_id": tab.device_id, "browser": tab.browser, "profile": tab.profile,
                 "generation": tab.generation, "tab_id": tab.tab_id}, message, ctx.task_id]}));
            let out = if message.contains("puzzle") {
                json!({"passed": false, "reason": "the page still shows it", "waited_s": 600})
            } else {
                json!({"passed": true, "waited_s": 12})
            };
            Ok(out.as_object().unwrap().clone())
        }))
    }
    fn resolve(
        &self,
        p: &str,
        _: &ferb_core::types::ToolContext,
        must_exist: bool,
    ) -> ferb_browser::Result<String> {
        if must_exist && !std::path::Path::new(p).exists() {
            // ferb/act/fs's PathRefused: not a FERB error, so it leaves the tool for the gateway
            return Err(ferb_browser::Error {
                subclass: Some("PathRefused"),
                ..ferb_core::errors::FerbError::Ferb(format!("{p} does not exist")).into()
            });
        }
        Ok(p.into())
    }
    fn owner_path(&self, p: &str, workspace: &str) -> String {
        if p.starts_with('/') {
            p.into()
        } else {
            format!("{workspace}/{p}")
        }
    }
    fn read(&self, p: &str) -> ferb_browser::Result<Vec<u8>> {
        std::fs::read(p).map_err(|e| ferb_browser::Error::refused(e.to_string()))
    }
    fn new_file(
        &self,
        p: &str,
        data: &[u8],
        _: &ferb_core::types::ToolContext,
    ) -> ferb_browser::Result<()> {
        use std::io::Write;
        let mut f = std::fs::File::options()
            .write(true)
            .create_new(true)
            .open(p)
            .map_err(|e| ferb_browser::Error::refused(e.to_string()))?;
        f.write_all(data)
            .map_err(|e| ferb_browser::Error::refused(e.to_string()))
    }
    fn display_of(&self, rect: ferb_browser::mask::Rect) -> Option<i64> {
        self.log
            .lock()
            .unwrap()
            .push(json!({"seam": ["display_of", [rect.0, rect.1, rect.2, rect.3]]}));
        if rect.0 < 300.0 {
            Some(1)
        } else if rect.0 < 1000.0 {
            Some(2)
        } else {
            None
        }
    }
    fn recordings(&self) -> Option<Arc<dyn ferb_browser::tools::Recordings>> {
        Some(Arc::new(MoreRecordings {
            log: self.log.clone(),
        }))
    }
}

// decisions: HAND-207, HAND-427, HAND-428, HAND-511
#[tokio::test(start_paused = true)]
async fn the_remaining_tool_runs_replay_python_transcripts() {
    let _one = FIXED_FOLDERS.lock().await;
    let f = fixture("tools_more.json");
    let more = f["more"].as_str().unwrap();
    let _ = std::fs::remove_dir_all(more);
    std::fs::create_dir_all(more).unwrap();
    std::fs::write(format!("{more}/resume.html"), f["html"].as_str().unwrap()).unwrap();
    std::fs::write(
        format!("{more}/big.html"),
        vec![b'x'; ferb_browser::tools::PDF_HTML_MAX + 1],
    )
    .unwrap();
    let _files = ops::upload_files(&f);
    ops::policy(&f);
    ferb_browser::tools::PAGE_PORT.store(
        f["page_port"].as_u64().unwrap() as u16,
        std::sync::atomic::Ordering::Relaxed,
    );
    let scenario = &f["scenarios"][0];
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            ops::set_host(None);
        }
    }
    let _reset = Reset;
    let n = replay(&f, scenario).await;
    assert!(n > 90, "{n}");
    // the PDFs written, byte for byte, and the downloaded file
    for (name, data) in f["written"].as_object().unwrap() {
        assert_eq!(
            ferb_identity::py::b64encode(&std::fs::read(format!("{more}/{name}")).unwrap()),
            data.as_str().unwrap(),
            "{name}"
        );
    }
    let mut got: Vec<String> = std::fs::read_dir(format!("{more}/downloads"))
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    got.sort();
    assert_eq!(json!(got), f["downloads"]);
    std::fs::remove_dir_all(more).unwrap();
}

/// The one-time local page (`_serve_once`): each raw request and the exact bytes Python answered.
#[tokio::test]
async fn the_one_time_page_answers_as_python_did() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let f = fixture("tools_more.json");
    let s = &f["serve_once"];
    let (url, server) = ferb_browser::tools::serve_once(
        s["html"].as_str().unwrap().as_bytes().to_vec(),
        &Counter::new(),
    )
    .await
    .unwrap();
    let (host_port, path) = url
        .strip_prefix("http://")
        .unwrap()
        .split_once('/')
        .unwrap();
    assert_eq!(format!("/{path}"), s["path"].as_str().unwrap());
    for case in s["cases"].as_array().unwrap() {
        let mut c = tokio::net::TcpStream::connect(host_port).await.unwrap();
        let req: Vec<u8> = case["request"]
            .as_str()
            .unwrap()
            .chars()
            .map(|ch| ch as u32 as u8)
            .collect();
        c.write_all(&req).await.unwrap();
        c.shutdown().await.unwrap();
        let mut got = vec![];
        c.read_to_end(&mut got).await.unwrap();
        let got: String = got.iter().map(|b| char::from(*b)).collect();
        assert_eq!(
            got,
            case["response"].as_str().unwrap(),
            "{:?}",
            case["request"]
        );
    }
    server.abort();
}
