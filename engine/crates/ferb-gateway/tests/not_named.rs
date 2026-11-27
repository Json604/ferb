//! ORCH-676 (the owner's D4, ORCH-669) through the real `Gateway` on a temp store: at a send or a submit on a page,
//! when the request named a file on the Mac and what code sees going out isn't that file (FERB's uploads carried other
//! bytes, the page's field holds other bytes, or a file field holds a file no upload of the task could have put there),
//! FERB asks once before pressing; a No presses nothing, a Yes presses and carries across a retry. Where code can't see
//! the file, or no file was named, nothing is asked.
#![cfg(all(feature = "store", target_os = "macos"))]

#[allow(dead_code)]
mod support;

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use ferb_core::interfaces::{BoxFuture, Clock, IdSource};
use ferb_core::types::{ActionFacts, ActionSpec, Target, ToolContext, ToolResult};
use ferb_core::when::{self, DateTime};
use ferb_gateway::gateway::{
    Cancel, Deps, FsRoots, Gateway, StoreIds, StoreTime, SyncProtection, TaskRecord, POSIX_FILES,
};
use ferb_gateway::seams::{Describe, Usage};
use ferb_gateway::tool::{Args, Registry, Tool, ToolError, ToolOutput};
use ferb_identity::owner_secrets::Places;
use ferb_policy::confirm::{Apps, AppsUnread, RunningApp};
use ferb_policy::small::{Request, Targets};
use ferb_rpc::auth::{Identity, RunGrant};
use ferb_sanitize::Redactor;
use ferb_store::Db;
use serde_json::{json, Map, Value};

use support::ledger::RealLedger;

const T0: &str = "2026-01-01T09:00:00+00:00";

/// The daemon's clock, moved by a test (seconds after T0).
#[derive(Default)]
struct FakeClock(Mutex<f64>);
impl Clock for FakeClock {
    fn now(&self) -> DateTime {
        when::parse(T0).unwrap()
            + chrono::Duration::milliseconds((*self.0.lock().unwrap() * 1000.0) as i64)
    }
    fn monotonic(&self) -> f64 {
        1000.0
    }
    fn sleep_until(&self, _t: DateTime) -> BoxFuture<'_, ()> {
        Box::pin(async {})
    }
}

struct Seq(Mutex<u64>);
impl IdSource for Seq {
    fn new_id(&self) -> String {
        let mut n = self.0.lock().unwrap();
        *n += 1;
        format!("id-{:06}", *n)
    }
}

struct NoApps;
impl Apps for NoApps {
    fn candidates(&self, _name: &str) -> Result<Vec<RunningApp>, AppsUnread> {
        Ok(Vec::new())
    }
    fn installed_bundle(&self, _name: &str) -> Result<Option<String>, AppsUnread> {
        Ok(None)
    }
    fn is_browser_app(&self, _app: &str) -> bool {
        false
    }
}

struct Words;
impl Describe for Words {
    fn describe(
        &self,
        _tool: &str,
        _args: &Map<String, Value>,
        _kind: &str,
        _site: Option<&str>,
        _app: Option<&str>,
    ) -> String {
        "working".into()
    }
    fn app_name(&self, _app: &str) -> String {
        "an app".into()
    }
}

struct NoUsage;
impl Usage for NoUsage {
    fn count<'a>(
        &'a self,
        _path: &'a str,
        _closed: ferb_core::types::PathCheck,
    ) -> BoxFuture<'a, Map<String, Value>> {
        Box::pin(async { Map::new() })
    }
}

struct NoPlaces;
impl Places for NoPlaces {
    fn is_browser(&self, _bundle_id: &str) -> bool {
        false
    }
    fn url_host(&self, _url: &str) -> Option<String> {
        None
    }
}

const SITE: &str = "forms.example";

/// A page's Submit button: `browser.act` as the catalog has it, its facts set by the test (the page's own form
/// submit, the files the page's file fields held at the last read); it counts its presses.
#[derive(Default)]
struct Submit {
    presses: std::sync::atomic::AtomicUsize,
    page_files: Mutex<Option<i64>>,
}

impl Tool for Submit {
    fn name(&self) -> &str {
        "browser.act"
    }
    fn describe(&self, _args: &Args) -> ActionSpec {
        ActionSpec {
            target: Target {
                site: Some(SITE.into()),
                page: Some(format!("https://{SITE}/apply")),
                ..Target::default()
            },
            facts: Some(ActionFacts {
                submits_form: Some(true),
                page_files: *self.page_files.lock().unwrap(),
                page_url: Some(format!("https://{SITE}/apply")),
                ..ActionFacts::default()
            }),
            ..ActionSpec::new("click")
        }
    }
    fn run<'a>(
        &'a self,
        _args: &'a Args,
        _ctx: &'a ToolContext,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            self.presses
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let mut content = Map::new();
            content.insert("ok".into(), json!(true));
            Ok(ToolOutput::from(ToolResult {
                content,
                ..ToolResult::default()
            }))
        })
    }
}

struct Run {
    submit: Arc<Submit>,
    gw: Arc<Gateway>,
    grant: RunGrant,
    db: Arc<Mutex<Db>>,
    user: std::path::PathBuf,
    _dir: tempfile::TempDir,
}

/// The owner's attachment line as the chat's intake writes it (ferb-supervisor intake.rs).
fn attached(paths: &[&str]) -> String {
    format!(
        "Upload my report\n\n{}<untrusted-data id=\"a1\">{}</untrusted-data id=\"a1\">",
        ferb_policy::named::ATTACHED_MARK,
        json!({"input_paths": paths})
    )
}

fn task(words: &str, payload: impl Fn(&std::path::Path) -> String) -> Run {
    let dir = tempfile::Builder::new().prefix("fnn").tempdir().unwrap();
    let home = dir.path().join("ferb");
    let user = dir.path().join("user");
    std::fs::create_dir_all(home.join("workspace")).unwrap();
    std::fs::create_dir_all(user.join("Desktop")).unwrap();
    std::fs::create_dir_all(user.join("Downloads")).unwrap();
    std::fs::write(user.join("Desktop/report.pdf"), b"the desktop copy").unwrap();
    std::fs::write(user.join("Downloads/report.pdf"), b"another report").unwrap();
    let db = Db::open(home.join("ferb.db")).unwrap();
    db.migrate().unwrap();
    let db = Arc::new(Mutex::new(db));
    let fake = Arc::new(FakeClock::default());
    let clock: Arc<dyn Clock> = fake.clone();
    let ids: Arc<dyn IdSource> = Arc::new(Seq(Mutex::new(0)));
    let redactor = Arc::new(Redactor::new(&[], &[]));
    let never_read = json!(ferb_policy::protection::default_never_read());
    let never_write = json!(ferb_policy::protection::DEFAULT_NEVER_WRITE);
    let terminals = json!(ferb_policy::protection::DEFAULT_TERMINAL_APPS);
    let ports = json!([]);
    let lists = ferb_policy::protection::Lists {
        never_read: &never_read,
        never_write: &never_write,
        terminal_apps: &terminals,
        local_ports: &ports,
    };
    let protection = ferb_policy::protection::Protection::new(
        user.to_str().unwrap(),
        home.to_str().unwrap(),
        &lists,
        None,
        &POSIX_FILES,
    )
    .unwrap();
    let mut reg = Registry::new();
    let submit = Arc::new(Submit::default());
    reg.add(submit.clone()).unwrap();
    let deps = Deps {
        db: db.clone(),
        clock: clock.clone(),
        ids: ids.clone(),
        redactor: redactor.clone(),
        ledger: Arc::new(RealLedger {
            db: db.clone(),
            clock: clock.clone(),
            ids: ids.clone(),
        }),
        publish: Box::new(|_t, _d| {}),
        roots: FsRoots {
            workspace: home.join("workspace").to_str().unwrap().to_string(),
            owner_roots: vec![],
            untrusted_roots: vec![],
            home: user.to_str().unwrap().to_string(),
            protection: Arc::new(SyncProtection(protection)),
        },
        device_id: "dev".into(),
        recovery: Arc::new(ferb_gateway::adapters::fs::FsRecovery {
            db: db.clone(),
            clock: clock.clone(),
            ids: ids.clone(),
            root: home.join("recovery").to_str().unwrap().to_string(),
        }),
        snapshots: None,
        usage: Arc::new(NoUsage),
        apps: Arc::new(NoApps),
        describe: Arc::new(Words),
        journal_problem: Box::new(|_k, _d| {}),
        profile_name: Box::new(|_b, f| f.to_string()),
        profile_account: Box::new(|_b, _f| String::new()),
        profile_turned_off: Box::new(|_b, _f| false),
        owner_contacts: Box::new(Vec::new),
        pay_same_site_forms: Box::new(|| true),
        on_path: Box::new(|_p, _t| {}),
        proc_task: Box::new(|_p| None),
        route_question: None,
        hooks: Arc::new(ferb_gateway::seams::NoHooks),
        made_in_tree: None,
        excluded_host: None,
        owner_secrets: None,
        places: Arc::new(NoPlaces),
        username_holds: None,
    };
    let counter = Mutex::new(0u64);
    let gw = Arc::new(Gateway::new(reg, deps, move || {
        let mut n = counter.lock().unwrap();
        *n += 1;
        format!("ap-{n}")
    }));
    gw.record_request(Request {
        request_id: "req-1".into(),
        words: words.into(),
        targets: Targets::default(),
        payload: payload(&user),
        asked_origins: BTreeSet::new(),
    });
    let now = clock.now();
    let span_id = {
        let d = db.lock().unwrap();
        let spans = ferb_store::spans::SpanStore::new(
            &d,
            Box::new(StoreTime(clock.clone())),
            Box::new(StoreIds(ids.clone())),
            Box::new(|v: &Value| v.clone()),
            Box::new(|_k: &str| false),
        );
        let id = spans
            .start("task", &json!({}), Some("task-1"), None, None)
            .unwrap()
            .id;
        let at = when::isoformat(&now);
        d.conn()
            .execute(
                "INSERT INTO tasks (id, tree_id, parent_id, request_id, goal, identity, role, scopes_json, status, \
                 created_at, updated_at) VALUES ('task-1', 'task-1', NULL, 'req-1', 'g', 'owner', 'worker', ?, \
                 'running', ?, ?)",
                rusqlite::params!["[]", at, at],
            )
            .unwrap();
        id
    };
    gw.add_task(TaskRecord {
        task_id: "task-1".into(),
        request_id: "req-1".into(),
        started_at: now,
        deadline: Some(now + chrono::Duration::minutes(10)),
        span_id,
        tree_id: None,
    });
    let grant = RunGrant {
        task_id: "task-1".into(),
        worker_id: "w1".into(),
        scopes: submit.scopes(),
        identity: Identity::Owner,
        expires_at: 1e9,
        observe_only: false,
        tools: BTreeSet::new(),
        max_calls: 0,
    };
    Run {
        submit,
        gw,
        grant,
        db,
        user,
        _dir: dir,
    }
}

fn sha(bytes: &[u8]) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(bytes))
}

impl Run {
    fn desktop(&self) -> String {
        self.user
            .join("Desktop/report.pdf")
            .to_string_lossy()
            .into_owned()
    }
    fn downloads(&self) -> String {
        self.user
            .join("Downloads/report.pdf")
            .to_string_lossy()
            .into_owned()
    }

    /// One of the task's earlier calls, as recorded (FERB's upload receipt, a Mac input).
    fn recorded(&self, n: usize, input: Value, output: Value) {
        self.db
            .lock()
            .unwrap()
            .conn()
            .execute(
                "INSERT INTO spans (id, task_id, kind, started_at, ended_at, input_json, output_json) VALUES \
                 (?, 'task-1', 'tool.call', '2026-01-01T08:59:00+00:00', '2026-01-01T08:59:01+00:00', ?, ?)",
                rusqlite::params![format!("earlier-{n}"), input.to_string(), output.to_string()],
            )
            .unwrap();
    }

    fn uploaded(&self, n: usize, path: &str, bytes: &[u8], bytes_match: bool) {
        self.recorded(
            n,
            json!({"tool": "browser.act", "kind": "fill_form", "target": {"site": SITE}}),
            json!({"ok": bytes_match, "source": "local", "bytes_match": bytes_match,
                   "files": [{"path": path, "size": bytes.len(), "sha256": sha(bytes)}]}),
        );
    }

    /// The page's Submit pressed; each question answered `yes`; (the reply, the questions' (question, detail)).
    async fn press(&self, yes: bool) -> (Value, Vec<(String, String)>) {
        let args = json!({"tab": "t1", "element": "e1", "action": "click"});
        let fut = self.gw.call_as(
            &self.grant,
            "browser.act",
            args.as_object().unwrap(),
            Cancel::new(),
            Arc::new(|| false),
        );
        tokio::pin!(fut);
        let mut asked = Vec::new();
        let reply = loop {
            tokio::select! {
                biased;
                r = &mut fut => break r,
                () = tokio::task::yield_now() => {
                    if let Some(ap) = self.gw.pending().into_iter().next() {
                        asked.push((ap.summary.clone(), ap.detail.clone()));
                        self.gw.resolve(&ap.id, yes, Some(if yes { 1 } else { 2 }), None).unwrap();
                    }
                }
            }
        };
        (reply.expect("the call returns").to_json(), asked)
    }

    fn presses(&self) -> usize {
        self.submit
            .presses
            .load(std::sync::atomic::Ordering::SeqCst)
    }
}

fn the_question(asked: &[(String, String)]) -> bool {
    asked
        .iter()
        .any(|(q, _)| q.contains("isn't report.pdf from your Mac"))
}

/// FERB uploaded another file (same name, another folder) and the owner attached the Desktop one: asked before the
/// press; a No presses nothing and its refusal says so; asked again, the same No stands without a new question.
// decision: ORCH-676, ORCH-669
#[tokio::test]
async fn another_file_uploaded_is_asked_and_a_no_presses_nothing() {
    let r = task("Upload my report and submit the form", |u| {
        attached(&[&u.join("Desktop/report.pdf").to_string_lossy()])
    });
    let other = r.downloads();
    r.uploaded(1, &other, b"another report", true);
    let (reply, asked) = r.press(false).await;
    assert!(the_question(&asked), "{asked:?}");
    assert!(
        asked[0]
            .1
            .contains("FERB attached report.pdf there, not report.pdf"),
        "{asked:?}"
    );
    assert_eq!(r.presses(), 0, "nothing pressed: {reply}");
    let (_, again) = r.press(false).await;
    assert!(
        again.is_empty(),
        "the No carries across the retry: {again:?}"
    );
    assert_eq!(r.presses(), 0);
    let key: String =
        r.db.lock()
            .unwrap()
            .conn()
            .query_row(
                "SELECT key FROM confirmations WHERE rule = 'floor:not_named' AND granted = 0",
                [],
                |row| row.get(0),
            )
            .unwrap();
    assert_eq!(
        ferb_policy::named::declined_line(&key).as_deref(),
        Some(
            "I didn't send it: the file attached on forms.example wasn't report.pdf from your Mac."
        )
    );
}

/// The page's field holds bytes other than the named file FERB gave it (bytes_match false): asked; a Yes presses
/// once, and the Yes carries across a retry (pressed again, not asked again).
// decision: ORCH-676, ORCH-669
#[tokio::test]
async fn other_bytes_in_the_field_are_asked_and_a_yes_carries() {
    let r = task("Upload my report and submit the form", |u| {
        attached(&[&u.join("Desktop/report.pdf").to_string_lossy()])
    });
    r.uploaded(1, &r.desktop(), b"the desktop copy", false);
    let (_, asked) = r.press(true).await;
    assert!(the_question(&asked), "{asked:?}");
    assert!(asked[0].1.contains("holds different bytes"), "{asked:?}");
    assert_eq!(r.presses(), 1);
    let (_, again) = r.press(true).await;
    assert!(again.is_empty(), "{again:?}");
    assert_eq!(r.presses(), 2);
}

/// The owner's Drive example where code can see it: a file field on the page holds a file, and nothing in the task
/// could have put the named file there (no upload of it, no Mac input, script or shell command): asked.
// decision: ORCH-676, ORCH-669
#[tokio::test]
async fn a_field_filled_from_elsewhere_is_asked() {
    let r = task(
        "Upload ~/Desktop/report.pdf to the form and submit it",
        |_| String::new(),
    );
    *r.submit.page_files.lock().unwrap() = Some(1);
    let (_, asked) = r.press(false).await;
    assert!(the_question(&asked), "{asked:?}");
    assert!(
        asked[0].1.contains("wasn't uploaded from your Mac by FERB"),
        "{asked:?}"
    );
    assert_eq!(r.presses(), 0);
}

/// No false alarms: the right bytes (or the named path, whatever its bytes are now: ORCH-670), a file field code
/// can't see, a Mac input that could have carried the file, or no file named in the request: nothing asked.
// decision: ORCH-676, ORCH-670
#[tokio::test]
async fn nothing_is_asked_when_it_is_the_named_file_or_code_cannot_see() {
    // the right bytes, even uploaded from another path
    let r = task("Upload my report", |u| {
        attached(&[&u.join("Desktop/report.pdf").to_string_lossy()])
    });
    let copy = r
        .user
        .join("Downloads/copy.pdf")
        .to_string_lossy()
        .into_owned();
    r.uploaded(1, &copy, b"the desktop copy", true);
    *r.submit.page_files.lock().unwrap() = Some(1);
    let (_, asked) = r.press(false).await;
    assert!(asked.is_empty(), "{asked:?}");
    assert_eq!(r.presses(), 1);
    // the named path, edited on disk since its upload
    let r = task("Upload my report", |u| {
        attached(&[&u.join("Desktop/report.pdf").to_string_lossy()])
    });
    r.uploaded(1, &r.desktop(), b"before the edit", true);
    let (_, asked) = r.press(false).await;
    assert!(asked.is_empty(), "{asked:?}");
    // no file field code can read (a site's own picker): nothing to compare
    let r = task("Upload my report", |u| {
        attached(&[&u.join("Desktop/report.pdf").to_string_lossy()])
    });
    let (_, asked) = r.press(false).await;
    assert!(asked.is_empty(), "{asked:?}");
    // a filled field, but the file could have come through the Mac tools (a drag, a file dialog)
    let r = task("Upload my report", |u| {
        attached(&[&u.join("Desktop/report.pdf").to_string_lossy()])
    });
    r.recorded(
        1,
        json!({"tool": "mac.act", "kind": "mac_input", "target": {"site": "com.google.Chrome"}}),
        json!({"ok": true}),
    );
    *r.submit.page_files.lock().unwrap() = Some(1);
    let (_, asked) = r.press(false).await;
    assert!(asked.is_empty(), "{asked:?}");
    // no file named in the request (only a name in words is never resolved by code)
    let r = task("Upload my report.pdf and submit", |_| String::new());
    *r.submit.page_files.lock().unwrap() = Some(1);
    let (_, asked) = r.press(false).await;
    assert!(asked.is_empty(), "{asked:?}");
    assert_eq!(r.presses(), 1);
}
