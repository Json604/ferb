//! TCC grant facts and the setup rows (ferb/device/permissions.py). Never prompts during a capability check; the
//! requests are explicit setup calls. The native reads (AXIsProcessTrusted, CGPreflightScreenCaptureAccess,
//! IOHIDCheckAccess, AEDeterminePermissionToAutomateTarget, the Full Disk Access probe) are `native::permissions`.
//!
//! An Automation check is one shared native call per (target, mode): a waiter that gives up stops waiting, never
//! the call, and a later caller shares it (permissions.py:181-221, [`AutomationChecks`]).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::{json, Map, Value};
use tokio::sync::watch;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Status {
    Granted,
    Denied,
    NotDetermined,
    TargetNotRunning,
    Unknown,
}

impl Status {
    pub fn value(self) -> &'static str {
        match self {
            Status::Granted => "granted",
            Status::Denied => "denied",
            Status::NotDetermined => "not_determined",
            Status::TargetNotRunning => "target_not_running",
            Status::Unknown => "unknown",
        }
    }
}

/// AEDeterminePermissionToAutomateTarget's answer (permissions.py:160).
pub fn ae_status(code: i32) -> Status {
    match code {
        0 => Status::Granted,
        -1743 => Status::Denied,
        -1744 => Status::NotDetermined,
        -600 => Status::TargetNotRunning,
        _ => Status::Unknown,
    }
}

/// IOHIDCheckAccess's answer (permissions.py:106).
pub fn hid_status(code: u32) -> Status {
    match code {
        0 => Status::Granted,
        1 => Status::Denied,
        2 => Status::NotDetermined,
        _ => Status::Unknown,
    }
}

/// permissions.py:77: the plain API's answer, unless a real read of what it protects disproves it. `probe`: Some(true)
/// confirmed, Some(false) proven denied, None inconclusive (or it failed).
pub fn confirmed(api_says: bool, probe: impl FnOnce() -> Option<bool>) -> bool {
    api_says && probe() != Some(false)
}

/// The Full Disk Access probe's outcome for one file (permissions.py:120).
pub enum Probe {
    Read,
    PermissionDenied,
    NotFound,
    OtherError,
}

pub fn full_disk_access(probes: impl IntoIterator<Item = Probe>) -> Status {
    for p in probes {
        match p {
            Probe::Read => return Status::Granted,
            Probe::PermissionDenied => return Status::Denied,
            Probe::NotFound => continue,
            Probe::OtherError => return Status::Unknown,
        }
    }
    Status::Unknown
}

pub const PANES: [(&str, &str); 8] = [
    ("accessibility", "Privacy_Accessibility"),
    ("screen_recording", "Privacy_ScreenCapture"),
    ("input_monitoring", "Privacy_ListenEvent"),
    ("full_disk_access", "Privacy_AllFiles"),
    ("automation", "Privacy_Automation"),
    ("microphone", "Privacy_Microphone"),
    ("speech_recognition", "Privacy_SpeechRecognition"),
    ("clipboard", "Privacy_Pasteboard"),
];

pub fn pane_url(permission: &str) -> Option<String> {
    PANES.iter().find(|p| p.0 == permission).map(|p| {
        format!(
            "x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?{}",
            p.1
        )
    })
}

pub fn notifications_pane_url(bundle_id: &str) -> String {
    format!("x-apple.systempreferences:com.apple.Notifications-Settings.extension?id={bundle_id}")
}

pub fn app_panes(bundle_id: &str) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert(
        "notifications".into(),
        json!(notifications_pane_url(bundle_id)),
    );
    m.insert("microphone".into(), json!(pane_url("microphone")));
    m.insert(
        "speech_recognition".into(),
        json!(pane_url("speech_recognition")),
    );
    m
}

pub const DAEMON_PERMISSIONS: [&str; 4] = [
    "accessibility",
    "screen_recording",
    "full_disk_access",
    "input_monitoring",
];
pub const SETUP_VERSION: i64 = 27;
pub const REQUESTABLE: [&str; 4] = [
    "accessibility",
    "screen_recording",
    "input_monitoring",
    "clipboard",
];
pub const RESTART_TO_CHECK: [&str; 3] =
    ["screen_recording", "input_monitoring", "full_disk_access"];
pub const FRESH_READ: [&str; 2] = ["screen_recording", "input_monitoring"];
pub const FDA_WAITS: &str = "FERB's shell sandbox and network proxy must be working first";

/// permissions.py:278: `grant_probe`'s printed answer, known keys with string values only.
pub fn fresh_answer(out: &[u8]) -> Map<String, Value> {
    let Ok(text) = std::str::from_utf8(out) else {
        return Map::new();
    };
    let Ok(Value::Object(answer)) = ferb_py::json::loads(text) else {
        return Map::new();
    };
    answer
        .into_iter()
        .filter(|(k, v)| FRESH_READ.contains(&k.as_str()) && v.is_string())
        .collect()
}

/// permissions.py:289.
pub fn restart_needed(grants: &Map<String, Value>, fresh: &Map<String, Value>) -> Vec<String> {
    let on = Value::from("granted");
    let mut out: Vec<String> = FRESH_READ
        .iter()
        .filter(|p| fresh.get(**p) == Some(&on) && grants.get(**p) != Some(&on))
        .map(|p| p.to_string())
        .collect();
    out.sort();
    out
}

fn truthy(v: Option<&Value>) -> bool {
    v.is_some_and(ferb_py::value::truthy)
}

/// permissions.py:297 (CQ5).
pub fn fda_ready(containment: &Map<String, Value>) -> bool {
    truthy(containment.get("profile_ok"))
        && truthy(containment.get("shell"))
        && truthy(containment.get("proxy"))
        && containment
            .get("proxy_alive")
            .map(ferb_py::value::truthy)
            .unwrap_or(true)
}

/// permissions.py:305: the daemon's own grants as setup rows.
pub fn rows(
    grants: &Map<String, Value>,
    containment: &Map<String, Value>,
    restart: &[String],
) -> Vec<Value> {
    let ready = fda_ready(containment);
    DAEMON_PERMISSIONS
        .iter()
        .map(|p| {
            json!({"id": p, "status": grants.get(*p).cloned().unwrap_or(json!("unknown")), "process": "daemon",
                   "required": true, "pane": pane_url(p), "requestable": REQUESTABLE.contains(p),
                   "restart_to_check": RESTART_TO_CHECK.contains(p), "restart_needed": restart.iter().any(|r| r == p),
                   "blocked": if *p != "full_disk_access" || ready { "" } else { FDA_WAITS }})
        })
        .collect()
}

/// permissions.py:321: the clipboard row, from the pasteboard's accessBehavior (never its contents).
pub fn clipboard_row(state: Option<&str>) -> Value {
    let status = match state.unwrap_or("") {
        "allow" | "unrestricted" => "granted",
        "default" => "not_determined",
        "ask" | "deny" => "denied",
        _ => "unknown",
    };
    json!({"id": "clipboard", "status": status, "process": "daemon", "required": true, "pane": pane_url("clipboard"),
           "requestable": state == Some("default"), "restart_to_check": false, "blocked": ""})
}

/// permissions.py:333: (id, status) of every row not granted.
pub fn missing(rows: &[Value]) -> Vec<(String, String)> {
    rows.iter()
        .filter(|r| r["status"] != json!("granted"))
        .map(|r| {
            (
                ferb_py::value::py_str(&r["id"]),
                ferb_py::value::py_str(&r["status"]),
            )
        })
        .collect()
}

/// permissions.py:339.
pub fn report_rows(report: &Value) -> Vec<Value> {
    let perms = report
        .get("permissions")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let mut grants = Map::new();
    for p in DAEMON_PERMISSIONS {
        grants.insert(
            p.into(),
            json!(perms
                .get(p)
                .map(ferb_py::value::py_str)
                .unwrap_or_else(|| "unknown".into())),
        );
    }
    let containment = match report.get("containment") {
        Some(Value::Object(m)) if !m.is_empty() => m.clone(),
        _ => Map::new(),
    };
    let mut out = rows(&grants, &containment, &[]);
    out.push(clipboard_row(
        report.get("clipboard").and_then(Value::as_str),
    ));
    out
}

// ---------- the shared Automation check ----------

/// The native check: (bundle id, ask) -> Status. Blocking; run on its own thread.
pub type AutomationFn = Arc<dyn Fn(&str, bool) -> Status + Send + Sync>;

/// permissions.py:181-221: one in-flight native check per (target, mode). A caller that stops waiting (dropped
/// future, timeout) never stops the call; later callers share it until it answers.
#[derive(Clone)]
pub struct AutomationChecks {
    native: AutomationFn,
    jobs: Arc<Mutex<HashMap<(String, bool), watch::Receiver<Option<Status>>>>>,
}

impl AutomationChecks {
    pub fn new(native: AutomationFn) -> Self {
        AutomationChecks {
            native,
            jobs: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// The answer; dropping this future leaves the native call running for the others.
    pub async fn check(&self, bundle_id: &str, ask: bool) -> Status {
        let key = (bundle_id.to_string(), ask);
        let mut rx = {
            let mut jobs = self.jobs.lock().expect("lock");
            match jobs.get(&key) {
                Some(rx) => rx.clone(),
                None => {
                    let (tx, rx) = watch::channel(None);
                    jobs.insert(key.clone(), rx.clone());
                    let native = self.native.clone();
                    let jobs_ref = self.jobs.clone();
                    let k = key.clone();
                    std::thread::Builder::new()
                        .name("ferb-automation-permission".into())
                        .spawn(move || {
                            let status = native(&k.0, k.1);
                            jobs_ref.lock().expect("lock").remove(&k);
                            let _ = tx.send(Some(status));
                        })
                        .expect("a thread");
                    rx
                }
            }
        };
        loop {
            if let Some(s) = *rx.borrow() {
                return s;
            }
            if rx.changed().await.is_err() {
                return rx.borrow().unwrap_or(Status::Unknown);
            }
        }
    }

    /// How many native checks are in flight (tests).
    pub fn in_flight(&self) -> usize {
        self.jobs.lock().expect("lock").len()
    }
}
