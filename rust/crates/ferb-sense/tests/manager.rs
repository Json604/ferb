//! The watcher manager and the watchers' run loops (`ferb/sense/manager.py`, `watchers/*.py`), from the Python tests'
//! scenarios (tests/test_watcher_permissions.py, test_owner_input_tap.py, test_sense.py, test_typed_text_capture.py),
//! on tokio's paused clock: no real waiting, no native tap, no Accessibility.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use ferb_core::interfaces::BoxFuture;
use ferb_sense::capture::Field;
use ferb_sense::manager::{Emit, Hooks, OwnerInputTap, Sense, Stop, Watcher, INPUT_MONITORING_OFF};
use ferb_sense::observation::{Dict, NoRedact, Observation};
use ferb_sense::watchers::accessibility::AxElement;
use ferb_sense::watchers::base::Focused;
use ferb_sense::watchers::notifications::{NotificationReader, NotificationWatcher};
use ferb_sense::watchers::owner_input::{KeyEvent, OwnerInput, TapBackend, TapSink};
use ferb_sense::watchers::text_input::{
    focused_field, FieldReads, FocusedElement, Terminals, TextInputWatcher,
};
use serde_json::{json, Value};

fn home() -> std::path::PathBuf {
    tempfile::tempdir().unwrap().keep()
}

struct Flaky {
    runs: AtomicUsize,
}

impl Watcher for Flaky {
    fn source(&self) -> &str {
        "notification"
    }
    fn run(self: Arc<Self>, _emit: Emit, stop: Stop) -> BoxFuture<'static, Result<(), String>> {
        Box::pin(async move {
            if self.runs.fetch_add(1, Ordering::SeqCst) == 0 {
                return Err("RuntimeError: unable to open database file".into());
            }
            stop.wait().await;
            Ok(())
        })
    }
}

// decision: SENSE-405
#[tokio::test(start_paused = true)]
async fn a_failed_watcher_tries_again_instead_of_stopping() {
    let flaky = Arc::new(Flaky {
        runs: AtomicUsize::new(0),
    });
    let failures: Arc<Mutex<Vec<String>>> = Arc::default();
    let published: Arc<Mutex<Vec<Value>>> = Arc::default();
    let (f, p) = (failures.clone(), published.clone());
    let hooks = Hooks {
        on_fail: Arc::new(move |_, why| f.lock().unwrap().push(why.to_string())),
        publish: Arc::new(move |t, v| p.lock().unwrap().push(json!([t, v]))),
        ..Hooks::default()
    };
    let mut s = Sense::new(
        home(),
        vec![flaky.clone()],
        Arc::new(|_| {}),
        None,
        None,
        hooks,
    );
    s.retry_s = 0.01;
    s.start().await;
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert_eq!(s.running(), vec!["notification".to_string()]);
    s.stop().await;
    assert_eq!(flaky.runs.load(Ordering::SeqCst), 2);
    assert_eq!(
        *failures.lock().unwrap(),
        vec!["RuntimeError: unable to open database file"]
    );
    assert_eq!(
        *published.lock().unwrap(),
        vec![
            json!(["sense.watcher_failed", {"source": "notification", "error": "RuntimeError: unable to open database file"}])
        ]
    );
}

struct Tap {
    granted: AtomicBool,
    running: AtomicBool,
}

impl OwnerInputTap for Tap {
    fn allowed(&self) -> Result<bool, String> {
        Ok(self.granted.load(Ordering::SeqCst))
    }
    fn running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }
    fn start(&self) -> Result<bool, String> {
        let g = self.granted.load(Ordering::SeqCst);
        self.running.store(g, Ordering::SeqCst);
        Ok(g)
    }
    fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
    }
}

// decision: SENSE-406
#[tokio::test(start_paused = true)]
async fn the_input_tap_starts_once_input_monitoring_is_granted_without_a_restart() {
    let tap = Arc::new(Tap {
        granted: AtomicBool::new(false),
        running: AtomicBool::new(false),
    });
    let changes: Arc<Mutex<Vec<String>>> = Arc::default();
    let c = changes.clone();
    let hooks = Hooks {
        on_tap: Arc::new(move |why| c.lock().unwrap().push(why.to_string())),
        ..Hooks::default()
    };
    let mut s = Sense::new(
        home(),
        vec![],
        Arc::new(|_| {}),
        Some(tap.clone()),
        None,
        hooks,
    );
    s.tap_recheck_s = 30.0;
    s.start().await;
    assert!(!tap.running() && s.tap_down() == INPUT_MONITORING_OFF);
    tap.granted.store(true, Ordering::SeqCst); // the owner grants it while FERB runs
    for _ in 0..50 {
        tokio::time::sleep(std::time::Duration::from_secs(31)).await;
        if s.tap_down().is_empty() {
            break;
        }
    }
    assert!(tap.running() && s.tap_down().is_empty());
    tap.granted.store(false, Ordering::SeqCst); // taken away: the tap is stopped
    tokio::time::sleep(std::time::Duration::from_secs(31)).await;
    for _ in 0..50 {
        if !s.tap_down().is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
    s.stop().await;
    assert!(!tap.running());
    assert_eq!(
        *changes.lock().unwrap(),
        vec![
            INPUT_MONITORING_OFF.to_string(),
            String::new(),
            INPUT_MONITORING_OFF.to_string()
        ]
    );
}

struct Counting {
    stops: AtomicUsize,
}

impl Watcher for Counting {
    fn source(&self) -> &str {
        "app_focus"
    }
    fn run(self: Arc<Self>, emit: Emit, stop: Stop) -> BoxFuture<'static, Result<(), String>> {
        Box::pin(async move {
            emit(Observation::new("app_focus", Dict::new()));
            stop.wait().await;
            self.stops.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
    }
}

// decision: SENSE-403
#[tokio::test(start_paused = true)]
async fn pause_stops_every_watcher_and_the_tap_and_survives_a_restart() {
    let dir = home();
    let w = Arc::new(Counting {
        stops: AtomicUsize::new(0),
    });
    let tap = Arc::new(Tap {
        granted: AtomicBool::new(true),
        running: AtomicBool::new(false),
    });
    let paused: Arc<Mutex<Vec<bool>>> = Arc::default();
    let p = paused.clone();
    let hooks = Hooks {
        on_pause: Arc::new(move |b| p.lock().unwrap().push(b)),
        ..Hooks::default()
    };
    let seen = Arc::new(AtomicUsize::new(0));
    let sn = seen.clone();
    let s = Sense::new(
        dir.clone(),
        vec![w.clone()],
        Arc::new(move |_| {
            sn.fetch_add(1, Ordering::SeqCst);
        }),
        Some(tap.clone()),
        None,
        hooks.clone(),
    );
    s.start().await;
    tokio::task::yield_now().await;
    assert!(tap.running());
    s.pause().await.unwrap();
    assert!(
        s.paused()
            && !tap.running()
            && w.stops.load(Ordering::SeqCst) == 1
            && s.running().is_empty()
    );
    // a new manager over the same home (a restart) stays paused and starts nothing
    let again = Sense::new(
        dir.clone(),
        vec![w.clone()],
        Arc::new(|_| {}),
        Some(tap.clone()),
        None,
        Hooks::default(),
    );
    again.start().await;
    assert!(again.paused() && again.running().is_empty() && !tap.running());
    s.resume().await.unwrap();
    tokio::task::yield_now().await;
    assert!(!s.paused() && tap.running());
    s.stop().await;
    assert_eq!(*paused.lock().unwrap(), vec![true, false]);
    assert_eq!(seen.load(Ordering::SeqCst), 2);
}

struct Reader(AtomicUsize);
impl NotificationReader for Reader {
    fn read(&self) -> Result<Vec<Observation>, String> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(vec![])
    }
}

// decision: SENSE-405
#[tokio::test(start_paused = true)]
async fn the_notification_watcher_reads_nothing_until_full_disk_access() {
    let reader = Arc::new(Reader(AtomicUsize::new(0)));
    let granted = Arc::new(AtomicBool::new(false));
    let g = granted.clone();
    let mut w = NotificationWatcher::new(
        reader.clone(),
        Arc::new(|_| false),
        Arc::new(|_| {}),
        Arc::new(|o| o),
        Arc::new(move || g.load(Ordering::SeqCst)),
    );
    w.interval_s = 0.01;
    w.recheck_s = 0.01;
    let w = Arc::new(w);
    let stop = Stop::new();
    let task = tokio::spawn(w.clone().run(Arc::new(|_| {}), stop.clone()));
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let before = reader.0.load(Ordering::SeqCst);
    granted.store(true, Ordering::SeqCst);
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    stop.set();
    task.await.unwrap().unwrap();
    assert_eq!(before, 0);
    assert!(reader.0.load(Ordering::SeqCst) > 0);
}

// ---------- text capture: every check before the key's text is read (rule 7) ----------

struct Key {
    flags: u64,
    text: &'static str,
    read: Arc<AtomicUsize>,
}

impl KeyEvent for Key {
    fn flags(&self) -> u64 {
        self.flags
    }
    fn text(&self) -> String {
        self.read.fetch_add(1, Ordering::SeqCst);
        self.text.to_string()
    }
}

#[derive(Clone)]
struct El {
    attrs: serde_json::Map<String, Value>,
    value_reads: Arc<AtomicUsize>,
}

impl AxElement for El {
    fn attr(&self, name: &str) -> Option<Value> {
        if name == "AXValue" {
            self.value_reads.fetch_add(1, Ordering::SeqCst);
        }
        self.attrs.get(name).cloned()
    }
    fn window(&self) -> Option<Box<dyn AxElement + '_>> {
        None
    }
    fn hash_id(&self) -> String {
        "h".into()
    }
}

impl FocusedElement for El {
    fn surface_facts(&self) -> Vec<Dict> {
        vec![]
    }
}

struct Reads {
    secure: AtomicBool,
    el: Mutex<El>,
}

impl FieldReads for Reads {
    fn secure_input(&self) -> bool {
        self.secure.load(Ordering::SeqCst)
    }
    fn focused_element(&self, _pid: i64) -> Option<Box<dyn FocusedElement>> {
        Some(Box::new(self.el.lock().unwrap().clone()))
    }
}

struct NoTerminals;
impl Terminals for NoTerminals {
    fn app(&self, app: &Focused) -> bool {
        app.bundle_id == "com.apple.Terminal"
    }
    fn field(&self, _app: &Focused, _facts: &[Dict]) -> bool {
        false
    }
}

struct NoTap;
impl TapBackend for NoTap {
    fn create(&self, _sink: Arc<dyn TapSink>) -> bool {
        true
    }
    fn destroy(&self) {}
}

fn element(role: &str, extra: Value) -> El {
    let mut attrs = serde_json::Map::new();
    attrs.insert("AXRole".into(), json!(role));
    attrs.insert("AXIdentifier".into(), json!("f1"));
    attrs.insert("AXTitle".into(), json!("To"));
    attrs.insert("AXValue".into(), json!("draft"));
    for (k, v) in extra.as_object().unwrap() {
        attrs.insert(k.clone(), v.clone());
    }
    El {
        attrs,
        value_reads: Arc::default(),
    }
}

// decision: SENSE-401
#[test]
fn a_secure_or_password_field_is_never_read() {
    let notes = Some(Focused {
        pid: 1,
        bundle_id: "com.apple.Notes".into(),
        name: "Notes".into(),
    });
    let none = |_: &str, _: &str| false;
    let check = |el: El| {
        let reads = Reads {
            secure: AtomicBool::new(false),
            el: Mutex::new(el.clone()),
        };
        (
            focused_field(notes.clone(), &none, &NoTerminals, &reads),
            el.value_reads.load(Ordering::SeqCst),
        )
    };
    let (f, reads) = check(element("AXTextField", json!({})));
    assert_eq!(f.map(|f: Field| f.field_id), Some("f1".into()));
    assert_eq!(reads, 1);
    for el in [
        element("AXSecureTextField", json!({})),
        element("AXTextField", json!({"AXSubrole": "AXSecureTextField"})),
        element("AXTextField", json!({"AXPlaceholderValue": "Password"})),
        element("AXStaticText", json!({})),
    ] {
        let (f, reads) = check(el);
        assert!(f.is_none());
        assert_eq!(
            reads, 0,
            "the value of a field that may not be captured is never read"
        );
    }
    let terminal = Some(Focused {
        pid: 2,
        bundle_id: "com.apple.Terminal".into(),
        name: "Terminal".into(),
    });
    let reads = Reads {
        secure: AtomicBool::new(false),
        el: Mutex::new(element("AXTextField", json!({}))),
    };
    assert!(focused_field(terminal, &none, &NoTerminals, &reads).is_none());
    let own = Some(Focused {
        pid: 3,
        bundle_id: "com.ferb.FERB".into(),
        name: "FERB".into(),
    });
    assert!(focused_field(own, &none, &NoTerminals, &reads).is_none());
}

// decision: SENSE-401
#[tokio::test(start_paused = true)]
async fn keys_under_secure_input_or_in_a_secure_field_are_never_extracted() {
    let oi = OwnerInput::new(|| 0.0, || 0.0, || true, Box::new(NoTap));
    assert!(oi.start_tap());
    let reads = Arc::new(Reads {
        secure: AtomicBool::new(false),
        el: Mutex::new(element("AXTextField", json!({}))),
    });
    let emitted: Arc<Mutex<Vec<Value>>> = Arc::default();
    let e = emitted.clone();
    let mono = Arc::new(Mutex::new(0.0f64));
    let m = mono.clone();
    let w = Arc::new(TextInputWatcher::new(
        oi.clone(),
        Arc::new(NoRedact),
        Arc::new(|_: &str, _: &str| false),
        Arc::new(NoTerminals),
        reads.clone(),
        Arc::new(|| {
            Some(Focused {
                pid: 1,
                bundle_id: "com.apple.Notes".into(),
                name: "Notes".into(),
            })
        }),
        Arc::new(move || *m.lock().unwrap()),
    ));
    let stop = Stop::new();
    let task = tokio::spawn(w.run(
        Arc::new(move |o: Observation| e.lock().unwrap().push(json!(o.data["text"]))),
        stop.clone(),
    ));
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    let extracted = Arc::new(AtomicUsize::new(0));
    // the tap's thread hands each key-down to capture through the sink
    let sink = ferb_sense::watchers::owner_input::sink(oi.clone());
    let press = |text: &'static str, flags: u64| {
        let key = Key {
            flags,
            text,
            read: extracted.clone(),
        };
        sink.event(false, Some(&key));
    };
    press("hi", 0);
    assert_eq!(extracted.load(Ordering::SeqCst), 1);
    press("c", 1 << 20); // Command-C: a shortcut, not text
    assert_eq!(extracted.load(Ordering::SeqCst), 1);
    reads.secure.store(true, Ordering::SeqCst);
    press("pw", 0); // Secure Input: nothing held is kept, nothing read
    assert_eq!(extracted.load(Ordering::SeqCst), 1);
    reads.secure.store(false, Ordering::SeqCst);
    *reads.el.lock().unwrap() = element("AXSecureTextField", json!({}));
    press("pw", 0);
    assert_eq!(
        extracted.load(Ordering::SeqCst),
        1,
        "a secure field's key is never extracted"
    );
    *reads.el.lock().unwrap() = element("AXTextField", json!({}));
    press("ok", 0);
    *mono.lock().unwrap() = 5.0;
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    stop.set();
    task.await.unwrap().unwrap();
    assert_eq!(*emitted.lock().unwrap(), vec![json!("ok")]);
}
