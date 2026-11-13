//! Port of `ferb/clock/timers.py`: durable timers, SQLite-backed, delivered exactly once.
//!
//! - Due times are checked against the wall clock at least every [`POLL_S`]: the monotonic clock stops while the Mac
//!   sleeps, so a timer due during sleep (or while FERB was down) fires on the next check, with `overdue_s`.
//! - Exactly once: a due timer is claimed (pending → claimed, with a token), handed to its consumer, then
//!   acknowledged (→ delivered). A crash between claim and acknowledgement leaves it claimed; it is handed over again
//!   on start, and the consumer skips a timer it already acted on ([`mark_handled`]).
//! - When the owner is at the Mac (ORCH-317): a timer added `when_active` is not claimed while the Mac is out of use;
//!   one with `return_after` waits for an active stretch that began after that time. The Mac's presence comes in
//!   through [`Presence`].
//!
//! Rows are written byte for byte as Python writes them (decision: ENG-3; `rust/fixtures/clock/timers.json`).
//! Decisions: DMN-547, DMN-548, ORCH-511, ORCH-542.

use std::future::Future;

use ferb_store::Db;
use rusqlite::{named_params, params, OptionalExtension, Row};
use tokio::sync::Notify;

use crate::clock::Clock;
use crate::py::{self, Time};

/// The longest wait between two looks at the wall clock.
pub const POLL_S: f64 = 30.0;

/// `recheck_in`: seconds to wait before looking at the wall clock again for something due at `due`.
pub fn recheck_in(due: &Time, now: &Time) -> f64 {
    py::total_seconds(due, now).clamp(0.0, POLL_S)
}

/// A timer's row (`Timer`, `timers.py:38`).
#[derive(Clone, Debug, PartialEq)]
pub struct Timer {
    pub id: String,
    pub kind: String,
    pub fire_at: Time,
    pub message: String,
    pub task_id: Option<String>,
    pub request_id: Option<String>,
    pub state: String,
    pub overdue_s: Option<f64>,
    /// Its short name (T-1055); "" for one made before, and in a claimed row.
    pub name: String,
    pub created_at: Option<Time>,
}

/// What a timer's row state means to the owner (DMN-548).
pub fn shown_state(state: &str) -> &str {
    match state {
        "pending" | "claimed" => "running",
        "delivered" => "fired",
        "cancelled" => "cancelled",
        other => other,
    }
}

/// What can go wrong (Python's exceptions from the store).
#[derive(Debug)]
pub enum TimerError {
    /// The database refused (sqlite3's errors).
    Store(rusqlite::Error),
    /// A stored time Python's `fromisoformat` would refuse.
    BadTime(String),
}

impl std::error::Error for TimerError {}

impl std::fmt::Display for TimerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TimerError::Store(e) => write!(f, "{e}"),
            TimerError::BadTime(t) => write!(f, "Invalid isoformat string: {}", py::repr(t)),
        }
    }
}

impl From<rusqlite::Error> for TimerError {
    fn from(e: rusqlite::Error) -> Self {
        TimerError::Store(e)
    }
}

pub(crate) fn time_of(text: &str) -> Result<Time, TimerError> {
    py::parse(text).ok_or_else(|| TimerError::BadTime(text.to_string()))
}

fn row(r: &Row<'_>, with_name: bool) -> Result<Timer, TimerError> {
    let created: Option<String> = r.get("created_at")?;
    Ok(Timer {
        id: r.get("id")?,
        kind: r.get("kind")?,
        fire_at: time_of(&r.get::<_, String>("fire_at")?)?,
        message: r.get("message")?,
        task_id: r.get("task_id")?,
        request_id: r.get("request_id")?,
        state: r.get("state")?,
        overdue_s: r.get("overdue_s")?,
        name: if with_name {
            r.get::<_, Option<String>>("name")?.unwrap_or_default()
        } else {
            String::new()
        },
        created_at: match created.filter(|c| !c.is_empty()) {
            Some(c) => Some(time_of(&c)?),
            None => None,
        },
    })
}

const SELECT: &str =
    "SELECT t.*, n.name FROM timers t LEFT JOIN timer_names n ON n.timer_id = t.id";

/// The Mac's presence (ORCH-317), from the platform: whether it is out of use (asleep, displays asleep, locked or
/// switched away; never input idleness), and when the current stretch of use began (None: FERB can't tell, which
/// counts as present).
pub trait Presence {
    fn owner_away(&self) -> bool;
    fn present_since(&self) -> Option<Time>;
}

/// No presence signal: always present, since unknown.
pub struct AlwaysPresent;

impl Presence for AlwaysPresent {
    fn owner_away(&self) -> bool {
        false
    }
    fn present_since(&self) -> Option<Time> {
        None
    }
}

/// What a timer's consumer did with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Delivery {
    /// Handled: the timer is acknowledged now (Python's only outcome).
    Done,
    /// Taken over: the consumer acknowledges it itself, in the same transaction as its own record (a schedule's run,
    /// `schedule_store::RunClaim`), so a crash in between hands it over again.
    TakenOver,
}

type OnChange<'a> = Box<dyn Fn(&Timer, &str) -> Result<(), String> + 'a>;
type OnError<'a> = Box<dyn Fn(Option<&Timer>, &str) + 'a>;

/// The timer store and its delivery loop (`Timers`, `timers.py:63`).
pub struct Timers<'a> {
    db: &'a Db,
    clock: &'a dyn Clock,
    ids: Box<dyn Fn() -> String + 'a>,
    tokens: Box<dyn Fn() -> String + 'a>,
    presence: Box<dyn Presence + 'a>,
    /// Told of every change to a timer that isn't a sleep (DMN-548); an `Err` is reported, never undoes the write.
    pub on_change: Option<OnChange<'a>>,
    /// Told of a consumer that failed (the timer) or a check that failed (None).
    pub on_error: Option<OnError<'a>>,
    wake: Notify,
}

impl<'a> Timers<'a> {
    /// `ids`: the id source (`IdSource.new_id`). Claim tokens are 8 random bytes in hex unless [`Timers::tokens`]
    /// replaces them.
    pub fn new(db: &'a Db, clock: &'a dyn Clock, ids: impl Fn() -> String + 'a) -> Timers<'a> {
        Timers {
            db,
            clock,
            ids: Box::new(ids),
            tokens: Box::new(|| {
                let mut b = [0u8; 8];
                getrandom::fill(&mut b).expect("the system's random source");
                b.iter().map(|x| format!("{x:02x}")).collect()
            }),
            presence: Box::new(AlwaysPresent),
            on_change: None,
            on_error: None,
            wake: Notify::new(),
        }
    }

    /// Replace the claim tokens' source (tests: `secrets.token_hex` scripted).
    pub fn tokens(mut self, tokens: impl Fn() -> String + 'a) -> Self {
        self.tokens = Box::new(tokens);
        self
    }

    /// The Mac's presence signal (the daemon's `MacPresence`).
    pub fn set_presence(&mut self, presence: impl Presence + 'a) {
        self.presence = Box::new(presence);
    }

    pub fn db(&self) -> &'a Db {
        self.db
    }

    // ---------- writes ----------

    /// `add`: `when_active` (ORCH-317) fires only while the owner is at the Mac; `return_after` only in an active
    /// stretch that began after it; `repeat_of`: the fired timer whose Repeat this is (DMN-548). Its rows are
    /// written in one transaction (Python writes them one by one, so a crash could leave a when-active timer without
    /// its hold: fixed against Python, the rows are the same).
    #[allow(clippy::too_many_arguments)]
    pub fn add(
        &self,
        kind: &str,
        fire_at: &Time,
        message: &str,
        task_id: Option<&str>,
        request_id: Option<&str>,
        when_active: bool,
        return_after: Option<&Time>,
        name: &str,
        repeat_of: Option<&str>,
    ) -> Result<Timer, TimerError> {
        let now = self.clock.now();
        let tid = format!("tm-{}", (self.ids)());
        self.db.transaction(|c| -> Result<(), TimerError> {
            c.execute(
                "INSERT INTO timers (id, kind, fire_at, message, task_id, request_id, state, created_at) \
                 VALUES (?, ?, ?, ?, ?, ?, 'pending', ?)",
                params![tid, kind, py::iso(&py::utc(fire_at)), message, task_id, request_id, py::iso(&now)],
            )?;
            if when_active || return_after.is_some() {
                c.execute(
                    "INSERT INTO timer_when_active (timer_id, return_after) VALUES (?, ?)",
                    params![tid, return_after.map(|r| py::iso(&py::utc(r)))],
                )?;
            }
            if !py::strip(name).is_empty() {
                c.execute(
                    "INSERT INTO timer_names (timer_id, name) VALUES (?, ?)",
                    params![tid, py::squeeze(name)],
                )?;
            }
            if let Some(of) = repeat_of {
                c.execute(
                    "INSERT INTO timer_repeats (timer_id, repeat_id) VALUES (?, ?)",
                    params![of, tid],
                )?;
            }
            Ok(())
        })?;
        self.wake.notify_waiters();
        let t = self.get(&tid)?.expect("the row just written");
        self.changed(Some(&t));
        Ok(t)
    }

    pub fn get(&self, tid: &str) -> Result<Option<Timer>, TimerError> {
        let mut st = self
            .db
            .conn()
            .prepare(&format!("{SELECT} WHERE t.id = ?"))?;
        let mut rows = st.query(params![tid])?;
        match rows.next()? {
            Some(r) => Ok(Some(row(r, true)?)),
            None => Ok(None),
        }
    }

    /// It fires only while the owner is at the Mac (ORCH-317).
    pub fn when_active(&self, tid: &str) -> Result<bool, TimerError> {
        Ok(self
            .db
            .conn()
            .query_row(
                "SELECT 1 FROM timer_when_active WHERE timer_id = ?",
                params![tid],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    /// Look at the timers again now (the owner is back at the Mac).
    pub fn wake(&self) {
        self.wake.notify_waiters();
    }

    fn held(&self) -> (i64, Option<String>) {
        let since = self.presence.present_since();
        (
            i64::from(self.presence.owner_away()),
            since.map(|s| py::iso(&py::utc(&s))),
        )
    }

    const HELD: &'static str = "id IN (SELECT timer_id FROM timer_when_active WHERE :away OR (return_after IS NOT NULL AND \
         :since IS NOT NULL AND julianday(return_after) >= julianday(:since)))";

    /// A delivery of this timer is recorded (its consumer handled it).
    pub fn delivered(&self, tid: &str) -> Result<bool, TimerError> {
        Ok(self
            .db
            .conn()
            .query_row(
                "SELECT 1 FROM timer_deliveries WHERE timer_id = ?",
                params![tid],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    /// Every cancel of one timer comes here: told once.
    pub fn cancel(&self, tid: &str) -> Result<bool, TimerError> {
        let n = self.db.conn().execute(
            "UPDATE timers SET state = 'cancelled' WHERE id = ? AND state IN ('pending', 'claimed')",
            params![tid],
        )?;
        if n > 0 {
            let t = self.get(tid)?;
            self.changed(t.as_ref());
        }
        Ok(n > 0)
    }

    /// The timer a fired timer's Repeat started (DMN-548).
    pub fn repeated_by(&self, tid: &str) -> Result<Option<String>, TimerError> {
        Ok(self
            .db
            .conn()
            .query_row(
                "SELECT repeat_id FROM timer_repeats WHERE timer_id = ?",
                params![tid],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// The fired timer this one repeats.
    pub fn repeat_of(&self, tid: &str) -> Result<Option<String>, TimerError> {
        Ok(self
            .db
            .conn()
            .query_row(
                "SELECT timer_id FROM timer_repeats WHERE repeat_id = ?",
                params![tid],
                |r| r.get(0),
            )
            .optional()?)
    }

    fn changed(&self, t: Option<&Timer>) {
        let (Some(t), Some(on_change)) = (t, self.on_change.as_ref()) else {
            return;
        };
        if t.kind == "sleep" {
            return;
        }
        if let Err(e) = on_change(t, shown_state(&t.state)) {
            self.report(Some(t), &e);
        }
    }

    /// A task ended: its sleeps go (reminders it set for the owner stay).
    pub fn cancel_task(&self, task_id: &str) -> Result<usize, TimerError> {
        Ok(self.db.conn().execute(
            "UPDATE timers SET state = 'cancelled' WHERE task_id = ? AND kind = 'sleep' AND state IN ('pending', \
             'claimed')",
            params![task_id],
        )?)
    }

    /// After a restart: every sleeping worker's task was interrupted.
    pub fn cancel_all_sleeps(&self) -> Result<usize, TimerError> {
        Ok(self.db.conn().execute(
            "UPDATE timers SET state = 'cancelled' WHERE kind = 'sleep' AND state IN ('pending', 'claimed')",
            [],
        )?)
    }

    /// Pending timers of these kinds (default `("reminder", "timer")`), soonest first.
    pub fn pending(&self, kinds: &[&str]) -> Result<Vec<Timer>, TimerError> {
        let q = vec!["?"; kinds.len()].join(",");
        let mut st = self.db.conn().prepare(&format!(
            "{SELECT} WHERE t.state = 'pending' AND t.kind IN ({q}) ORDER BY t.fire_at"
        ))?;
        let mut rows = st.query(rusqlite::params_from_iter(kinds.iter()))?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            out.push(row(r, true)?);
        }
        Ok(out)
    }

    // ---------- firing ----------

    /// Due pending timers plus claims a crash left unacknowledged, each under a fresh token.
    pub fn claim_due(&self) -> Result<Vec<Timer>, TimerError> {
        let now = self.clock.now();
        let token = (self.tokens)();
        let (away, since_text) = self.held();
        self.db.conn().execute(
            &format!(
                "UPDATE timers SET state = 'claimed', claim_token = :token, claimed_at = :at WHERE (state = 'pending' \
                 AND fire_at <= :now AND NOT {}) OR state = 'claimed'",
                Self::HELD
            ),
            named_params! {":token": token, ":at": py::iso(&now), ":now": py::iso(&py::utc(&now)),
                           ":away": away, ":since": since_text},
        )?;
        let claimed = {
            let mut st = self.db.conn().prepare(
                "SELECT * FROM timers WHERE claim_token = ? AND state = 'claimed' ORDER BY fire_at",
            )?;
            let mut rows = st.query(params![token])?;
            let mut out = Vec::new();
            while let Some(r) = rows.next()? {
                out.push(row(r, false)?);
            }
            out
        };
        let since = self.presence.present_since();
        let mut out = Vec::new();
        for mut t in claimed {
            let mut due = t.fire_at;
            if let Some(s) = since {
                if s > due && self.when_active(&t.id)? {
                    due = s; // it waited for the owner: late only from their return
                }
            }
            let overdue = py::total_seconds(&now, &due).max(0.0);
            self.db.conn().execute(
                "UPDATE timers SET overdue_s = ? WHERE id = ?",
                params![overdue, t.id],
            )?;
            if overdue != 0.0 {
                t.overdue_s = Some(overdue);
            }
            out.push(t);
        }
        Ok(out)
    }

    /// Acknowledge a claimed timer (→ delivered).
    pub fn ack(&self, tid: &str) -> Result<(), TimerError> {
        let n = self.db.conn().execute(
            "UPDATE timers SET state = 'delivered', delivered_at = ? WHERE id = ? AND state = 'claimed'",
            params![py::iso(&self.clock.now()), tid],
        )?;
        if n > 0 {
            let t = self.get(tid)?;
            self.changed(t.as_ref());
        }
        Ok(())
    }

    /// Hand each due timer to `consume` and acknowledge it (unless the consumer took it over). A consumer that fails
    /// leaves that timer claimed (handed over again at the next check) and is reported; the timers after it are
    /// still delivered. Returns how many were delivered.
    pub async fn tick<F, Fut>(&self, mut consume: F) -> Result<usize, TimerError>
    where
        F: FnMut(Timer) -> Fut,
        Fut: Future<Output = Result<Delivery, String>>,
    {
        let mut n = 0;
        for t in self.claim_due()? {
            let id = t.id.clone();
            match consume(t.clone()).await {
                Err(e) => {
                    self.report(Some(&t), &e);
                    continue;
                }
                Ok(Delivery::Done) => self.ack(&id)?,
                Ok(Delivery::TakenOver) => {}
            }
            n += 1;
        }
        Ok(n)
    }

    /// Tell `on_error`; reporting never stops delivery.
    pub fn report(&self, timer: Option<&Timer>, error: &str) {
        if let Some(on_error) = self.on_error.as_ref() {
            on_error(timer, error);
        }
    }

    /// Seconds until the next look at the wall clock.
    pub fn next_due_in(&self) -> Result<f64, TimerError> {
        let (away, since) = self.held();
        let f: Option<String> = self.db.conn().query_row(
            &format!(
                "SELECT MIN(fire_at) AS f FROM timers WHERE state = 'pending' AND NOT {}",
                Self::HELD
            ),
            named_params! {":away": away, ":since": since},
            |r| r.get(0),
        )?;
        match f.filter(|f| !f.is_empty()) {
            None => Ok(POLL_S),
            Some(f) => Ok(recheck_in(&time_of(&f)?, &self.clock.now())),
        }
    }

    /// The daemon's loop: fire what's due (and what a crash left claimed), then wait for the next one or a
    /// [`Timers::wake`]. `rest(seconds)` is the wait (the daemon: a tokio sleep; tests: a stepped clock). Never
    /// returns; cancel it by dropping it.
    ///
    /// Fixed against Python: a failure computing the next check (`next_due_in`, outside Python's `try`) is reported
    /// and the loop waits [`POLL_S`]; in Python it ends the loop, and no timer fires again until a restart.
    pub async fn run<F, Fut, R, RFut>(&self, mut consume: F, mut rest: R)
    where
        F: FnMut(Timer) -> Fut,
        Fut: Future<Output = Result<Delivery, String>>,
        R: FnMut(f64) -> RFut,
        RFut: Future<Output = ()>,
    {
        loop {
            if let Err(e) = self.tick(&mut consume).await {
                self.report(None, &e.to_string());
            }
            let woken = self.wake.notified();
            tokio::pin!(woken);
            woken.as_mut().enable();
            let wait = match self.next_due_in() {
                Ok(s) => s + 0.05,
                Err(e) => {
                    self.report(None, &e.to_string());
                    POLL_S
                }
            };
            tokio::select! {
                _ = woken => {}
                _ = rest(wait) => {}
            }
        }
    }
}

/// `mark_handled`: the consumer's idempotency record; false if this timer was already handled.
pub fn mark_handled(
    conn: &rusqlite::Connection,
    timer_id: &str,
    at: &Time,
) -> Result<bool, TimerError> {
    Ok(conn.execute(
        "INSERT OR IGNORE INTO timer_deliveries (timer_id, at) VALUES (?, ?)",
        params![timer_id, py::iso(at)],
    )? == 1)
}

impl crate::tools::TimerBook for Timers<'_> {
    fn add_timer(
        &self,
        kind: &str,
        at: &Time,
        message: &str,
        task_id: &str,
        request_id: &str,
    ) -> Result<(String, Time), String> {
        self.add(
            kind,
            at,
            message,
            Some(task_id),
            Some(request_id),
            false,
            None,
            "",
            None,
        )
        .map(|t| (t.id, t.fire_at))
        .map_err(|e| e.to_string())
    }
}
