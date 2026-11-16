//! This Mac's own facts (T-1130; SENSE-420, the owner's: city level always, the exact location only when a task needs
//! it, asked at that moment; a ninth permission).
//!
//! - The always-present line (SENSE-421): device, local date, weekday and time with the time zone and its UTC offset,
//!   and the city when FERB.app has the Location permission and a fix; else "place unknown (time zone only)". It is
//!   the first line of the live facts every planner turn and worker run gets.
//! - `device.facts` (SENSE-422), a read-only lookup on the shared gateway path: the line's facts plus battery, Low
//!   Power Mode, network, screen lock, displays, free disk, boot and wake, audio output, appearance, locale and
//!   languages, macOS and the model (`ferb_mac_hands::native::device`), and the Wi-Fi name FERB.app reported. With
//!   `exact_location`, the owner is asked first (GATE-613: once per task tree, the answer kept for the tree), then
//!   FERB.app reads one precise fix and hands it back.
//! - `device.report` (FERB.app's own, app_only): the Location permission's state, the city FERB.app reverse-geocoded
//!   and the Wi-Fi name, pushed at launch and on change; or the answer to one exact-location request.
//!
//! Who holds the permission: FERB.app (APP-925). The daemon is a background agent with no window; macOS gives no
//! documented way for it to show the Location prompt itself, and FERB.app already holds the owner's other app grants.
//! The place, the Wi-Fi name and a fix live only in this process's memory: nothing here is stored, and after a restart
//! the place is unknown until FERB.app reports it again.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ferb_core::device::{line, DeviceLine, Place};
use ferb_core::interfaces::BoxFuture;
use ferb_core::types::{ActionSpec, ToolContext, ToolResult};
use ferb_gateway::tool::{Args, Tool, ToolOutput};
use ferb_rpc::{HandlerError, Params, Server, Session};
use serde_json::{json, Map, Value};

use super::tools::{Catalogued, Parts};
use crate::basics::lock;

pub const NAME: &str = "device.facts";
/// The bus event FERB.app answers with one precise fix (`device.report` with its `request_id`).
pub const WANTED: &str = "device.location_wanted";
/// The question's capability (its card's kind).
pub const QUESTION: &str = "device.location";
/// How long a precise fix may take FERB.app (a technical bound: CoreLocation's first precise fix is usually a few
/// seconds).
pub const FIX_WAIT: Duration = Duration::from_secs(30);
/// The longest Wi-Fi name kept (802.11 allows 32 bytes).
pub const WIFI_CHARS: usize = 64;

/// The exact-location question (GATE-613) in the owner's words (APP-926: plain and about FERB): the card's first
/// line, then what it means.
pub const ASK_SUMMARY: &str = "Use this Mac's exact location for this task?";
pub const ASK_DETAIL: &str =
    "FERB will use where this Mac is right now. Until now it only knew your city. Your \
answer holds for the rest of this task.";
/// The card's buttons (FERB's own Yes/No, as a forget's).
pub const ASK_LABELS: [&str; 2] = ["Use it", "Not now"];

/// What an observe-only call (the planner's lookups, the check) gets for the exact location: it can't ask.
pub const OBSERVING: &str =
    "this call only looks (a planner lookup or the check), so it can't ask the owner: ask \
for the exact location in a task step";

/// What the tool says when FERB has no Location permission.
pub const NO_PERMISSION: &str =
    "FERB doesn't have the Location permission, so it can't read where this Mac is: the \
owner turns it on in FERB.app → Settings → Permissions → Location";

/// The Location permission's state as FERB.app reports it.
fn shown_permission(status: &str) -> &'static str {
    match status {
        "granted" => "on",
        "denied" | "restricted" => "off",
        "not_determined" => "not asked yet",
        _ => "unknown (FERB.app hasn't reported it)",
    }
}

/// What FERB.app last reported (memory only).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Reported {
    /// "granted", "denied", "restricted", "not_determined"; "" before FERB.app's first report.
    pub location: String,
    pub place: Option<Place>,
    pub wifi: Option<String>,
}

/// One precise fix FERB.app read on request.
#[derive(Clone, Debug, PartialEq)]
pub struct Fix {
    pub latitude: f64,
    pub longitude: f64,
    pub accuracy_m: f64,
}

type Waiter = tokio::sync::oneshot::Sender<Result<Fix, String>>;

/// The owner's question, as the composition asks it (the gateway's question store); `Some(true)`: Yes.
pub struct Ask {
    pub task_id: String,
    pub request_id: String,
}
pub type AskFn = Arc<dyn Fn(Ask) -> BoxFuture<'static, Option<bool>> + Send + Sync>;
/// Asks FERB.app for one precise fix under this id (the bus event).
pub type WantFn = Arc<dyn Fn(&str) + Send + Sync>;

/// This daemon's device state.
#[derive(Default)]
pub struct DeviceState {
    reported: Mutex<Reported>,
    waiting: Mutex<HashMap<String, Waiter>>,
    /// GATE-613: the owner's answer per task tree (its root task), kept while the daemon runs.
    answers: Mutex<BTreeMap<String, bool>>,
}

/// A task tree's root: the task id up to its first dot (a step's id extends its parent's).
pub fn root_of(task_id: &str) -> &str {
    task_id.split('.').next().unwrap_or(task_id)
}

fn finite(v: Option<&Value>) -> Option<f64> {
    v.and_then(Value::as_f64).filter(|x| x.is_finite())
}

impl DeviceState {
    pub fn reported(&self) -> Reported {
        lock(&self.reported).clone()
    }

    /// `device.report`: FERB.app's report, or its answer to one request. The place and Wi-Fi name are kept only while
    /// the permission is on.
    pub fn report(&self, p: &Params) -> Result<Value, String> {
        if let Some(id) = p.get("request_id").and_then(Value::as_str) {
            let answer = match p.get("exact") {
                Some(e) => {
                    let lat = finite(e.get("latitude")).filter(|x| (-90.0..=90.0).contains(x));
                    let lon = finite(e.get("longitude")).filter(|x| (-180.0..=180.0).contains(x));
                    let acc = finite(e.get("accuracy_m")).filter(|x| *x >= 0.0);
                    match (lat, lon, acc) {
                        (Some(latitude), Some(longitude), Some(accuracy_m)) => Ok(Fix {
                            latitude,
                            longitude,
                            accuracy_m,
                        }),
                        _ => {
                            return Err(
                                "exact needs latitude, longitude and accuracy_m in range".into()
                            )
                        }
                    }
                }
                None => Err(p
                    .get("error")
                    .and_then(Value::as_str)
                    .map(|e| e.chars().take(300).collect())
                    .unwrap_or_else(|| "FERB.app couldn't read the location".into())),
            };
            let delivered = match lock(&self.waiting).remove(id) {
                Some(w) => w.send(answer).is_ok(),
                None => false,
            };
            return Ok(json!({"delivered": delivered}));
        }
        let mut r = lock(&self.reported);
        if let Some(status) = p.get("location").and_then(Value::as_str) {
            if !["granted", "denied", "restricted", "not_determined"].contains(&status) {
                return Err(format!("unknown location status {status:?}"));
            }
            r.location = status.to_string();
        }
        if p.contains_key("place") {
            r.place = p.get("place").and_then(Place::from_report);
        }
        if p.contains_key("wifi") {
            r.wifi = p
                .get("wifi")
                .and_then(Value::as_str)
                .map(|w| {
                    ferb_core::device::clean_part(w)
                        .chars()
                        .take(WIFI_CHARS)
                        .collect::<String>()
                })
                .filter(|w| !w.is_empty());
        }
        if r.location != "granted" {
            r.place = None;
            r.wifi = None;
        }
        Ok(json!({"ok": true}))
    }

    /// The exact location for a task (GATE-613): refused to the check and without the permission; else the owner is
    /// asked once per task tree, then FERB.app reads one fix within `wait`. Never coordinates without the Yes.
    pub async fn exact(
        &self,
        ctx: &ToolContext,
        ask: &AskFn,
        want: &WantFn,
        id: String,
        wait: Duration,
    ) -> Value {
        let refused = |why: &str| json!({"given": false, "why": why});
        if ctx.observe_only {
            return refused(OBSERVING);
        }
        if lock(&self.reported).location != "granted" {
            return refused(NO_PERMISSION);
        }
        let root = root_of(&ctx.task_id).to_string();
        let known = lock(&self.answers).get(&root).copied();
        let yes = match known {
            Some(yes) => yes,
            None => {
                let Some(yes) = ask(Ask {
                    task_id: ctx.task_id.clone(),
                    request_id: ctx.request_id.clone(),
                })
                .await
                else {
                    return refused("the question went before the owner answered it");
                };
                lock(&self.answers).insert(root, yes);
                yes
            }
        };
        if !yes {
            return refused("the owner said No to sharing this Mac's exact location in this task");
        }
        let (tx, rx) = tokio::sync::oneshot::channel();
        lock(&self.waiting).insert(id.clone(), tx);
        want(&id);
        let got = tokio::time::timeout(wait, rx).await;
        lock(&self.waiting).remove(&id);
        match got {
            Ok(Ok(Ok(f))) => {
                json!({"given": true, "latitude": f.latitude, "longitude": f.longitude, "accuracy_m": f.accuracy_m.round()})
            }
            Ok(Ok(Err(why))) => refused(&why),
            _ => refused("FERB.app didn't give a location in time (it reads the location; it may not be open)"),
        }
    }
}

/// The device's line for the live facts (SENSE-421).
pub fn facts_line(
    state: &DeviceState,
    name: &str,
    now: &ferb_core::when::DateTime,
    zone: &str,
) -> String {
    let r = state.reported();
    let place = r.place.filter(|_| r.location == "granted");
    line(&DeviceLine {
        label: "This Mac",
        name,
        now,
        zone,
        place: place.as_ref(),
    })
}

/// What the tool reads besides FERB.app's reports.
pub struct Now {
    pub device_id: String,
    pub name: String,
    pub now: ferb_core::when::DateTime,
    pub zone: String,
    pub reading: ferb_mac_hands::device::Reading,
}

/// The tool's content: the device, time and place first, then the Mac's state; facts it couldn't read are left out.
pub fn content(state: &DeviceState, n: &Now) -> Map<String, Value> {
    let r = state.reported();
    let mut m = Map::new();
    m.insert(
        "device".into(),
        json!({"id": n.device_id, "kind": "mac", "name": n.name}),
    );
    m.insert(
        "time".into(),
        json!({"local": ferb_core::when::isoformat(&n.now), "weekday": n.now.format("%A").to_string(),
               "time_zone": n.zone, "utc_offset": ferb_core::device::utc_offset(&n.now)}),
    );
    m.insert(
        "location_permission".into(),
        json!(shown_permission(&r.location)),
    );
    match r.place.filter(|_| r.location == "granted") {
        Some(p) => {
            m.insert("place".into(), p.to_json());
        }
        None => {
            let why = if r.location == "granted" {
                "unknown: FERB.app has no fix yet; the time zone is the only hint"
            } else {
                "unknown: FERB has no Location permission; the time zone is the only hint"
            };
            m.insert("place".into(), json!(why));
        }
    }
    if let Some(w) = r.wifi.filter(|_| r.location == "granted") {
        m.insert("wifi_name".into(), json!(w));
    }
    let offset = *n.now.offset();
    let when = move |t: f64| {
        chrono::DateTime::from_timestamp(t.trunc() as i64, 0)
            .map(|d| ferb_core::when::isoformat(&d.with_timezone(&offset)))
            .unwrap_or_default()
    };
    m.extend(n.reading.facts(&when));
    m
}

/// The tool (after abilities.read, as `catalog_additions.json` says).
pub fn tool(p: &Parts) -> Arc<dyn Tool> {
    let parts = p.clone();
    Arc::new(Catalogued {
        name: NAME,
        describe: Arc::new(|_a| ActionSpec::new("read")),
        run: Arc::new(move |a: &Args, ctx: &ToolContext| {
            let p = parts.clone();
            let exact = a
                .get("exact_location")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            Box::pin(async move {
                let state = p.device();
                let (native, home) = (p.native(), p.user_home().clone());
                let (name, reading) = tokio::task::spawn_blocking(move || read_mac(native, &home))
                    .await
                    .unwrap_or_default();
                let clock = p.zoned();
                let zone = clock.zone();
                let n = Now {
                    device_id: device_id(&p),
                    name,
                    now: zone.local(&clock.now()),
                    zone: zone.key().to_string(),
                    reading,
                };
                let mut m = content(&state, &n);
                if exact {
                    let (ask, want) = (ask_owner(&p), want_fix(&p));
                    let id = p.ids().new_id();
                    m.insert(
                        "exact_location".into(),
                        state.exact(ctx, &ask, &want, id, FIX_WAIT).await,
                    );
                }
                Ok(ToolOutput::from(ToolResult {
                    content: m,
                    ..ToolResult::default()
                }))
            })
        }),
    })
}

/// The computer name and the Mac's state (a stand-in daemon reads nothing: TEST-512).
fn read_mac(native: bool, home: &std::path::Path) -> (String, ferb_mac_hands::device::Reading) {
    if !native {
        return (String::new(), Default::default());
    }
    #[cfg(target_os = "macos")]
    {
        let r = ferb_mac_hands::native::device::read(home);
        (r.name.clone().unwrap_or_default(), r)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = home;
        (String::new(), Default::default())
    }
}

/// The Mac's computer name for the line ("" in a stand-in daemon).
pub fn computer_name(native: bool) -> String {
    #[cfg(target_os = "macos")]
    if native {
        return ferb_mac_hands::native::device::computer_name().unwrap_or_default();
    }
    let _ = native;
    String::new()
}

fn device_id(p: &Parts) -> String {
    match p.config().get("device", "id") {
        Ok(Some(Value::String(s))) => s,
        _ => String::new(),
    }
}

/// GATE-613's question through the gateway's one question store: a task's question, FERB's own Yes/No with its
/// buttons, closed (set aside) if the call ends before an answer.
fn ask_owner(p: &Parts) -> AskFn {
    let gw = p.gateway();
    Arc::new(move |a: Ask| {
        let gw = gw.clone();
        Box::pin(async move {
            let gw = gw?;
            let ap = gw.ask_own(&ferb_gateway::gateway::OwnQuestion {
                capability: QUESTION.into(),
                reason: format!("{ASK_SUMMARY}\n{ASK_DETAIL}"),
                options: vec!["Yes".into(), "No".into()],
                request_id: a.request_id,
                summary: Some(ASK_SUMMARY.into()),
                detail: Some(ASK_DETAIL.into()),
                labels: ASK_LABELS.iter().map(|l| l.to_string()).collect(),
                task_id: a.task_id,
                options_only: true,
                approval_id: None,
                part: String::new(),
                runtime_switch: None,
                from_task: false,
                corrects: false,
                exact: String::new(),
                event: ferb_gateway::gateway::OwnEvent::Publish,
            });
            let close = CloseOnDrop(Some((gw.clone(), ap.id.clone())));
            let answered = gw
                .await_own(&ap.id, &ferb_gateway::gateway::Cancel::new())
                .await
                .ok()
                .flatten();
            drop(close.disarm());
            let ap = answered?;
            Some(
                ap.state == ferb_gateway::questions::State::Granted
                    && ap.choice.as_deref() == Some("Yes"),
            )
        })
    })
}

/// Sets the question aside when the call is dropped mid-wait (a stop).
struct CloseOnDrop(Option<(Arc<ferb_gateway::gateway::Gateway>, String)>);

impl CloseOnDrop {
    fn disarm(mut self) -> Self {
        self.0 = None;
        self
    }
}

impl Drop for CloseOnDrop {
    fn drop(&mut self) {
        if let Some((gw, id)) = self.0.take() {
            gw.close_own(&id);
        }
    }
}

fn want_fix(p: &Parts) -> WantFn {
    let events = p.events();
    Arc::new(move |id: &str| {
        let mut data = Map::new();
        data.insert("request_id".into(), json!(id));
        events.publish(WANTED, data);
    })
}

/// `device.report` (app_only: FERB.app's own).
pub fn register(server: &Server, p: &Parts) {
    let parts = p.clone();
    server.register_unary("device.report", move |_s: Session, params: Params| {
        let state = parts.device();
        async move {
            state
                .report(&params)
                .map(crate::basics::object)
                .map_err(|e| HandlerError::fault("ValueError", e))
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn params(v: Value) -> Params {
        crate::basics::object(v)
    }

    fn at(text: &str) -> ferb_core::when::DateTime {
        chrono::DateTime::parse_from_rfc3339(text).unwrap()
    }

    fn ctx(task: &str, observe_only: bool) -> ToolContext {
        ToolContext {
            task_id: task.into(),
            worker_id: "w".into(),
            request_id: "r1".into(),
            identity: ferb_core::types::Identity::Owner,
            scopes: Default::default(),
            device_id: "dev".into(),
            span_id: None,
            workspace: String::new(),
            owner_roots: vec![],
            untrusted_roots: vec![],
            intent_id: None,
            observe_only,
            probe: false,
            path_closed: None,
            escape: None,
            terminal: None,
            folders: vec![],
        }
    }

    fn granted(state: &DeviceState) {
        state
            .report(&params(json!({"location": "granted",
                "place": {"city": "Pune", "region": "Maharashtra", "country": "India", "country_code": "IN"},
                "wifi": "Home"})))
            .unwrap();
    }

    fn asker(answer: Option<bool>, asked: Arc<AtomicUsize>) -> AskFn {
        Arc::new(move |_a| {
            asked.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move { answer })
        })
    }

    /// FERB.app answers the fix request at once (as its LocationReporter does).
    fn app_answers(state: Arc<DeviceState>, wanted: Arc<AtomicUsize>) -> WantFn {
        Arc::new(move |id: &str| {
            wanted.fetch_add(1, Ordering::SeqCst);
            let got = state
                .report(&params(json!({"request_id": id,
                    "exact": {"latitude": 18.5204, "longitude": 73.8567, "accuracy_m": 12.4}})))
                .unwrap();
            assert_eq!(got["delivered"], true);
        })
    }

    // decision: SENSE-421
    #[test]
    fn the_line_has_the_city_only_while_location_is_on() {
        let state = DeviceState::default();
        let now = at("2026-11-16T10:05:00+05:30");
        assert_eq!(
            facts_line(&state, "Studio", &now, "Asia/Kolkata"),
            "- This Mac “Studio”: Monday 2026-11-16 10:05, time zone Asia/Kolkata (UTC+05:30); place unknown (time \
             zone only)"
        );
        granted(&state);
        assert!(facts_line(&state, "Studio", &now, "Asia/Kolkata")
            .ends_with("place: Pune, Maharashtra, India (city level)"));
        state
            .report(&params(json!({"location": "denied"})))
            .unwrap();
        assert_eq!(state.reported().place, None);
        assert_eq!(state.reported().wifi, None);
        assert!(facts_line(&state, "Studio", &now, "Asia/Kolkata")
            .ends_with("place unknown (time zone only)"));
        assert!(state
            .report(&params(json!({"location": "always"})))
            .is_err());
    }

    // decision: SENSE-422
    #[test]
    fn the_lookup_reports_device_time_place_and_state_and_leaves_unread_facts_out() {
        let state = DeviceState::default();
        let n = Now {
            device_id: "dev-1".into(),
            name: "Studio".into(),
            now: at("2026-11-16T10:05:31+05:30"),
            zone: "Asia/Kolkata".into(),
            reading: ferb_mac_hands::device::Reading {
                battery: Some(Some(ferb_mac_hands::device::Battery {
                    percent: 81,
                    charging: true,
                    power_source: "AC Power".into(),
                })),
                booted_at: Some(1_763_000_000.0),
                ..Default::default()
            },
        };
        let off = Value::Object(content(&state, &n));
        assert_eq!(
            off["location_permission"],
            "unknown (FERB.app hasn't reported it)"
        );
        assert!(off["place"].as_str().unwrap().starts_with("unknown"));
        assert!(off.get("wifi_name").is_none() && off.get("exact_location").is_none());
        granted(&state);
        let on = Value::Object(content(&state, &n));
        assert_eq!(
            on.to_string(),
            r#"{"device":{"id":"dev-1","kind":"mac","name":"Studio"},"time":{"local":"2026-11-16T10:05:31+05:30","weekday":"Monday","time_zone":"Asia/Kolkata","utc_offset":"UTC+05:30"},"location_permission":"on","place":{"city":"Pune","region":"Maharashtra","country":"India","country_code":"IN"},"wifi_name":"Home","battery":{"percent":81,"charging":true,"power_source":"AC Power"},"booted_at":"2025-11-13T07:43:20+05:30"}"#
        );
    }

    // decision: GATE-613
    #[tokio::test]
    async fn exact_location_asks_once_per_task_tree_and_a_no_gives_nothing() {
        let state = Arc::new(DeviceState::default());
        let (asked, wanted) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
        let want = app_answers(state.clone(), wanted.clone());
        // no permission: no question, no fix
        let yes = asker(Some(true), asked.clone());
        let got = state
            .exact(&ctx("t1", false), &yes, &want, "q0".into(), Duration::ZERO)
            .await;
        assert_eq!(got, json!({"given": false, "why": NO_PERMISSION}));
        granted(&state);
        // the check and the planner's lookups never ask
        let got = state
            .exact(&ctx("t1", true), &yes, &want, "q0".into(), Duration::ZERO)
            .await;
        assert_eq!(got, json!({"given": false, "why": OBSERVING}));
        assert_eq!(
            (asked.load(Ordering::SeqCst), wanted.load(Ordering::SeqCst)),
            (0, 0)
        );
        // Yes: one question for the whole tree, a fix each time
        let got = state
            .exact(&ctx("t1", false), &yes, &want, "q1".into(), FIX_WAIT)
            .await;
        assert_eq!(
            got,
            json!({"given": true, "latitude": 18.5204, "longitude": 73.8567, "accuracy_m": 12.0})
        );
        let again = state
            .exact(&ctx("t1.2", false), &yes, &want, "q2".into(), FIX_WAIT)
            .await;
        assert_eq!(again["given"], true);
        assert_eq!(
            (asked.load(Ordering::SeqCst), wanted.load(Ordering::SeqCst)),
            (1, 2)
        );
        // No: nothing read, and the No holds for the tree
        let no = asker(Some(false), asked.clone());
        let got = state
            .exact(&ctx("t2", false), &no, &want, "q3".into(), FIX_WAIT)
            .await;
        assert_eq!(got["given"], false);
        assert!(got.get("latitude").is_none());
        let got = state
            .exact(&ctx("t2.1", false), &yes, &want, "q4".into(), FIX_WAIT)
            .await;
        assert_eq!(got["given"], false);
        assert_eq!(
            (asked.load(Ordering::SeqCst), wanted.load(Ordering::SeqCst)),
            (2, 2)
        );
        // FERB.app silent: no coordinates, said plainly
        let silent: WantFn = Arc::new(|_id: &str| {});
        let got = state
            .exact(
                &ctx("t1", false),
                &yes,
                &silent,
                "q5".into(),
                Duration::ZERO,
            )
            .await;
        assert_eq!(got["given"], false);
        assert!(got["why"]
            .as_str()
            .unwrap()
            .contains("didn't give a location"));
        // a late answer finds nobody waiting
        let late = state
            .report(&params(json!({"request_id": "q5", "error": "late"})))
            .unwrap();
        assert_eq!(late["delivered"], false);
        // a fix out of range is refused
        assert!(state
            .report(&params(json!({"request_id": "x", "exact": {"latitude": 91.0, "longitude": 0.0, "accuracy_m": 1.0}})))
            .is_err());
    }
}
