//! The driver's page operations in the transcript replay (rust/fixtures/browser/ops.json, made by
//! driver_ops.py): the daemon's seams as the Python harness recorded them, the calls by name, and
//! what differs by nature between the two runs (the owner's uid, the inbox's random tokens).

use std::path::Path;
use std::sync::Arc;

use ferb_browser::driver::Driver;
use ferb_browser::mask::{ImageBox, Images, Rect};
use ferb_browser::ops::{DownloadsView, Native, Seams, Staging};
use ferb_browser::owner::{Inbox, ProfilesView};
use ferb_browser::profiles::BrowserProfile;
use ferb_core::interfaces::BoxFuture;
use ferb_core::types::{
    BrowserAction, ElementRef, PathCheck, TabRef, TerminalGrant, TerminalInput, WaitCondition,
};
use ferb_identity::secrets::SecretRef;
use serde_json::{json, Map, Value};

use super::{outcome, Log, Run};

const UPLOADS: &str = "/private/tmp/ferb-browser-ops-fixture";

struct Rec(Log);

impl Rec {
    fn log(&self, v: Value) {
        self.0.lock().unwrap().push(json!({ "seam": v }));
    }
}

impl Images for Rec {
    fn size(&self, image: &[u8]) -> Result<(i64, i64), String> {
        if image.starts_with(b"bad") {
            return Err("not an image".into());
        }
        Ok((2560, 1800))
    }
    fn mask(&self, image: &[u8], boxes: &[ImageBox]) -> Result<Vec<u8>, String> {
        let b: Vec<Value> = boxes.iter().map(|b| json!([b.0, b.1, b.2, b.3])).collect();
        self.log(json!(["mask", String::from_utf8_lossy(image), b]));
        Ok([b"painted:".as_slice(), image].concat())
    }
}

fn r4(r: Rect) -> Value {
    json!([r.0, r.1, r.2, r.3])
}

impl Native for Rec {
    fn secrets(
        &self,
        pid: i32,
        bounds: Rect,
        title: &str,
    ) -> BoxFuture<'_, Option<(Vec<Rect>, Rect, bool)>> {
        self.log(json!(["secrets", pid, r4(bounds), title]));
        let r = (title != "unreadable").then(|| {
            (
                vec![(100.0, 200.0, 50.0, 10.0)],
                (0.0, 80.0, 1280.0, 820.0),
                title != "partial",
            )
        });
        Box::pin(async move { r })
    }
    fn capture(
        &self,
        pid: i32,
        bounds: Rect,
        title: &str,
    ) -> BoxFuture<'_, (Option<Vec<u8>>, String)> {
        self.log(json!(["capture", pid, r4(bounds), title]));
        let r = match title {
            "gone" => (None, "the window is off screen".to_string()),
            "plain" => (Some(b"native-jpeg".to_vec()), String::new()),
            _ => (
                Some(b"native-jpeg".to_vec()),
                "AX painted 1 field".to_string(),
            ),
        };
        Box::pin(async move { r })
    }
    fn place(
        &self,
        rect: Rect,
        viewport: Rect,
        pid: i32,
        bounds: Rect,
        title: &str,
    ) -> BoxFuture<'_, Option<Rect>> {
        self.log(json!([
            "place",
            r4(rect),
            r4(viewport),
            pid,
            r4(bounds),
            title
        ]));
        let r = Some((rect.0 + bounds.0, rect.1 + bounds.1 + 80.0, rect.2, rect.3));
        Box::pin(async move { r })
    }
}

struct Downloads(Log, Arc<std::sync::Mutex<i64>>);

impl DownloadsView for Downloads {
    fn busy(&self, _: &Arc<ferb_browser::bridge::Bridge>, _: i64) -> bool {
        false
    }
    fn settle(
        &self,
        br: &Arc<ferb_browser::bridge::Bridge>,
        chrome_id: i64,
        timeout: f64,
    ) -> BoxFuture<'_, ()> {
        self.0
            .lock()
            .unwrap()
            .push(json!({"seam": ["settle", br.name(), chrome_id, timeout]}));
        Box::pin(async {})
    }
    fn unreported(
        &self,
        br: &Arc<ferb_browser::bridge::Bridge>,
        chrome_id: i64,
    ) -> Vec<Map<String, Value>> {
        self.0
            .lock()
            .unwrap()
            .push(json!({"seam": ["unreported", br.name(), chrome_id]}));
        if chrome_id == 101 {
            vec![
                json!({"name": "report.pdf", "path": "/ws/downloads/report.pdf"})
                    .as_object()
                    .unwrap()
                    .clone(),
            ]
        } else {
            vec![]
        }
    }
    fn facts_for(&self, tasks: &[String], identity: &str) -> Vec<Map<String, Value>> {
        let mut t = tasks.to_vec();
        t.sort();
        self.0
            .lock()
            .unwrap()
            .push(json!({"seam": ["facts_for", t, identity]}));
        vec![json!({"name": "report.pdf"}).as_object().unwrap().clone()]
    }
    fn task_bytes(&self) -> i64 {
        *self.1.lock().unwrap()
    }
    fn total_bytes(&self) -> i64 {
        20_000_000_000
    }
}

struct Stage(Log);

impl Staging for Stage {
    fn stage_all(
        &self,
        tree: &str,
        sources: &[(i32, String, String)],
        tab: &str,
    ) -> Result<Vec<String>, String> {
        let listed: Vec<Value> = sources.iter().map(|(_, p, s)| json!([p, s])).collect();
        self.0
            .lock()
            .unwrap()
            .push(json!({"seam": ["stage", tree, listed, tab]}));
        Ok(sources
            .iter()
            .map(|(_, p, _)| format!("{UPLOADS}/staged/{}", p.rsplit('/').next().unwrap()))
            .collect())
    }
    fn closed(&self, tab: &str) -> bool {
        self.0
            .lock()
            .unwrap()
            .push(json!({"seam": ["closed", tab]}));
        false
    }
}

struct Profiles;

impl ProfilesView for Profiles {
    fn discovered(&self) -> Vec<BrowserProfile> {
        vec![
            BrowserProfile::new("chrome", "Profile 1", "Work", "work@example.test"),
            BrowserProfile::new("chrome", "Profile 3", "Home", ""),
        ]
    }
    fn known(&self) -> Vec<BrowserProfile> {
        self.discovered()
    }
    fn unseen(&self) -> Option<String> {
        None
    }
}

/// The task budget the harness sets per scenario (50 GB; 100 bytes for one call).
pub type Budget = Arc<std::sync::Mutex<i64>>;

thread_local! {
    static BUDGET: Budget = Arc::new(std::sync::Mutex::new(50_000_000_000));
}

thread_local! {
    static MAX_TABS: std::cell::Cell<i64> = const { std::cell::Cell::new(50) };
}

pub fn set_budget(n: i64) {
    BUDGET.with(|b| *b.lock().unwrap() = n);
}

pub fn set_max_tabs(n: i64) {
    MAX_TABS.with(|m| m.set(n));
}

pub fn max_tabs() -> i64 {
    MAX_TABS.with(std::cell::Cell::get)
}

pub fn seams(run: &Run) {
    let (router, ferb) = run.drivers.clone().unwrap();
    let budget = BUDGET.with(Arc::clone);
    *budget.lock().unwrap() = 50_000_000_000;
    set_max_tabs(50);
    for d in [&router, &ferb] {
        let rec = Arc::new(Rec(run.log.clone()));
        d.set_seams(Seams {
            downloads: Some(Arc::new(Downloads(run.log.clone(), budget.clone()))),
            uploads: Some(Arc::new(Stage(run.log.clone()))),
            clipboard: Some(Arc::new(|| (7, Some("echo pasted".to_string()), false))),
            images: Some(rec.clone()),
            native: Some(rec),
            files: Some(Arc::new(ferb_browser::upload::unix::UnixFiles {
                home: "/nonexistent-home".into(),
            })),
            profiles: Some(Arc::new(Profiles)),
            inbox: Some(Inbox {
                address: Arc::new(|| "ferb@example.test".to_string()),
                feed_url: ferb_identity::mailbox::FEED_URL.into(),
            }),
        });
    }
}

/// The router's upload copies switched off (`router.uploads = None`).
pub fn no_staging(run: &Run) {
    let (router, _) = run.drivers.clone().unwrap();
    let mut s = router_seams(&router);
    s.uploads = None;
    router.set_seams(s);
}

fn router_seams(d: &Driver) -> Seams {
    d.seams_now()
}

/// The upload files, as driver_ops.py writes them (fixed bytes and times); removed when dropped.
pub struct Files;

impl Drop for Files {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(UPLOADS);
    }
}

pub fn upload_files(f: &Value) -> Files {
    let _ = std::fs::remove_dir_all(UPLOADS);
    std::fs::create_dir_all(UPLOADS).unwrap();
    let mtime = f["mtime_ns"].as_u64().unwrap();
    let at = std::time::UNIX_EPOCH + std::time::Duration::from_nanos(mtime);
    for (name, data) in f["files"].as_object().unwrap() {
        let p = Path::new(UPLOADS).join(name);
        std::fs::write(
            &p,
            ferb_identity::py::b64decode(data.as_str().unwrap()).unwrap(),
        )
        .unwrap();
        let file = std::fs::File::options().write(true).open(&p).unwrap();
        file.set_modified(at).unwrap();
    }
    std::os::unix::fs::symlink(
        Path::new(UPLOADS).join("a.txt"),
        Path::new(UPLOADS).join("link.txt"),
    )
    .unwrap();
    Files
}

fn action(a: &Value) -> BrowserAction {
    let kind = serde_json::from_value(a["kind"].clone()).unwrap();
    let mut b = BrowserAction::new(kind);
    let s = |k: &str| a.get(k).and_then(Value::as_str).map(String::from);
    b.text = s("text");
    b.trusted = a.get("trusted").and_then(Value::as_bool).unwrap_or(false);
    b.page_mode = a.get("page_mode").and_then(Value::as_bool).unwrap_or(false);
    b.expect_dialog = a
        .get("expect_dialog")
        .and_then(|d| serde_json::from_value(d.clone()).ok());
    b.dialog_text = s("dialog_text");
    b.files = a
        .get("files")
        .and_then(Value::as_array)
        .map(|f| f.iter().map(|x| x.as_str().unwrap().to_string()).collect())
        .unwrap_or_default();
    b.end_element = s("end_element");
    b.mode = a
        .get("mode")
        .and_then(|m| serde_json::from_value(m.clone()).ok());
    b.html = s("html");
    b.modifiers = a
        .get("modifiers")
        .and_then(Value::as_array)
        .map(|m| {
            m.iter()
                .map(|x| serde_json::from_value(x.clone()).unwrap())
                .collect()
        })
        .unwrap_or_default();
    if let Some(g) = a.get("terminal") {
        let mut input = serde_json::to_value(TerminalInput::default()).unwrap();
        for (k, v) in g["input"].as_object().unwrap() {
            input[k] = v.clone();
        }
        b.terminal = Some(TerminalGrant {
            input: serde_json::from_value(input).unwrap(),
            confirmation_id: g["confirmation_id"].as_str().unwrap().into(),
        });
    }
    if let Some(closed) = a.get("closed").and_then(Value::as_array) {
        let closed: Vec<String> = closed
            .iter()
            .map(|c| c.as_str().unwrap().to_string())
            .collect();
        b.files_closed = Some(PathCheck(Arc::new(move |p: &Path, _how: &str| {
            let p = p.to_string_lossy().into_owned();
            closed
                .contains(&p)
                .then(|| format!("{p} is closed by the protection lists"))
        })));
    }
    b
}

fn ok(v: Value) -> Value {
    json!({ "ok": v })
}

fn res<T: serde::Serialize>(r: Result<T, ferb_browser::Error>) -> Value {
    match r {
        Ok(v) => ok(serde_json::to_value(v).unwrap()),
        Err(e) => outcome(Err(e)),
    }
}

fn b64(v: &[u8]) -> Value {
    json!({ "bytes": ferb_identity::py::b64encode(v) })
}

fn rect(r: Option<Rect>) -> Value {
    r.map_or(Value::Null, r4)
}

fn action_shape(r: ferb_core::types::ActionResult) -> Value {
    json!({"action": {"ok": r.ok, "detail": r.detail, "facts": r.facts,
                      "observed": r.observed.map(|o| serde_json::to_value(o).unwrap())}})
}

/// The calls ops.json makes that transcripts.json's driver calls don't.
pub async fn call(
    d: &Arc<Driver>,
    method: &str,
    a: &Value,
    tab: Option<TabRef>,
    el: Option<ElementRef>,
    intent: &str,
) -> Value {
    let s = |k: &str| a.get(k).and_then(Value::as_str).map(String::from);
    let n = |k: &str| a.get(k).and_then(Value::as_u64).unwrap_or(0) as usize;
    let f = |k: &str| a.get(k).and_then(Value::as_f64);
    let tab = || tab.clone().unwrap();
    let act_of = || a.get("action").filter(|x| !x.is_null()).map(action);
    match method {
        "act" => match d.act(&el.unwrap(), &action(&a["action"]), intent).await {
            Ok(r) => ok(action_shape(r)),
            Err(e) => outcome(Err(e)),
        },
        "act_facts" => ok(d
            .act_facts(
                &s("tab_id").unwrap(),
                &s("element_id").unwrap(),
                &s("kind").unwrap(),
                s("text").as_deref(),
            )
            .map_or(Value::Null, |x| json!({"ActionFacts": x}))),
        "element_identity" => ok(json!(
            d.element_identity(&s("tab_id").unwrap(), &s("element_id").unwrap())
        )),
        "terminal_input" => ok(d
            .terminal_input(
                &s("tab_id").unwrap(),
                &s("element_id").unwrap(),
                &s("kind").unwrap(),
                s("text").as_deref(),
                s("end_element").as_deref(),
            )
            .map_or(Value::Null, |x| json!({"TerminalInput": x}))),
        "html" => res(d
            .html(
                &tab(),
                s("element").as_deref(),
                s("selector").as_deref(),
                a.get("frame").and_then(Value::as_i64),
                a["scripts"].as_bool().unwrap(),
                n("offset"),
                n("max_chars"),
            )
            .await),
        "find" => res(d
            .find(&tab(), &s("text").unwrap(), n("offset"), n("max_chars"))
            .await),
        "scroll" => res(d
            .scroll(
                &tab(),
                &s("direction").unwrap(),
                s("container").as_deref(),
                f("screens"),
                s("to").as_deref(),
            )
            .await),
        "wait" => {
            let until = WaitCondition {
                kind: serde_json::from_value(a["until"].clone()).unwrap(),
                value: s("value"),
            };
            res(d.wait(&tab(), &until, f("timeout").unwrap()).await)
        }
        "shows" => res(d.shows(&tab(), &s("text").unwrap()).await),
        "show_tab" => res(d.show_tab(&tab()).await),
        "tab_sound" => res(d.tab_sound(&tab(), a["on"].as_bool().unwrap()).await),
        "media" => res(d
            .media(
                &tab(),
                &s("action").unwrap(),
                a.get("index").and_then(Value::as_i64),
                f("to"),
                s("media_id").as_deref(),
            )
            .await),
        "downloads_for" => res(d.downloads_for(&tab(), f("wait").unwrap()).await),
        "downloads_of" => ok(json!(d.downloads_of(&s("task").unwrap()))),
        "typed_since_load" => ok(json!(d.typed_since_load(&tab()))),
        "closes" => ok(json!(d.closes(&tab()))),
        "shutdown" => {
            d.shutdown().await;
            ok(Value::Null)
        }
        "screenshot" => match d
            .screenshot(
                &tab(),
                a.get("looks").and_then(Value::as_bool).unwrap_or(true),
            )
            .await
        {
            Ok(j) => ok(b64(&j)),
            Err(e) => outcome(Err(e)),
        },
        "masking" => res(d.masking(&tab())),
        "input_note" => res(d.input_note(&tab())),
        "timing" => res(d.timing(&tab())),
        "ids_note" => ok(Value::Object(d.ids_note(&tab()))),
        "pointable" => res(d.pointable(&tab(), &s("shot").unwrap())),
        "visual_snapshot" => match d.visual_snapshot(&tab()).await {
            Ok((shot, jpeg, w, h)) => ok(json!([shot, b64(&jpeg), w, h])),
            Err(e) => outcome(Err(e)),
        },
        "point" => match d
            .point(
                &tab(),
                &s("shot").unwrap(),
                f("x").unwrap(),
                f("y").unwrap(),
            )
            .await
        {
            Ok(e) => ok(json!({ "PageElement": e })),
            Err(e) => outcome(Err(e)),
        },
        "page_rect" => ok(rect(d.page_rect(
            &el.unwrap(),
            act_of().as_ref(),
            a.get("visible").and_then(Value::as_bool).unwrap_or(false),
        ))),
        "screen_rect" => ok(rect(
            d.screen_rect(
                &el.unwrap(),
                act_of().as_ref(),
                a.get("visible").and_then(Value::as_bool).unwrap_or(false),
            )
            .await,
        )),
        "pdf" => match d.pdf(&tab(), a["landscape"].as_bool().unwrap()).await {
            Ok(p) => ok(b64(&p)),
            Err(e) => outcome(Err(e)),
        },
        "owner_view" => match d.owner_view(&tab()).await {
            Ok(img) => ok(json!({"owner_only": img.jpeg().len()})),
            Err(e) => outcome(Err(e)),
        },
        "owner_view_supported" => res(d.owner_view_supported(&tab())),
        "otp_candidates" => res(d
            .otp_candidates(
                &el.unwrap(),
                &SecretRef {
                    key: s("secret").unwrap(),
                },
            )
            .await),
        "fill_otp" => match d
            .fill_otp(
                &el.unwrap(),
                &SecretRef {
                    key: s("secret").unwrap(),
                },
                &s("candidate").unwrap(),
                intent,
            )
            .await
        {
            Ok(r) => ok(action_shape(r)),
            Err(e) => outcome(Err(e)),
        },
        "session_ok" => res(d.session_ok().await),
        "open_tabs" => res(d
            .open_tabs(s("profile").as_deref(), s("task").as_deref())
            .await),
        "profile_candidates" => res(d.profile_candidates()),
        "not_connected" => res(d.not_connected()),
        "rest" => res(d
            .rest(&s("owner").unwrap(), a["resting"].as_bool().unwrap())
            .await),
        "opened_for" => ok(json!(d.opened_for(&tab()))),
        "closed_for" => ok(json!(d.closed_for(&tab()))),
        "close" => res(d.close(&tab(), None).await),
        "tool" => tool(d, a).await,
        other => panic!("driver method {other}"),
    }
}

/// "$candidate-<n>": the latest inventory's n-th token (Rust's own, as Python's harness used its own).
pub fn candidate(tokens: &[String], v: &mut Value) {
    let Some(c) = v["args"].get("candidate").and_then(Value::as_str) else {
        return;
    };
    if let Some(i) = c.strip_prefix("$candidate-") {
        let i: usize = i.parse().unwrap();
        v["args"]["candidate"] = tokens
            .get(i)
            .cloned()
            .unwrap_or_else(|| "none".into())
            .into();
    }
}

/// The tokens were taken when the inventory's result came back (`normalize`).
pub fn tokens(_run: &mut Run) {}

fn uid_free(v: &mut Value) {
    match v {
        Value::Object(m) => {
            for (k, x) in m.iter_mut() {
                if k == "owner_uid" {
                    *x = "$UID".into();
                } else {
                    uid_free(x);
                }
            }
        }
        Value::Array(a) => a.iter_mut().for_each(uid_free),
        _ => {}
    }
}

/// What differs by nature: the owner's uid; an error's step times (the harness records none). The
/// inbox inventory's tokens are drawn from the fixtures' counter on both sides (the driver hands its
/// random source to ferb-identity's mailbox), so they are the same; the latest are kept for
/// "$candidate-<n>".
pub fn normalize(run: &mut Run, mut got: Value, want: &Value) -> Value {
    uid_free(&mut got);
    if got.get("error").is_some() {
        got["timing"] = Value::Null;
    }
    let at = if want.pointer("/ok/content/candidates").is_some() {
        "/ok/content/candidates"
    } else {
        "/ok/candidates"
    };
    if let Some(g) = got.pointer(at).and_then(Value::as_array) {
        run.tokens = g
            .iter()
            .map(|c| c["candidate"].as_str().unwrap().to_string())
            .collect();
    }
    got
}

// ---------- the tool layer ----------

struct PolicyTable(Value);

impl ferb_browser::tools::Policy for PolicyTable {
    fn parse(&self, url: &str) -> Option<(String, String)> {
        let v = self.0["parse"]
            .get(url)
            .unwrap_or_else(|| panic!("Python's urls.parse of {url:?}"));
        v.as_array().map(|a| {
            (
                a[0].as_str().unwrap().to_string(),
                a[1].as_str().unwrap().to_string(),
            )
        })
    }
    fn typed_destination(&self, text: &str) -> Option<String> {
        self.0["typed"]
            .get(text)
            .unwrap_or_else(|| panic!("Python's typed_destination of {text:?}"))
            .as_str()
            .map(String::from)
    }
}

struct NoHost;

impl ferb_browser::tools::Host for NoHost {
    fn pair(
        &self,
        _: &str,
        _: &str,
    ) -> Option<BoxFuture<'_, ferb_browser::Result<Map<String, Value>>>> {
        None
    }
    fn handoff(
        &self,
        _: TabRef,
        _: String,
        _: ferb_core::types::ToolContext,
    ) -> Option<BoxFuture<'_, ferb_browser::Result<Map<String, Value>>>> {
        None
    }
    fn resolve(
        &self,
        p: &str,
        _: &ferb_core::types::ToolContext,
        _: bool,
    ) -> ferb_browser::Result<String> {
        Ok(p.into())
    }
    fn owner_path(&self, p: &str, workspace: &str) -> String {
        if p.starts_with('/') {
            p.into()
        } else {
            format!("{workspace}/{p}")
        }
    }
    fn read(&self, _: &str) -> ferb_browser::Result<Vec<u8>> {
        Ok(vec![])
    }
    fn new_file(
        &self,
        _: &str,
        _: &[u8],
        _: &ferb_core::types::ToolContext,
    ) -> ferb_browser::Result<()> {
        Ok(())
    }
    fn display_of(&self, _: Rect) -> Option<i64> {
        None
    }
    fn recordings(&self) -> Option<Arc<dyn ferb_browser::tools::Recordings>> {
        None
    }
}

thread_local! {
    static POLICY: std::cell::RefCell<Value> = const { std::cell::RefCell::new(Value::Null) };
    static HOST: std::cell::RefCell<Option<Arc<dyn ferb_browser::tools::Host>>> = const { std::cell::RefCell::new(None) };
}

/// The daemon's part of the tools for the transcripts that follow (`None`: none of it).
pub fn set_host(host: Option<Arc<dyn ferb_browser::tools::Host>>) {
    HOST.with(|h| *h.borrow_mut() = host);
}

pub fn policy(f: &Value) {
    POLICY.with(|p| *p.borrow_mut() = f["policy"].clone());
}

fn tools_for(d: &Arc<Driver>, family: &str) -> ferb_browser::tools::Tools {
    ferb_browser::tools::Tools {
        family: if family == "ferb" {
            ferb_browser::tools::FERB_BROWSER
        } else {
            ferb_browser::tools::OWNER_BROWSER
        },
        driver: d.clone(),
        policy: Arc::new(PolicyTable(POLICY.with(|p| p.borrow().clone()))),
        host: HOST
            .with(|h| h.borrow().clone())
            .unwrap_or_else(|| Arc::new(NoHost)),
    }
}

fn ctx_of(c: &Value, family: &str) -> ferb_core::types::ToolContext {
    ferb_core::types::ToolContext {
        task_id: c
            .get("task")
            .and_then(Value::as_str)
            .unwrap_or("task-a")
            .into(),
        worker_id: "w".into(),
        request_id: "r".into(),
        identity: if family == "ferb" {
            ferb_core::types::Identity::Ferb
        } else {
            ferb_core::types::Identity::Owner
        },
        scopes: c
            .get("scopes")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default(),
        device_id: "dev-1".into(),
        span_id: None,
        workspace: "/tmp/ferb-ws".into(),
        owner_roots: vec![],
        untrusted_roots: vec![],
        intent_id: c.get("intent").and_then(Value::as_str).map(String::from),
        observe_only: c
            .get("observe_only")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        probe: c.get("probe").and_then(Value::as_bool).unwrap_or(false),
        path_closed: None,
        escape: None,
        terminal: None,
        folders: vec![],
    }
}

/// The arguments of the tool call running now (a test host keys its scripted answers by them).
pub static TOOL_ARGS: std::sync::Mutex<Value> = std::sync::Mutex::new(Value::Null);

/// A tool's method (`tcall` in driver_ops.py).
pub async fn tool(d: &Arc<Driver>, a: &Value) -> Value {
    *TOOL_ARGS.lock().unwrap() = a["args"].clone();
    let family = a["family"].as_str().unwrap();
    let t = tools_for(d, family);
    let v = ferb_browser::tools::verb(a["verb"].as_str().unwrap()).unwrap();
    let args = &a["args"];
    match a["what"].as_str().unwrap() {
        "validate" => {
            ok(ferb_browser::tools::validate(v.verb, args).map_or(Value::Null, |m| json!([m])))
        }
        "run" => match t
            .run(v, args, &ctx_of(a.get("ctx").unwrap_or(&json!({})), family))
            .await
        {
            Ok(r) => ok(json!({"content": r.content, "is_error": r.is_error,
                               "read_sources": r.read_sources.iter().map(|(k, v)| json!([k, v])).collect::<Vec<_>>(),
                               "image": r.image.map(|i| ferb_identity::py::b64encode(&i)),
                               "facts": r.facts.map(|f| serde_json::to_value(f).unwrap()),
                               "timing": r.timing})),
            Err(e) => outcome(Err(e)),
        },
        "describe" => ok(serde_json::to_value(t.describe(v, args)).unwrap()),
        "locks" => ok(json!(t.locks(v, args))),
        "file_paths" => ok(json!(t
            .file_paths(v, args)
            .into_iter()
            .map(|(p, how)| json!([p, how]))
            .collect::<Vec<_>>())),
        "ledger_args" => ok(Value::Object(
            t.ledger_args(args, a["hashed"].as_object().unwrap()),
        )),
        "point" => {
            let task = a.get("task").and_then(Value::as_str).unwrap_or("task-a");
            ok(t.point(v, args, task).await.map_or(Value::Null, |p| {
                json!({"kind": p.kind, "rect": [p.rect.0, p.rect.1, p.rect.2, p.rect.3], "display": p.display})
            }))
        }
        "terminal_input" => ok(t
            .terminal_input(args)
            .map_or(Value::Null, |x| serde_json::to_value(x).unwrap())),
        _ => ok(t
            .preview_target(args, a.get("observed").is_some_and(|o| !o.is_null()))
            .map_or(Value::Null, |p| json!({"scope": p.scope, "label": p.label}))),
    }
}
