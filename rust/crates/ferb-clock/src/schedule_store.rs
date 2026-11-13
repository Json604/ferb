//! The schedule store (`Schedules`, `ferb/orchestrator/schedules.py:554`): rows written byte for byte as Python
//! writes them (`rust/fixtures/clock/schedules_store.json`), and the claim-and-handoff of a scheduled run that
//! closes the crash window of `ferb/daemon/areas/schedules.py:659-687` (see [`Schedules::claim_run`]).

use ferb_store::Db;
use rusqlite::{params, Connection, Row};
use serde_json::{Map, Value};

use crate::clock::Clock;
use crate::py::{self, Time};
use crate::schedules::{until_said, Rule, Schedule, ScheduleError, WATCHES};
use crate::timers::{self, mark_handled, Timer, TimerError, Timers};
use crate::zone::Zone;

/// `SCHEDULE_RETRY` (`areas/schedules.py:19`): a schedule timer's message for a failed run's one retry.
pub const SCHEDULE_RETRY: &str = "retry:";

/// What the store refuses or fails with.
#[derive(Debug)]
pub enum StoreError {
    /// `ScheduleError` with its words.
    Schedule(ScheduleError),
    Timer(TimerError),
    /// A stored rule or targets that `json.loads` refuses.
    BadJson(String),
    /// The timers and the schedules given are on different connections: the claim couldn't be one transaction.
    NotOneStore,
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::Schedule(e) => write!(f, "{e}"),
            StoreError::Timer(e) => write!(f, "{e}"),
            StoreError::BadJson(e) => write!(f, "{e}"),
            StoreError::NotOneStore => {
                write!(f, "the timers and the schedules must share one store")
            }
        }
    }
}

impl std::error::Error for StoreError {}

impl From<rusqlite::Error> for StoreError {
    fn from(e: rusqlite::Error) -> Self {
        StoreError::Timer(TimerError::Store(e))
    }
}

impl From<TimerError> for StoreError {
    fn from(e: TimerError) -> Self {
        StoreError::Timer(e)
    }
}

impl From<ScheduleError> for StoreError {
    fn from(e: ScheduleError) -> Self {
        StoreError::Schedule(e)
    }
}

const SELECT: &str =
    "SELECT s.*, e.ends_at, n.name FROM schedules s LEFT JOIN schedule_ends e ON e.schedule_id = \
                      s.id LEFT JOIN schedule_names n ON n.schedule_id = s.id";

fn rule_json(rule: &Rule, first: &Time) -> String {
    let mut m = rule.to_json();
    m.insert("first".into(), Value::String(py::iso(&py::utc(first))));
    ferb_py::json::dumps(&Value::Object(m))
}

fn row(r: &Row<'_>) -> Result<Schedule, StoreError> {
    let rule_text: String = r.get("rule_json")?;
    let mut rule = match ferb_py::json::loads(&rule_text).map_err(StoreError::BadJson)? {
        Value::Object(m) => m,
        _ => return Err(StoreError::BadJson("a rule is an object".into())),
    };
    let first = rule
        .remove("first")
        .and_then(|v| v.as_str().map(str::to_string))
        .ok_or_else(|| StoreError::BadJson("a rule has its first run".into()))?;
    let targets = match ferb_py::json::loads(&r.get::<_, String>("intent_json")?)
        .map_err(StoreError::BadJson)?
    {
        Value::Object(m) => m,
        _ => Map::new(),
    };
    let opt_time = |k: &str| -> Result<Option<Time>, StoreError> {
        match r.get::<_, Option<String>>(k)?.filter(|t| !t.is_empty()) {
            Some(t) => Ok(Some(timers::time_of(&t)?)),
            None => Ok(None),
        }
    };
    let met: Option<i64> = r.get("last_reported_met")?;
    Ok(Schedule {
        id: r.get("id")?,
        request: r.get("request")?,
        own_words: r.get("own_words")?,
        targets,
        rule: Rule::from_json(&rule),
        next_run: opt_time("next_run")?,
        timer_id: r.get("timer_id")?,
        state: r.get("state")?,
        failures: r.get("failures")?,
        first_run: timers::time_of(&first)?,
        watch: r.get("watch")?,
        last_reported_met: met.map(|m| m != 0),
        until: opt_time("ends_at")?,
        name: r.get::<_, Option<String>>("name")?.unwrap_or_default(),
    })
}

/// A schedule's run taken from its timer (see [`Schedules::claim_run`]).
#[derive(Clone, Debug, PartialEq)]
pub enum RunClaim {
    /// This timer's run was already handed to a task (a redelivery): acknowledge the timer, nothing more.
    AlreadyHandled,
    /// No schedule, or one that isn't active (Python's early return): acknowledge the timer, nothing more.
    NotRunnable,
    /// A run to start: the next run is booked (or the schedule ended), and this timer stays claimed until
    /// [`Schedules::start_run`] records the task in the same transaction that marks the timer handled.
    Run(Box<ClaimedRun>),
}

/// A claimed run.
#[derive(Clone, Debug, PartialEq)]
pub struct ClaimedRun {
    /// The schedule as it stood before this claim changed it (`s` in `_run_schedule`).
    pub schedule: Schedule,
    pub timer_id: String,
    /// A failed run's one retry (`retry:` timers): the regular runs were left alone.
    pub retry: bool,
    pub overdue_s: f64,
    /// The next run this claim booked: when and its timer.
    pub booked: Option<(Time, String)>,
    /// This claim found no next run and marked the schedule done.
    pub ended: bool,
    /// Its end, not its rule, made this the last run (ORCH-320: the daemon journals `schedule.ended`).
    pub ended_by_end: bool,
    /// A crash came between an earlier claim and its task: the next run was already booked then.
    pub redelivered: bool,
}

/// The schedule store.
pub struct Schedules<'a> {
    db: &'a Db,
    clock: &'a dyn Clock,
    ids: Box<dyn Fn() -> String + 'a>,
    zone: Zone,
}

impl<'a> Schedules<'a> {
    /// `zone`: the Mac's (Python's `local_zone()`, used in `until_said`).
    pub fn new(
        db: &'a Db,
        clock: &'a dyn Clock,
        ids: impl Fn() -> String + 'a,
        zone: Zone,
    ) -> Self {
        Schedules {
            db,
            clock,
            ids: Box::new(ids),
            zone,
        }
    }

    /// `add` (`schedules.py:569`).
    #[allow(clippy::too_many_arguments)]
    pub fn add(
        &self,
        request: &str,
        own_words: &str,
        targets: &Map<String, Value>,
        rule: &Rule,
        first: &Time,
        watch: &str,
        until: Option<&Time>,
        name: &str,
    ) -> Result<Schedule, StoreError> {
        if *first <= self.clock.now() {
            return Err(
                ScheduleError::Invalid("the first run must be in the future".into()).into(),
            );
        }
        if !WATCHES.contains(&watch) {
            return Err(
                ScheduleError::Invalid(format!("unknown watch {}", py::repr(watch))).into(),
            );
        }
        if let Some(u) = until {
            if u < first {
                return Err(ScheduleError::Invalid(format!(
                    "it would end ({}) before its first run",
                    until_said(u, &self.zone)
                ))
                .into());
            }
        }
        let sid = format!("sc-{}", py::prefix(&(self.ids)(), 10));
        self.db.conn().execute(
            "INSERT INTO schedules (id, request, own_words, intent_json, rule_json, next_run, state, created_at, \
             watch) VALUES (?, ?, ?, ?, ?, ?, 'active', ?, ?)",
            params![
                sid,
                request,
                own_words,
                ferb_py::json::dumps(&Value::Object(targets.clone())),
                rule_json(rule, first),
                py::iso(&py::utc(first)),
                py::iso(&self.clock.now()),
                watch
            ],
        )?;
        if let Some(u) = until {
            self.set_until(&sid, u)?;
        }
        if !py::strip(name).is_empty() {
            self.set_name(&sid, name)?;
        }
        Ok(self.get(&sid)?.expect("the row just written"))
    }

    /// `edit` (T-1066): changes in place; `rule` with `first`, `targets` and the owner's words for the change.
    pub fn edit(
        &self,
        sid: &str,
        request: Option<&str>,
        rule: Option<(&Rule, &Time)>,
        targets: Option<&Map<String, Value>>,
        own_words: &str,
    ) -> Result<(), StoreError> {
        let mut sets: Vec<&str> = Vec::new();
        let mut args: Vec<String> = Vec::new();
        if let Some(r) = request {
            sets.push("request = ?");
            args.push(r.to_string());
        }
        if let Some((rule, first)) = rule {
            sets.push("rule_json = ?");
            args.push(rule_json(rule, first));
        }
        if let Some(t) = targets {
            sets.push("intent_json = ?");
            args.push(ferb_py::json::dumps(&Value::Object(t.clone())));
        }
        if !py::strip(own_words).is_empty() {
            sets.push("own_words = own_words || ?");
            args.push(format!("\n{own_words}"));
        }
        if !sets.is_empty() {
            args.push(sid.to_string());
            self.db.conn().execute(
                &format!("UPDATE schedules SET {} WHERE id = ?", sets.join(", ")),
                rusqlite::params_from_iter(args.iter()),
            )?;
        }
        Ok(())
    }

    /// Its end (ORCH-320), replacing any earlier one.
    pub fn set_until(&self, sid: &str, until: &Time) -> Result<(), StoreError> {
        self.db.conn().execute(
            "INSERT INTO schedule_ends (schedule_id, ends_at) VALUES (?, ?) ON CONFLICT(schedule_id) DO UPDATE SET \
             ends_at = excluded.ends_at",
            params![sid, py::iso(&py::utc(until))],
        )?;
        Ok(())
    }

    /// Its short name (T-1055), replacing any earlier one.
    pub fn set_name(&self, sid: &str, name: &str) -> Result<(), StoreError> {
        self.db.conn().execute(
            "INSERT INTO schedule_names (schedule_id, name) VALUES (?, ?) ON CONFLICT(schedule_id) DO UPDATE SET \
             name = excluded.name",
            params![sid, py::squeeze(name)],
        )?;
        Ok(())
    }

    pub fn get(&self, sid: &str) -> Result<Option<Schedule>, StoreError> {
        get_on(self.db.conn(), sid)
    }

    /// The schedules in these states (default active and paused), by next run.
    pub fn all(&self, states: &[&str]) -> Result<Vec<Schedule>, StoreError> {
        let q = vec!["?"; states.len()].join(",");
        let mut st = self.db.conn().prepare(&format!(
            "{SELECT} WHERE s.state IN ({q}) ORDER BY s.next_run"
        ))?;
        let mut rows = st.query(rusqlite::params_from_iter(states.iter()))?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            out.push(row(r)?);
        }
        Ok(out)
    }

    pub fn set_timer(
        &self,
        sid: &str,
        timer_id: Option<&str>,
        next_run: Option<&Time>,
    ) -> Result<(), StoreError> {
        set_timer_on(self.db.conn(), sid, timer_id, next_run)
    }

    pub fn set_state(&self, sid: &str, state: &str) -> Result<(), StoreError> {
        self.db.conn().execute(
            "UPDATE schedules SET state = ? WHERE id = ?",
            params![state, sid],
        )?;
        Ok(())
    }

    /// A watch's finished check reported its condition.
    pub fn reported(&self, sid: &str, met: bool) -> Result<(), StoreError> {
        self.db.conn().execute(
            "UPDATE schedules SET last_reported_met = ? WHERE id = ?",
            params![i64::from(met), sid],
        )?;
        Ok(())
    }

    /// Record a run; returns the failures in a row. `None`: finished but unconfirmed (neither clears nor counts).
    pub fn ran(&self, sid: &str, ok: Option<bool>) -> Result<i64, StoreError> {
        let c = self.db.conn();
        let now = py::iso(&self.clock.now());
        let failures = |c: &Connection| -> Result<i64, StoreError> {
            Ok(c.query_row(
                "SELECT failures FROM schedules WHERE id = ?",
                params![sid],
                |r| r.get(0),
            )?)
        };
        match ok {
            None => {
                c.execute(
                    "UPDATE schedules SET last_run = ? WHERE id = ?",
                    params![now, sid],
                )?;
                failures(c)
            }
            Some(true) => {
                c.execute(
                    "UPDATE schedules SET failures = 0, last_run = ? WHERE id = ?",
                    params![now, sid],
                )?;
                Ok(0)
            }
            Some(false) => {
                c.execute(
                    "UPDATE schedules SET failures = failures + 1, last_run = ? WHERE id = ?",
                    params![now, sid],
                )?;
                failures(c)
            }
        }
    }

    // ---------- a run's claim and handoff (fixed against Python) ----------

    /// The first half of a scheduled run, for the timer consumer (`_timer_fired` → `_run_schedule`, Python's
    /// `areas/schedules.py:147-156, 651-666`), in one transaction: book the schedule's next run (or mark it done),
    /// leaving this timer claimed and *not* handled. The run is handed to its task by [`Schedules::start_run`],
    /// which marks the timer handled and acknowledges it in the transaction that creates the task. The consumer
    /// returns `Delivery::TakenOver` for a [`RunClaim::Run`] and `Delivery::Done` otherwise.
    ///
    /// Fixed against Python: Python marks the timer handled (`:150`), books the next run (`:659-661`), and only later
    /// creates the task (`:684-687`); a crash in between lost that run for good. Here a crash before
    /// `start_run` leaves the timer claimed, so it is handed over again at the next start; the next run is not
    /// booked twice, because the schedule's timer is then no longer this one (`redelivered`).
    pub fn claim_run(&self, timers: &Timers<'_>, timer: &Timer) -> Result<RunClaim, StoreError> {
        if !std::ptr::eq(timers.db(), self.db) {
            return Err(StoreError::NotOneStore);
        }
        let now = self.clock.now();
        self.db.transaction(|c| -> Result<RunClaim, StoreError> {
            if timers.delivered(&timer.id)? {
                return Ok(RunClaim::AlreadyHandled);
            }
            let retry = timer.message.starts_with(SCHEDULE_RETRY);
            let sid = timer
                .message
                .strip_prefix(SCHEDULE_RETRY)
                .unwrap_or(&timer.message);
            let Some(s) = get_on(c, sid)? else {
                return Ok(RunClaim::NotRunnable);
            };
            let redelivered = !retry && s.timer_id.as_deref() != Some(timer.id.as_str());
            let runnable = s.state == "active" || (redelivered && s.state == "done");
            if !runnable {
                return Ok(RunClaim::NotRunnable);
            }
            let mut claim = ClaimedRun {
                schedule: s.clone(),
                timer_id: timer.id.clone(),
                retry,
                overdue_s: timer.overdue_s.unwrap_or(0.0),
                booked: None,
                ended: false,
                ended_by_end: false,
                redelivered,
            };
            if retry || redelivered {
                return Ok(RunClaim::Run(Box::new(claim))); // the regular next run stays as it is
            }
            match s.next_after(&now)? {
                Some(nxt) => {
                    let t = timers.add(
                        "schedule",
                        &nxt,
                        &s.id,
                        None,
                        None,
                        s.rule.when_active,
                        None,
                        "",
                        None,
                    )?;
                    set_timer_on(c, &s.id, Some(&t.id), Some(&nxt))?;
                    claim.booked = Some((nxt, t.id));
                }
                None => {
                    set_timer_on(c, &s.id, None, None)?;
                    c.execute(
                        "UPDATE schedules SET state = 'done' WHERE id = ?",
                        params![s.id],
                    )?;
                    claim.ended = true;
                    claim.ended_by_end = s.ends_after(&now)?;
                }
            }
            Ok(RunClaim::Run(Box::new(claim)))
        })
    }

    /// The second half: in one transaction, record that this run's timer was handled, acknowledge it, and run
    /// `create` (the caller's task row and its own records, on the same connection). If `create` fails nothing is
    /// recorded and the timer stays claimed, so it is handed over again. A run skipped on purpose (the previous run
    /// still under way, FERB stopped) passes a `create` that records the skip.
    pub fn start_run<T, E>(
        &self,
        timers: &Timers<'_>,
        run: &ClaimedRun,
        create: impl FnOnce(&Connection) -> Result<T, E>,
    ) -> Result<T, E>
    where
        E: From<rusqlite::Error> + From<StoreError>,
    {
        if !std::ptr::eq(timers.db(), self.db) {
            return Err(StoreError::NotOneStore.into());
        }
        let now = self.clock.now();
        self.db.transaction(|c| -> Result<T, E> {
            mark_handled(c, &run.timer_id, &now).map_err(StoreError::from)?;
            let out = create(c)?;
            timers.ack(&run.timer_id).map_err(StoreError::from)?;
            Ok(out)
        })
    }
}

fn get_on(c: &Connection, sid: &str) -> Result<Option<Schedule>, StoreError> {
    let mut st = c.prepare(&format!("{SELECT} WHERE s.id = ?"))?;
    let mut rows = st.query(params![sid])?;
    match rows.next()? {
        Some(r) => Ok(Some(row(r)?)),
        None => Ok(None),
    }
}

fn set_timer_on(
    c: &Connection,
    sid: &str,
    timer_id: Option<&str>,
    next_run: Option<&Time>,
) -> Result<(), StoreError> {
    c.execute(
        "UPDATE schedules SET timer_id = ?, next_run = ? WHERE id = ?",
        params![timer_id, next_run.map(|n| py::iso(&py::utc(n))), sid],
    )?;
    Ok(())
}
