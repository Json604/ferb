//! ENG-3: group B's smaller areas (about, sensing, jev, notify, intro, egress) answer the Python daemon's recorded
//! lines byte for byte: `engine/fixtures/daemon/rpc_transcripts.json` (the base recordings) and
//! `engine/fixtures/daemon-b/small.json` (`small.py`: real paths), journal rows and published events included.
//!
//! Seam values another part owns come from the recording itself and are named where they are fed: perception's
//! status, the store's size, the records area's permissions in force and saved logins.

mod b_support;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use b_support::*;
use ferb_daemon::areas_b::{self, about, banners, egress, intro, jev, notify, sensing};
use ferb_identity::owner_secrets::{Keychain, OwnerSecrets, Places};
use ferb_identity::Secret;
use ferb_rpc::BoxFuture;
use ferb_sense::manager::{Emit, Hooks, Sense, Stop, Watcher};
use serde_json::{json, Map, Value};

const MINE: &[&str] = &[
    "about.get",
    "about.guide",
    "secrets.saved",
    "secrets.delete",
    "seen.list",
    "seen.image",
    "sense.pause",
    "sense.resume",
    "sense.status",
    "jev.stats",
    "jev.enable",
    "jev.reload",
    "notifications.subscribe",
    "notifications.categories",
    "attention.ack",
    "notices.list",
    "intro.suggest",
    "intro.preview",
    "ps.list",
    "ps.kill",
    "ps.attach",
    "ps.send",
];

/// The secret helper the recorder had: never found (make.py STUBS `_helper`).
struct NoHelper;

impl Keychain for NoHelper {
    fn store(&self, _: &str, _: &Secret) -> ferb_identity::Result<()> {
        Err(ferb_identity::Error::Refused(
            "the signed secret helper is unavailable; owner setup is required".into(),
        ))
    }
    fn lookup(&self, _: &str) -> ferb_identity::Result<Option<Secret>> {
        self.store("", &Secret::new(String::new()))?;
        Ok(None)
    }
    fn delete(&self, _: &str) -> ferb_identity::Result<bool> {
        self.store("", &Secret::new(String::new()))?;
        Ok(false)
    }
}

struct NoPlaces;

impl Places for NoPlaces {
    fn is_browser(&self, _: &str) -> bool {
        false
    }
    fn url_host(&self, _: &str) -> Option<String> {
        None
    }
}

/// The perception watcher the recorder's daemon ran (its source is "jev_batches"): runs until stopped.
struct Batches;

impl Watcher for Batches {
    fn source(&self) -> &str {
        "jev_batches"
    }
    fn run(self: Arc<Self>, _emit: Emit, stop: Stop) -> BoxFuture<'static, Result<(), String>> {
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

struct JevOff;

impl jev::JevSeams for JevOff {
    fn key(&self) -> BoxFuture<'static, Option<String>> {
        panic!("Jev is off: nothing may read its key")
    }
    fn build(&self, _key: String) -> Arc<dyn jev::Decider> {
        panic!("Jev is off: no decider is built")
    }
    fn wire(&self, jev: Option<Arc<dyn jev::Decider>>) {
        assert!(jev.is_none());
    }
}

fn store(home: &Home) -> notify::SharedDb {
    let db = ferb_store::Db::open(home.path().join("ferb.db")).unwrap();
    db.migrate().unwrap();
    Arc::new(Mutex::new(db))
}

/// What the recorder's stand-ins answered (make.py `GRANTS`).
fn grants() -> Map<String, Value> {
    json!({"accessibility": "granted", "screen_recording": "granted", "input_monitoring": "granted",
           "full_disk_access": "denied"})
    .as_object()
    .unwrap()
    .clone()
}

struct Seams {
    perception: Value,
    store_facts: (u64, bool),
    in_force: Value,
    saved_logins: Value,
    scan: Value,
    answers: Vec<String>,
    sense: Option<Arc<Sense>>,
}

impl Default for Seams {
    fn default() -> Self {
        Seams {
            perception: json!({}),
            store_facts: (0, false),
            in_force: json!({}),
            saved_logins: json!({}),
            scan: json!({"state": "not_started"}),
            answers: vec![],
            sense: None,
        }
    }
}

/// The recorded `ctx` each intro suggestion call was asked with.
type Asked = Arc<Mutex<Vec<String>>>;

fn deps(home: &Home, s: Seams, db: &notify::SharedDb, asked: &Asked) -> areas_b::Deps {
    let h = &home.host;
    let mut d = areas_b::Deps::new(h.clone());
    let perception = s.perception.clone();
    d.sensing = Some(sensing::Deps {
        sense: s.sense.clone(),
        owner_input_running: Arc::new(|| false),
        watch_problems: Arc::new(Mutex::new(vec![])),
        perception_status: Some(Arc::new(move || Ok(perception.clone()))),
    });
    d.jev = Some(jev::Deps {
        seams: Arc::new(JevOff),
        jev: Arc::new(Mutex::new(None)),
    });
    d.notify = Some(notify::Deps {
        posters: banners::Posters::default(),
        notices: Some(db.clone()),
    });
    d.egress = Some(egress::Deps {
        shells: None,
        poll: Duration::from_millis(100),
    });
    let (in_force, saved, facts) = (s.in_force.clone(), s.saved_logins.clone(), s.store_facts);
    d.about = Some(Arc::new(about::Deps {
        version: "0.27.3".into(),
        guide: Arc::new(Mutex::new(None)),
        grants: Arc::new(grants),
        containment: Arc::new(|| {
            json!({"profile_ok": true, "shell": true, "proxy": true})
                .as_object()
                .unwrap()
                .clone()
        }),
        account: Arc::new(NoAccount),
        store: Some(db.clone()),
        store_facts: Arc::new(move || facts),
        permissions_in_force: Arc::new(move || Ok(in_force.clone())),
        saved_logins: Arc::new(move || Ok(saved.clone())),
        owner_secrets: Arc::new(OwnerSecrets::with_keychain(
            home.path(),
            Arc::new(NoHelper),
            Arc::new(NoPlaces),
        )),
        evidence: Some(Arc::new(about::Evidence {
            root: home.path().join("evidence"),
        })),
        tree_of: Arc::new(|_| None),
        app_location: Arc::new(String::new),
    }));
    let scan = s.scan.clone();
    let answers = Arc::new(Mutex::new(s.answers));
    let asked = asked.clone();
    d.intro = Some(Arc::new(intro::Deps {
        scan: Arc::new(move || scan.clone()),
        ask: Arc::new(move |_system, ctx| {
            asked.lock().unwrap().push(ctx);
            let next = answers.lock().unwrap().remove(0);
            Box::pin(async move { Ok(next) })
        }),
        system: Arc::new(|| Ok("intro instructions".into())),
        limit: Duration::from_secs(55),
        shutting_down: Arc::new(|| false),
        preview: None,
    }));
    d
}

fn keep(method: &str) -> bool {
    MINE.contains(&method)
}

/// ENG-6: the internal errors (-32603) Python gave bad parameters; Rust refuses in the area's own words.
fn fixes(request: &str, python: &str) -> Option<String> {
    let v: Value = serde_json::from_str(request).ok()?;
    let id = v["id"].clone();
    // decision: APP-927: the document carries why FERB has each permission after `permissions`
    if v["method"] == "about.get" {
        let mut line: Value = serde_json::from_str(python).ok()?;
        let doc = line.get_mut("result")?.as_object_mut()?;
        doc.contains_key("permissions").then_some(())?;
        about::add_reasons(doc, "");
        // decision: PHN-650: FERB's phone app is explained at the end of `app`
        about::add_phone_app(doc);
        // decision: APP-941: Jev's days are no setting any more, so the settings FERB lists leave them out
        if let Some(Value::Array(settings)) = doc.get_mut("settings") {
            settings.retain(|r| r["key"] != "retention.decision_days");
            // decision: APP-981: insomnia mode's three numbers are the owner's settings (T-1274), after Python's last
            let place = settings
                .last()
                .and_then(|r| r.get("where").cloned())
                .unwrap_or(Value::Null);
            // decision: MEM-447: the owner's answer about the coding assistants on this Mac, off until asked
            settings.push(json!({"key": "memory.local_import", "value": false, "where": place}));
            // decision: MEM-459: the owner\'s switch for ChatGPT and Claude, off until he chooses a browser
            settings.push(json!({"key": "memory.sites_import", "value": false, "where": place}));
            // decision: HAND-666: the owner's answer about his coding sessions, off until asked
            settings.push(json!({"key": "sessions.enabled", "value": false, "where": place}));
            for (key, value) in [
                ("insomnia.grace_minutes", 10),
                ("insomnia.battery_warn_percent", 20),
                ("insomnia.battery_urgent_percent", 10),
            ] {
                settings.push(json!({"key": key, "value": value, "where": place}));
            }
        }
        return Some(ferb_py::json::dumps(&line));
    }
    if !python.contains("-32603") {
        return None;
    }
    let words = match (v["method"].as_str()?, &v["params"]) {
        ("attention.ack", p) if p.get("ids") == Some(&json!(["no-such-notice"])) => {
            "No such owner notice"
        }
        ("attention.ack", _) => "ids must be a nonempty list of notice ids",
        ("jev.enable", _) => "name the question to switch on or off",
        ("seen.list", _) => "limit must be a number of screenshots",
        _ => return None,
    };
    Some(ferb_py::json::dumps(&json!({"jsonrpc": "2.0", "id": id,
                                       "error": {"code": -32004, "message": words}})))
}

/// The journal rows Python wrote, without its internal-error rows (the named fixes answer those as refusals).
fn python_rows(s: &Value) -> Vec<Value> {
    s["journal"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r[0] != "daemon.internal_error")
        .cloned()
        .collect()
}

async fn sense_started(home: &Home) -> Arc<Sense> {
    let h = home.host.clone();
    let hooks = Hooks {
        on_pause: Arc::new(move |paused| sensing::on_pause(&h, paused, &|| {}).unwrap()),
        ..Hooks::default()
    };
    let sense = Arc::new(Sense::new(
        home.path().to_path_buf(),
        vec![Arc::new(Batches)],
        Arc::new(|_| {}),
        None,
        None,
        hooks,
    ));
    sense.start().await;
    tokio::task::yield_now().await;
    sense
}

// decision: DMN-6
// decision: DMN-22
// decision: DMN-27
// decision: DMN-545
#[tokio::test(flavor = "multi_thread")]
async fn base_recordings_of_settings_setup_about() {
    let base = fixture("daemon/rpc_transcripts.json");
    let s = scenario(&base, "settings_setup_about");
    let home = Home::new("");
    let db = store(&home);
    let perception = {
        // the perception status the recorder's sense.status carried (perception is another part: a seam value)
        let line = s["steps"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|st| st["lines"].as_array().cloned().unwrap_or_default())
            .filter_map(|l| l.get("recv").map(line_text))
            .find(|l| l.contains("\"perception\""))
            .unwrap();
        serde_json::from_str::<Value>(&line).unwrap()["result"]["perception"].clone()
    };
    let sense = sense_started(&home).await;
    let asked = Asked::default();
    let deps = deps(
        &home,
        Seams {
            perception,
            sense: Some(sense),
            ..Seams::default()
        },
        &db,
        &asked,
    );
    let d = Daemon::start(|server| areas_b::register(server, &deps)).await;
    // about.get here follows settings.set calls this replay doesn't make: it is replayed in `small_about`
    let out = replay(
        &d,
        s["steps"].as_array().unwrap(),
        &|m| keep(m) && m != "about.get",
        &[],
        &fixes,
    )
    .await;
    out.assert_clean();
    eprintln!("same {:?} named {:?}", out.same, out.named);
    assert_eq!(out.named.get("attention.ack"), Some(&2));
}

/// FERB's shell with no process, as the recorder's daemon had it.
struct NoLists {
    home: std::path::PathBuf,
    ferb: std::path::PathBuf,
    workspace: std::path::PathBuf,
    uploads: std::path::PathBuf,
}

impl ferb_shell::protection::Protection for NoLists {
    fn home(&self) -> &std::path::Path {
        &self.home
    }
    fn ferb_home(&self) -> &std::path::Path {
        &self.ferb
    }
    fn workspace(&self) -> &std::path::Path {
        &self.workspace
    }
    fn uploads(&self) -> &std::path::Path {
        &self.uploads
    }
    fn reads(&self) -> &[ferb_shell::protection::Entry] {
        &[]
    }
    fn writes(&self) -> &[ferb_shell::protection::Entry] {
        &[]
    }
    fn app_entries(&self) -> Vec<std::path::PathBuf> {
        vec![]
    }
    fn service_files(&self) -> Vec<std::path::PathBuf> {
        vec![]
    }
    fn guarded_parents(&self) -> Vec<std::path::PathBuf> {
        vec![]
    }
    fn forms_now(&self, path: &std::path::Path) -> Vec<std::path::PathBuf> {
        vec![path.to_path_buf()]
    }
    fn app_data(&self, _path: &std::path::Path) -> bool {
        false
    }
    fn local_ports(&self) -> &[ferb_shell::protection::LocalPort] {
        &[]
    }
    fn rules(&self) -> Vec<Value> {
        vec![]
    }
}

fn shells(home: &Home) -> Arc<ferb_shell::manager::ShellManager> {
    let p = home.path();
    let lists = Arc::new(NoLists {
        home: p.parent().unwrap().join("user"),
        ferb: p.to_path_buf(),
        workspace: p.join("workspace"),
        uploads: p.join("workspace/uploads"),
    });
    let sb = Arc::new(
        ferb_shell::sandbox::Sandbox::new(lists, p.join("profiles"), Arc::new(|| None), None)
            .with_scan(Arc::new(|| Some(Default::default()))),
    );
    let r = home.host.redactor.clone();
    let cfg = ferb_shell::manager::ManagerConfig::new(
        p.join("logs"),
        Arc::new(|t: &str| t.to_string()),
        Arc::new(move || ferb_sanitize::stream::RedactionStream::new(r.clone())),
        sb,
    );
    ferb_shell::manager::ShellManager::new(cfg)
}

/// The supervisor's preview events as the recorder's stream carried them (the supervisor is another part).
fn recorded_preview(s: &Value) -> Vec<Map<String, Value>> {
    let steps = s["steps"].as_array().unwrap();
    let at = steps
        .iter()
        .position(|st| {
            st["do"] == "stream" && line_text(&st["lines"][0]["send"]).contains("find my three")
        })
        .unwrap();
    let end = steps[at..]
        .iter()
        .find(|st| st["do"] == "stream_end" && st["conn"] == steps[at]["conn"])
        .unwrap();
    end["lines"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|l| l.get("recv").map(line_text))
        .filter_map(|l| serde_json::from_str::<Value>(&l).ok())
        .filter_map(|v| v.get("params").and_then(Value::as_object).cloned())
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn base_recordings_of_streams_and_processes() {
    let base = fixture("daemon/rpc_transcripts.json");
    let s = scenario(&base, "streams_and_processes");
    let home = Home::new("");
    let db = store(&home);
    let asked = Asked::default();
    let mut deps = deps(&home, Seams::default(), &db, &asked);
    deps.egress = Some(egress::Deps {
        shells: Some(shells(&home)),
        poll: Duration::from_millis(100),
    });
    let events = recorded_preview(s);
    let old = deps.intro.take().unwrap();
    let preview: intro::PreviewStart = Arc::new(move |session: &str, request: &str| {
        assert_eq!(
            (session, request),
            (intro::INTRO_SESSION, "find my three largest files")
        );
        let (tx, rx) = tokio::sync::mpsc::channel(events.len() + 1);
        for e in &events {
            tx.try_send(Ok(e.clone())).unwrap();
        }
        rx
    });
    deps.intro = Some(Arc::new(intro::Deps {
        scan: old.scan.clone(),
        ask: old.ask.clone(),
        system: old.system.clone(),
        limit: old.limit,
        shutting_down: old.shutting_down.clone(),
        preview: Some(preview),
    }));
    let d = Daemon::start(|server| areas_b::register(server, &deps)).await;
    let out = replay(&d, s["steps"].as_array().unwrap(), &keep, &[], &fixes).await;
    out.assert_clean();
    for m in [
        "ps.list",
        "ps.kill",
        "ps.send",
        "ps.attach",
        "intro.preview",
    ] {
        assert!(out.same.contains_key(m), "{m} not compared: {:?}", out.same);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn small_about_with_stored_screenshots() {
    let file = fixture("daemon-b/small.json");
    let s = scenario(&file, "small_about");
    let setup = &s["setup"];
    let home = Home::new(&line_text(&setup["config"]));
    let apps: Vec<String> = home
        .host
        .config
        .exclude_apps()
        .unwrap()
        .into_iter()
        .collect();
    let apps: Vec<&str> = apps.iter().map(String::as_str).collect();
    home.host.redactor.configure(&["excluded.example"], &apps);
    let db = store(&home);
    {
        let db = db.lock().unwrap();
        for r in setup["evidence"].as_array().unwrap() {
            let data = match &r[5] {
                Value::String(s) => s.clone(),
                other => serde_json::to_string(other).unwrap(),
            };
            let data = if r[5].is_string() {
                data
            } else {
                py_dumps(&r[5])
            };
            db.conn().execute(
                "INSERT INTO evidence (id, task_id, span_id, kind, mime, path, data_json, origin, captured_at) \
                 VALUES (?, ?, NULL, ?, ?, ?, ?, ?, ?)",
                rusqlite::params![r[0].as_str(), r[1].as_str(), r[2].as_str(), r[3].as_str(), r[4].as_str(),
                                  data, r[6].as_str(), r[7].as_str()],
            ).unwrap();
        }
        for t in setup["tasks"].as_array().unwrap() {
            db.conn().execute(
                "INSERT INTO tasks (id, tree_id, parent_id, request_id, goal, identity, role, scopes_json, status, \
                 created_at, updated_at) VALUES (?, ?, NULL, ?, 'invented', 'owner', 'mixed', '[]', 'done', ?, ?)",
                rusqlite::params![t[0].as_str(), t[1].as_str(), t[2].as_str(), "2026-01-01T08:00:00+00:00",
                                  "2026-01-01T08:00:00+00:00"],
            ).unwrap();
        }
    }
    use base64::Engine;
    for (rel, b64) in setup["files"].as_object().unwrap() {
        let p = home.path().join("evidence").join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(
            &p,
            base64::engine::general_purpose::STANDARD
                .decode(b64.as_str().unwrap())
                .unwrap(),
        )
        .unwrap();
    }
    // seam values from the recording: the store's size, the records area's two sections
    let about_line = s["steps"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|st| st["lines"].as_array().cloned().unwrap_or_default())
        .filter_map(|l| l.get("recv").map(line_text))
        .find(|l| l.contains("\"saved_logins\""))
        .unwrap();
    let about: Value = serde_json::from_str(&about_line).unwrap();
    let r = &about["result"];
    let seams = Seams {
        store_facts: (r["health"]["database_bytes"].as_u64().unwrap(), false),
        in_force: r["permissions_in_force"].clone(),
        saved_logins: r["saved_logins"].clone(),
        ..Seams::default()
    };
    let asked = Asked::default();
    let deps = deps(&home, seams, &db, &asked);
    let d = Daemon::start(|server| areas_b::register(server, &deps)).await;
    let out = replay(&d, s["steps"].as_array().unwrap(), &keep, &[], &fixes).await;
    out.assert_clean();
    assert_eq!(home.journal_rows(), python_rows(s));
    assert_eq!(out.named.get("seen.list"), Some(&1));
}

fn py_dumps(v: &Value) -> String {
    ferb_py::json::dumps(v)
}

// decision: DMN-23
#[tokio::test(flavor = "multi_thread")]
async fn small_notices_and_banner_switches() {
    let file = fixture("daemon-b/small.json");
    let s = scenario(&file, "small_notices");
    let home = Home::new("");
    let db = store(&home);
    let asked = Asked::default();
    let deps = deps(&home, Seams::default(), &db, &asked);
    // the notices the recorder posted, through the same path (`_tell_owner`, `_tell_refused`), 5 s apart
    let notifier = notify::Notifier {
        host: home.host.clone(),
        posters: banners::Posters::default(),
        desk: Arc::new(Desk),
        listeners: Arc::new(Quiet),
        notices: Some(db.clone()),
        held: Mutex::new(vec![]),
        uid: Arc::new(|| "0001".into()),
    };
    notifier
        .tell_owner(
            "loop-s1-0001",
            &["task:t1".into()],
            "Paused the invented loop.",
            "ferb",
            "loop",
            "t1",
            &[],
            &Value::Null,
            None,
        )
        .await
        .unwrap();
    advance(&home, 5);
    let refused = json!({"what": "password", "place": "example.test", "words": "Try again", "span_id": "sp-1",
                         "task_id": "t2"});
    notifier
        .tell_refused(refused.as_object().unwrap())
        .await
        .unwrap();
    advance(&home, 5);
    notifier
        .tell_owner(
            "plain-1",
            &[],
            "An invented note.",
            "ferb",
            "",
            "",
            &[json!({"type": "reminder",
                    "title": "Invented"})],
            &Value::Null,
            None,
        )
        .await
        .unwrap();
    let d = Daemon::start(|server| areas_b::register(server, &deps)).await;
    let alert_fixes = |request: &str, python: &str| {
        fixes(request, python).or_else(|| {
            let line: Value = serde_json::from_str(python).ok()?;
            python
                .contains(REFUSED)
                .then(|| ferb_py::json::dumps(&as_alert(&line)))
        })
    };
    let out = replay(&d, s["steps"].as_array().unwrap(), &keep, &[], &alert_fixes).await;
    out.assert_clean();
    let rows: Vec<Value> = python_rows(s).iter().map(as_alert).collect();
    assert_eq!(home.journal_rows(), rows);
    let published: Vec<Value> = s["published"]
        .as_array()
        .unwrap()
        .iter()
        .map(as_alert)
        .collect();
    assert_eq!(home.events(), published);
    assert_eq!(out.named.get("attention.ack"), Some(&3));
    assert_eq!(out.named.get("notices.list"), Some(&3));
}

/// The refused value's notice (`_tell_refused`, an `alert` notice) as Python recorded its id.
const REFUSED: &str = "refused-sp-1";

/// decision: APP-1106 — the one named difference from Python's recording here: an `alert` notice's id is
/// `alert:<Python's id>`, and every view of it says `"alert": true` (after its `at`), so no client keeps it in the chat.
fn as_alert(v: &Value) -> Value {
    match v {
        Value::Array(items) => Value::Array(items.iter().map(as_alert).collect()),
        Value::Object(m) if m.get("id") == Some(&json!(REFUSED)) => {
            let mut out = serde_json::Map::new();
            for (k, val) in m {
                if k == "id" {
                    out.insert(k.clone(), json!(format!("alert:{REFUSED}")));
                } else {
                    out.insert(k.clone(), val.clone());
                }
                if k == "at" {
                    out.insert("alert".into(), json!(true));
                }
            }
            Value::Object(out)
        }
        Value::Object(m) => Value::Object(
            m.iter()
                .map(|(k, val)| (k.clone(), as_alert(val)))
                .collect(),
        ),
        other => other.clone(),
    }
}

fn advance(home: &Home, seconds: i64) {
    let mut now = home.clock.now.lock().unwrap();
    *now += chrono::Duration::seconds(seconds);
}

struct Desk;

impl notify::Desk for Desk {
    fn osascript(&self, _script: &str) -> BoxFuture<'static, Result<(), String>> {
        Box::pin(async { Ok(()) })
    }
    fn away(&self) -> bool {
        false
    }
}

/// A connected client shows every notice (the recorder had none, and its daemon had notifications off).
struct Quiet;

impl notify::Listeners for Quiet {
    fn client_shows(&self, _topic: &str) -> bool {
        true
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn small_jev_switches_while_off() {
    let file = fixture("daemon-b/small.json");
    let s = scenario(&file, "small_jev");
    let home = Home::new("");
    let db = store(&home);
    let asked = Asked::default();
    let deps = deps(&home, Seams::default(), &db, &asked);
    let d = Daemon::start(|server| areas_b::register(server, &deps)).await;
    let out = replay(&d, s["steps"].as_array().unwrap(), &keep, &[], &fixes).await;
    out.assert_clean();
    assert_eq!(home.journal_rows(), python_rows(s));
    assert_eq!(out.named.get("jev.enable"), Some(&1));
}

// decision: DMN-500
// decision: DMN-542
#[tokio::test(flavor = "multi_thread")]
async fn small_intro_suggestions_from_a_seen_scan() {
    let file = fixture("daemon-b/small.json");
    let s = scenario(&file, "small_intro");
    let home = Home::new("");
    let db = store(&home);
    let asked = Asked::default();
    let seams = Seams {
        scan: s["setup"]["scan"].clone(),
        answers: s["setup"]["answers"]
            .as_array()
            .unwrap()
            .iter()
            .map(py_dumps)
            .collect(),
        ..Seams::default()
    };
    let deps = deps(&home, seams, &db, &asked);
    let d = Daemon::start(|server| areas_b::register(server, &deps)).await;
    let out = replay(&d, s["steps"].as_array().unwrap(), &keep, &[], &fixes).await;
    out.assert_clean();
    assert_eq!(home.journal_rows(), python_rows(s));
    // what the model was asked: the redacted scan, as the recorder's model call received it
    let calls = s["model_calls"].as_array().unwrap();
    let asked = asked.lock().unwrap();
    assert_eq!(asked.len(), calls.len());
    for (ctx, call) in asked.iter().zip(calls) {
        let parts: Vec<String> = call["parts"]
            .as_array()
            .unwrap()
            .iter()
            .map(line_text)
            .collect();
        assert!(
            parts.iter().any(|p| p.contains(ctx.as_str())),
            "{ctx}\nnot in {parts:?}"
        );
    }
}

/// What `small_tell` asked of the requests table: none of its tasks is recorded (`tasks.column` → None).
struct NoClient;

impl notify::Listeners for NoClient {
    fn client_shows(&self, _topic: &str) -> bool {
        false
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn small_telling_with_ferb_app_as_poster() {
    let file = fixture("daemon-b/small.json");
    let s = scenario(&file, "small_tell");
    let home = Home::new("");
    let db = store(&home);
    let posters = banners::Posters::default();
    let mut poster = posters.open();
    let notifier = notify::Notifier {
        host: home.host.clone(),
        posters: posters.clone(),
        desk: Arc::new(Desk),
        listeners: Arc::new(NoClient),
        notices: Some(db.clone()),
        held: Mutex::new(vec![]),
        uid: Arc::new(|| "000000000000".into()),
    };
    for t in s["setup"]["told"].as_array().unwrap() {
        notifier
            .task_result(&t[0], t[1] == true, t[2] == true, t[3] == true)
            .await
            .unwrap();
    }
    let mut asked = vec![];
    for q in s["setup"]["questions"].as_array().unwrap() {
        asked.extend(notifier.approval_notice(q));
    }
    notifier
        .tell_owner(
            "loop-s9-0009",
            &["task:t9".into()],
            "Backing off the invented loop.",
            "ferb",
            "loop",
            "t9",
            &[],
            &Value::Null,
            None,
        )
        .await
        .unwrap();
    // the question banners are spawned in Python (`_spawn`): they reach the poster after the notice
    for (banner, urgent) in asked {
        notifier.notify(banner, urgent).await;
    }
    let mut delivered = vec![];
    while let Ok(item) = poster.queue.try_recv() {
        delivered.push(Value::Object(item));
    }
    assert_eq!(delivered, s["delivered"].as_array().unwrap().clone());
    assert_eq!(home.events(), s["published"].as_array().unwrap().clone());
    assert_eq!(home.journal_rows(), python_rows(s));
    let rows: Vec<Value> = {
        let db = db.lock().unwrap();
        let mut stmt = db.conn().prepare(
            "SELECT r.task_id, r.at, r.status, r.goal, r.answer, r.shown, n.name, c.cards_json FROM task_results r \
             LEFT JOIN task_result_names n USING (task_id) LEFT JOIN task_result_cards c USING (task_id) \
             ORDER BY r.task_id").unwrap();
        let rows = stmt
            .query_map([], |r| {
                Ok(json!([
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, i64>(5)?,
                    r.get::<_, Option<String>>(6)?,
                    r.get::<_, Option<String>>(7)?
                ]))
            })
            .unwrap();
        rows.map(Result::unwrap).collect()
    };
    assert_eq!(rows, s["task_results"].as_array().unwrap().clone());
}

/// events.py: each event is redacted once, then handed to its consumers in the order they were added, a copy each,
/// only for the topics each asked for.
#[test]
fn events_are_redacted_once_and_handed_out_in_order() {
    use ferb_daemon::areas_b::events::Events;
    let calls = Arc::new(Mutex::new(0));
    let c = calls.clone();
    let events = Events::new(move || {
        let c = c.clone();
        Arc::new(move |v: &Value| {
            *c.lock().unwrap() += 1;
            let mut v = v.clone();
            v["redacted"] = json!(true);
            v
        })
    });
    let seen = Arc::new(Mutex::new(vec![]));
    for (name, topics) in [
        ("all", None),
        ("tasks", Some(vec!["task.result"])),
        ("later", None),
    ] {
        let seen = seen.clone();
        let topics: Option<Vec<&str>> = topics;
        events.on(
            topics.as_deref(),
            Arc::new(move |topic: &str, mut data: Map<String, Value>| {
                data.insert("by".into(), json!(name));
                seen.lock().unwrap().push(json!([name, topic, data]));
            }),
        );
    }
    events.publish("task.result", json!({"x": 1}).as_object().unwrap());
    events.publish("sense.paused", &Map::new());
    assert_eq!(*calls.lock().unwrap(), 2);
    let seen = seen.lock().unwrap();
    let names: Vec<&Value> = seen.iter().map(|s| &s[0]).collect();
    assert_eq!(
        names,
        [
            &json!("all"),
            &json!("tasks"),
            &json!("later"),
            &json!("all"),
            &json!("later")
        ]
    );
    // one consumer's added key never reaches the next
    assert_eq!(seen[1][2], json!({"x": 1, "redacted": true, "by": "tasks"}));
}

/// notify.py `_notify`: while the Mac is out of use a non-urgent banner waits; an urgent one goes now; a category
/// the owner switched off posts nothing; `_flush_held` posts what waited once the Mac is back.
#[tokio::test]
async fn banners_wait_while_the_mac_is_away() {
    struct Away(Mutex<bool>);
    impl notify::Desk for Away {
        fn osascript(&self, _script: &str) -> BoxFuture<'static, Result<(), String>> {
            Box::pin(async { Ok(()) })
        }
        fn away(&self) -> bool {
            *self.0.lock().unwrap()
        }
    }
    let home = Home::new("[banners]\nloop = false\n");
    let posters = banners::Posters::default();
    let mut poster = posters.open();
    let desk = Arc::new(Away(Mutex::new(true)));
    let n = notify::Notifier {
        host: home.host.clone(),
        posters,
        desk: desk.clone(),
        listeners: Arc::new(NoClient),
        notices: None,
        held: Mutex::new(vec![]),
        uid: Arc::new(|| "u".into()),
    };
    let b = |title: &str, category: &str| banners::Banner {
        title: title.into(),
        category: category.into(),
        ..banners::Banner::default()
    };
    n.notify(b("later", "reminder"), false).await;
    n.notify(b("now", "question"), true).await;
    n.notify(b("never", "loop"), true).await;
    n.flush_held().await; // still away
    let titles = |p: &mut banners::Poster| {
        let mut out = vec![];
        while let Ok(i) = p.queue.try_recv() {
            out.push(i["title"].as_str().unwrap().to_owned());
        }
        out
    };
    assert_eq!(titles(&mut poster), ["now"]);
    *desk.0.lock().unwrap() = false;
    n.flush_held().await;
    assert_eq!(titles(&mut poster), ["later"]);
}

/// GATE-602 (owner GATE-601: "always"): the owner's "don't mask this" for one saved identifier, through FERB.app's
/// own calls. secrets.unmask makes the redactor show that value (and only it); secrets.saved lists it as shown;
/// secrets.mask and a delete mask it again; a password can't be shown.
// decision: GATE-602
// decision: GATE-601
#[tokio::test(flavor = "multi_thread")]
async fn the_owner_shows_one_identifier_and_takes_it_back() {
    #[derive(Default)]
    struct Memory(Mutex<Vec<(String, String)>>);
    impl Keychain for Memory {
        fn store(&self, account: &str, value: &Secret) -> ferb_identity::Result<()> {
            let mut k = self.0.lock().unwrap();
            k.retain(|(a, _)| a != account);
            k.push((account.into(), value.expose().into()));
            Ok(())
        }
        fn lookup(&self, account: &str) -> ferb_identity::Result<Option<Secret>> {
            let k = self.0.lock().unwrap();
            Ok(k.iter()
                .find(|(a, _)| a == account)
                .map(|(_, v)| Secret::new(v.clone())))
        }
        fn delete(&self, account: &str) -> ferb_identity::Result<bool> {
            let mut k = self.0.lock().unwrap();
            let before = k.len();
            k.retain(|(a, _)| a != account);
            Ok(k.len() < before)
        }
    }
    struct Hosts;
    impl Places for Hosts {
        fn is_browser(&self, _: &str) -> bool {
            false
        }
        fn url_host(&self, url: &str) -> Option<String> {
            url.split_once("://")
                .map(|(_, rest)| rest.split('/').next().unwrap_or("").to_owned())
        }
    }
    let home = Home::new("");
    let db = store(&home);
    let asked = Asked::default();
    let mut deps = deps(&home, Seams::default(), &db, &asked);
    let secrets = Arc::new(OwnerSecrets::with_keychain(
        home.path(),
        Arc::new(Memory::default()),
        Arc::new(Hosts),
    ));
    secrets
        .remember_identifier("PAN", &Secret::from("ABCPE1234F"))
        .unwrap();
    secrets
        .remember(
            "password",
            "https://bank.example",
            &Secret::from("invented-pw"),
            &[],
            None,
        )
        .unwrap();
    let old = deps.about.take().unwrap();
    deps.about = Some(Arc::new(about::Deps {
        version: old.version.clone(),
        guide: old.guide.clone(),
        grants: old.grants.clone(),
        containment: old.containment.clone(),
        account: old.account.clone(),
        store: old.store.clone(),
        store_facts: old.store_facts.clone(),
        permissions_in_force: old.permissions_in_force.clone(),
        saved_logins: old.saved_logins.clone(),
        owner_secrets: secrets.clone(),
        evidence: None,
        tree_of: old.tree_of.clone(),
        app_location: old.app_location.clone(),
    }));
    let d = Daemon::start(|server| areas_b::register(server, &deps)).await;
    let mut c = d.connect().await;
    c.send(r#"{"jsonrpc": "2.0", "id": 0, "method": "auth.hello", "params": {"role": "cli"}}"#)
        .await;
    c.recv().await.unwrap();
    let mut n = 0;
    let mut call = async |method: &str, params: Value| -> Value {
        n += 1;
        let line = json!({"jsonrpc": "2.0", "id": n, "method": method, "params": params});
        c.send(&line.to_string()).await;
        serde_json::from_str(&c.recv().await.unwrap()).unwrap()
    };
    let text = "PAN ABCPE1234F, and ABCPE1234G is my brother's";
    let r = &home.host.redactor;
    assert!(!r.text(text).contains("ABCPE1234F"));
    let shown = call("secrets.unmask", json!({"name": "PAN"})).await;
    assert_eq!(
        shown["result"],
        json!({"name": "PAN", "shown": true}),
        "{shown}"
    );
    assert_eq!(
        r.text(text),
        "PAN ABCPE1234F, and [redacted] is my brother's"
    );
    let saved = call("secrets.saved", json!({})).await;
    assert!(saved["result"]["entries"]
        .as_array()
        .unwrap()
        .contains(&json!({"place": "any site", "name": "PAN", "kind": "identifier", "unusable": false, "shown": true})));
    // never a password, never a name that isn't saved
    for name in ["password", "nothing"] {
        let refused = call("secrets.unmask", json!({"name": name})).await;
        assert!(refused.get("error").is_some(), "{refused}");
    }
    let masked = call("secrets.mask", json!({"name": "PAN"})).await;
    assert_eq!(masked["result"], json!({"name": "PAN", "shown": false}));
    assert!(!r.text(text).contains("ABCPE1234F"));
    call("secrets.unmask", json!({"name": "PAN"})).await;
    assert!(r.text(text).contains("ABCPE1234F"));
    call(
        "secrets.delete",
        json!({"place": "any site", "name": "PAN"}),
    )
    .await;
    assert!(
        !r.text(text).contains("ABCPE1234F"),
        "a deleted identifier is masked again"
    );
    let kinds: Vec<String> = home
        .journal_rows()
        .iter()
        .filter_map(|row| row[0].as_str().map(str::to_owned))
        .collect();
    assert!(
        kinds.contains(&"secret.unmasked".to_string())
            && kinds.contains(&"secret.masked".to_string()),
        "{kinds:?}"
    );
}

/// PHN-653 (owner, 2026-11-17): a paused or interrupted upload from a phone is kept 7 days (PHN-582's number,
/// confirmed); and the numbers the phone app's guide states (PHN-650) are the ones FERB enforces.
// decision: PHN-653
// decision: PHN-650
#[test]
fn a_paused_phone_upload_is_kept_seven_days_as_the_guide_says() {
    assert_eq!(
        ferb_daemon::phone::files::UPLOAD_PARTIAL_KEEP_S,
        7.0 * 86_400.0
    );
    let guide = about::phone_app().to_string();
    assert!(guide.contains("kept 7 days"), "{guide}");
}
