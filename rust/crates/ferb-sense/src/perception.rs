//! Perception's wiring (`ferb/sense/perception.py`): Notification Center records into the trail exactly once, FERB's
//! own banners recorded as FERB's (never an arrival), the install-time floor, the installed-app list the software
//! watcher reads, the 7-day expiry of typed and copied text, and perception's status. Jev's decisions stay
//! observations, never owner authority (SENSE-2: the batcher is built switched off).

use std::cell::RefCell;
use std::collections::BTreeSet;

use chrono::TimeDelta;
use ferb_core::interfaces::Clock;
use ferb_core::when::isoformat;
use ferb_sanitize::parity::lower;
use ferb_store::retention::TYPED_DAYS;
use ferb_store::Db;
use rusqlite::{params, OptionalExtension};
use serde_json::{Map, Value};

use crate::notifications::NotificationPolicy;
use crate::observation::{py_str, Dict, Observation};
use crate::trail::Trail;

/// device/install.py:61: FERB.app's bundle id (it posts only FERB's own banners).
pub const APP_BUNDLE_ID: &str = "com.ferb.FERB";
/// device/install.py:79: the development notch build.
pub const DEV_APP_BUNDLE_ID: &str = "dev.ferb.notch";

/// A macOS banner FERB posted that Notification Center hasn't shown back yet (L10).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Banner {
    pub title: String,
    pub body: String,
    pub poster: String,
    pub at: String,
}

/// What perception needs from other parts (seams).
pub struct Seams<'a> {
    /// `daemon._journal().append(kind, data)`.
    pub journal: Box<dyn Fn(&str, Value) + 'a>,
    /// `daemon.sense.paused`.
    pub paused: Box<dyn Fn() -> bool + 'a>,
    /// `daemon.daily_activity.roll_over()`.
    pub roll_over: Box<dyn Fn() + 'a>,
    /// `orchestrator/attention.py` `expire_derived(db, refs, replacement, now)`: notices derived from expired rows.
    pub expire_derived: Box<dyn Fn(&BTreeSet<String>, &str, &str) + 'a>,
}

/// perception.py:12.
pub struct Perception<'a, 't> {
    db: &'a Db,
    clock: &'a dyn Clock,
    trail: &'a Trail<'t>,
    seams: Seams<'a>,
    pub policy: NotificationPolicy<'a>,
    pub banners: RefCell<Vec<Banner>>,
}

impl<'a, 't> Perception<'a, 't> {
    pub fn new(
        db: &'a Db,
        clock: &'a dyn Clock,
        trail: &'a Trail<'t>,
        policy: NotificationPolicy<'a>,
        seams: Seams<'a>,
    ) -> Perception<'a, 't> {
        Perception {
            db,
            clock,
            trail,
            seams,
            policy,
            banners: RefCell::new(Vec::new()),
        }
    }

    /// perception.py:48: how macOS treats FERB reading the clipboard changed (or was first seen).
    pub fn clipboard_access(&self, state: &str) {
        let mut d = Map::new();
        d.insert("access".into(), Value::from(state));
        (self.seams.journal)("clipboard.access", Value::Object(d));
    }

    /// perception.py:52.
    pub fn seen(&self, external_id: &str) -> rusqlite::Result<bool> {
        Ok(self
            .db
            .conn()
            .query_row(
                "SELECT 1 FROM perception_seen WHERE source='notification' AND external_id=?",
                params![external_id],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    /// perception.py:56 (I1): the first Notification Center read after perception's install marks every record
    /// already there as seen (never recorded); every later read passes through.
    pub fn notification_floor(
        &self,
        observations: Vec<Observation>,
    ) -> rusqlite::Result<Vec<Observation>> {
        let c = self.db.conn();
        let row: Option<Option<i64>> = c
            .query_row(
                "SELECT seq FROM perception_floor WHERE source='notification'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        match row {
            Some(None) => {}
            _ => return Ok(observations),
        }
        for o in &observations {
            c.execute(
                "INSERT OR IGNORE INTO perception_seen VALUES('notification',?,NULL)",
                params![py_str(o.data.get("external_id"))],
            )?;
        }
        c.execute(
            "UPDATE perception_floor SET seq=?,at=? WHERE source='notification'",
            params![observations.len() as i64, isoformat(&self.clock.now())],
        )?;
        Ok(Vec::new())
    }

    /// perception.py:70: FERB is posting a banner with this (redacted) title and body under the app id `poster`.
    pub fn banner_posted(&self, title: &str, body: &str, poster: &str) -> Banner {
        let b = Banner {
            title: title.into(),
            body: body.into(),
            poster: poster.into(),
            at: isoformat(&self.clock.now()),
        };
        self.banners.borrow_mut().push(b.clone());
        b
    }

    /// perception.py:77: that banner was never posted.
    pub fn banner_failed(&self, banner: &Banner) {
        let mut banners = self.banners.borrow_mut();
        if let Some(i) = banners.iter().position(|b| b == banner) {
            banners.remove(i);
        }
    }

    /// perception.py:81 `_own_banner`.
    pub fn own_banner(&self, o: &Observation) -> Observation {
        let bundle = o
            .data
            .get("bundle_id")
            .map(ferb_py::value::py_str)
            .unwrap_or_default();
        let without_body = |extra: Value| {
            let mut kept: Dict = o
                .data
                .iter()
                .filter(|(k, _)| *k != "body")
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            kept.insert("ferb_banner".into(), extra);
            Observation::by(&o.source, kept, "ferb")
        };
        let b = lower(&bundle);
        if b == lower(APP_BUNDLE_ID) || b == lower(DEV_APP_BUNDLE_ID) {
            let mut extra = Map::new();
            extra.insert("poster".into(), Value::from(bundle.clone()));
            return without_body(Value::Object(extra));
        }
        let mut banners = self.banners.borrow_mut();
        let hit = banners.iter().position(|banner| {
            lower(&banner.poster) == b
                && o.data.get("title") == Some(&Value::from(banner.title.as_str()))
                && o.data.get("body") == Some(&Value::from(banner.body.as_str()))
        });
        if let Some(i) = hit {
            let banner = banners.remove(i);
            let mut extra = Map::new();
            extra.insert("posted_at".into(), Value::from(banner.at));
            return without_body(Value::Object(extra));
        }
        o.clone()
    }

    /// perception.py:100: record first; if the daemon dies before the receipt, the durable row prevents a duplicate.
    pub fn notification(&self, o: &Observation) -> crate::trail::Result<()> {
        if (self.seams.paused)() {
            return Ok(());
        }
        let external_id = py_str(o.data.get("external_id"));
        if self.seen(&external_id)? {
            return Ok(());
        }
        let c = self.db.conn();
        let previous: Option<String> = c
            .query_row(
                "SELECT id FROM trail WHERE source='notification' AND json_extract(data_json,'$.external_id')=?",
                params![external_id],
                |r| r.get(0),
            )
            .optional()?;
        let event_id = match previous {
            Some(p) => Some(p),
            None => self.trail.record(&self.own_banner(o))?.map(|e| e.id),
        };
        if let Some(id) = event_id {
            c.execute(
                "INSERT OR IGNORE INTO perception_seen VALUES('notification',?,?)",
                params![external_id, id],
            )?;
        }
        Ok(())
    }

    /// perception.py:115: (bundle id, path) of the apps the trail saw installed and not removed since.
    pub fn installed(&self) -> rusqlite::Result<Vec<(String, String)>> {
        let mut st = self
            .db
            .conn()
            .prepare("SELECT data_json FROM trail WHERE source='app_install' ORDER BY seq")?;
        let rows = st.query_map([], |r| r.get::<_, String>(0))?;
        let mut apps: Vec<(String, String)> = Vec::new();
        for r in rows {
            let data = ferb_py::json::loads(&r?).unwrap_or(Value::Null);
            let get = |k: &str| data.as_object().and_then(|m| m.get(k));
            let change = get("change");
            if change == Some(&Value::from("removed")) {
                let path = get("path");
                apps.retain(|(_, p)| Some(&Value::from(p.as_str())) != path);
            } else if change == Some(&Value::from("installed"))
                && get("bundle_id").is_some_and(ferb_py::value::truthy)
                && get("path").is_some_and(ferb_py::value::truthy)
            {
                let bundle = py_str(get("bundle_id"));
                let path = py_str(get("path"));
                match apps.iter_mut().find(|(b, _)| *b == bundle) {
                    Some(slot) => slot.1 = path,
                    None => apps.push((bundle, path)),
                }
            }
        }
        Ok(apps)
    }

    /// perception.py:131 (D14/P8): typed and copied text expires after `TYPED_DAYS` even without a daily summary;
    /// source ids stay, and notice text derived from any expired row expires with it. How many rows this pass expired.
    pub fn purge_text(&self) -> rusqlite::Result<usize> {
        let now = self.clock.now();
        let cutoff = isoformat(&(now - TimeDelta::days(TYPED_DAYS)));
        let c = self.db.conn();
        let rows: Vec<(String, String)> = {
            let mut st = c.prepare(
                "SELECT id,data_json FROM trail WHERE source IN ('typed_text','clipboard') AND at < ? AND \
                 json_extract(data_json,'$.content_expired') IS NULL",
            )?;
            let r = st.query_map(params![cutoff], |r| Ok((r.get(0)?, r.get(1)?)))?;
            r.collect::<rusqlite::Result<_>>()?
        };
        for (id, data) in &rows {
            let data = ferb_py::json::loads(data).unwrap_or(Value::Null);
            let mut kept = Map::new();
            for k in [
                "bundle_id",
                "app",
                "pid",
                "window_id",
                "field_id",
                "role",
                "change_count",
            ] {
                if let Some(v) = data.as_object().and_then(|m| m.get(k)) {
                    kept.insert(k.into(), v.clone());
                }
            }
            kept.insert("content_expired".into(), Value::Bool(true));
            c.execute(
                "UPDATE trail SET data_json=? WHERE id=?",
                params![ferb_py::json::dumps(&Value::Object(kept)), id],
            )?;
        }
        if !rows.is_empty() {
            (self.seams.roll_over)();
        }
        let expired: BTreeSet<String> = {
            let mut st = c.prepare(
                "SELECT id FROM trail WHERE source IN ('typed_text','clipboard') \
                 AND json_extract(data_json,'$.content_expired') IS NOT NULL",
            )?;
            let r = st.query_map([], |r| r.get::<_, String>(0))?;
            r.map(|id| id.map(|id| format!("trail:{id}")))
                .collect::<rusqlite::Result<_>>()?
        };
        if !expired.is_empty() {
            (self.seams.expire_derived)(
                &expired,
                &format!(
                    "This notice came from typed or copied text, which expires after {TYPED_DAYS} days."
                ),
                &isoformat(&self.clock.now()),
            );
        }
        Ok(rows.len())
    }

    /// perception.py:159: `unsent_events` and `batch_age_s` come from the batcher.
    pub fn status(&self, unsent_events: i64, batch_age_s: f64) -> rusqlite::Result<Value> {
        let mut states = Map::new();
        {
            let mut st = self
                .db
                .conn()
                .prepare("SELECT state,COUNT(*) n FROM perception_batches GROUP BY state")?;
            let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
            for r in rows {
                let (s, n) = r?;
                states.insert(s, Value::from(n));
            }
        }
        let mut now_playing = Map::new();
        now_playing.insert("status".into(), Value::from("pending_owner_probe"));
        now_playing.insert(
            "detail".into(),
            Value::from(
                "MediaRemote access after macOS 15.4 is unverified; no fabricated playback events.",
            ),
        );
        let mut o = Map::new();
        o.insert("batches".into(), Value::Object(states));
        o.insert("unsent_events".into(), Value::from(unsent_events));
        o.insert("batch_age_s".into(), Value::from(batch_age_s));
        o.insert("notification_triage".into(), self.policy.measurements()?);
        o.insert("now_playing".into(), Value::Object(now_playing));
        Ok(Value::Object(o))
    }
}
