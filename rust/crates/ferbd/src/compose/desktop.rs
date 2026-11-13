//! The disturbance sentinel in the composition (main.py:204 `DisturbanceSentinel(DesktopProbe(), ...)`, DMN-5): the
//! desktop is sampled right before and after a tool call that can change the owner's screen (its capability declares
//! `on_screen`), never around read-only tools and never on a timer, plus a baseline when a task tree starts.
//!
//! [`MacDesktop`] is Python's `DesktopProbe`: the focused app (AX's system-wide `AXFocusedApplication`, its pid, the
//! running app's bundle id and name), its focused window (an identity: `CFHash` of the AX element), whether that
//! window is full screen, and a Space epoch counted by NSWorkspace's active-Space notification (delivered on the main
//! thread, which `mainloop` turns). [`Unread`] stands in when the Mac's own parts are off: every field unavailable.
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
    let parent = p.task(task_id).map(|t| t.span_id);
    let id = {
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
        let Ok(mut span) = spans.start(
            kind,
            &json!({"operation": "observe desktop"}),
            Some(task_id),
            parent.as_deref(),
            None,
        ) else {
            return;
        };
        let _ = spans.finish(
            &mut span,
            ferb_store::spans::Finish {
                output: Some(&data),
                error: None,
                cost_usd: None,
                origin: None,
                summary: None,
            },
        );
        span.id
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
    use std::ptr::NonNull;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};

    use block2::RcBlock;
    use ferb_daemon::areas_b::disturbance::DesktopRead;
    use ferb_mac_hands::ax::{AxSource, AxValue};
    use ferb_mac_hands::native::ax::NativeAx;
    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, NSObjectProtocol, ProtocolObject};
    use objc2_app_kit::{NSRunningApplication, NSWorkspace};
    use objc2_foundation::{NSNotification, NSString};
    use serde_json::{json, Map, Value};

    type Token = Retained<ProtocolObject<dyn NSObjectProtocol>>;

    /// A token held across threads: it is only ever handed back to the notification center that made it.
    struct Held(Token);
    // SAFETY: the observer token is an opaque object passed only to `removeObserver:`, which is thread-safe.
    unsafe impl Send for Held {}

    /// disturbance.py `DesktopProbe`.
    #[derive(Default)]
    pub struct MacDesktop {
        epoch: Arc<AtomicU64>,
        observer: Mutex<Option<Held>>,
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
            let epoch = self.epoch.clone();
            let token = objc2::rc::autoreleasepool(|_| {
                let center = NSWorkspace::sharedWorkspace().notificationCenter();
                let block = RcBlock::new(move |_note: NonNull<NSNotification>| {
                    epoch.fetch_add(1, Ordering::SeqCst);
                });
                let name = NSString::from_str("NSWorkspaceActiveSpaceDidChangeNotification");
                // SAFETY: a nil object and queue: the block runs on the posting (main) thread; it lives as long as
                // the observer, which `close` removes.
                unsafe {
                    center.addObserverForName_object_queue_usingBlock(
                        Some(&name),
                        None,
                        None,
                        &block,
                    )
                }
            });
            *self.observer.lock().unwrap_or_else(|e| e.into_inner()) = Some(Held(token));
        }

        fn close(&self) {
            if let Some(Held(token)) = self
                .observer
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take()
            {
                objc2::rc::autoreleasepool(|_| {
                    let center = NSWorkspace::sharedWorkspace().notificationCenter();
                    let obj: &AnyObject = token.as_ref();
                    // SAFETY: the token was returned by this center's addObserverForName.
                    unsafe { center.removeObserver(obj) };
                });
            }
        }

        fn read(&self) -> Result<Map<String, Value>, String> {
            let mut unavailable = Map::new();
            let mut out = Map::new();
            // `{"unavailable": ...}` first, as Python's dict is built
            out.insert("unavailable".into(), Value::Null);
            if self
                .observer
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .is_some()
            {
                out.insert(
                    "active_space_epoch".into(),
                    json!(self.epoch.load(Ordering::SeqCst)),
                );
            } else {
                unavailable.insert(
                    "active_space_epoch".into(),
                    json!("Space observer not registered"),
                );
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
