//! The desktop area's seams (`ferb/daemon/areas/desktop.py`, a mixin of Python's daemon) over this program's parts:
//! what a task lets go of at its end (`_task_end`), what a pause stops (`_task_paused`, ORCH-312) and a resume
//! restarts (`_task_resumed`), a tree's end (`_tree_finished`), and the native reads a browser tab's capture needs
//! (`_native_page_secrets`, `_native_place`, `_native_window_capture`). ferb-daemon's `areas_b::desktop` holds the
//! behavior; this file only fills its [`Seams`] and [`MacDriver`] with the real parts.

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};

use ferb_core::interfaces::BoxFuture;
use ferb_daemon::areas_b::core::Host;
use ferb_daemon::areas_b::desktop::{
    self, CaptureError, MacDriver, Probe, Rect, Seams, ShellProc, Spans, TaskRef, WindowImage,
};
use ferb_daemon::areas_b::environment::TabDriver;
use ferb_sense::front_window::Window;
use ferb_sense::watchers::browser_tabs::ScriptedWindow;
use serde_json::{json, Map, Value};

use super::tools::{mac::off_thread, Parts};

/// The desktop area's seams over the daemon's parts.
pub struct DesktopSeams {
    pub parts: Parts,
    pub tokens: Arc<ferb_rpc::RunTokens>,
}

impl DesktopSeams {
    /// The host the desktop functions journal and publish through.
    pub fn host(&self) -> Host {
        Host {
            home: self.parts.home().clone(),
            config: self.parts.config(),
            redactor: self.parts.redactor(),
            journal: self.parts.journal(),
            bus: self.parts.events(),
            clock: self.parts.zoned(),
        }
    }
}

/// `self.spans`: a span started and finished by the desktop area (its observations), held open in between.
struct SpansOver {
    parts: Parts,
    open: Mutex<HashMap<String, ferb_store::spans::Span>>,
}

impl SpansOver {
    fn with<T>(&self, f: impl FnOnce(&ferb_store::spans::SpanStore<'_>) -> T) -> T {
        let shared = self.parts.db();
        let db = crate::basics::lock(&shared);
        let r = self.parts.redactor();
        let store = ferb_store::spans::SpanStore::new(
            &db,
            Box::new(ferb_gateway::gateway::StoreTime(self.parts.clock())),
            Box::new(ferb_gateway::gateway::StoreIds(self.parts.ids())),
            Box::new(move |v: &Value| r.value(v)),
            Box::new(|kind: &str| ferb_policy::rules::REVERSIBLE.contains(&kind)),
        );
        f(&store)
    }
}

impl Spans for SpansOver {
    fn start(&self, kind: &str, task_id: &str, parent_id: Option<&str>, input: Value) -> String {
        let Ok(span) = self.with(|s| s.start(kind, &input, Some(task_id), parent_id, None)) else {
            return String::new();
        };
        let id = span.id.clone();
        crate::basics::lock(&self.open).insert(id.clone(), span);
        id
    }
    fn finish(&self, span_id: &str, output: Value) {
        let Some(mut span) = crate::basics::lock(&self.open).remove(span_id) else {
            return;
        };
        self.with(|s| {
            let _ = s.finish(
                &mut span,
                ferb_store::spans::Finish {
                    output: Some(&output),
                    error: None,
                    cost_usd: None,
                    origin: None,
                    summary: None,
                },
            );
        });
    }
}

/// `permissions.automation_check(bundle, ask=False)`'s future: its answer once the shared check returns.
struct Check(Arc<Mutex<Option<ferb_mac_hands::permissions::Status>>>);

impl Probe for Check {
    fn done(&self) -> bool {
        crate::basics::lock(&self.0).is_some()
    }
    fn granted(&self) -> bool {
        *crate::basics::lock(&self.0) == Some(ferb_mac_hands::permissions::Status::Granted)
    }
}

fn hand_name(e: &ferb_mac_hands::error::HandError) -> String {
    let d = format!("{e:?}");
    d.split(['(', ' ', '{'])
        .next()
        .unwrap_or("Error")
        .to_string()
}

/// The Mac driver as the desktop area reads it (`self.mac`).
pub struct MacSide {
    tools: Arc<ferb_mac_hands::tools::MacTools>,
}

#[cfg(target_os = "macos")]
fn window_now(id: i64) -> Option<Window> {
    let w = ferb_mac_hands::shots::window(&ferb_mac_hands::native::windows::NativeWindows, id)?;
    Some(Window {
        id: w.id,
        pid: i64::from(w.pid),
        bounds: w.bounds,
        title: w.title,
        layer: w.layer,
        alpha: w.alpha,
    })
}

#[cfg(not(target_os = "macos"))]
fn window_now(_id: i64) -> Option<Window> {
    None
}

impl MacDriver for MacSide {
    fn page_secrets<'a>(
        &'a self,
        pid: i64,
        bounds: Rect,
        title: &'a str,
    ) -> BoxFuture<'a, Result<Value, String>> {
        let (d, title) = (self.tools.driver.clone(), title.to_string());
        Box::pin(async move {
            let got = off_thread(move || {
                Box::pin(async move { d.page_secrets(pid as i32, bounds, &title).await })
            })
            .await
            .unwrap_or_else(|| {
                Err(ferb_mac_hands::error::HandError::Other(
                    "the read's thread ended".into(),
                ))
            });
            match got {
                Ok(None) => Ok(Value::Null),
                Ok(Some((rects, area, whole))) => Ok(json!([
                    rects
                        .iter()
                        .map(|r| json!([r.0, r.1, r.2, r.3]))
                        .collect::<Vec<_>>(),
                    [area.0, area.1, area.2, area.3],
                    whole
                ])),
                Err(e) => Err(hand_name(&e)),
            }
        })
    }
    fn place_on_web_area<'a>(
        &'a self,
        rect: Rect,
        viewport: Rect,
        pid: i64,
        bounds: Rect,
        title: &'a str,
    ) -> BoxFuture<'a, Result<Option<Rect>, String>> {
        let (d, title) = (self.tools.driver.clone(), title.to_string());
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                d.place_on_web_area(rect, viewport, pid as i32, bounds, &title)
            })
            .await
            .map_err(|e| e.to_string())
        })
    }
    fn snapshot_images(&self) -> bool {
        (self.tools.driver.snapshot_images)()
    }
    fn window_image<'a>(
        &'a self,
        target: &'a str,
        window: &'a str,
    ) -> BoxFuture<'a, Result<WindowImage, CaptureError>> {
        let (tools, target, window) = (self.tools.clone(), target.to_string(), window.to_string());
        Box::pin(async move {
            let got = off_thread(move || {
                Box::pin(async move {
                    let paint = tools.paint.clone();
                    tools
                        .driver
                        .window_image(&target, Some(&window), &*paint)
                        .await
                        .map(|(_, shot, jpeg)| (jpeg, shot.masking, shot.tab))
                })
            })
            .await;
            use ferb_mac_hands::error::HandError;
            match got {
                Some(Ok(w)) => Ok(w),
                Some(Err(HandError::NoTarget(m))) => Err(CaptureError::NoTarget(m)),
                Some(Err(HandError::Withheld(m))) => Err(CaptureError::Withheld(m)),
                Some(Err(HandError::Refused(m))) => Err(CaptureError::Refused(m)),
                Some(Err(e)) => Err(CaptureError::Failed(hand_name(&e))),
                None => Err(CaptureError::Failed("RuntimeError".into())),
            }
        })
    }
    fn normal_windows(&self) -> Result<Vec<Window>, String> {
        Ok(super::front::normal_windows())
    }
    fn window(&self, id: i64) -> Result<Option<Window>, String> {
        Ok(window_now(id))
    }
    fn has_launched(&self) -> bool {
        self.tools.driver.launched.is_some()
    }
    fn release_launched<'a>(
        &'a self,
        task_id: &'a str,
    ) -> BoxFuture<'a, Result<Vec<Value>, String>> {
        let launched = self.tools.driver.launched.clone();
        Box::pin(async move {
            match launched {
                Some(l) => Ok(l.release(task_id).await),
                None => Ok(Vec::new()),
            }
        })
    }
    /// The gateway's first look at a Mac app runs through the call hooks' `AppWindows` (tools/mac.rs), the one path;
    /// nothing in this program calls the desktop area's `app_window`.
    fn app_window<'a>(
        &'a self,
        _app: &'a str,
        _scopes: &'a BTreeSet<String>,
    ) -> BoxFuture<'a, Result<Option<Value>, String>> {
        crate::off::off("desktop.app_window", Box::pin(async { Ok(None) }))
    }
}

impl Seams for DesktopSeams {
    fn task(&self, task_id: &str) -> Option<TaskRef> {
        self.parts.task(task_id).map(|r| TaskRef {
            span_id: Some(r.span_id),
            tree_id: r.tree_id,
        })
    }
    fn spans(&self) -> Option<Arc<dyn Spans>> {
        Some(Arc::new(SpansOver {
            parts: self.parts.clone(),
            open: Mutex::new(HashMap::new()),
        }))
    }
    fn mac(&self) -> Option<Arc<dyn MacDriver>> {
        self.parts.mac().map(|m| {
            Arc::new(MacSide {
                tools: m.tools.clone(),
            }) as Arc<dyn MacDriver>
        })
    }
    fn hearing_end_task<'a>(&'a self, task_id: &'a str) -> BoxFuture<'a, Vec<Value>> {
        let hearing = self.parts.mac().map(|m| m.hearing.clone());
        Box::pin(async move {
            match hearing {
                Some(h) => h.end_task(task_id).await,
                None => Vec::new(),
            }
        })
    }
    fn recordings_end_task<'a>(&'a self, task_id: &'a str) -> BoxFuture<'a, Vec<Value>> {
        let recordings = self.parts.mac().map(|m| m.recordings.clone());
        Box::pin(async move {
            match recordings {
                Some(r) => r
                    .end_task(task_id)
                    .await
                    .into_iter()
                    .map(Value::Object)
                    .collect(),
                None => Vec::new(),
            }
        })
    }
    fn listening_owners(&self) -> Vec<Option<String>> {
        self.parts
            .mac()
            .map(|m| {
                crate::basics::lock(&m.hearing.listening)
                    .values()
                    .map(|l| Some(l.owner.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }
    fn recording_owners(&self) -> Vec<Option<String>> {
        self.parts
            .mac()
            .map(|m| m.recordings.owners().into_iter().map(Some).collect())
            .unwrap_or_default()
    }
    fn identity_browser(&self) -> Option<Arc<dyn TabDriver>> {
        self.parts
            .ferb_driver()
            .map(|d| Arc::new(super::env_seams::Tabs(d)) as Arc<dyn TabDriver>)
    }
    fn router(&self) -> Option<Arc<dyn TabDriver>> {
        self.parts
            .owner_driver()
            .map(|d| Arc::new(super::env_seams::Tabs(d)) as Arc<dyn TabDriver>)
    }
    fn forget_downloads(&self, task_id: &str) {
        if let Some(d) = self.parts.browsers().map(|b| b.downloads.clone()) {
            d.forget_task(task_id);
        }
    }
    fn has_shells(&self) -> bool {
        self.parts.shells().is_some()
    }
    fn shells_end_task<'a>(&'a self, task_id: &'a str) -> BoxFuture<'a, Result<(), String>> {
        let shells = self.parts.shells();
        Box::pin(async move {
            if let Some(s) = shells {
                s.end_task(task_id).await;
            }
            Ok(())
        })
    }
    fn shells_stop_running<'a>(
        &'a self,
        mine: &'a (dyn Fn(Option<&str>) -> bool + Send + Sync),
    ) -> BoxFuture<'a, Result<usize, String>> {
        let shells = self.parts.shells();
        Box::pin(async move {
            match shells {
                Some(s) => Ok(s.stop_running(|owner: &str| mine(Some(owner))).await),
                None => Ok(0),
            }
        })
    }
    fn shell_processes(&self) -> Vec<ShellProc> {
        self.parts
            .shells()
            .map(|s| {
                s.list()
                    .into_iter()
                    .map(|p| ShellProc {
                        id: p.id,
                        owner_task: Some(p.owner_task).filter(|o| !o.is_empty()),
                        command: p.command,
                        long_lived: p.long_lived,
                        running: p.running,
                        started: p.started,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
    fn has_schedules(&self) -> bool {
        self.parts.group_a().is_some()
    }
    fn schedule_state(&self, schedule_id: &str) -> Option<String> {
        let a = self.parts.group_a()?;
        let sid = schedule_id.to_string();
        a.call_blocking(move |st| async move {
            st.schedules().get(&sid).ok().flatten().map(|s| s.state)
        })
        .ok()
        .flatten()
    }
    fn schedule_change(&self, schedule_id: &str, action: &str, with_task: &str) {
        let Some(a) = self.parts.group_a() else {
            return;
        };
        let (sid, action, with) = (
            schedule_id.to_string(),
            action.to_string(),
            with_task.to_string(),
        );
        let _ = a.spawn(move |st| async move {
            let _ = ferb_daemon::areas_a::schedules::schedule_change(&st, &sid, &action, &with);
        });
    }
    fn journal_latest_kind(&self, kinds: &[&str], key: &str, value: &str) -> Option<String> {
        self.parts.journal().latest_kind(kinds, key, value)
    }
    fn journal_of_kind(&self, kind: &str, key: &str, value: &str) -> Vec<Value> {
        self.parts
            .journal()
            .of_kind(kind, key, value)
            .into_iter()
            .map(|(_, data)| data)
            .collect()
    }
    fn revoke_tree(&self, tree_id: &str) {
        self.tokens.revoke_tree(tree_id);
    }
    fn disturbance_finish<'a>(&'a self, tree_id: &'a str) -> BoxFuture<'a, Result<(), String>> {
        let sentinel = self.parts.sentinel();
        Box::pin(async move {
            if let Some(s) = sentinel {
                s.finish(tree_id).await;
            }
            Ok(())
        })
    }
    fn changes_forget(&self, tree_id: &str) -> Option<Vec<String>> {
        Some(self.parts.changes().forget(tree_id))
    }
    fn has_uploads(&self) -> bool {
        self.parts.browsers().is_some()
    }
    /// browsers.py:84 `_clean_uploads(uploads.forget_tree, tree, about={"task_id": tree})`: in a worker thread; what
    /// couldn't be removed is journaled (paths, never contents).
    fn clean_uploads<'a>(&'a self, tree_id: &'a str) -> BoxFuture<'a, ()> {
        let uploads = self.parts.browsers().map(|b| b.uploads.clone());
        let journal = self.parts.journal();
        let tree = tree_id.to_string();
        Box::pin(async move {
            let Some(u) = uploads else { return };
            let t = tree.clone();
            let done = tokio::task::spawn_blocking(move || u.forget_tree(&t)).await;
            if let Ok(Err(e)) = done {
                journal.note(
                    "browser.uploads_cleanup_failed",
                    json!({"task_id": tree, "error": format!("{}: {}", e.class, e.message),
                           "left": e.left}),
                );
            }
        })
    }
    fn automation_check(&self, bundle_id: &str) -> Arc<dyn Probe> {
        let answer: Arc<Mutex<Option<ferb_mac_hands::permissions::Status>>> =
            Arc::new(Mutex::new(None));
        if !self.parts.native() {
            *crate::basics::lock(&answer) = Some(ferb_mac_hands::permissions::Status::Unknown);
            return Arc::new(Check(answer));
        }
        let (bundle, out) = (bundle_id.to_string(), answer.clone());
        let _ = std::thread::Builder::new()
            .name("ferb-automation".into())
            .spawn(move || {
                let status = ferb_mac_hands::native::permissions::automation(&bundle, false);
                *crate::basics::lock(&out) = Some(status);
            });
        Arc::new(Check(answer))
    }
    fn applescript_windows(&self, bundle_id: &str) -> Result<Option<Vec<ScriptedWindow>>, String> {
        if !self.parts.native() {
            return Ok(None);
        }
        ferb_sense::mac::system::browser_windows(bundle_id)
    }
    fn permission(&self, name: &str) -> Result<String, String> {
        if !self.parts.native() {
            return Err("the Mac's parts are off".into());
        }
        use ferb_mac_hands::native::permissions as p;
        let status = match name {
            "accessibility" => p::accessibility(),
            "screen" | "screen_recording" => p::screen_recording(),
            other => return Err(format!("no such permission: {other}")),
        };
        Ok(status.value().to_string())
    }
    fn running_apps(&self) -> Result<Vec<(String, String, i64)>, String> {
        if !self.parts.native() {
            return Ok(Vec::new());
        }
        Ok(ferb_mac_hands::native::apps::running()
            .into_iter()
            .map(|a| (a.bundle_id, a.name, i64::from(a.pid)))
            .collect())
    }
}

/// `drv.native_secrets` / `native_capture` / `native_place` (browsers.py:265-267): the browser drivers' native reads,
/// through the desktop area (its journaling included).
pub struct BrowserNative(pub Arc<DesktopSeams>);

impl ferb_browser::ops::Native for BrowserNative {
    fn secrets(
        &self,
        pid: i32,
        bounds: ferb_browser::mask::Rect,
        title: &str,
    ) -> BoxFuture<
        '_,
        Option<(
            Vec<ferb_browser::mask::Rect>,
            ferb_browser::mask::Rect,
            bool,
        )>,
    > {
        let title = title.to_string();
        Box::pin(async move {
            let host = self.0.host();
            let got = desktop::native_page_secrets(
                &host,
                self.0.as_ref(),
                i64::from(pid),
                bounds,
                &title,
            )
            .await
            .ok()
            .flatten()?;
            let rect = |v: &Value| -> Option<ferb_browser::mask::Rect> {
                let a = v.as_array()?;
                Some((
                    a.first()?.as_f64()?,
                    a.get(1)?.as_f64()?,
                    a.get(2)?.as_f64()?,
                    a.get(3)?.as_f64()?,
                ))
            };
            let parts = got.as_array()?;
            let rects: Vec<_> = parts.first()?.as_array()?.iter().filter_map(rect).collect();
            Some((rects, rect(parts.get(1)?)?, parts.get(2)?.as_bool()?))
        })
    }
    fn capture(
        &self,
        pid: i32,
        bounds: ferb_browser::mask::Rect,
        title: &str,
    ) -> BoxFuture<'_, (Option<Vec<u8>>, String)> {
        let title = title.to_string();
        Box::pin(async move {
            let host = self.0.host();
            desktop::native_window_capture(&host, self.0.as_ref(), i64::from(pid), bounds, &title)
                .await
                .unwrap_or_else(|e| (None, e.message))
        })
    }
    fn place(
        &self,
        rect: ferb_browser::mask::Rect,
        viewport: ferb_browser::mask::Rect,
        pid: i32,
        bounds: ferb_browser::mask::Rect,
        title: &str,
    ) -> BoxFuture<'_, Option<ferb_browser::mask::Rect>> {
        let title = title.to_string();
        Box::pin(async move {
            desktop::native_place(
                self.0.as_ref(),
                rect,
                viewport,
                i64::from(pid),
                bounds,
                &title,
            )
            .await
            .ok()
            .flatten()
        })
    }
}

/// The task events the desktop area answers (compose.py:244): a task paused stops its commands, listening and
/// recordings (ORCH-312); a scheduled run resumed resumes the schedule its pause paused.
pub fn on_task_events(seams: Arc<DesktopSeams>, events: &crate::basics::Events) {
    events.events.on(
        Some(&["task.paused", "task.resumed"]),
        Arc::new(move |topic: &str, data: Map<String, Value>| {
            let (s, topic) = (seams.clone(), topic.to_string());
            if let Ok(rt) = tokio::runtime::Handle::try_current() {
                rt.spawn(async move {
                    if topic == "task.paused" {
                        let host = s.host();
                        if let Err(e) = desktop::task_paused(&host, s.as_ref(), &data).await {
                            s.parts
                                .journal()
                                .note("task.pause_failed", json!({"error": e.message}));
                        }
                    } else {
                        desktop::task_resumed(s.as_ref(), &data).await;
                    }
                });
            }
        }),
    );
}
