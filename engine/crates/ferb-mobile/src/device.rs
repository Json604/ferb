//! The phone's own facts for FERB (kb T-1214; SENSE-420, SENSE-421, PHN-467), and finding the Mac's files from the
//! phone (T-1211): its recent files and a search of file names.
//!
//! **The phone's record.** The app hands the core what it reads from the platform's own callbacks (time zone, the city
//! when the owner let this phone share it, battery, network, language, the screen); the core sends it to the Mac as
//! `phone.device_report` when it differs from what this connection already sent, and again on every new connection
//! (the next sync). Nothing is polled: a report goes only when something changed or the link came back. The record
//! holds the city, never coordinates. The Mac keeps it in memory only.
//!
//! **The exact location.** The Mac asks for one precise fix only after the owner said Yes to it for a task (GATE-613):
//! its pushed `location_wanted` event reaches the app as [`crate::Change::LocationWanted`]; the app reads one fix (asking
//! Android's own permission then, if it has to) and answers with [`crate::Mobile::answer_location`]. Coordinates are a
//! few bytes of text: they may cross any path of the link, the public relay included (end-to-end encrypted like every
//! message; PHN-444 keeps only bulk off it).
//!
//! **Finding files.** `phone.recent` and `phone.search` answer listings judged by the same lists as every file call
//! (PHN-482): key places are left out and counted, FERB's own data never shows.

use std::sync::Arc;

use serde::Serialize;
use serde_json::{json, Value};

use crate::api::{Change, ErrorCode, Mobile, MobileError, MobileResult, Shared};
use crate::transport::{TransferSink, Transport};

/// A coarse place as the phone's geocoder named it (never coordinates).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct Place {
    pub city: String,
    pub region: String,
    pub country: String,
    /// ISO 3166-1 alpha-2 ("" when none).
    pub country_code: String,
}

/// What the phone tells FERB about itself (`phone.device_report`). A field the phone could not read is left empty or
/// `None`, and the Mac leaves it out.
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct DeviceReport {
    /// The phone's own name in its settings ("" when the app can't read it without a permission).
    pub name: String,
    /// Maker and model ("motorola moto g84 5G").
    pub model: String,
    /// "Android 15".
    pub os: String,
    /// IANA time zone ("Asia/Kolkata").
    pub time_zone: String,
    /// The zone's offset from UTC now, in minutes (the Mac's fallback for a zone it doesn't know).
    pub utc_offset_minutes: i32,
    /// This phone's location sharing for FERB: "on" (the owner allowed it and Android granted it), "off" (the owner
    /// turned it off in FERB, or never turned it on), "denied" (Android's permission is refused), "unavailable"
    /// (location services are off).
    pub location: String,
    /// The city, when `location` is "on" and the phone could name where it is.
    pub place: Option<Place>,
    pub battery_percent: Option<u8>,
    pub charging: Option<bool>,
    /// Battery saver.
    pub power_saver: Option<bool>,
    /// "wifi", "mobile", "ethernet", "other" or "offline" ("" unknown).
    pub network: String,
    /// "en-IN".
    pub locale: String,
    /// The phone's languages in order.
    pub languages: Vec<String>,
    /// The screen is on.
    pub screen_on: Option<bool>,
    /// The phone's own lock screen is showing.
    pub screen_locked: Option<bool>,
}

impl DeviceReport {
    /// The report as the Mac reads it: only what is known.
    pub fn to_json(&self) -> Value {
        let mut m = serde_json::Map::new();
        let mut text = |k: &str, v: &str| {
            if !v.is_empty() {
                m.insert(k.into(), json!(v));
            }
        };
        text("name", &self.name);
        text("model", &self.model);
        text("os", &self.os);
        text("time_zone", &self.time_zone);
        text("location", &self.location);
        text("network", &self.network);
        text("locale", &self.locale);
        m.insert("utc_offset_minutes".into(), json!(self.utc_offset_minutes));
        if let Some(p) = &self.place {
            m.insert(
                "place".into(),
                json!({"city": p.city, "region": p.region, "country": p.country, "country_code": p.country_code}),
            );
        }
        if !self.languages.is_empty() {
            m.insert("languages".into(), json!(self.languages));
        }
        let mut battery = serde_json::Map::new();
        if let Some(p) = self.battery_percent {
            battery.insert("percent".into(), json!(p));
        }
        if let Some(c) = self.charging {
            battery.insert("charging".into(), json!(c));
        }
        if let Some(s) = self.power_saver {
            battery.insert("power_saver".into(), json!(s));
        }
        if !battery.is_empty() {
            m.insert("battery".into(), Value::Object(battery));
        }
        let mut screen = serde_json::Map::new();
        if let Some(on) = self.screen_on {
            screen.insert("on".into(), json!(on));
        }
        if let Some(l) = self.screen_locked {
            screen.insert("locked".into(), json!(l));
        }
        if !screen.is_empty() {
            m.insert("screen".into(), Value::Object(screen));
        }
        Value::Object(m)
    }
}

/// One precise fix the app read for the Mac's request.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fix {
    pub latitude: f64,
    pub longitude: f64,
    /// Meters; a negative or not-finite value means the fix had none, and none is sent.
    pub accuracy_m: f64,
}

/// What the core keeps of the phone's record (memory only).
#[derive(Default)]
pub(crate) struct DeviceState {
    /// The newest report the app gave.
    latest: Option<DeviceReport>,
    /// What this connection already told the Mac (none after a disconnect: the next connection is told again).
    sent: Option<DeviceReport>,
    /// The Mac's open request for a precise fix (its id), until answered.
    pub(crate) wanted: Option<String>,
}

impl DeviceState {
    /// The link went away: the next connection hears the record again.
    pub(crate) fn offline(&mut self) {
        self.sent = None;
    }
}

/// One file the Mac found for the phone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FoundFile {
    pub name: String,
    /// Its whole path on the Mac (what `fetch` takes).
    pub path: String,
    pub dir: bool,
    pub size: u64,
    /// Seconds since 1970 (UTC).
    pub modified: i64,
    /// Recent files: "made" (a task's result), "fetched" (this phone took it), "sent" (this phone sent it); "" for a
    /// search.
    pub source: String,
    /// When it was made, fetched or sent (ISO 8601).
    pub at: Option<String>,
    /// The task that made it.
    pub task_id: Option<String>,
    pub task_name: Option<String>,
}

/// The Mac's recent files for this phone (`phone.recent`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RecentFiles {
    pub files: Vec<FoundFile>,
    /// Left out: key places (each opens only with the fingerprint, from its folder) and files gone since.
    pub skipped: u64,
}

/// A search of file names on the Mac (`phone.search`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FoundFiles {
    /// The folder searched (as the Mac resolved it).
    pub path: String,
    pub query: String,
    pub files: Vec<FoundFile>,
    /// Names looked at.
    pub looked: u64,
    /// Key places and FERB's own data not looked into.
    pub skipped: u64,
    /// More matches remain after this page (true exactly when `next` is given).
    pub cut: bool,
    /// The search stopped before it looked at every name (it looks at a bounded number): a narrower folder finds more.
    pub incomplete: bool,
    /// The index of this page's first match among all the matches (0 for the first page).
    pub from: u64,
    /// "Load more" (PHN-589): the marker that asks for the next page (`search_files`' `after`), valid only for this
    /// phone, folder and query; none on the last page.
    pub next: Option<String>,
}

fn text(v: &Value) -> String {
    v.as_str().map(str::to_string).unwrap_or_default()
}

fn opt(v: &Value) -> Option<String> {
    v.as_str().filter(|s| !s.is_empty()).map(str::to_string)
}

fn found(v: &Value) -> FoundFile {
    FoundFile {
        name: text(&v["name"]),
        path: text(&v["path"]),
        dir: v["dir"].as_bool().unwrap_or(false),
        size: v["size"].as_u64().unwrap_or(0),
        modified: v["modified"]
            .as_i64()
            .or_else(|| v["modified"].as_f64().map(|f| f as i64))
            .unwrap_or(0),
        source: text(&v["source"]),
        at: opt(&v["at"]),
        task_id: opt(&v["task_id"]),
        task_name: opt(&v["task_name"]),
    }
}

fn list(v: &Value) -> Vec<FoundFile> {
    v["files"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[])
        .iter()
        .map(found)
        .collect()
}

impl Shared {
    /// Tells the Mac the phone's record when this connection hasn't heard it as it is now. A refusal (an app locked on
    /// a connection that hasn't proved the phone's device key yet) leaves it for the next sync.
    pub(crate) fn send_device(this: &Arc<Shared>, t: &Arc<dyn Transport>) -> MobileResult<bool> {
        let due = {
            let st = this.open_state()?;
            match &st.device.latest {
                Some(r) if st.device.sent.as_ref() != Some(r) => Some(r.clone()),
                _ => None,
            }
        };
        let Some(report) = due else {
            return Ok(false);
        };
        Self::call(t, "phone.device_report", report.to_json())?;
        let mut st = this.open_state()?;
        // (a newer report given meanwhile stays due)
        st.device.sent = Some(report);
        Ok(true)
    }
}

impl Mobile {
    // ---- the phone's own record (T-1214) ----------------------------------------------------------------------

    /// The phone's facts as the app read them now. Sent to the Mac at once when online and different from what this
    /// connection already sent, and again on every new connection; kept in memory only. Returns whether it went now.
    pub fn report_device(&self, report: DeviceReport) -> MobileResult<bool> {
        {
            let mut st = self.shared.open_state()?;
            if st.device.latest.as_ref() == Some(&report)
                && st.device.sent.as_ref() == Some(&report)
            {
                return Ok(false);
            }
            st.device.latest = Some(report);
        }
        let Ok(t) = self.shared.transport_online() else {
            return Ok(false);
        };
        Shared::send_device(&self.shared, &t)
    }

    /// The Mac's open request for one precise fix (its id), if any.
    pub fn location_wanted(&self) -> MobileResult<Option<String>> {
        Ok(self.shared.open_state()?.device.wanted.clone())
    }

    /// Answers the Mac's request for one precise fix: the fix, or why there is none (the owner said no on the phone,
    /// Android refused, no fix in time). Never queued: the Mac waits only a short while.
    pub fn answer_location(
        &self,
        request_id: &str,
        fix: Option<Fix>,
        why_not: Option<&str>,
    ) -> MobileResult<()> {
        {
            let mut st = self.shared.open_state()?;
            if st.device.wanted.as_deref() == Some(request_id) {
                st.device.wanted = None;
            }
        }
        let t = self.shared.transport_online()?;
        let params = match fix {
            Some(f) if f.latitude.is_finite() && f.longitude.is_finite() => {
                let mut exact = json!({"latitude": f.latitude, "longitude": f.longitude});
                // a fix without an accuracy (Android's hasAccuracy false) says none rather than a made-up one
                if f.accuracy_m.is_finite() && f.accuracy_m >= 0.0 {
                    exact["accuracy_m"] = json!(f.accuracy_m);
                }
                json!({"request_id": request_id, "exact": exact})
            }
            _ => json!({"request_id": request_id,
                        "error": why_not.unwrap_or("The phone couldn't read its location.")}),
        };
        Shared::call(&t, "phone.device_report", params)?;
        Ok(())
    }

    // ---- finding the Mac's files (T-1211) ---------------------------------------------------------------------

    /// The Mac's recent files: what FERB made for the owner's tasks and what this phone fetched or sent, newest
    /// first (`phone.recent`; needs the unlock). A listing: it crosses any path.
    pub fn recent_files(&self, limit: u32) -> MobileResult<RecentFiles> {
        let t = self.shared.transport_online()?;
        let v = Shared::call(&t, "phone.recent", json!({"limit": limit.clamp(1, 200)}))?;
        Ok(RecentFiles {
            files: list(&v),
            skipped: v["skipped"].as_u64().unwrap_or(0),
        })
    }

    /// Finds files and folders whose names hold `query` under `path` on the Mac (`~` when none), never by their
    /// contents (`phone.search`; needs the unlock), a page at a time: `after` is the last page's `next` ("load more",
    /// PHN-589; a marker of another folder, query or phone is refused).
    pub fn search_files(
        &self,
        query: &str,
        path: Option<&str>,
        after: Option<&str>,
    ) -> MobileResult<FoundFiles> {
        let query = query.trim();
        if query.is_empty() {
            return Err(MobileError::new(
                ErrorCode::Refused,
                "Type part of a file's name.",
            ));
        }
        let t = self.shared.transport_online()?;
        let mut params = json!({"query": query, "path": path.unwrap_or("~")});
        if let Some(a) = after {
            params["after"] = json!(a);
        }
        let v = Shared::call(&t, "phone.search", params)?;
        Ok(FoundFiles {
            path: text(&v["path"]),
            query: text(&v["query"]),
            files: list(&v),
            looked: v["looked"].as_u64().unwrap_or(0),
            skipped: v["skipped"].as_u64().unwrap_or(0),
            cut: v["cut"].as_bool().unwrap_or(false),
            incomplete: v["incomplete"].as_bool().unwrap_or(false),
            from: v["from"].as_u64().unwrap_or(0),
            next: opt(&v["next"]),
        })
    }

    /// Fetches the `index`-th file a finished task made (its result card's file) into `save_to`, like `fetch` (direct
    /// or the owner's relay only).
    pub fn fetch_task_file(
        &self,
        id: &str,
        task_id: &str,
        index: u64,
        save_to: &str,
        sink: Arc<dyn TransferSink>,
    ) -> MobileResult<crate::remote::Fetched> {
        self.fetch_what(
            id,
            json!({"what": "file", "task_id": task_id, "index": index}),
            save_to,
            sink,
        )
    }
}

/// The Mac's pushed request for a precise fix (`{"type": "location_wanted", "request_id"}`): kept until answered.
pub(crate) fn wanted(this: &Arc<Shared>, ev: &Value) {
    let Some(id) = ev["request_id"].as_str().filter(|s| !s.is_empty()) else {
        return;
    };
    {
        let mut st = this.state.lock();
        if st.store.is_none() {
            return;
        }
        st.device.wanted = Some(id.to_string());
    }
    this.notify(Change::LocationWanted {
        request_id: id.to_string(),
    });
}
