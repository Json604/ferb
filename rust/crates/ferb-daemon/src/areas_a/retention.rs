//! The retention pass (D21; STORE-400 and 3) and the downloads folder's cap question (HAND-211). Port of
//! `ferb/daemon/areas/retention.py`. The rules are ferb-store's `retention`; this runs them. Must not purge the task
//! list, episodes, chat, results or memory.
//!
//! The stores the pass reaches outside the database's own tables (the trail, evidence, recovery copies and snapshots,
//! FERB's downloads folder, resource samples, shell logs, perception's typed text) and the episode builder come in
//! through [`Purges`]. The resource sampler (`_resources_loop`, L30) is the resource log's, not ported here.

use std::rc::Rc;

use ferb_clock::py::{self, Time};
use ferb_store::retention::{self, Plan as Rows, BATCH};
use rusqlite::types::Value as Sql;
use serde_json::{json, Map, Value};

use super::memory::{ask_owner, Answered};
use super::State;

/// Old trees purged in one pass at most.
pub const ROOTS_PER_PURGE: usize = 200;
/// After a No, or while the question waits, the downloads cap question comes again only once the folder has grown
/// past the size it was asked at by this share of the cap (ruling, step 12e).
pub const DOWNLOADS_REASK: f64 = 0.1;
/// `recovery.TRASH_RECORD_MAX_DAYS`: a trash record stays while its item is in the Trash, at most this long.
pub const TRASH_RECORD_MAX_DAYS: i64 = 90;

/// FERB's downloads folder over its cap (`downloads.over_cap()`).
#[derive(Clone, Debug, PartialEq)]
pub struct OverCap {
    pub ids: Vec<String>,
    pub total: i64,
    pub cap: i64,
    pub bytes: i64,
}

/// The stores outside the pass's own tables, each isolated: a failure is the caller's `retention.failed` row.
pub trait Purges: Send + Sync {
    /// `compose.compose(...)` of a finished tree's spans, for `episodes.record` (the trace part's).
    fn compose_episode(
        &self,
        db: &ferb_store::Db,
        task_id: &str,
    ) -> Result<Value, (String, String)>;
    /// `trail.purge_source_steps("accessibility", days)`: rows deleted.
    fn trail_accessibility(&self, days: i64) -> Result<i64, String>;
    /// `trail.purge_steps(days)`: the days purged and the rows deleted.
    fn trail(&self, days: i64) -> Result<(Vec<String>, i64), String>;
    /// `evidence.purge_before(iso)`.
    fn evidence(&self, before: &str) -> i64;
    /// `recovery.purge_before(iso, kept.db, kept.root, trash_backstop_iso=)`.
    fn recovery(&self, before: &str, trash_backstop: &str) -> i64;
    /// `resource_use.purge(db, today, days)`.
    fn resources(&self, today: chrono::NaiveDate, days: i64) -> Result<i64, String>;
    /// `shells.purge_logs(before_ts)`.
    fn shell_logs(&self, before_ts: f64) -> Result<i64, String>;
    /// `perception.purge_text()`.
    fn perception_text(&self);
    /// `downloads.over_cap()`; None: under it (or no downloads folder).
    fn downloads_over_cap(&self) -> Result<Option<OverCap>, String>;
    /// `downloads.delete(ids)`: (files, bytes) deleted, only inside the folder.
    fn downloads_delete(&self, ids: &[String]) -> (i64, i64);
}

/// `_gb` (ferb/act/browser/downloads.py:125).
pub fn gb(n: i64) -> String {
    ferb_browser::downloads::gb(n)
}

/// The pass's clock, its windows, and what it deletes (`_PurgePlan`).
pub struct PurgePlan {
    pub now: Time,
    pub windows: Vec<(&'static str, i64)>,
    pub rows: Rows,
}

impl PurgePlan {
    pub fn days(&self, window: &str) -> i64 {
        self.windows
            .iter()
            .find(|(w, _)| *w == window)
            .map_or(1, |(_, d)| *d)
    }
}

fn windows(st: &State) -> Vec<(&'static str, i64)> {
    retention::windows(&|section, key, default| {
        st.config_get(section, key)
            .and_then(|v| v.as_i64())
            .unwrap_or(default)
    })
}

fn back(now: &Time, days: i64) -> String {
    py::iso(&py::plus_us(now, -days * 86_400_000_000).unwrap_or(*now))
}

/// `_purge_plan`: read what the pass deletes, now, on the loop's connection.
pub fn purge_plan(st: &State) -> Result<PurgePlan, String> {
    let now = st.now();
    let w = windows(st);
    let day = |name: &str| w.iter().find(|(n, _)| *n == name).map_or(1, |(_, d)| *d);
    let rows = retention::plan(
        st.db().conn(),
        &back(&now, day("trace_days")),
        &back(&now, day("decision_days")),
        &back(&now, day("operational_days")),
        ROOTS_PER_PURGE,
    )
    .map_err(|e| e.to_string())?;
    Ok(PurgePlan {
        now,
        windows: w,
        rows,
    })
}

#[derive(Default)]
struct Counts {
    spans: i64,
    decisions: i64,
    operational: i64,
    trail: i64,
    roots_done: usize,
    by_table: Map<String, Value>,
}

fn texts(ids: &[String]) -> Vec<Sql> {
    ids.iter().map(|i| Sql::Text(i.clone())).collect()
}

fn delete(st: &State, table: &str, key: &str, ids: &[Sql]) -> Result<i64, String> {
    let mut n = 0;
    for step in retention::delete_rows(st.db(), table, key, ids, BATCH) {
        n += step.map_err(|e| e.to_string())? as i64;
    }
    Ok(n)
}

/// One pass (`_purge_pass`'s steps in order; Python yields to the loop between batches, which the caller does here
/// by running each pass as its own job): old trees (each once its episode exists), loose spans, Jev decisions,
/// operational records, activity rows; then evidence, recovery copies, the downloads cap, typed text, resource
/// samples and shell logs. `retention.purged` is journaled when something went or roots wait.
pub async fn purge_pass(st: &Rc<State>, purges: &dyn Purges) -> Result<Map<String, Value>, String> {
    let plan = purge_plan(st)?;
    let mut c = Counts::default();
    for (root, ids) in &plan.rows.trees {
        c.spans += purge_root(st, purges, root, ids, &plan);
        c.roots_done += 1;
        tokio::task::yield_now().await;
    }
    c.spans += delete(st, "spans", "id", &texts(&plan.rows.loose))?;
    let decisions: Vec<Sql> = plan
        .rows
        .decisions
        .iter()
        .map(|r| Sql::Integer(*r))
        .collect();
    c.decisions += delete(st, "decisions", "rowid", &decisions)?;
    for (table, ids) in &plan.rows.operational {
        let rowids: Vec<Sql> = ids.iter().map(|r| Sql::Integer(*r)).collect();
        let n = delete(st, table, "rowid", &rowids)?;
        c.operational += n;
        if n != 0 {
            let before = c.by_table.get(*table).and_then(Value::as_i64).unwrap_or(0);
            c.by_table.insert((*table).to_owned(), json!(before + n));
        }
        tokio::task::yield_now().await;
    }
    purge_trail(st, purges, &plan, &mut c);
    let out = purge_rest(st, purges, &plan, &c);
    let _ = st.db().checkpoint();
    Ok(out)
}

/// `_purge_root`: a tree's spans go only once its episode exists (built now if missing). Isolated.
fn purge_root(
    st: &State,
    purges: &dyn Purges,
    root: &str,
    ids: &[String],
    plan: &PurgePlan,
) -> i64 {
    let run = || -> Result<i64, String> {
        let db = st.db();
        if ferb_orchestrator::episodes::get(db, root)
            .map_err(|e| e.to_string())?
            .is_none()
        {
            ferb_orchestrator::episodes::record(
                db,
                || purges.compose_episode(db, root),
                &py::iso(&plan.now),
                root,
                |kind, data| {
                    st.journal(kind, data, None)
                        .map(|_| ())
                        .map_err(|e| e.message)
                },
            );
        }
        if ferb_orchestrator::episodes::get(db, root)
            .map_err(|e| e.to_string())?
            .is_none()
        {
            return Ok(0); // no episode: keep the spans
        }
        let status = st.tasks.task_column(root, "status");
        if matches!(status.as_deref(), Some("running" | "planned" | "paused")) {
            return Ok(0); // resumed since the pass read it
        }
        delete(st, "spans", "id", &texts(ids))
    };
    run().unwrap_or_else(|e| {
        let _ = st.journal(
            "retention.failed",
            json!({"task_id": root, "error": e}),
            None,
        );
        0
    })
}

/// `_purge_trail`: accessibility rows after 7 days (SENSE-1), the rest after `sense.trail_retention_days`.
fn purge_trail(st: &State, purges: &dyn Purges, plan: &PurgePlan, c: &mut Counts) {
    let run = || -> Result<i64, String> {
        let gone = purges.trail_accessibility(retention::ACCESSIBILITY_DAYS)?;
        if gone != 0 {
            st.journal(
                "trail.purged",
                json!({"source": "accessibility", "rows": gone}),
                None,
            )
            .map_err(|e| e.message)?;
        }
        let (days, rows) = purges.trail(plan.days("trail_days"))?;
        if !days.is_empty() {
            st.journal("trail.purged", json!({"days": days}), None)
                .map_err(|e| e.message)?;
        }
        Ok(gone + rows)
    };
    match run() {
        Ok(n) => c.trail += n,
        Err(e) => {
            let _ = st.journal("retention.failed", json!({"trail": true, "error": e}), None);
        }
    }
}

fn journal_purged(
    st: &State,
    plan: &PurgePlan,
    c: &Counts,
    evidence: i64,
    recovery: i64,
    left: usize,
) {
    let _ = st.journal(
        "retention.purged",
        json!({"trace_days": plan.days("trace_days"), "spans": c.spans,
               "decision_days": plan.days("decision_days"), "decisions": c.decisions,
               "evidence_days": plan.days("evidence_days"), "evidence": evidence,
               "recovery_days": plan.days("recovery_days"), "recovery": recovery,
               "operational_days": plan.days("operational_days"), "operational": c.by_table,
               "roots_left": left}),
        None,
    );
}

/// `_purge_rest`.
fn purge_rest(st: &State, purges: &dyn Purges, plan: &PurgePlan, c: &Counts) -> Map<String, Value> {
    let evidence = purges.evidence(&back(&plan.now, plan.days("evidence_days")));
    let recovery = purges.recovery(
        &back(&plan.now, plan.days("recovery_days")),
        &back(&plan.now, TRASH_RECORD_MAX_DAYS),
    );
    if c.spans != 0
        || c.decisions != 0
        || c.operational != 0
        || evidence != 0
        || recovery != 0
        || plan.rows.left != 0
    {
        journal_purged(st, plan, c, evidence, recovery, plan.rows.left);
    }
    if let Err(e) = downloads_cap(st, purges) {
        let _ = st.journal(
            "retention.failed",
            json!({"downloads": true, "error": e}),
            None,
        );
    }
    purges.perception_text();
    let resources = purges
        .resources(st.clock.zone().date(&plan.now), plan.days("resource_days"))
        .unwrap_or_else(|e| {
            let _ = st.journal(
                "retention.failed",
                json!({"resources": true, "error": e}),
                None,
            );
            0
        });
    let before = py::plus_us(&plan.now, -plan.days("evidence_days") * 86_400_000_000)
        .unwrap_or(plan.now)
        .timestamp_micros() as f64
        / 1e6;
    let shell_logs = purges.shell_logs(before).unwrap_or_else(|e| {
        let _ = st.journal(
            "retention.failed",
            json!({"shell_logs": true, "error": e}),
            None,
        );
        0
    });
    crate::shared::object(
        json!({"spans": c.spans, "decisions": c.decisions, "operational": c.operational,
                                 "trail": c.trail, "evidence": evidence, "recovery": recovery, "downloads": 0,
                                 "resources": resources, "shell_logs": shell_logs}),
    )
}

/// `_downloads_cap` (HAND-211): FERB's downloads folder over its cap: the owner is asked, outside any task, whether to
/// delete for good the oldest files that bring it back under. Asked once until the folder grows a further share of
/// the cap; journaled with counts and bytes only, never names.
pub fn downloads_cap(st: &State, purges: &dyn Purges) -> Result<(), String> {
    let Some(over) = purges.downloads_over_cap()? else {
        return Ok(());
    };
    let conn = st.db().conn();
    let asked: Option<(i64, Option<i64>)> = conn
        .query_row(
            "SELECT seq, json_extract(data_json, '$.total_bytes') FROM journal WHERE kind = 'downloads.cap_asked' \
             ORDER BY seq DESC LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok();
    let deleted: Option<i64> = conn
        .query_row(
            "SELECT seq FROM journal WHERE kind = 'downloads.deleted' ORDER BY seq DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .ok();
    if let Some((seq, total)) = asked {
        if deleted.is_none_or(|d| d < seq)
            && (over.total as f64) < total.unwrap_or(0) as f64 + over.cap as f64 * DOWNLOADS_REASK
        {
            return Ok(()); // declined or waiting, and not grown enough since
        }
    }
    let older: Vec<String> = st
        .followups
        .borrow()
        .iter()
        .filter(|(_, (kind, _))| kind.starts_with("downloads:"))
        .map(|(id, _)| id.clone())
        .collect();
    for approval_id in older {
        st.followups.borrow_mut().remove(&approval_id);
        let _ = (ferb_learning::questions::MemoryQuestions {
            store: st.store.clone(),
        })
        .answered(&approval_id);
        if st.questions.supersede(&approval_id) {
            st.publish(
                "approval.resolved",
                json!({"approval_id": approval_id, "state": "superseded"}),
            );
        }
    }
    let many = over.ids.len() != 1;
    let reason = format!(
        "Delete FERB's {} for good?\nFERB's downloads folder is at {}, over its {} limit; this frees {}. {} the \
         Trash. Nothing is deleted unless you choose Delete.",
        if many {
            format!("{} oldest downloads", over.ids.len())
        } else {
            "oldest download".to_owned()
        },
        gb(over.total),
        gb(over.cap),
        gb(over.bytes),
        if many { "They skip" } else { "It skips" }
    );
    ask_owner(
        st,
        "downloads:over_cap",
        &ferb_py::json::dumps(&json!(over.ids)),
        &reason,
        &format!("downloads-cap-{}", py::fmt(&st.now(), "%Y%m%dT%H%M%S")),
        None,
        None,
    )?;
    st.journal(
        "downloads.cap_asked",
        json!({"files": over.ids.len(), "bytes": over.bytes, "total_bytes": over.total, "cap_bytes": over.cap}),
        None,
    )
    .map_err(|e| e.message)?;
    Ok(())
}

/// `_downloads_answered`: on Yes the files the question showed are deleted for good; on No nothing is. A line.
pub fn downloads_answered(st: &State, ap: &Answered<'_>, ident: &str) -> String {
    let purges = st.purges.clone();
    let (Some(purges), true) = (purges, ap.state == "granted" && ap.choice == Some("Yes")) else {
        let _ = st.journal("downloads.cap_declined", json!({}), None);
        return "Kept: nothing was deleted from FERB's downloads folder.".into();
    };
    let ids: Vec<String> = ferb_py::json::loads(ident)
        .ok()
        .and_then(|v| v.as_array().cloned())
        .map(|a| a.iter().map(ferb_py::value::py_str).collect())
        .unwrap_or_default();
    let (count, size) = purges.downloads_delete(&ids);
    let _ = st.journal(
        "downloads.deleted",
        json!({"files": count, "bytes": size}),
        Some("owner"),
    );
    format!(
        "Deleted {count} file{} ({}) from FERB's downloads folder.",
        if count != 1 { "s" } else { "" },
        gb(size)
    )
}
