//! The Mac driver's and the desktop observer's wiring: native captures, the front window, a task's released
//! resources, installed and running apps. Port of `ferb/daemon/areas/desktop.py` (a mixin of the Daemon). Must not
//! act on the owner's screen itself.
//!
//! Every engine another part owns comes in through [`Seams`] (the composition implements it), the Mac driver
//! through [`MacDriver`], the window server and process facts the front-window capture reads through
//! [`NativeFront`]. FERB's home, config, redactor, journal, bus and clock come from [`Host`].
//!
//! Not here (main.py's composition, a later part): `_build_mac_and_shell`.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex};

use ferb_core::interfaces::BoxFuture;
use ferb_device::profile::{AppInfo, DeviceProfile, ProfileStore};
use ferb_rpc::HandlerError;
use ferb_sense::front_window::{self as fw, FrontWindow, ShotError, Sources, TabLookup, Window};
use ferb_sense::watchers::browser_tabs::{self, ScriptedWindow};
use ferb_sense::{Dict, Redact};
use serde_json::{json, Map, Value};

use super::core::{refusal, Host};
use super::environment::TabDriver;

/// x, y, w, h in screen points (top-left origin).
pub type Rect = (f64, f64, f64, f64);

/// A task as the desktop's records read it (`self.tasks.get(id)`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TaskRef {
    pub span_id: Option<String>,
    pub tree_id: Option<String>,
}

/// One of FERB's shell processes (`ShellManager.list()`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ShellProc {
    pub id: String,
    pub owner_task: Option<String>,
    pub command: String,
    pub long_lived: bool,
    pub running: bool,
    pub started: f64,
}

/// The trace's spans (`self.spans`).
pub trait Spans: Send + Sync {
    /// `spans.start(kind, task_id=…, parent_id=…, input=…)`: the new span's id.
    fn start(&self, kind: &str, task_id: &str, parent_id: Option<&str>, input: Value) -> String;
    fn finish(&self, span_id: &str, output: Value);
}

/// Why a window's picture wasn't taken (act/mac errors): `NoTarget`, `Withheld` and `Refused` carry FERB's own
/// words; `Failed` is any other error, by its name (call_hooks.py `error_name`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CaptureError {
    NoTarget(String),
    Withheld(String),
    Refused(String),
    Failed(String),
}

/// A window's picture: the JPEG, how its secret fields were found, and the tab the extension named for it.
pub type WindowImage = (Vec<u8>, String, Option<(String, String)>);

/// FERB's Mac driver (`self.mac`, `MacComputerDriver`) and the window server reads beside it
/// (`act/mac/screenshot.py`).
pub trait MacDriver: Send + Sync {
    /// B6: the secret fields Accessibility finds in the page area of that browser window. Err: the error's name.
    fn page_secrets<'a>(
        &'a self,
        pid: i64,
        bounds: Rect,
        title: &'a str,
    ) -> BoxFuture<'a, Result<Value, String>>;
    /// L33: a task tab's element in screen points.
    fn place_on_web_area<'a>(
        &'a self,
        rect: Rect,
        viewport: Rect,
        pid: i64,
        bounds: Rect,
        title: &'a str,
    ) -> BoxFuture<'a, Result<Option<Rect>, String>>;
    /// The owner's `[mac] snapshot_image` switch.
    fn snapshot_images(&self) -> bool;
    /// `mac.window_image(target, window=…)`.
    fn window_image<'a>(
        &'a self,
        target: &'a str,
        window: &'a str,
    ) -> BoxFuture<'a, Result<WindowImage, CaptureError>>;
    /// `screenshot.normal_windows()`: on-screen normal windows, front to back. Err: the error's name.
    fn normal_windows(&self) -> Result<Vec<Window>, String>;
    /// `screenshot.window(id)`: that window now.
    fn window(&self, id: i64) -> Result<Option<Window>, String>;
    /// `self.mac.launched`: whether the driver tracks the apps tasks launch (L39).
    fn has_launched(&self) -> bool;
    /// `launched.release(task_id)`: the apps a task launched, let go of. Err: `"<Type>: <text>"`.
    fn release_launched<'a>(
        &'a self,
        task_id: &'a str,
    ) -> BoxFuture<'a, Result<Vec<Value>, String>>;
    /// `act/mac/tools.py` `app_window`: one background picture of an app's window, or None.
    fn app_window<'a>(
        &'a self,
        app: &'a str,
        scopes: &'a BTreeSet<String>,
    ) -> BoxFuture<'a, Result<Option<Value>, String>>;
}

/// A running Automation check (`permissions.automation_check(bundle, ask=False)`'s future).
pub trait Probe: Send + Sync {
    fn done(&self) -> bool;
    /// Its answer is GRANTED (a failed check is not).
    fn granted(&self) -> bool;
}

/// The rest of the daemon as desktop.py reads it.
pub trait Seams: Send + Sync {
    fn task(&self, task_id: &str) -> Option<TaskRef>;
    fn spans(&self) -> Option<Arc<dyn Spans>>;
    fn mac(&self) -> Option<Arc<dyn MacDriver>>;
    /// `self.hearing.end_task(task)` (L26).
    fn hearing_end_task<'a>(&'a self, task_id: &'a str) -> BoxFuture<'a, Vec<Value>>;
    /// `self.recordings.end_task(task)`.
    fn recordings_end_task<'a>(&'a self, task_id: &'a str) -> BoxFuture<'a, Vec<Value>>;
    /// The owners of what hearing is listening to now.
    fn listening_owners(&self) -> Vec<Option<String>>;
    /// The owners of the recordings running now.
    fn recording_owners(&self) -> Vec<Option<String>>;
    /// `self.identity_browser`.
    fn identity_browser(&self) -> Option<Arc<dyn TabDriver>>;
    /// `self.router`.
    fn router(&self) -> Option<Arc<dyn TabDriver>>;
    /// `self.downloads.forget_task(task)` when there is a downloads keeper.
    fn forget_downloads(&self, task_id: &str);
    /// `self.shells is not None`.
    fn has_shells(&self) -> bool;
    fn shells_end_task<'a>(&'a self, task_id: &'a str) -> BoxFuture<'a, Result<(), String>>;
    /// `shells.stop_running(mine)`: how many of the matching running commands stopped.
    fn shells_stop_running<'a>(
        &'a self,
        mine: &'a (dyn Fn(Option<&str>) -> bool + Send + Sync),
    ) -> BoxFuture<'a, Result<usize, String>>;
    fn shell_processes(&self) -> Vec<ShellProc>;
    /// `self.schedules is not None`.
    fn has_schedules(&self) -> bool;
    /// A schedule's state ("active", "paused", …); None: no such schedule.
    fn schedule_state(&self, schedule_id: &str) -> Option<String>;
    /// `_schedule_change(id, action, with_task=…)`.
    fn schedule_change(&self, schedule_id: &str, action: &str, with_task: &str);
    /// `journal.latest_kind(kinds, key, value)`.
    fn journal_latest_kind(&self, kinds: &[&str], key: &str, value: &str) -> Option<String>;
    /// `journal.of_kind(kind, key, value)`: each row's data, oldest first.
    fn journal_of_kind(&self, kind: &str, key: &str, value: &str) -> Vec<Value>;
    /// `tokens.revoke_tree(tree)`.
    fn revoke_tree(&self, tree_id: &str);
    /// `disturbance.finish(tree)`. Err: `"<Type>: <text>"`.
    fn disturbance_finish<'a>(&'a self, tree_id: &'a str) -> BoxFuture<'a, Result<(), String>>;
    /// `changes.forget(tree)` (blocking file work): the paths that couldn't be removed; None: no change records.
    fn changes_forget(&self, tree_id: &str) -> Option<Vec<String>>;
    /// `self.uploads is not None`.
    fn has_uploads(&self) -> bool;
    /// `_clean_uploads(uploads.forget_tree, tree, about={"task_id": tree})`.
    fn clean_uploads<'a>(&'a self, tree_id: &'a str) -> BoxFuture<'a, ()>;
    /// `permissions.automation_check(bundle, ask=False)`: never asks.
    fn automation_check(&self, bundle_id: &str) -> Arc<dyn Probe>;
    /// `browser_tabs.applescript_windows(bundle)`; Err: osascript failed to run.
    fn applescript_windows(&self, bundle_id: &str) -> Result<Option<Vec<ScriptedWindow>>, String>;
    /// `permissions.accessibility()` / `screen_recording()` as their status names; Err: they can't be read.
    fn permission(&self, name: &str) -> Result<String, String>;
    /// `apps.running()`: (bundle id, name, pid). Err: they can't be read.
    fn running_apps(&self) -> Result<Vec<(String, String, i64)>, String>;
}

fn tree_of(task_id: &str) -> &str {
    task_id.split('.').next().unwrap_or(task_id)
}

/// desktop.py `_record_disturbance`: an observation persisted before broadcast; tool arguments stay in their
/// referenced action span.
pub fn record_disturbance(
    host: &Host,
    s: &dyn Seams,
    task_id: &str,
    kind: &str,
    data: Map<String, Value>,
) {
    let Some(spans) = s.spans() else {
        return;
    };
    let parent = s.task(task_id).and_then(|t| t.span_id);
    let span = spans.start(
        kind,
        task_id,
        parent.as_deref(),
        json!({"operation": "observe desktop"}),
    );
    spans.finish(&span, Value::Object(data.clone()));
    if kind == "disturbance.change" || kind == "disturbance.error" {
        let mut out = Map::new();
        out.insert("task_id".into(), Value::from(task_id));
        out.insert("span_id".into(), Value::from(span));
        for (k, v) in data {
            out.insert(k, v);
        }
        host.publish("disturbance.observed", Value::Object(out));
    }
}

/// desktop.py `_task_end`: a task's resources let go of at its end: listening (L26), recordings, apps it launched
/// (L39), its tabs in FERB's browser then the owner's (each tab left behind named, HAND-419), its downloads' facts,
/// its shell processes.
pub async fn task_end(s: &dyn Seams, task_id: &str) -> Vec<Value> {
    let mut resources = s.hearing_end_task(task_id).await;
    resources.extend(s.recordings_end_task(task_id).await);
    if let Some(mac) = s.mac().filter(|m| m.has_launched()) {
        match mac.release_launched(task_id).await {
            Ok(r) => resources.extend(r),
            Err(e) => resources.push(json!({"resource": "app", "released": false, "error": e})),
        }
    }
    for (name, drv) in [
        ("ferb_browser", s.identity_browser()),
        ("browser", s.router()),
    ] {
        let Some(drv) = drv else {
            continue;
        };
        match drv.release(task_id).await {
            Ok(r) => resources.extend(r),
            Err(e) => {
                let mut m = Map::new();
                m.insert("resource".into(), Value::from(name));
                m.insert("released".into(), Value::Bool(false));
                m.insert("error".into(), Value::from(e.error));
                if let Some(tabs) = e.tabs.filter(ferb_py::value::truthy) {
                    m.insert("tabs".into(), tabs);
                }
                resources.push(Value::Object(m));
            }
        }
    }
    // after its tabs are let go of: their locks and unreported facts
    s.forget_downloads(task_id);
    if s.has_shells() {
        if let Err(e) = s.shells_end_task(task_id).await {
            resources.push(json!({"resource": "shell", "released": false, "error": e}));
        }
    }
    resources
}

/// desktop.py `_task_paused` (ORCH-312): a paused task keeps only its tabs and progress. Its terminal commands still
/// running (not long-lived ones) and its listening and recordings stop now; the owner's pause of a scheduled run
/// pauses that schedule's next runs (journaled `with_task`, so resuming the run resumes it).
pub async fn task_paused(
    host: &Host,
    s: &dyn Seams,
    data: &Map<String, Value>,
) -> Result<(), HandlerError> {
    let tree = data
        .get("task_id")
        .filter(|v| ferb_py::value::truthy(v))
        .map(ferb_py::value::py_str)
        .unwrap_or_default();
    if tree.is_empty() {
        return Ok(());
    }
    let mine = |owner: Option<&str>| owner.is_some_and(|o| !o.is_empty() && tree_of(o) == tree);
    let mut stopped: Vec<Value> = Vec::new();
    if s.has_shells() {
        match s.shells_stop_running(&mine).await {
            Ok(0) => {}
            Ok(n) => stopped.push(json!({"resource": "shell", "stopped": n})),
            Err(e) => stopped.push(json!({"resource": "shell", "error": e})),
        }
    }
    let owners = |list: Vec<Option<String>>| -> BTreeSet<String> {
        list.into_iter()
            .flatten()
            .filter(|o| mine(Some(o)))
            .collect()
    };
    for owner in owners(s.listening_owners()) {
        stopped.extend(s.hearing_end_task(&owner).await);
    }
    for owner in owners(s.recording_owners()) {
        stopped.extend(s.recordings_end_task(&owner).await);
    }
    if !stopped.is_empty() {
        host.journal(
            "task.pause_stopped",
            json!({"task_id": tree, "resources": stopped}),
            None,
        )?;
    }
    if let Some(sid) = data
        .get("schedule_id")
        .filter(|v| ferb_py::value::truthy(v))
    {
        let sid = ferb_py::value::py_str(sid);
        if s.has_schedules() && s.schedule_state(&sid).as_deref() == Some("active") {
            s.schedule_change(&sid, "pause", &tree);
        }
    }
    Ok(())
}

/// desktop.py `_task_resumed`: a scheduled run resumed: the schedule its pause paused (ORCH-312) runs again, unless
/// the owner paused, resumed or cancelled it themselves since.
pub async fn task_resumed(s: &dyn Seams, data: &Map<String, Value>) {
    let Some(sid) = data
        .get("schedule_id")
        .filter(|v| ferb_py::value::truthy(v))
        .map(ferb_py::value::py_str)
    else {
        return;
    };
    let tree = data
        .get("task_id")
        .filter(|v| ferb_py::value::truthy(v))
        .map(ferb_py::value::py_str)
        .unwrap_or_default();
    if !s.has_schedules() {
        return;
    }
    let state = s.schedule_state(&sid);
    let last = s.journal_latest_kind(
        &["schedule.pause", "schedule.resume", "schedule.cancel"],
        "schedule_id",
        &sid,
    );
    let paused = s.journal_of_kind("schedule.pause", "schedule_id", &sid);
    let with_task = paused.last().and_then(|d| d.get("with_task"));
    if state.as_deref() == Some("paused")
        && last.as_deref() == Some("schedule.pause")
        && with_task == Some(&Value::from(tree.clone()))
    {
        s.schedule_change(&sid, "resume", &tree);
    }
}

/// desktop.py `_tree_finished`: a task tree ended: its run tokens, the desktop observer's summary, then its change
/// record's copies (L39) and FERB's upload copies for it go (rule 17). What couldn't be removed is journaled; the
/// next start-up sweeps it. The observer's failure is returned after the clean-up, as Python's `finally` does.
pub async fn tree_finished(host: &Host, s: &dyn Seams, tree_id: &str) -> Result<(), String> {
    s.revoke_tree(tree_id);
    let finished = s.disturbance_finish(tree_id).await;
    if let Some(stuck) = s.changes_forget(tree_id) {
        if !stuck.is_empty() {
            host.journal(
                "changes.cleanup_failed",
                json!({"task_id": tree_id, "paths": stuck.len()}),
                None,
            )
            .map_err(|e| e.to_string())?;
        }
    }
    if s.has_uploads() {
        s.clean_uploads(tree_id).await;
    }
    finished
}

/// act/mac/launched.py `brought_forward`: whether the desktop observer recorded the app (by pid) coming forward in
/// this task tree after `since` with no FERB call that can change the screen in flight or just ended (P26's facts,
/// never who did it).
pub fn brought_forward(
    conn: &rusqlite::Connection,
    tree_id: &str,
    pid: i64,
    since: &str,
) -> rusqlite::Result<bool> {
    let mut stmt = conn.prepare(
        "SELECT output_json FROM spans WHERE task_id = ? AND kind = 'disturbance.change' AND started_at >= ? AND \
         json_extract(output_json, '$.changes.frontmost_app.after.pid') = ?",
    )?;
    let rows = stmt.query_map(rusqlite::params![tree_id, since, pid], |r| {
        r.get::<_, Option<String>>(0)
    })?;
    for row in rows {
        let text = row?.unwrap_or_else(|| "{}".into());
        let data: Value =
            serde_json::from_str(if text.is_empty() { "{}" } else { &text }).unwrap_or(Value::Null);
        let actions = data.get("actions").and_then(Value::as_array);
        let on_screen = actions.into_iter().flatten().any(|a| {
            a.as_object()
                .and_then(|o| o.get("on_screen"))
                .is_some_and(ferb_py::value::truthy)
        });
        if !on_screen {
            return Ok(true);
        }
    }
    Ok(false)
}

/// desktop.py `_owner_used_app` (L39): the desktop observer recorded the app coming forward in the task's tree after
/// FERB launched it at `at`, with no FERB on-screen call near.
pub fn owner_used_app(
    s: &dyn Seams,
    conn: &rusqlite::Connection,
    task_id: &str,
    pid: i64,
    at: &str,
) -> rusqlite::Result<bool> {
    let tree = s
        .task(task_id)
        .and_then(|t| t.tree_id)
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| tree_of(task_id).to_owned());
    brought_forward(conn, &tree, pid, at)
}

/// desktop.py `_running_processes`: FERB-started processes running now, commands redacted: a front-app or Space
/// change's record names them (P26). Facts only.
pub fn running_processes(host: &Host, s: &dyn Seams) -> Vec<Value> {
    if !s.has_shells() {
        return Vec::new();
    }
    s.shell_processes()
        .into_iter()
        .filter(|p| p.running)
        .map(|p| {
            json!({"id": p.id, "task": p.owner_task, "command": host.redact_text(&p.command),
                   "background": p.long_lived})
        })
        .collect()
}

/// desktop.py `_shell_process` (ORCH-628): one of FERB's shell commands by its id, for the live line of a task
/// waiting on it.
pub fn shell_process(s: &dyn Seams, proc_id: &str) -> Option<Value> {
    let procs = if s.has_shells() {
        s.shell_processes()
    } else {
        Vec::new()
    };
    let p = procs.into_iter().find(|p| p.id == proc_id)?;
    Some(
        json!({"command": p.command, "started": p.started, "owner_task": p.owner_task, "running": p.running}),
    )
}

/// desktop.py `_native_page_secrets` (B6): for a task tab's capture whose page the extension couldn't read, the
/// secret fields Accessibility finds. None: no Mac driver, or it can't tell (journaled by type).
pub async fn native_page_secrets(
    host: &Host,
    s: &dyn Seams,
    pid: i64,
    bounds: Rect,
    title: &str,
) -> Result<Option<Value>, HandlerError> {
    let Some(mac) = s.mac() else {
        return Ok(None);
    };
    match mac.page_secrets(pid, bounds, title).await {
        Ok(v) => Ok(Some(v)),
        Err(name) => {
            host.journal("screen.page_secrets_failed", json!({"error": name}), None)?;
            Ok(None)
        }
    }
}

/// desktop.py `_native_place` (L33): a task tab's element in screen points for FERB's cursor; None without a Mac
/// driver. A driver's failure is the caller's.
pub async fn native_place(
    s: &dyn Seams,
    rect: Rect,
    viewport: Rect,
    pid: i64,
    bounds: Rect,
    title: &str,
) -> Result<Option<Rect>, String> {
    let Some(mac) = s.mac() else {
        return Ok(None);
    };
    mac.place_on_web_area(rect, viewport, pid, bounds, title)
        .await
}

/// desktop.py `_native_window_capture` (round 3, I1): for a task tab the debugger can't attach to, the on-screen
/// browser window showing it captured natively: (JPEG, masking note), or (None, why) in FERB's words.
pub async fn native_window_capture(
    host: &Host,
    s: &dyn Seams,
    pid: i64,
    bounds: Rect,
    title: &str,
) -> Result<(Option<Vec<u8>>, String), HandlerError> {
    let Some(mac) = s.mac() else {
        return Ok((
            None,
            "FERB has no Mac driver to capture windows with".into(),
        ));
    };
    if !mac.snapshot_images() {
        return Ok((
            None,
            "screenshots are switched off ([mac] snapshot_image)".into(),
        ));
    }
    let failed = |name: String| -> Result<(Option<Vec<u8>>, String), HandlerError> {
        host.journal("screen.native_capture_failed", json!({"error": name}), None)?;
        Ok((None, format!("the window capture failed ({name})")))
    };
    let windows = match mac.normal_windows() {
        Ok(w) => w,
        Err(name) => return failed(name),
    };
    let at = [bounds.0, bounds.1, bounds.2, bounds.3];
    let mut near: Vec<Window> = windows
        .into_iter()
        .filter(|w| {
            let b = [w.bounds.0, w.bounds.1, w.bounds.2, w.bounds.3];
            w.pid == pid && b.iter().zip(at.iter()).all(|(a, b)| (a - b).abs() <= 2.0)
        })
        .collect();
    if !title.is_empty() {
        // the window showing the tab carries its title, even when it is the only one there (rule 5)
        near.retain(|w| ferb_mac_hands::mask::same_title(&w.title, title));
    }
    if near.is_empty() {
        return Ok((
            None,
            "no window of the browser showing that tab is on screen (it may be on another Space)"
                .into(),
        ));
    }
    if near.len() > 1 {
        return Ok((
            None,
            "more than one window of the browser at that place shows that tab's title".into(),
        ));
    }
    let id = near[0].id;
    let (jpeg, masking) = match mac
        .window_image(&format!("pid:{pid}"), &id.to_string())
        .await
    {
        Ok((jpeg, masking, _)) => (jpeg, masking),
        Err(CaptureError::NoTarget(_)) => {
            return Ok((
                None,
                "the window went away before it could be captured".into(),
            ))
        }
        Err(CaptureError::Withheld(text)) => {
            let text = text.replace("; pixels withheld", "");
            let text = text.strip_prefix("pixels withheld: ").unwrap_or(&text);
            return Ok((None, text.to_owned()));
        }
        Err(CaptureError::Refused(text)) => return Ok((None, text)),
        Err(CaptureError::Failed(name)) => return failed(name),
    };
    let now = match mac.window(id) {
        Ok(w) => w,
        Err(name) => return failed(name),
    };
    // checked again (round 5): its page may have changed
    if !title.is_empty() && !now.is_some_and(|w| ferb_mac_hands::mask::same_title(&w.title, title))
    {
        return Ok((
            None,
            "the window's page changed while it was captured (its title isn't the tab's now)"
                .into(),
        ));
    }
    Ok((Some(jpeg), masking))
}

/// desktop.py's Automation state: `_automation_cache`, `_automation_checks` and `_tab_granted`.
#[derive(Default)]
pub struct Automation {
    cache: Mutex<HashMap<String, (f64, bool)>>,
    checks: Mutex<HashMap<String, Arc<dyn Probe>>>,
    tab_granted: Mutex<HashSet<String>>,
}

impl Automation {
    /// The bundles with a kept answer, a check still running, and a kept GRANTED for the after-shot read.
    pub fn state(&self) -> (Vec<String>, Vec<String>, Vec<String>) {
        let sorted = |mut v: Vec<String>| {
            v.sort();
            v
        };
        (
            sorted(self.cache.lock().unwrap().keys().cloned().collect()),
            sorted(self.checks.lock().unwrap().keys().cloned().collect()),
            sorted(self.tab_granted.lock().unwrap().iter().cloned().collect()),
        )
    }

    fn probe(&self, bundle_id: &str) -> Option<Arc<dyn Probe>> {
        self.checks.lock().unwrap().get(bundle_id).cloned()
    }

    /// desktop.py `_automation_granted`: already granted by the owner; never asks (the prompt would appear out of
    /// nowhere). An answer is kept 60 seconds; while a check is still running this answers false.
    pub fn granted(&self, host: &Host, s: &dyn Seams, bundle_id: &str) -> bool {
        let now = host.clock.monotonic();
        if let Some((at, ok)) = self.cache.lock().unwrap().get(bundle_id) {
            if now - at < 60.0 {
                return *ok;
            }
        }
        let probe = {
            let mut checks = self.checks.lock().unwrap();
            checks
                .entry(bundle_id.to_owned())
                .or_insert_with(|| s.automation_check(bundle_id))
                .clone()
        };
        if !probe.done() {
            return false; // a stuck native call must not hold the caller
        }
        self.checks.lock().unwrap().remove(bundle_id);
        let ok = probe.granted();
        self.cache
            .lock()
            .unwrap()
            .insert(bundle_id.to_owned(), (now, ok));
        ok
    }

    /// desktop.py `_front_tab` (L12): the active tab (URL, title) of the browser window the front-window capture
    /// took, every window of the browser read through AppleScript and the captured one picked by its place and
    /// title (Err when no single window is it). Only when the owner already granted Automation for that browser. A
    /// kept GRANTED answer serves only the read after the shot, and only while a fresh check is still pending.
    pub fn front_tab(
        &self,
        host: &Host,
        s: &dyn Seams,
        bundle_id: &str,
        window: &Window,
        after_shot: bool,
    ) -> Result<Option<(String, String)>, TabLookup> {
        if browser_tabs::windows_script(bundle_id).is_none() {
            return Ok(None);
        }
        if !self.granted(host, s, bundle_id) {
            let probe = self.probe(bundle_id);
            if probe.as_ref().is_none_or(|p| p.done()) {
                // a check answered something other than GRANTED
                self.tab_granted.lock().unwrap().remove(bundle_id);
                return Ok(None);
            }
            if !(after_shot && self.tab_granted.lock().unwrap().contains(bundle_id)) {
                return Ok(None);
            }
        }
        self.tab_granted
            .lock()
            .unwrap()
            .insert(bundle_id.to_owned());
        let listed = s.applescript_windows(bundle_id).ok().flatten();
        let Some(listed) = listed else {
            self.tab_granted.lock().unwrap().remove(bundle_id);
            return Ok(None);
        };
        let found = browser_tabs::window_tab(&listed, window.bounds, &window.title)?;
        Ok((!found.url.is_empty()).then_some((found.url, found.title)))
    }
}

/// desktop.py `_device_profile`: this Mac's device profile (`[device] id`), None without an id or a profile.
pub fn device_profile(host: &Host) -> Result<Option<DeviceProfile>, HandlerError> {
    let Some(did) = host.get("device", "id")?.filter(ferb_py::value::truthy) else {
        return Ok(None);
    };
    ProfileStore::new(&host.home)
        .load(&ferb_py::value::py_str(&did))
        .map_err(|e| refusal(e.to_string()))
}

/// desktop.py `_installed_apps`: (bundle id, name) of the Mac's apps, for matching the names the owner uses.
pub fn installed_apps(host: &Host) -> Result<Vec<(String, String)>, HandlerError> {
    Ok(device_profile(host)?
        .map(|p| p.apps.into_iter().map(|a| (a.bundle_id, a.name)).collect())
        .unwrap_or_default())
}

/// desktop.py `_app_changed` / `_app_changed_off_loop`: an app appeared or disappeared: that app only rescanned into
/// the device profile (the composition runs this off the event loop: the profile is a file read and written whole),
/// then clients told (`device.changed`).
pub fn app_changed(
    host: &Host,
    change: &str,
    info: Option<&AppInfo>,
    path: &str,
) -> Result<(), HandlerError> {
    let did = host
        .get("device", "id")?
        .filter(ferb_py::value::truthy)
        .map(|v| ferb_py::value::py_str(&v))
        .unwrap_or_default();
    let store = ProfileStore::new(&host.home);
    let fault = |e: std::io::Error| refusal(e.to_string());
    let profile = if did.is_empty() {
        None
    } else {
        store.load(&did).map_err(fault)?
    };
    if let Some(mut profile) = profile {
        let mut apps: Vec<AppInfo> = profile
            .apps
            .into_iter()
            .filter(|x| x.path != path && info.is_none_or(|i| x.bundle_id != i.bundle_id))
            .collect();
        if let Some(i) = info {
            apps.push(i.clone());
        }
        apps.sort_by(|a, b| a.bundle_id.cmp(&b.bundle_id));
        profile.apps = apps;
        let named = match info {
            Some(i) => i.bundle_id.clone(),
            None => Path::new(path)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
        };
        store
            .save(&profile, &format!("app {change}: {named}"))
            .map_err(fault)?;
    }
    host.publish(
        "device.changed",
        json!({"change": change, "path": path, "bundle_id": info.map(|i| i.bundle_id.clone())}),
    );
    Ok(())
}

/// desktop.py `_app_window` (D26, D29): the gateway's first look at a Mac app a worker started on: one background
/// picture of its window, or None (no Mac driver, or nothing to take under mac.snapshot's image rules).
pub async fn app_window(
    s: &dyn Seams,
    app: &str,
    scopes: &BTreeSet<String>,
) -> Result<Option<Value>, String> {
    match s.mac() {
        None => Ok(None),
        Some(mac) => mac.app_window(app, scopes).await,
    }
}

/// desktop.py `_permissions`: the daemon's own macOS permissions (the process that acts). Can't tell: both true
/// (plan validation still guards).
pub fn permissions(s: &dyn Seams) -> Map<String, Value> {
    let read = || -> Result<(bool, bool), String> {
        Ok((
            s.permission("accessibility")? == "granted",
            s.permission("screen")? == "granted",
        ))
    };
    let (ax, screen) = read().unwrap_or((true, true));
    let mut m = Map::new();
    m.insert("accessibility".into(), Value::Bool(ax));
    m.insert("screen".into(), Value::Bool(screen));
    m
}

/// desktop.py `_running_apps`: (bundle id, name, pid) of the apps running now; none when they can't be read.
pub fn running_apps(s: &dyn Seams) -> Vec<(String, String, i64)> {
    s.running_apps().unwrap_or_default()
}

/// The window server and process facts the front-window capture reads (`screenshot`, `front_window`,
/// `accessibility`, `browsers`, `tools._sources`, the image redactor): native, so the composition supplies them.
pub trait NativeFront: Send + Sync {
    fn windows(&self) -> Vec<Window>;
    fn app_of(&self, pid: i64) -> Option<(String, String)>;
    fn parent(&self, pid: i64) -> Option<i64>;
    fn main_frame(&self, pid: i64) -> Option<Rect>;
    fn is_browser(&self, bundle: &str) -> bool;
    fn read_sources(&self, bundle: &str) -> Vec<(String, String)>;
    /// `ImageRedactor(redactor)`: on-device image redaction.
    fn redact_image(&self, raw: &[u8]) -> Result<Vec<u8>, String>;
    /// The owner's exclusion list (`self._exclusions()`): an app by bundle id or name, a site by URL.
    fn excluded(&self, bundle: &str, name: &str, url: Option<&str>) -> bool;
    /// `evidence.put_async(request_id, None, "front_window", image=…, data=…, origin=…)`. Err: the error's name.
    fn keep<'a>(
        &'a self,
        request_id: &'a str,
        image: &'a [u8],
        facts: &'a Dict,
        origin: &'a str,
    ) -> BoxFuture<'a, Result<String, String>>;
}

/// desktop.py `_front_window`'s `fw.Sources`: the native reads plus the daemon's own parts (FERB's processes, the
/// screenshot switch, the evidence store under this request, the front tab through Automation).
pub struct DaemonSources<'a> {
    pub host: &'a Host,
    pub seams: &'a dyn Seams,
    pub native: &'a dyn NativeFront,
    pub mac: Arc<dyn MacDriver>,
    pub automation: &'a Automation,
    pub request_id: &'a str,
    pub client: Option<i64>,
    pub daemon_pid: i64,
}

impl Sources for DaemonSources<'_> {
    fn windows(&self) -> Vec<Window> {
        self.native.windows()
    }
    fn app_of(&self, pid: i64) -> Option<(String, String)> {
        self.native.app_of(pid)
    }
    fn own(&self, pid: i64, bundle: &str) -> bool {
        fw::ferb_process(pid, bundle, self.daemon_pid, &|p| self.native.parent(p))
    }
    fn excluded(&self, bundle: &str, name: &str, url: Option<&str>) -> bool {
        self.native.excluded(bundle, name, url)
    }
    fn tab(
        &self,
        bundle: &str,
        w: &Window,
        after_shot: bool,
    ) -> Result<Option<(String, String)>, TabLookup> {
        self.automation
            .front_tab(self.host, self.seams, bundle, w, after_shot)
    }
    fn shoot<'b>(&'b self, w: &'b Window) -> BoxFuture<'b, Result<fw::Shot, ShotError>> {
        Box::pin(async move {
            // the owner's `[mac] snapshot_image` switch
            if !self.mac.snapshot_images() {
                return Err(ShotError::Refused(
                    "screenshots are switched off ([mac] snapshot_image)".into(),
                ));
            }
            match self
                .mac
                .window_image(&format!("pid:{}", w.pid), &w.id.to_string())
                .await
            {
                Ok(shot) => Ok(shot),
                Err(
                    CaptureError::NoTarget(t)
                    | CaptureError::Withheld(t)
                    | CaptureError::Refused(t),
                ) => Err(ShotError::Refused(t)),
                Err(CaptureError::Failed(name)) => Err(ShotError::Failed(name)),
            }
        })
    }
    fn redact_image(&self, raw: &[u8]) -> Result<Vec<u8>, String> {
        self.native.redact_image(raw)
    }
    fn redact(&self) -> &dyn Redact {
        self.host.redactor.as_ref()
    }
    fn keep<'b>(
        &'b self,
        image: &'b [u8],
        facts: &'b Dict,
        origin: &'b str,
    ) -> BoxFuture<'b, Result<String, String>> {
        self.native.keep(self.request_id, image, facts, origin)
    }
    fn client(&self) -> Option<i64> {
        self.client
    }
    fn parent(&self, pid: i64) -> Option<i64> {
        self.native.parent(pid)
    }
    fn main_frame(&self, pid: i64) -> Option<Rect> {
        self.native.main_frame(pid)
    }
    fn is_browser(&self, bundle: &str) -> bool {
        self.native.is_browser(bundle)
    }
    fn read_sources(&self, bundle: &str) -> Vec<(String, String)> {
        self.native.read_sources(bundle)
    }
}

/// Journals `screen.capture_stalled` when the capture is dropped before it finished (the planner stopped waiting:
/// which step held it, DMN-520).
struct Stalled<'a> {
    host: &'a Host,
    request_id: &'a str,
    stage: &'a Mutex<Vec<String>>,
    started: f64,
    done: bool,
}

impl Drop for Stalled<'_> {
    fn drop(&mut self) {
        if self.done {
            return;
        }
        let stage = self
            .stage
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .last()
            .cloned()
            .unwrap_or_else(|| "start".into());
        let seconds = ((self.host.clock.monotonic() - self.started) * 100.0).round() / 100.0;
        let _ = self.host.journal(
            "screen.capture_stalled",
            json!({"request_id": self.request_id, "stage": stage, "seconds": seconds}),
            None,
        );
    }
}

/// desktop.py `_front_window` (L12): the owner's front window as this message arrived: the topmost window that isn't
/// FERB's (nor, while in front, that of the app hosting the CLI that sent it), its redacted screenshot kept as
/// evidence under the request. None: no Mac driver or evidence store (`sources` None), sensing paused, or nothing but
/// FERB in front. Dropping the future before it ends journals the step it was on.
pub async fn front_window(
    host: &Host,
    sources: Option<&DaemonSources<'_>>,
    sense_paused: bool,
) -> Option<FrontWindow> {
    let src = sources?;
    if sense_paused {
        return None;
    }
    let stage = Mutex::new(Vec::new());
    let mut guard = Stalled {
        host,
        request_id: src.request_id,
        stage: &stage,
        started: host.clock.monotonic(),
        done: false,
    };
    let out = fw::capture(src, &stage).await;
    guard.done = true;
    out
}
