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
use ferb_daemon::phone::device::{PhoneRecord, PhoneView};

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
/// GATE-614: the same question for one of the owner's phones (its name in FERB's list).
pub fn ask_phone_summary(name: &str) -> String {
    format!("Use the exact location of your phone “{name}” for this task?")
}
pub const ASK_PHONE_DETAIL: &str =
    "FERB will ask your phone where it is right now. Until now it only knew its \
city. Your answer holds for the rest of this task.";
/// How long a phone may take to give one precise fix: FERB's app on it may need the owner's tap and Android's own
/// location question first (a technical bound).
pub const PHONE_FIX_WAIT: Duration = Duration::from_secs(60);

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
    /// The phone whose location is asked (its name in FERB's list); `None`: this Mac.
    pub phone: Option<String>,
}
pub type AskFn = Arc<dyn Fn(Ask) -> BoxFuture<'static, Option<bool>> + Send + Sync>;
/// Asks FERB.app for one precise fix under this id (the bus event).
pub type WantFn = Arc<dyn Fn(&str) + Send + Sync>;

/// This daemon's device state.
#[derive(Default)]
pub struct DeviceState {
    reported: Mutex<Reported>,
    waiting: Mutex<HashMap<String, Waiter>>,
    /// GATE-613: the owner's answer per task tree (its root task) and device ("" for this Mac), kept while the daemon
    /// runs.
    answers: Mutex<BTreeMap<(String, String), bool>>,
    /// The paired phones' own records (T-1214), once the phone area is built.
    phones: std::sync::OnceLock<ferb_daemon::phone::Phones>,
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

    /// The owner's answer for this task tree and device (GATE-613): asked once, kept for the tree.
    async fn answer(
        &self,
        ctx: &ToolContext,
        ask: &AskFn,
        phone: Option<(&str, &str)>,
    ) -> Result<bool, String> {
        let root = root_of(&ctx.task_id).to_string();
        let key = (root, phone.map(|p| p.0.to_string()).unwrap_or_default());
        if let Some(yes) = lock(&self.answers).get(&key).copied() {
            return Ok(yes);
        }
        let Some(yes) = ask(Ask {
            task_id: ctx.task_id.clone(),
            request_id: ctx.request_id.clone(),
            phone: phone.map(|p| p.1.to_string()),
        })
        .await
        else {
            return Err("the question went before the owner answered it".into());
        };
        lock(&self.answers).insert(key, yes);
        Ok(yes)
    }

    /// The paired phones' area, for their lines, `device.facts` and their exact location (set once at start).
    pub fn set_phones(&self, phones: ferb_daemon::phone::Phones) {
        let _ = self.phones.set(phones);
    }

    /// The paired phones as the line and the lookup show them (none before the phone area is built).
    pub fn phone_views(&self) -> Vec<PhoneView> {
        self.phones
            .get()
            .map(ferb_daemon::phone::Phones::device_views)
            .unwrap_or_default()
    }

    /// A phone's exact location for a task (GATE-614): refused to the check; the owner asked once per task tree and
    /// phone; then the phone is asked over the link for one fix within `wait`. Never coordinates without the Yes.
    pub async fn exact_phone(
        &self,
        ctx: &ToolContext,
        ask: &AskFn,
        phone: &PhoneView,
        id: String,
        wait: Duration,
    ) -> Value {
        let refused = |why: &str| json!({"given": false, "why": why});
        if ctx.observe_only {
            return refused(OBSERVING);
        }
        if !phone.connected {
            return refused(&format!(
                "the phone “{}” isn't connected to FERB now, so it can't be asked where it is",
                phone.name
            ));
        }
        let Some(phones) = self.phones.get() else {
            return refused("FERB's phone link isn't running");
        };
        let yes = match self.answer(ctx, ask, Some((&phone.id, &phone.name))).await {
            Ok(yes) => yes,
            Err(why) => return refused(&why),
        };
        if !yes {
            return refused(
                "the owner said No to sharing this phone's exact location in this task",
            );
        }
        let rx = match phones.want_fix(&phone.id, &id) {
            Ok(rx) => rx,
            Err(why) => return refused(&why),
        };
        let got = tokio::time::timeout(wait, rx).await;
        phones.drop_fix(&id);
        match got {
            Ok(Ok(Ok(f))) => {
                json!({"given": true, "latitude": f.latitude, "longitude": f.longitude, "accuracy_m": f.accuracy_m.map(f64::round)})
            }
            Ok(Ok(Err(why))) => refused(&why),
            _ => refused("the phone didn't give its location in time (FERB's app on it may be closed, or the owner didn't answer there)"),
        }
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
        let yes = match self.answer(ctx, ask, None).await {
            Ok(yes) => yes,
            Err(why) => return refused(&why),
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

/// The devices' lines for the live facts (SENSE-421): this Mac's, then `asleep` (SENSE-430: the one line while the Mac
/// is asleep in a brief background wake, "" otherwise), then one per paired phone FERB has heard from (SENSE-425).
pub fn facts_line(
    state: &DeviceState,
    name: &str,
    now: &ferb_core::when::DateTime,
    zone: &str,
    asleep: &str,
) -> String {
    let r = state.reported();
    let place = r.place.filter(|_| r.location == "granted");
    let mut lines = vec![line(&DeviceLine {
        label: "This Mac",
        name,
        now,
        zone,
        place: place.as_ref(),
        note: "",
    })];
    if !asleep.is_empty() {
        lines.push(asleep.to_string());
    }
    lines.extend(
        state
            .phone_views()
            .iter()
            .filter_map(|v| phone_line(v, now)),
    );
    lines.join("\n")
}

/// A phone's wall time now: in its own time zone, else at the offset it last reported.
fn phone_now(
    r: &PhoneRecord,
    now: &ferb_core::when::DateTime,
) -> Option<ferb_core::when::DateTime> {
    if let Some(z) = ferb_clock::zone::Zone::named(&r.time_zone) {
        return Some(z.local(now));
    }
    let offset = chrono::FixedOffset::east_opt(r.utc_offset_minutes? * 60)?;
    Some(now.with_timezone(&offset))
}

/// How a phone is with the owner now, from facts only (SENSE-425): connected or not; its screen as it last said while
/// connected; when it was last seen when it is not.
pub fn phone_note(v: &PhoneView, now: &ferb_core::when::DateTime) -> String {
    if !v.connected {
        let seen = ferb_core::when::parse(&v.last_seen)
            .map(|t| format!(", last seen {}", ferb_clock::context::ago(&t, now)))
            .unwrap_or_default();
        return format!("not connected{seen}");
    }
    let r = v.record.as_ref();
    match (r.and_then(|r| r.screen_on), r.and_then(|r| r.screen_locked)) {
        (Some(false), _) => "connected, screen off".into(),
        (_, Some(true)) => "connected, locked".into(),
        // SENSE-425: the screen as it said, never "in use" (where a message came from is the message's own fact,
        // SENSE-424)
        (Some(true), Some(false)) => "connected, screen on and unlocked".into(),
        _ => "connected".into(),
    }
}

/// One phone's line (SENSE-425): its wall time, zone and city as it reported them, and how it is with the owner; never
/// coordinates. A phone that has said nothing since FERB started gets a short line of what is known.
pub fn phone_line(v: &PhoneView, now: &ferb_core::when::DateTime) -> Option<String> {
    let note = phone_note(v, now);
    let Some((r, local)) = v
        .record
        .as_ref()
        .and_then(|r| phone_now(r, now).map(|t| (r, t)))
    else {
        return v.connected.then(|| {
            format!(
                "- Phone “{}”: {note}; it hasn't told FERB its time or place since FERB started",
                v.name
            )
        });
    };
    let place = r.place.as_ref().filter(|_| r.location == "on");
    Some(line(&DeviceLine {
        label: "Phone",
        name: &v.name,
        now: &local,
        zone: &r.time_zone,
        place,
        note: &note,
    }))
}

/// What the lookup says of one phone: the line's facts and the rest it reported (each left out when unsaid).
pub fn phone_content(v: &PhoneView, now: &ferb_core::when::DateTime) -> Value {
    let mut m = Map::new();
    m.insert(
        "device".into(),
        json!({"id": v.id, "kind": "phone", "name": v.name}),
    );
    m.insert("connected".into(), json!(v.connected));
    m.insert("status".into(), json!(phone_note(v, now)));
    if !v.last_seen.is_empty() {
        m.insert("last_seen".into(), json!(v.last_seen));
    }
    let Some(r) = &v.record else {
        m.insert(
            "reported".into(),
            json!("nothing yet: the phone tells FERB its time and place when it connects"),
        );
        return Value::Object(m);
    };
    if let Some(at) = &v.reported_at {
        m.insert("reported_at".into(), json!(at));
    }
    let text = |m: &mut Map<String, Value>, k: &str, v: &str| {
        if !v.is_empty() {
            m.insert(k.into(), json!(v));
        }
    };
    text(&mut m, "own_name", &r.name);
    text(&mut m, "model", &r.model);
    text(&mut m, "os", &r.os);
    if let Some(t) = phone_now(r, now) {
        m.insert(
            "time".into(),
            json!({"local": ferb_core::when::isoformat(&t), "weekday": t.format("%A").to_string(),
                   "time_zone": r.time_zone, "utc_offset": ferb_core::device::utc_offset(&t)}),
        );
    }
    let shown = match r.location.as_str() {
        "on" => "on",
        "off" => "off (the owner hasn't let this phone share its city)",
        "denied" => "off (the phone's own permission is refused)",
        "unavailable" => "unavailable (the phone's location is turned off)",
        _ => "unknown",
    };
    m.insert("location_permission".into(), json!(shown));
    match r.place.as_ref().filter(|_| r.location == "on") {
        Some(p) => {
            m.insert("place".into(), p.to_json());
        }
        None => {
            m.insert(
                "place".into(),
                json!("unknown: the time zone is the only hint"),
            );
        }
    }
    let mut battery = Map::new();
    if let Some(p) = r.battery_percent {
        battery.insert("percent".into(), json!(p));
    }
    if let Some(c) = r.charging {
        battery.insert("charging".into(), json!(c));
    }
    if !battery.is_empty() {
        m.insert("battery".into(), Value::Object(battery));
    }
    if let Some(s) = r.power_saver {
        m.insert("battery_saver".into(), json!(s));
    }
    text(&mut m, "network", &r.network);
    if let Some(on) = r.screen_on {
        m.insert("screen_on".into(), json!(on));
    }
    if let Some(l) = r.screen_locked {
        m.insert("screen_locked".into(), json!(l));
    }
    text(&mut m, "locale", &r.locale);
    if !r.languages.is_empty() {
        m.insert("languages".into(), json!(r.languages));
    }
    Value::Object(m)
}

/// What the tool reads besides FERB.app's reports.
pub struct Now {
    pub device_id: String,
    pub name: String,
    pub now: ferb_core::when::DateTime,
    pub zone: String,
    pub reading: ferb_mac_hands::device::Reading,
    /// The Mac's sleep and power and FERB's Keep my Mac working mode (the insomnia area's; none before it is built).
    pub mac: Option<ferb_daemon::mac_now::MacState>,
    /// The sleep record asked for (`since`, `until`): its entries and how many older ones were left out.
    pub record: Option<Record>,
}

/// The sleep record one call asked for (SENSE-429).
#[derive(Clone, Debug, PartialEq)]
pub struct Record {
    pub since: ferb_core::when::DateTime,
    pub until: Option<ferb_core::when::DateTime>,
    pub entries: Vec<Value>,
    pub left_out: usize,
}

/// The kinds the sleep record reads: the Mac's power changes and FERB's mode.
pub const RECORD_KINDS: [&str; 2] = [
    ferb_daemon::mac_now::JOURNAL_KIND,
    ferb_daemon::insomnia::JOURNAL_KIND,
];

/// The recorded rows (at, kind, data) of the sleep record from `since` on, oldest first (SENSE-429).
pub fn record_rows(
    db: &crate::basics::SharedDb,
    since: &ferb_core::when::DateTime,
) -> Vec<(String, String, Value)> {
    let d = lock(db);
    let Ok(mut st) = d.conn().prepare(
        "SELECT at, kind, data_json FROM journal WHERE kind IN (?1, ?2) AND julianday(at) >= julianday(?3) \
         ORDER BY seq",
    ) else {
        return Vec::new();
    };
    let since = ferb_core::when::isoformat(since);
    let rows = st.query_map(
        rusqlite::params![RECORD_KINDS[0], RECORD_KINDS[1], since],
        |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        },
    );
    rows.map(|r| {
        r.flatten()
            .filter_map(|(at, kind, data)| Some((at, kind, serde_json::from_str(&data).ok()?)))
            .collect()
    })
    .unwrap_or_default()
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
    // SENSE-428: the Mac's sleep and power, Power Nap, and FERB's own Keep my Mac working mode
    if let Some(mac) = &n.mac {
        m.extend(ferb_daemon::mac_now::power_facts(mac));
    }
    if let Some(r) = &n.record {
        let mut rec = Map::new();
        rec.insert("since".into(), json!(ferb_core::when::isoformat(&r.since)));
        if let Some(u) = &r.until {
            rec.insert("until".into(), json!(ferb_core::when::isoformat(u)));
        }
        rec.insert("entries".into(), Value::Array(r.entries.clone()));
        if r.left_out > 0 {
            rec.insert(
                "older_left_out".into(),
                json!(format!(
                    "{} older entries left out: ask for a later since",
                    r.left_out
                )),
            );
        }
        if r.entries.is_empty() {
            rec.insert("note".into(), json!(RECORD_EMPTY));
        }
        rec.insert("about".into(), json!(ferb_daemon::mac_now::HISTORY_ABOUT));
        m.insert("sleep_record".into(), Value::Object(rec));
    }
    // SENSE-425: the owner's paired phones, as each reported itself (none listed when no phone is paired)
    let phones = state.phone_views();
    if !phones.is_empty() {
        m.insert(
            "phones".into(),
            Value::Array(phones.iter().map(|v| phone_content(v, &n.now)).collect()),
        );
    }
    m
}

/// What the sleep record says when it holds nothing for the stretch asked.
pub const RECORD_EMPTY: &str = "nothing recorded in this stretch: FERB records when this Mac sleeps and wakes, its \
lid and power source, and its Keep my Mac working mode, from the FERB version that added this record on, and keeps \
them as long as its other operational records";

/// The sleep record's stretch from the call's `since` and `until` (ISO dates or date-times, the Mac's zone unless
/// they name one; a date as `until` covers its day): None without `since`; Err for a malformed one (the caller's).
pub fn record_range(
    a: &Args,
    zone: &ferb_clock::zone::Zone,
) -> Result<Option<(ferb_core::when::DateTime, Option<ferb_core::when::DateTime>)>, String> {
    let text = |k: &str| a.get(k).and_then(Value::as_str).map(str::to_string);
    let since = ferb_sense::tools::time_bound(text("since").as_deref(), false, None, zone)?;
    let until = ferb_sense::tools::time_bound(text("until").as_deref(), true, None, zone)?;
    Ok(since.map(|s| (s, until)))
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
            let device = a
                .get("device")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|d| !d.is_empty())
                .map(str::to_string);
            let range = record_range(a, &p.zoned().zone());
            Box::pin(async move {
                let range = range.map_err(|e| ferb_gateway::tool::ToolError {
                    class: "ValueError".into(),
                    message: e,
                    before_acting: true,
                })?;
                let state = p.device();
                let (native, home) = (p.native(), p.user_home().clone());
                let (name, reading) = tokio::task::spawn_blocking(move || read_mac(native, &home))
                    .await
                    .unwrap_or_default();
                let clock = p.zoned();
                let zone = clock.zone();
                let record = range.map(|(since, until)| {
                    let rows = record_rows(&p.db(), &since);
                    let z = zone;
                    let local = move |t: &ferb_core::when::DateTime| {
                        ferb_core::when::isoformat(&z.local(t))
                    };
                    let (entries, left_out) =
                        ferb_daemon::mac_now::history(&rows, &since, until.as_ref(), &local);
                    Record {
                        since: zone.local(&since),
                        until: until.map(|u| zone.local(&u)),
                        entries,
                        left_out,
                    }
                });
                let n = Now {
                    device_id: device_id(&p),
                    name,
                    now: zone.local(&clock.now()),
                    zone: zone.key().to_string(),
                    reading,
                    mac: p.insomnia().map(|i| i.mac_state()),
                    record,
                };
                let mut m = content(&state, &n);
                if exact {
                    let (ask, want) = (ask_owner(&p), want_fix(&p));
                    let id = p.ids().new_id();
                    let got = match device.as_deref().filter(|d| !is_this_mac(d)) {
                        None => state.exact(ctx, &ask, &want, id, FIX_WAIT).await,
                        Some(d) => match find_phone(&state.phone_views(), d) {
                            Some(v) => state.exact_phone(ctx, &ask, &v, id, PHONE_FIX_WAIT).await,
                            None => json!({"given": false, "why": format!(
                                "no device named “{d}”: name this Mac or one of the phones listed under phones"
                            )}),
                        },
                    };
                    m.insert("exact_location".into(), got);
                }
                Ok(ToolOutput::from(ToolResult {
                    content: m,
                    ..ToolResult::default()
                }))
            })
        }),
    })
}

/// Whether a `device` argument names this Mac.
fn is_this_mac(d: &str) -> bool {
    let d = d.to_lowercase();
    d == "mac" || d == "this mac" || d == "this_mac"
}

/// The phone a `device` argument names: by its id, or its name in FERB's list (case ignored).
pub fn find_phone(views: &[PhoneView], d: &str) -> Option<PhoneView> {
    let low = d.to_lowercase();
    views
        .iter()
        .find(|v| v.id == d)
        .or_else(|| views.iter().find(|v| v.name.to_lowercase() == low))
        .cloned()
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
            // GATE-614: a phone's question names it
            let (summary, detail) = match &a.phone {
                None => (ASK_SUMMARY.to_string(), ASK_DETAIL.to_string()),
                Some(name) => (ask_phone_summary(name), ASK_PHONE_DETAIL.to_string()),
            };
            let ap = gw.ask_own(&ferb_gateway::gateway::OwnQuestion {
                capability: QUESTION.into(),
                reason: format!("{summary}\n{detail}"),
                options: vec!["Yes".into(), "No".into()],
                request_id: a.request_id,
                summary: Some(summary),
                detail: Some(detail),
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
            facts_line(&state, "Studio", &now, "Asia/Kolkata", ""),
            "- This Mac “Studio”: Monday 2026-11-16 10:05, time zone Asia/Kolkata (UTC+05:30); place unknown (time \
             zone only)"
        );
        granted(&state);
        assert!(facts_line(&state, "Studio", &now, "Asia/Kolkata", "")
            .ends_with("place: Pune, Maharashtra, India (city level)"));
        state
            .report(&params(json!({"location": "denied"})))
            .unwrap();
        assert_eq!(state.reported().place, None);
        assert_eq!(state.reported().wifi, None);
        assert!(facts_line(&state, "Studio", &now, "Asia/Kolkata", "")
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
            mac: None,
            record: None,
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

    fn view(connected: bool, record: Option<PhoneRecord>) -> PhoneView {
        PhoneView {
            id: "ep-1".into(),
            name: "Moto".into(),
            connected,
            last_seen: "2026-11-16T13:35:00+05:30".into(),
            record,
            reported_at: Some("2026-11-16T13:30:00+05:30".into()),
        }
    }

    fn record() -> PhoneRecord {
        PhoneRecord {
            name: "moto g84".into(),
            model: "motorola moto g84 5G".into(),
            os: "Android 15".into(),
            time_zone: "Europe/London".into(),
            utc_offset_minutes: Some(0),
            location: "on".into(),
            place: Place::from_report(
                &json!({"city": "London", "region": "England", "country": "United Kingdom", "country_code": "GB"}),
            ),
            battery_percent: Some(42),
            charging: Some(false),
            power_saver: Some(true),
            network: "mobile".into(),
            locale: "en-GB".into(),
            languages: vec!["en-GB".into()],
            screen_on: Some(true),
            screen_locked: Some(false),
        }
    }

    /// A phone's line (SENSE-425): its own time and zone (London here, the Mac in Kolkata), its city only while it
    /// shares it, and how it is with the owner from facts only; never coordinates.
    // decision: SENSE-425
    #[test]
    fn a_phone_s_line_has_its_own_time_city_and_state() {
        let now = at("2026-11-16T15:35:00+05:30");
        assert_eq!(
            phone_line(&view(true, Some(record())), &now).unwrap(),
            "- Phone “Moto”: Monday 2026-11-16 10:05, time zone Europe/London (UTC+00:00); place: London, England, \
             United Kingdom (city level); connected, screen on and unlocked"
        );
        let mut off = record();
        off.location = "off".into();
        off.screen_locked = Some(true);
        let line = phone_line(&view(true, Some(off)), &now).unwrap();
        assert!(
            line.ends_with("place unknown (time zone only); connected, locked"),
            "{line}"
        );
        let mut zoneless = record();
        zoneless.time_zone = String::new();
        zoneless.utc_offset_minutes = Some(-240);
        assert!(phone_line(&view(false, Some(zoneless)), &now)
            .unwrap()
            .contains("06:05, time zone UTC-04:00"));
        let away = phone_line(&view(false, Some(record())), &now).unwrap();
        assert!(
            away.ends_with("; not connected, last seen 2 h ago (Mon 16 Nov, 13:35)"),
            "{away}"
        );
        assert_eq!(
            phone_line(&view(true, None), &now).unwrap(),
            "- Phone “Moto”: connected; it hasn't told FERB its time or place since FERB started"
        );
        assert_eq!(phone_line(&view(false, None), &now), None);
    }

    /// device.facts lists each phone with what it reported, a fact it didn't say left out; a phone is found by its
    /// id or its name.
    // decision: SENSE-425
    #[test]
    fn the_lookup_lists_each_phone_as_it_reported_itself() {
        let now = at("2026-11-16T15:35:00+05:30");
        let v = phone_content(&view(true, Some(record())), &now);
        assert_eq!(
            v["device"],
            json!({"id": "ep-1", "kind": "phone", "name": "Moto"})
        );
        assert_eq!(v["battery"], json!({"percent": 42, "charging": false}));
        assert_eq!(v["battery_saver"], true);
        assert_eq!(v["place"]["city"], "London");
        assert_eq!(v["time"]["local"], "2026-11-16T10:05:00+00:00");
        assert_eq!(v["status"], "connected, screen on and unlocked");
        let bare = phone_content(&view(false, None), &now);
        assert!(bare.get("time").is_none() && bare.get("battery").is_none());
        let views = vec![view(true, None)];
        assert!(find_phone(&views, "moto").is_some());
        assert!(find_phone(&views, "ep-1").is_some());
        assert!(find_phone(&views, "pixel").is_none());
        assert!(is_this_mac("This Mac"));
    }

    /// GATE-614: a phone's exact location is never asked of the owner when the phone can't be reached, nor by a look;
    /// the Mac's own answer for a tree is not the phone's.
    // decision: GATE-614
    #[tokio::test]
    async fn a_phone_s_exact_location_is_asked_per_phone_and_only_when_it_can_answer() {
        let state = DeviceState::default();
        let asked = Arc::new(AtomicUsize::new(0));
        let yes = asker(Some(true), asked.clone());
        let got = state
            .exact_phone(
                &ctx("t1", true),
                &yes,
                &view(true, None),
                "q".into(),
                Duration::ZERO,
            )
            .await;
        assert_eq!(got, json!({"given": false, "why": OBSERVING}));
        let got = state
            .exact_phone(
                &ctx("t1", false),
                &yes,
                &view(false, None),
                "q".into(),
                Duration::ZERO,
            )
            .await;
        assert!(
            got["why"].as_str().unwrap().contains("isn't connected"),
            "{got}"
        );
        assert_eq!(asked.load(Ordering::SeqCst), 0);
        // the Mac's Yes for the tree doesn't answer for the phone: separate keys
        assert_eq!(state.answer(&ctx("t1", false), &yes, None).await, Ok(true));
        assert_eq!(
            state
                .answer(&ctx("t1.2", false), &yes, Some(("ep-1", "Moto")))
                .await,
            Ok(true)
        );
        assert_eq!(asked.load(Ordering::SeqCst), 2);
        assert_eq!(
            state
                .answer(&ctx("t1.3", false), &yes, Some(("ep-1", "Moto")))
                .await,
            Ok(true)
        );
        assert_eq!(asked.load(Ordering::SeqCst), 2);
        assert_eq!(
            ask_phone_summary("Moto"),
            "Use the exact location of your phone “Moto” for this task?"
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
