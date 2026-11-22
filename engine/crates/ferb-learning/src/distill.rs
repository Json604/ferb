//! `distill.py`'s daily pass, its code half only. For each past day the permanent summary is written by code
//! (`summary.rs`, no model); retention purges only the days it summarized. The model half (the day narrative
//! `trail.day_narrative` and the facts and retirements it proposed) is off: the owner turned it off (MEM-468). The
//! narratives already stored stay and are read as before.
//!
//! A day whose summary fails is journaled `memory.distill_failed` and tried again on the next hourly passes, at most
//! [`SUMMARY_TRIES`] times in all (MEM-470; the owner's number, MEM-473); one day's failure never stops the
//! days after it.

use std::collections::BTreeSet;
use std::rc::Rc;

use serde_json::{json, Value};

use ferb_memory::times::{self, LocalZone};

use crate::journal::Journal;
use crate::py;
use crate::sql;
use crate::store::Store;
use crate::summary;
use crate::{Clock, Result};

/// How many times a day's code summary is tried before the hourly pass leaves it (MEM-470; the owner's number,
/// MEM-473). Its activity stays in the trail until it is summarized.
pub const SUMMARY_TRIES: i64 = 3;

/// What `distill_runs.detail` says for a day done by the code half alone.
pub const SUMMARY_ONLY: &str = "summary only: the day narrative is off (MEM-468)";

/// One quiet line in the owner's chat, once per id (the daemon's notice line, journaled `chat.notice`).
#[derive(Clone, Debug, PartialEq)]
pub struct Notice {
    pub id: String,
    pub refs: Vec<String>,
    pub text: String,
    pub origin: String,
    /// Note cards, for the import's summary notice.
    pub cards: Option<Value>,
}

/// distill.py:336 `yesterday`.
pub fn yesterday(now: &crate::DateTime, zone: &dyn LocalZone) -> String {
    let local = py::local(now, zone);
    times::ymd(&py::shift(&local, -86_400_000_000))
}

/// The daily pass over past days (distill.py:112 `Distiller`, its code half).
pub struct Distiller<'a> {
    pub store: Rc<Store>,
    pub journal: Rc<Journal>,
    pub clock: Rc<dyn Clock>,
    pub zone: Rc<dyn LocalZone>,
    pub summary: Rc<dyn summary::Seams>,
    pub purge: Box<dyn Fn() + 'a>,
}

impl Distiller<'_> {
    /// distill.py:124 `pending_days`: every past day with activity or the owner's words that isn't done or has no
    /// summary, except a day whose summary failed [`SUMMARY_TRIES`] times (MEM-470).
    pub fn pending_days(&self) -> Result<Vec<String>> {
        let today = py::local_day(&self.clock.now(), self.zone.as_ref());
        let mut days: BTreeSet<String> = self
            .store
            .query("SELECT DISTINCT day FROM trail WHERE day < ?", sql![today])?
            .iter()
            .map(|r| r.s("day"))
            .collect();
        for r in self
            .store
            .query("SELECT at FROM journal WHERE kind = 'chat.message'", sql![])?
        {
            let d = py::local_day(&py::parse(&r.s("at"))?, self.zone.as_ref());
            if d < today {
                days.insert(d);
            }
        }
        let done = self.days_where("status = 'distilled'", None)?;
        let given_up =
            self.days_where("status = 'failed' AND attempts >= ?", Some(SUMMARY_TRIES))?;
        let mut out = Vec::new();
        for d in days {
            let summarized = summary::summarized(&self.store, &d)?;
            // a day the model half failed on has its summary: it is finished now, whatever its tries
            if (summarized && done.contains(&d)) || (!summarized && given_up.contains(&d)) {
                continue;
            }
            out.push(d);
        }
        Ok(out)
    }

    fn days_where(&self, cond: &str, n: Option<i64>) -> Result<BTreeSet<String>> {
        let q = format!("SELECT day FROM distill_runs WHERE {cond}");
        let rows = match n {
            Some(n) => self.store.query(&q, sql![n])?,
            None => self.store.query(&q, sql![])?,
        };
        Ok(rows.iter().map(|r| r.s("day")).collect())
    }

    /// distill.py:138 `write_summary`: the day's summary by code, once.
    pub fn write_summary(&self, day: &str) -> Result<()> {
        if !summary::summarized(&self.store, day)? {
            let ev = summary::build(&self.store, self.summary.as_ref(), self.zone.as_ref(), day)?;
            let mut digest = ev.digest;
            digest["made_by"] = json!(summary::SUMMARY_MADE_BY);
            self.journal
                .append(summary::SUMMARY_KIND, Some(digest), None)?;
        }
        Ok(())
    }

    /// distill.py:145 `run_day`, its code half: the summary, then the day is done. No model runs (MEM-468). A failed
    /// summary is recorded (`failed`, one more try) and journaled `memory.distill_failed` with the tries so far.
    pub async fn run_day(&self, day: &str) -> Result<Value> {
        if let Some(row) = self
            .store
            .one("SELECT status FROM distill_runs WHERE day = ?", sql![day])?
        {
            if row.s("status") == "distilled" && summary::summarized(&self.store, day)? {
                return Ok(json!({"day": day, "status": "distilled"}));
            }
        }
        if let Err(e) = self.write_summary(day) {
            let why = e.described();
            self.run(day, "failed", Some(&why))?;
            let attempts = self
                .store
                .count("SELECT attempts FROM distill_runs WHERE day = ?", sql![day])?;
            self.journal.append(
                "memory.distill_failed",
                Some(json!({"day": day, "error": why, "attempts": attempts})),
                None,
            )?;
            return Ok(json!({"day": day, "status": "failed", "error": why}));
        }
        self.run(day, "distilled", Some(SUMMARY_ONLY))?;
        Ok(json!({"day": day, "status": "distilled"}))
    }

    /// distill.py:293 `catch_up`.
    pub async fn catch_up(&self) -> Result<Vec<Value>> {
        let mut out = Vec::new();
        for d in self.pending_days()? {
            out.push(self.run_day(&d).await?);
        }
        (self.purge)();
        Ok(out)
    }

    /// One more try of `day`, its status and detail (a `distilled` day stays so).
    fn run(&self, day: &str, status: &str, detail: Option<&str>) -> Result<()> {
        let now = py::iso(&self.clock.now());
        self.store.exec(
            "INSERT INTO distill_runs (day, attempts, status, at, detail) VALUES (?, 1, ?, ?, ?) \
             ON CONFLICT(day) DO UPDATE SET attempts = attempts + 1, status = CASE WHEN distill_runs.status = \
             'distilled' THEN 'distilled' ELSE excluded.status END, at = excluded.at, detail = COALESCE(excluded.detail, \
             distill_runs.detail)",
            sql![day, status, now, detail.map(str::to_owned)],
        )?;
        Ok(())
    }
}
