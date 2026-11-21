//! The disturbance sentinel in the composition (main.py:204 `DisturbanceSentinel(DesktopProbe(), ...)`, DMN-5): the
//! desktop is sampled right before and after a tool call that can change the owner's screen (its capability declares
//! `on_screen`), never around read-only tools and never on a timer, plus a baseline when a task tree starts.
//!
//! [`MacDesktop`] is Python's `DesktopProbe`: the focused app (AX's system-wide `AXFocusedApplication`, its pid, the
//! running app's bundle id and name), its focused window (an identity: `CFHash` of the AX element), whether that
//! window is full screen, and a Space epoch: one more each time the window server's active Space differs from the
//! last read (HAND-648; NSWorkspace's notification never reached the daemon). [`Unread`] stands in when the Mac's own parts are off: every field unavailable.
//! The record is desktop.py `_record_disturbance`: a span under the task's own, then `disturbance.observed`.

use std::sync::Arc;

use ferb_daemon::areas_b::disturbance::{DesktopRead, DisturbanceSentinel, Seams};
use serde_json::{json, Map, Value};

use super::tools::Parts;
use crate::basics::lock;

/// What a sample says when the Mac's own parts are off (a test, a stripped run): nothing could be read.
pub struct Unread;

impl DesktopRead for Unread {
    fn read(&self) -> Result<Map<String, Value>, String> {
        let why = "the Mac's own parts are off in this run";
        let mut unavailable = Map::new();
        for k in [
            "active_space_epoch",
            "frontmost_app",
            "key_window",
            "fullscreen",
        ] {
            unavailable.insert(k.into(), json!(why));
        }
        let mut out = Map::new();
        out.insert("unavailable".into(), Value::Object(unavailable));
        Ok(out)
    }
}

/// The sentinel over this composition's parts: the record writes a span and publishes `disturbance.observed`; the
/// owner's pause of sensing stops it.
pub fn sentinel(p: &Parts, read: Arc<dyn DesktopRead>) -> Arc<DisturbanceSentinel> {
    let parts = p.clone();
    let record: ferb_daemon::areas_b::disturbance::Record =
        Arc::new(move |task_id: &str, kind: &str, data: Value| record(&parts, task_id, kind, data));
    let mut seams = Seams::new(read, record);
    let clock = p.clock();
    seams.now = Arc::new(move || clock.now());
    if let Some(o) = p.owner().cloned() {
        // main.py:206-208: the owner-input tap's facts
        seams.owner_input = Arc::new(move || Ok(o.input_facts()));
    }
    if let Some(shells) = p.shells() {
        // desktop.py:133-139 `_running_processes` (main.py:208): FERB-started processes running now, commands
        // redacted; facts only (DMN-501)
        let r = p.redactor();
        seams.processes = Arc::new(move || {
            Ok(shells
                .list()
                .into_iter()
                .filter(|x| x.running)
                .map(|x| {
                    json!({"id": x.id, "task": x.owner_task, "command": r.text(&x.command),
                           "background": x.long_lived})
                })
                .collect())
        });
    }
    let marker = p.home().join("PAUSED");
    // `sense.paused` (the menu bar's Pause FERB): the sense manager's marker in FERB's home
    seams.paused = Arc::new(move || marker.exists());
    Arc::new(DisturbanceSentinel::new(seams))
}

/// desktop.py `_record_disturbance`: persisted before broadcast; tool arguments stay in their own action span.
fn record(p: &Parts, task_id: &str, kind: &str, data: Value) {
    let Some(id) = fact_span(p, task_id, kind, "observe desktop", &data) else {
        return;
    };
    if kind == "disturbance.change" || kind == "disturbance.error" {
        let mut out = Map::new();
        out.insert("task_id".into(), json!(task_id));
        out.insert("span_id".into(), json!(id));
        if let Value::Object(m) = data {
            out.extend(m);
        }
        p.events().publish("disturbance.observed", out);
    }
}

/// One finished span of a task's record (`kind`, under the task's own span while it runs) holding a fact; its id.
/// The desktop's records, and what FERB told the owner about a task (HAND-546: `owner.notice`).
pub(crate) fn fact_span(
    p: &Parts,
    task_id: &str,
    kind: &str,
    operation: &str,
    data: &Value,
) -> Option<String> {
    let parent = p.task(task_id).map(|t| t.span_id);
    let shared = p.db();
    let db = lock(&shared);
    let r = p.redactor();
    let spans = ferb_store::spans::SpanStore::new(
        &db,
        Box::new(ferb_gateway::gateway::StoreTime(p.clock())),
        Box::new(ferb_gateway::gateway::StoreIds(p.ids())),
        Box::new(move |v: &Value| r.value(v)),
        Box::new(|kind: &str| ferb_policy::rules::REVERSIBLE.contains(&kind)),
    );
    let mut span = spans
        .start(
            kind,
            &json!({ "operation": operation }),
            Some(task_id),
            parent.as_deref(),
            None,
        )
        .ok()?;
    let _ = spans.finish(
        &mut span,
        ferb_store::spans::Finish {
            output: Some(data),
            error: None,
            cost_usd: None,
            origin: None,
            summary: None,
        },
    );
    Some(span.id)
}

/// The sentinel behind ferb-gateway's call hooks (`call_hooks.py` `screen_before` / `screen_after`).
pub struct GatewaySentinel(pub Arc<DisturbanceSentinel>);

impl ferb_gateway::call_hooks::Sentinel for GatewaySentinel {
    fn call_started(&self, task: &str, span: &str, name: &str) {
        self.0.call_started(task, span, name)
    }
    fn before_action<'a>(
        &'a self,
        task: &'a str,
        span: &'a str,
        name: &'a str,
        arguments: &'a Map<String, Value>,
        on_screen: bool,
    ) -> ferb_core::interfaces::BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            self.0
                .before_action(task, span, name, arguments, on_screen)
                .await;
            Ok(())
        })
    }
    fn call_ended(&self, span: &str) {
        self.0.call_ended(span)
    }
    fn after_action<'a>(
        &'a self,
        span: &'a str,
    ) -> ferb_core::interfaces::BoxFuture<'a, Result<Map<String, Value>, String>> {
        Box::pin(async move { Ok(self.0.after_action(span).await) })
    }
}

#[cfg(target_os = "macos")]
pub use mac::MacDesktop;

#[cfg(target_os = "macos")]
mod mac {
    use std::ffi::{c_void, CString};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Mutex, OnceLock};

    use ferb_daemon::areas_b::disturbance::DesktopRead;
    use ferb_mac_hands::ax::{AxSource, AxValue};
    use ferb_mac_hands::native::ax::NativeAx;
    use objc2_app_kit::NSRunningApplication;
    use serde_json::{json, Map, Value};

    type MainConnection = unsafe extern "C" fn() -> i32;
    type ActiveSpace = unsafe extern "C" fn(i32) -> u64;

    /// HAND-648: the window server's id of the Space the owner is on now (SkyLight's `SLSGetActiveSpace` on this
    /// process's connection, read only, as FERB.app reads it); None when it can't be read. NSWorkspace's Space
    /// notification never reached the daemon (every trace of 2026-11-21 kept epoch 0 through dozens of Space changes
    /// the unified log shows), so the Space is read at each sample instead.
    fn active_space() -> Option<u64> {
        static FNS: OnceLock<Option<(usize, usize)>> = OnceLock::new();
        let (main, active) = (*FNS.get_or_init(|| {
            let path =
                CString::new("/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight")
                    .ok()?;
            // SAFETY: dlopen with a valid C string; the handle is kept for the process's life (never dlclosed).
            let lib = unsafe { libc::dlopen(path.as_ptr(), libc::RTLD_LAZY) };
            if lib.is_null() {
                return None;
            }
            let sym = |name: &str| -> Option<usize> {
                let c = CString::new(name).ok()?;
                // SAFETY: a valid handle and C string.
                let p: *mut c_void = unsafe { libc::dlsym(lib, c.as_ptr()) };
                (!p.is_null()).then_some(p as usize)
            };
            Some((sym("SLSMainConnectionID")?, sym("SLSGetActiveSpace")?))
        }))?;
        // SAFETY: both symbols are SkyLight's exported functions with these C signatures (as FERB.app calls them).
        let space = unsafe {
            let main: MainConnection = std::mem::transmute::<usize, MainConnection>(main);
            let active: ActiveSpace = std::mem::transmute::<usize, ActiveSpace>(active);
            active(main())
        };
        (space != 0).then_some(space)
    }

    /// disturbance.py `DesktopProbe`.
    #[derive(Default)]
    pub struct MacDesktop {
        /// HAND-648: counts each Space change seen between two reads (the window server's active Space id).
        epoch: AtomicU64,
        space: Mutex<Option<u64>>,
        ax: NativeAx,
    }

    impl MacDesktop {
        fn attr(&self, el: &ferb_mac_hands::ax::El, name: &str) -> Result<AxValue, String> {
            match self.ax.raw_attr(el, name) {
                (0, v) => Ok(v),
                (err, _) => Err(format!("{name}: AX error {err}")),
            }
        }
    }

    impl DesktopRead for MacDesktop {
        fn start(&self) {
            *self.space.lock().unwrap_or_else(|e| e.into_inner()) = active_space();
        }

        fn read(&self) -> Result<Map<String, Value>, String> {
            let mut unavailable = Map::new();
            let mut out = Map::new();
            // `{"unavailable": ...}` first, as Python's dict is built
            out.insert("unavailable".into(), Value::Null);
            match active_space() {
                Some(now) => {
                    let mut seen = self.space.lock().unwrap_or_else(|e| e.into_inner());
                    if seen.is_some_and(|before| before != now) {
                        self.epoch.fetch_add(1, Ordering::SeqCst);
                    }
                    *seen = Some(now);
                    out.insert(
                        "active_space_epoch".into(),
                        json!(self.epoch.load(Ordering::SeqCst)),
                    );
                }
                None => {
                    unavailable.insert(
                        "active_space_epoch".into(),
                        json!("the window server's active Space couldn't be read"),
                    );
                }
            }
            let front = (|| -> Result<(), String> {
                let system = self.ax.system();
                let app = match self.attr(&system, "AXFocusedApplication")? {
                    AxValue::El(e) => e,
                    _ => return Err("AXFocusedApplication: AX error -25212".into()),
                };
                let pid = self
                    .ax
                    .pid_of(&app)
                    .ok_or_else(|| "AX pid error -25202".to_string())?;
                let (bundle, name) = objc2::rc::autoreleasepool(|_| {
                    match NSRunningApplication::runningApplicationWithProcessIdentifier(pid) {
                        Some(a) => (
                            json!(a
                                .bundleIdentifier()
                                .map(|s| s.to_string())
                                .unwrap_or_default()),
                            json!(a.localizedName().map(|s| s.to_string()).unwrap_or_default()),
                        ),
                        None => (Value::Null, Value::Null),
                    }
                });
                out.insert(
                    "frontmost_app".into(),
                    json!({"pid": pid, "bundle_id": bundle, "name": name}),
                );
                match self.attr(&app, "AXFocusedWindow") {
                    Ok(AxValue::El(window)) => {
                        out.insert(
                            "key_window".into(),
                            json!({"pid": pid, "ax_identity": self.ax.hash(&window)}),
                        );
                        match self.attr(&window, "AXFullScreen") {
                            Ok(AxValue::Bool(b)) => {
                                out.insert("fullscreen".into(), json!(b));
                            }
                            Ok(AxValue::Int(n)) => {
                                out.insert("fullscreen".into(), json!(n != 0));
                            }
                            Ok(_) => {
                                out.insert("fullscreen".into(), json!(false));
                            }
                            Err(e) => {
                                unavailable.insert("fullscreen".into(), json!(e));
                            }
                        }
                    }
                    Ok(_) => {
                        out.insert("key_window".into(), Value::Null);
                        out.insert("fullscreen".into(), Value::Null);
                    }
                    Err(e) => {
                        unavailable.insert("key_window".into(), json!(e));
                        unavailable
                            .insert("fullscreen".into(), json!("No readable focused window"));
                    }
                }
                Ok(())
            })();
            if let Err(e) = front {
                for k in ["frontmost_app", "key_window", "fullscreen"] {
                    unavailable.insert(k.into(), json!(e));
                }
            }
            out.insert("unavailable".into(), Value::Object(unavailable));
            Ok(out)
        }
    }
}
