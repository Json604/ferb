//! Each paired phone's own record for FERB (kb T-1214; SENSE-420, SENSE-421, PHN-467): its time zone, its city when
//! the owner let the phone share it, its battery, network, language and screen, as the phone reports them by
//! `phone.device_report`; and the one precise fix the owner said Yes to for a task (GATE-613), asked of the phone over
//! the link and answered by the same method.
//!
//! - **Memory only**, like the Mac's place (journal 104): nothing here is stored; a daemon restart knows no phone's
//!   record until it reports again (it does on its next connection).
//! - **How long a record is kept** (PHN-523): while the phone is connected, and for [`super::Limits::unlock_max_s`]
//!   (12 hours, the longest one unlock of the phone lasts, PHN-141) after its last report or its last disconnect; then it
//!   goes, and the line says nothing of that phone. A removed phone's record and requests go at once.
//! - **Who may report** (PHN-523): the phone's own session, inside its unlock window, or, while the app is locked
//!   (Stay connected), a session that proved the phone's device key (PHN-126): the record gives the phone nothing of
//!   FERB's, and the device key keeps a copied link key from writing where FERB thinks the phone is.
//! - **What is kept**: the city, region and country, never coordinates (cleaned as the Mac's place is); a fix exists
//!   only inside the one request that asked for it.

use std::collections::HashMap;

use ferb_core::device::{clean_part, Place};
use ferb_rpc::phone_gate::{self, PhoneSession, Refusal};
use ferb_rpc::{Params, Session};
use serde_json::{json, Value};

use super::{lock, refused, Inner, Phones};

/// The location states a phone reports (what FERB may read of where the phone is).
pub const LOCATION_STATES: [&str; 4] = ["on", "off", "denied", "unavailable"];
/// The network kinds a phone reports (never a network's name).
pub const NETWORK_KINDS: [&str; 5] = ["wifi", "mobile", "ethernet", "other", "offline"];
/// A phone's languages kept (a technical bound: a phone lists a few).
const LANGUAGES_MAX: usize = 8;
/// The longest language tag kept (BCP 47 tags are short).
const TAG_CHARS: usize = 35;
/// The widest UTC offset there is (UTC−12 … UTC+14), in minutes.
const OFFSET_MAX_MIN: i64 = 14 * 60;

/// What a phone said about itself, cleaned. A fact it didn't say is empty or `None`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PhoneRecord {
    /// The phone's own name in its settings.
    pub name: String,
    pub model: String,
    pub os: String,
    /// An IANA time zone this Mac knows ("" when the phone named none or one unknown here).
    pub time_zone: String,
    /// The phone's offset from UTC when it reported, in minutes (the fallback for a zone unknown here).
    pub utc_offset_minutes: Option<i32>,
    /// "on", "off", "denied", "unavailable" ("" unsaid).
    pub location: String,
    /// Only while `location` is "on".
    pub place: Option<Place>,
    pub battery_percent: Option<u8>,
    pub charging: Option<bool>,
    pub power_saver: Option<bool>,
    pub network: String,
    pub locale: String,
    pub languages: Vec<String>,
    pub screen_on: Option<bool>,
    pub screen_locked: Option<bool>,
}

fn text(v: Option<&Value>) -> String {
    v.and_then(Value::as_str)
        .map(clean_part)
        .unwrap_or_default()
}

fn flag(v: Option<&Value>) -> Option<bool> {
    v.and_then(Value::as_bool)
}

impl PhoneRecord {
    /// A report as the phone sent it; `Err` for a value that is not one (a location state or network kind not known).
    pub fn from_report(p: &Params) -> Result<PhoneRecord, String> {
        let one_of = |key: &str, known: &[&str]| -> Result<String, String> {
            match p.get(key) {
                None | Some(Value::Null) => Ok(String::new()),
                Some(Value::String(s)) if known.contains(&s.as_str()) => Ok(s.clone()),
                Some(other) => Err(format!("{key} is not one of {known:?}: {other}")),
            }
        };
        let location = one_of("location", &LOCATION_STATES)?;
        let network = one_of("network", &NETWORK_KINDS)?;
        let zone = text(p.get("time_zone"));
        let time_zone = if ferb_clock::zone::Zone::named(&zone).is_some() {
            zone
        } else {
            String::new()
        };
        let utc_offset_minutes = p
            .get("utc_offset_minutes")
            .and_then(Value::as_i64)
            .filter(|m| m.abs() <= OFFSET_MAX_MIN)
            .map(|m| m as i32);
        let battery = p.get("battery");
        let battery_percent = battery
            .and_then(|b| b.get("percent"))
            .and_then(Value::as_u64)
            .filter(|n| *n <= 100)
            .map(|n| n as u8);
        let screen = p.get("screen");
        let place = if location == "on" {
            p.get("place").and_then(Place::from_report)
        } else {
            None
        };
        Ok(PhoneRecord {
            name: text(p.get("name")),
            model: text(p.get("model")),
            os: text(p.get("os")),
            time_zone,
            utc_offset_minutes,
            location,
            place,
            battery_percent,
            charging: flag(battery.and_then(|b| b.get("charging"))),
            power_saver: flag(battery.and_then(|b| b.get("power_saver"))),
            network,
            locale: text(p.get("locale")).chars().take(TAG_CHARS).collect(),
            languages: p
                .get("languages")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|l| l.as_str())
                        .map(|l| clean_part(l).chars().take(TAG_CHARS).collect::<String>())
                        .filter(|l| !l.is_empty())
                        .take(LANGUAGES_MAX)
                        .collect()
                })
                .unwrap_or_default(),
            screen_on: flag(screen.and_then(|s| s.get("on"))),
            screen_locked: flag(screen.and_then(|s| s.get("locked"))),
        })
    }
}

/// One precise fix a phone gave for a request.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fix {
    pub latitude: f64,
    pub longitude: f64,
    /// Meters; `None` when the phone's fix had no accuracy (Android's `hasAccuracy` false).
    pub accuracy_m: Option<f64>,
}

type Waiter = tokio::sync::oneshot::Sender<Result<Fix, String>>;

/// A record as held, with when it came.
#[derive(Clone, Debug)]
struct Held {
    record: PhoneRecord,
    /// Monotonic seconds at the report, or at the phone's last disconnect after it (the keep counts from there).
    heard: f64,
    /// The wall time of the report (ISO 8601).
    heard_at: String,
}

/// The phones' records and the open requests for a fix.
#[derive(Default)]
pub(crate) struct Devices {
    held: HashMap<String, Held>,
    /// Request id → (the phone asked, its answer's way back).
    waiting: HashMap<String, (String, Waiter)>,
}

/// One paired phone as the facts line and `device.facts` show it.
#[derive(Clone, Debug, PartialEq)]
pub struct PhoneView {
    /// The phone's id (its link endpoint).
    pub id: String,
    /// The name the phone has in FERB's list (the owner may rename it on the Mac).
    pub name: String,
    /// The link has a session of this phone now.
    pub connected: bool,
    /// When the phone last used the link (ISO 8601, written at most once a minute).
    pub last_seen: String,
    /// Its last report while it is kept.
    pub record: Option<PhoneRecord>,
    /// When that report came (ISO 8601).
    pub reported_at: Option<String>,
}

fn location_refusal() -> Refusal {
    refused(
        phone_gate::LOCKED,
        "FERB takes this phone's details while its app is locked only from a connection that proved it is this phone.",
    )
}

fn finite(v: Option<&Value>) -> Option<f64> {
    v.and_then(Value::as_f64).filter(|x| x.is_finite())
}

impl Inner {
    /// A phone's session ended: its record's keep counts from now if it has no other session.
    pub(crate) fn device_disconnected(&self, device: &str, sessions_left: usize) {
        if sessions_left == 0 {
            let now = self.mono();
            if let Some(h) = lock(&self.devices).held.get_mut(device) {
                h.heard = h.heard.max(now);
            }
        }
    }

    /// Forgets what a phone reported and its open requests (a removal).
    pub(crate) fn forget_device_record(&self, device: &str) {
        let mut d = lock(&self.devices);
        d.held.remove(device);
        d.waiting.retain(|_, (phone, _)| phone != device);
    }
}

impl Phones {
    /// `phone.device_report`: the phone's own record (SENSE-420), or its answer to one request for a precise fix
    /// (`{request_id, exact: {latitude, longitude, accuracy_m}}` or `{request_id, error}`).
    pub fn device_report(
        &self,
        session: &Session,
        params: &Params,
    ) -> ferb_rpc::server::HandlerResult {
        let inner = &self.inner;
        let phone: PhoneSession = session.phone.clone().unwrap_or_default();
        let coded = |r: Refusal| ferb_rpc::server::HandlerError::coded(r.code, r.message);
        inner.live(&phone.device).map_err(coded)?;
        // decision: PHN-523
        let may = {
            let mut st = lock(&inner.state);
            st.is_unlocked(&phone, inner.mono(), &inner.limits) || st.proved_present(&phone)
        };
        if !may {
            return Err(coded(location_refusal()));
        }
        let mut out = Params::new();
        if let Some(id) = params.get("request_id").and_then(Value::as_str) {
            let answer = match params.get("exact") {
                Some(e) => {
                    let lat = finite(e.get("latitude")).filter(|x| (-90.0..=90.0).contains(x));
                    let lon = finite(e.get("longitude")).filter(|x| (-180.0..=180.0).contains(x));
                    // (a fix without an accuracy says none, or a negative number)
                    let accuracy_m = finite(e.get("accuracy_m")).filter(|x| *x >= 0.0);
                    match (lat, lon) {
                        (Some(latitude), Some(longitude)) => Ok(Fix {
                            latitude,
                            longitude,
                            accuracy_m,
                        }),
                        _ => {
                            return Err(coded(refused(
                                phone_gate::BAD_REQUEST,
                                "exact needs latitude and longitude in range",
                            )))
                        }
                    }
                }
                None => Err(params
                    .get("error")
                    .and_then(Value::as_str)
                    .map(|e| clean_part(e).chars().take(300).collect())
                    .unwrap_or_else(|| "the phone couldn't read its location".into())),
            };
            let given = answer.is_ok();
            // only the phone that was asked answers its request
            let waiter = {
                let mut d = lock(&inner.devices);
                match d.waiting.get(id) {
                    Some((asked, _)) if *asked == phone.device => d.waiting.remove(id),
                    _ => None,
                }
            };
            let delivered = waiter.is_some_and(|(_, w)| w.send(answer).is_ok());
            inner.audit(
                &phone.device,
                "phone.device_report",
                "plain",
                if delivered { "ok" } else { "not_found" },
                json!({"request_id": id, "exact": given}),
            );
            out.insert("delivered".into(), json!(delivered));
            return Ok(out);
        }
        let record = PhoneRecord::from_report(params)
            .map_err(|e| coded(refused(phone_gate::BAD_REQUEST, e)))?;
        lock(&inner.devices).held.insert(
            phone.device.clone(),
            Held {
                record,
                heard: inner.mono(),
                heard_at: inner.now_iso(),
            },
        );
        out.insert("ok".into(), json!(true));
        Ok(out)
    }

    /// The paired phones as the facts line and `device.facts` show them, in pairing order. A record not connected for
    /// longer than the keep (PHN-523) goes here.
    pub fn device_views(&self) -> Vec<PhoneView> {
        let inner = &self.inner;
        let phones = {
            let db = lock(&inner.seams.db);
            ferb_store::phones::live(db.conn()).unwrap_or_default()
        };
        let now = inner.mono();
        let keep = inner.limits.unlock_max_s;
        let connected: HashMap<String, bool> = {
            let st = lock(&inner.state);
            phones
                .iter()
                .map(|p| (p.endpoint.clone(), st.sessions_of(&p.endpoint) > 0))
                .collect()
        };
        let mut d = lock(&inner.devices);
        d.held.retain(|id, h| {
            phones.iter().any(|p| &p.endpoint == id)
                && (connected.get(id).copied().unwrap_or(false) || now - h.heard <= keep)
        });
        phones
            .into_iter()
            .map(|p| {
                let held = d.held.get(&p.endpoint);
                PhoneView {
                    connected: connected.get(&p.endpoint).copied().unwrap_or(false),
                    record: held.map(|h| h.record.clone()),
                    reported_at: held.map(|h| h.heard_at.clone()),
                    id: p.endpoint,
                    name: p.name,
                    last_seen: p.last_seen,
                }
            })
            .collect()
    }

    /// Asks a connected phone for one precise fix (after the owner's Yes, GATE-613): the helper puts the request in
    /// that phone's link (`link.events location_wanted`); the phone answers by `phone.device_report`. `Err` says why it
    /// can't be asked.
    pub fn want_fix(
        &self,
        device: &str,
        request_id: &str,
    ) -> Result<tokio::sync::oneshot::Receiver<Result<Fix, String>>, String> {
        let inner = &self.inner;
        let phone = inner
            .live(device)
            .map_err(|_| "that phone is not one of FERB's paired phones".to_string())?;
        if lock(&inner.state).sessions_of(device) == 0 {
            return Err(format!(
                "the phone \"{}\" isn't connected to FERB now, so it can't be asked",
                phone.name
            ));
        }
        let (tx, rx) = tokio::sync::oneshot::channel();
        lock(&inner.devices)
            .waiting
            .insert(request_id.to_string(), (device.to_string(), tx));
        let _ = inner
            .events
            .send(json!({"event": "location_wanted", "id": device, "request_id": request_id}));
        Ok(rx)
    }

    /// A request that ended without an answer (its wait ran out, or the task went): forgotten.
    pub fn drop_fix(&self, request_id: &str) {
        lock(&self.inner.devices).waiting.remove(request_id);
    }
}
