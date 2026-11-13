//! Durable timers' delivery, reminders and schedules: changing and running them, and the RPCs the app calls about
//! them. Port of `ferb/daemon/areas/schedules.py`. Must not widen a schedule's recorded scope.
//!
//! Fixed against Python (ferb-clock's `claim_run`/`start_run`, journal 19): a scheduled run's timer is marked handled
//! in the same transaction that records the run's task (or its skip), so a crash between booking the next run and
//! creating the task no longer loses that run (`areas/schedules.py:659-687`).
//!
//! Not ported here: creating and editing a schedule from chat (`_schedule_create`, `_schedule_edit_from_chat`,
//! `_schedule_problems`, `_schedule_scopes`, `_typed_behind_memory`), the worker's sleeps and waits (`_sleep_until`,
//! `_wait_status`, `_wait_ended`), and the Mac's presence wiring (`_build_timers`' `MacPresence`, `_mac_started`).

use std::collections::BTreeSet;
use std::rc::Rc;

use ferb_clock::py::{self, Time};
use ferb_clock::schedule_store::{ClaimedRun, RunClaim, SCHEDULE_RETRY};
use ferb_clock::schedules::{until_said, watch_met, watch_said, Schedule, FINISHED};
use ferb_clock::timers::{mark_handled, shown_state, Delivery, Timer};
use ferb_orchestrator::names::shown_name;
use ferb_policy::small::blocked;
use ferb_rpc::server::{HandlerError, HandlerResult};
use ferb_rpc::wire::Params;
use serde_json::{json, Map, Value};

use super::seams::RunFailed;
use super::{iso_or_null, ok, Handler, State};
use crate::shared::{refusal, text};

/// A failed run's one retry, 10 minutes after the failure (owner 2026-10-27).
pub const SCHEDULE_RETRY_S: f64 = 600.0;
/// ORCH-592: a schedule's last answer's first line in the planner's list, at most.
pub const LAST_LINE: usize = 200;

fn sched_err(e: impl std::fmt::Display) -> HandlerError {
    super::store_error(e)
}

/// `Timer.label`: its short name, else its message's first clause, cut.
pub fn timer_label(t: &Timer) -> String {
    shown_name(Some(&t.name), &t.message)
}

/// `Schedule.label`.
pub fn label(s: &Schedule) -> String {
    shown_name(Some(&s.name), &s.request)
}

fn local(st: &State, t: &Time) -> Time {
    st.clock.zone().local(t)
}

fn seconds(t: &Time, s: f64) -> Time {
    py::plus_us(t, (s * 1e6).round() as i64).unwrap_or(*t)
}

// ---------- timer events ----------

/// `_timer_changed` (DMN-548): a timer was set, cancelled or fired; one event from the store's one write path.
pub fn timer_changed(st: &State, t: &Timer, state: &str) {
    let timers = st.timers_plain();
    let of = if state == "running" && t.kind == "timer" {
        timers.repeat_of(&t.id).ok().flatten()
    } else {
        None
    };
    let mut data = json!({"timer_id": t.id, "kind": t.kind, "state": state, "name": timer_label(t),
                          "at": py::iso(&t.fire_at), "message": t.message});
    if let (true, Some(c)) = (t.kind == "timer", t.created_at.as_ref()) {
        data["started"] = json!(py::iso(c));
    }
    if let Some(of) = of {
        data["repeat_of"] = json!(of);
    }
    st.publish("timer.changed", data);
}

/// `_timer_failed`: a timer's consumer raised (the timer stays claimed and is handed over again), or a check failed.
/// Not while shutting down: the consumer then refuses on purpose.
pub fn timer_failed(st: &State, timer: Option<&Timer>, error: &str) {
    if st.shutting_down.get() {
        return;
    }
    let _ = st.journal(
        "timer.failed",
        json!({"timer_id": timer.map(|t| t.id.clone()), "kind": timer.map(|t| t.kind.clone()),
               "error": st.redact_text(error)}),
        None,
    );
}

/// `_first_of`: the first timer of a Repeat chain this timer belongs to, or None (APP-769).
pub fn first_of(st: &State, tid: &str) -> Option<String> {
    let timers = st.timers_plain();
    let mut first: Option<String> = None;
    let mut seen = BTreeSet::from([tid.to_owned()]);
    loop {
        let at = first.clone().unwrap_or_else(|| tid.to_owned());
        match timers.repeat_of(&at).ok().flatten() {
            Some(earlier) if !seen.contains(&earlier) => {
                seen.insert(earlier.clone());
                first = Some(earlier);
            }
            _ => return first,
        }
    }
}

// ---------- start and the timer loop ----------

/// `_record_start`: journal `daemon.started` and say whether this start is in a new login session (SENSE-201).
/// `login`: the session's digest (`_login_session`, read from CoreGraphics by the platform), None when unreadable.
pub fn record_start(st: &State, pid: u32, login: Option<&str>) -> Result<bool, HandlerError> {
    let previous = st
        .journal
        .recent_values("daemon.started", "mac_login", 1)
        .map_err(super::learning_error)?;
    let mut data = json!({"pid": pid});
    if let Some(l) = login {
        data["mac_login"] = json!(l);
    }
    let entry = st.journal("daemon.started", data, None)?;
    let stored = entry.data.get("mac_login").cloned();
    Ok(match stored {
        None => false,
        Some(s) => previous != vec![Value::Null] && previous != vec![s],
    })
}

/// `_reconcile_schedules`: at start, every active schedule has a pending timer for its next run.
pub fn reconcile_schedules(st: &State) -> Result<(), HandlerError> {
    let schedules = st.schedules();
    let timers = st.timers();
    for s in schedules.all(&["active"]).map_err(sched_err)? {
        let timer = match &s.timer_id {
            Some(tid) => timers.get(tid).map_err(sched_err)?,
            None => None,
        };
        let handled = match &timer {
            Some(t) => timers.delivered(&t.id).map_err(sched_err)?,
            None => false,
        };
        if timer
            .as_ref()
            .is_some_and(|t| (t.state == "pending" || t.state == "claimed") && !handled)
        {
            continue;
        }
        let now = st.now();
        let due = match s.next_run {
            Some(n) => Some(n),
            None => s.next_after(&now).ok().flatten(),
        };
        let Some(due) = due else {
            schedules.set_timer(&s.id, None, None).map_err(sched_err)?;
            schedules.set_state(&s.id, "done").map_err(sched_err)?;
            if s.ends_after(&now).unwrap_or(false) {
                st.journal(
                    "schedule.ended",
                    json!({"schedule_id": s.id, "until": iso_or_null(s.until.as_ref())}),
                    None,
                )?;
            }
            continue;
        };
        let booked = book(st, &s, &due, "", None)?;
        schedules
            .set_timer(&s.id, Some(&booked.id), Some(&due))
            .map_err(sched_err)?;
        st.journal(
            "schedule.reconciled",
            json!({"schedule_id": s.id, "at": py::iso(&due)}),
            None,
        )?;
    }
    Ok(())
}

/// `_book`: a timer for one of a schedule's runs (`message`: its id, or a retry's), held for the owner when the
/// schedule waits for them (ORCH-590): the one place every run of a schedule is booked.
pub fn book(
    st: &State,
    s: &Schedule,
    due: &Time,
    message: &str,
    return_after: Option<&Time>,
) -> Result<Timer, HandlerError> {
    let message = if message.is_empty() { &s.id } else { message };
    let t = st
        .timers()
        .add(
            "schedule",
            due,
            message,
            None,
            None,
            s.rule.when_active,
            return_after,
            "",
            None,
        )
        .map_err(sched_err)?;
    st.timer_wake.notify_waiters();
    Ok(t)
}

/// `_owner_back` (ORCH-590): the Mac came back into use; what was held for the owner fires now, and paused tasks get
/// their mention of the day.
pub fn owner_back(st: &Rc<State>, signal: &str) {
    if st.shutting_down.get() {
        return;
    }
    let _ = st.journal("mac.back_in_use", json!({"signal": signal}), None);
    st.timer_wake.notify_waiters();
    tokio::task::spawn_local(st.tasks.mention_paused());
}

/// One pass of the timer loop (`Timers.tick` with `_timer_fired` as its consumer): how many timers were handed over.
pub async fn tick(st: &Rc<State>) -> Result<usize, HandlerError> {
    let timers = st.timers();
    let me = st.clone();
    timers
        .tick(move |t| {
            let me = me.clone();
            async move { timer_fired(&me, t).await }
        })
        .await
        .map_err(sched_err)
}

/// The timer loop (`Timers.run`): fire what's due, then wait for the next due time, at most `POLL_S`, or a wake.
/// Never returns; abort its task to stop it.
pub async fn run_timers(st: Rc<State>) {
    loop {
        if let Err(e) = tick(&st).await {
            timer_failed(&st, None, &e.message);
        }
        let wake = st.timer_wake.clone();
        let woken = wake.notified();
        tokio::pin!(woken);
        woken.as_mut().enable();
        let wait = match st.timers_plain().next_due_in() {
            Ok(s) => s + 0.05,
            Err(e) => {
                timer_failed(&st, None, &e.to_string());
                ferb_clock::timers::POLL_S
            }
        };
        tokio::select! {
            _ = woken => {}
            _ = tokio::time::sleep(std::time::Duration::from_secs_f64(wait)) => {}
        }
    }
}

/// Wait until every scheduled run the timer loop started has ended (tests; shutdown).
pub async fn settle_runs(st: &State) {
    loop {
        let runs: Vec<_> = st.runs.borrow_mut().drain(..).collect();
        if runs.is_empty() {
            return;
        }
        for r in runs {
            let _ = r.await;
        }
    }
}

/// `_timer_fired`: the consumer. Reminders and timers reach the owner once (notification, chat, logs, journal), even
/// when a crash made the timer fire again; a schedule's timer starts its run.
pub async fn timer_fired(st: &Rc<State>, t: Timer) -> Result<Delivery, String> {
    if st.shutting_down.get() {
        return Err("FerbError: FERB is shutting down; this timer fires after the restart".into());
    }
    let now = st.now();
    if t.kind == "sleep" {
        if let Some(ev) = st.sleepers.borrow().get(&t.id) {
            ev.notify_waiters();
        }
        return Ok(Delivery::Done);
    }
    if t.kind == "schedule" {
        let timers = st.timers();
        let claim = st
            .schedules()
            .claim_run(&timers, &t)
            .map_err(|e| e.to_string())?;
        return match claim {
            RunClaim::AlreadyHandled => Ok(Delivery::Done),
            RunClaim::NotRunnable => {
                // as Python's `handled(...)` before `_run_schedule`'s early return
                mark_handled(st.db().conn(), &t.id, &now).map_err(|e| e.to_string())?;
                Ok(Delivery::Done)
            }
            RunClaim::Run(run) => {
                if run.ended_by_end {
                    let _ = st.journal(
                        "schedule.ended",
                        json!({"schedule_id": run.schedule.id,
                               "until": iso_or_null(run.schedule.until.as_ref())}),
                        None,
                    );
                }
                let me = st.clone();
                let task = tokio::task::spawn_local(async move { run_schedule(&me, *run).await });
                st.runs.borrow_mut().push(task);
                Ok(Delivery::TakenOver)
            }
        };
    }
    let overdue = t.overdue_s.unwrap_or(0.0);
    let delivered = st
        .db()
        .transaction(|c| -> Result<bool, StartError> {
            if !mark_handled(c, &t.id, &now).map_err(|e| StartError(e.to_string()))? {
                return Ok(false); // already delivered before a crash
            }
            st.journal(
                "timer.delivered",
                json!({"timer_id": t.id, "kind": t.kind, "message": t.message,
                       "overdue_s": py::round_even(overdue)}),
                None,
            )
            .map_err(|e| StartError(e.message))?;
            Ok(true)
        })
        .map_err(|e| e.0)?;
    if !delivered {
        return Ok(Delivery::Done);
    }
    let late = if overdue >= 60.0 {
        format!(" (due {} min ago)", py::round_even(overdue / 60.0))
    } else {
        String::new()
    };
    let named = st.timers_plain().get(&t.id).ok().flatten();
    let name = named.as_ref().map_or_else(|| timer_label(&t), timer_label);
    st.publish(
        "timer.fired",
        json!({"timer_id": t.id, "kind": t.kind, "message": t.message, "name": name,
               "overdue_by": py::round_even(overdue)}),
    );
    let mut card = json!({"type": "reminder", "title": name, "kind": t.kind, "timer_id": t.id,
                          "due": py::iso(&t.fire_at)});
    if let (true, Some(created)) = (
        t.kind == "timer",
        named.as_ref().and_then(|n| n.created_at.as_ref()),
    ) {
        card["started"] = json!(py::iso(created));
    }
    if let Some(first) = first_of(st, &t.id) {
        card["repeat_of"] = json!(first);
    }
    // chat, logs and the journal still have it when the banner fails
    let _ = st
        .owner
        .notify(
            &name,
            &format!("{}{late}", t.message),
            &format!("reminder:{}", t.id),
            json!({"kind": "reminder", "timer_id": t.id}),
            card,
        )
        .await;
    Ok(Delivery::Done)
}

// ---------- a schedule's runs ----------

/// `_run_schedule`, after ferb-clock's claim booked the next run: the run is handed to its task (or skipped) in the
/// transaction that marks its timer handled. A run never overlaps the schedule's previous one still under way, and
/// none starts while FERB is stopped: each is skipped, journaled; a one-time schedule's skipped run is told.
async fn run_schedule(st: &Rc<State>, run: ClaimedRun) {
    let s = run.schedule.clone();
    let sid = s.id.clone();
    let now = st.now();
    let last = !run.retry && (run.ended || (run.redelivered && s.state == "done"));
    let schedules = st.schedules();
    let timers = st.timers();
    let busy = st.schedule_runs.borrow().contains(&sid) || st.tasks.schedule_running(&sid);
    if busy || st.stopped.get() {
        let why = if busy {
            "the previous run is still under way"
        } else {
            "FERB was stopped"
        };
        let recorded = schedules.start_run(&timers, &run, |_c| -> Result<(), StartError> {
            st.journal(
                "schedule.skipped",
                json!({"schedule_id": sid, "why": why, "retry": run.retry}),
                None,
            )
            .map_err(|e| StartError(e.message))?;
            Ok(())
        });
        if recorded.is_err() {
            return; // left claimed: handed over again
        }
        if last {
            let ended = match &s.until {
                Some(u) if s.ends_after(&now).unwrap_or(false) => {
                    format!("\n\n{}", ended_line(st, &label(&s), u))
                }
                _ => String::new(),
            };
            let event = json!({"task_id": format!("{sid}:{}", py::fmt(&now, "%Y%m%d%H%M")),
                               "status": "cancelled", "goal": s.request, "name": label(&s),
                               "answer": format!("“{}” was due, but {why}, so it didn't run.{ended}", label(&s))});
            st.owner
                .task_result(crate::shared::object(event), false, false)
                .await;
        }
        return;
    }
    let late = run.overdue_s > 60.0;
    let created = schedules.start_run(&timers, &run, |c| {
        st.tasks.create_scheduled(c, &run, late).map_err(StartError)
    });
    let Ok(created) = created else {
        return; // nothing recorded: the timer stays claimed and is handed over again
    };
    if last {
        st.last_runs.borrow_mut().insert(sid.clone());
    }
    st.schedule_runs.borrow_mut().insert(sid.clone());
    let out = st
        .tasks
        .run_scheduled(s.clone(), created, late, run.overdue_s, run.retry)
        .await;
    st.last_runs.borrow_mut().remove(&sid);
    st.schedule_runs.borrow_mut().remove(&sid);
    let final_ = match out {
        Ok(f) => f,
        Err(RunFailed { described, reason }) => {
            let _ = st.journal(
                "schedule.failed",
                json!({"schedule_id": sid, "why": described}),
                None,
            );
            crate::shared::object(json!({
                "task_id": format!("{sid}:{}", py::fmt(&now, "%Y%m%d%H%M")), "status": "failed",
                "answer": format!("“{}” didn't work this time; I'll try again at the next run. Why: {reason}",
                                  label(&s))}))
        }
    };
    if final_.get("suspended").is_some_and(ferb_py::value::truthy) {
        return; // FERB shut down during the run: it resumes, and reports, after the restart
    }
    let _ = schedule_finished(st, &s, final_, run.overdue_s, run.retry, &now).await;
}

/// What a run's start failed with (its records are rolled back and its timer stays claimed).
#[derive(Debug)]
struct StartError(String);

impl From<rusqlite::Error> for StartError {
    fn from(e: rusqlite::Error) -> Self {
        StartError(e.to_string())
    }
}

impl From<ferb_clock::schedule_store::StoreError> for StartError {
    fn from(e: ferb_clock::schedule_store::StoreError) -> Self {
        StartError(e.to_string())
    }
}

/// `_resumed_outcome`: a task resumed after a restart ended; a scheduled run's result goes through its schedule's
/// bookkeeping; any other is told as a result.
pub async fn resumed_outcome(
    st: &Rc<State>,
    event: Map<String, Value>,
) -> Result<(), HandlerError> {
    let held = event.get("schedule").and_then(Value::as_object).cloned();
    let s = match &held {
        Some(h) => st
            .schedules()
            .get(&ferb_py::value::py_str(h.get("id").unwrap_or(&Value::Null)))
            .map_err(sched_err)?,
        None => None,
    };
    match (s, held) {
        (Some(s), Some(h)) => {
            let overdue = h.get("overdue_s").and_then(Value::as_f64).unwrap_or(0.0);
            let retry = h.get("retry").is_some_and(ferb_py::value::truthy);
            schedule_finished(st, &s, event, overdue, retry, &st.now()).await
        }
        _ => {
            st.owner.task_result(event, false, false).await;
            Ok(())
        }
    }
}

/// `_schedule_finished`: a scheduled run's result counted (only a verified run counts as success), its one retry
/// queued when it failed, its watch's crossing told once, then the result told (ORCH-590: every run posts).
pub async fn schedule_finished(
    st: &Rc<State>,
    s: &Schedule,
    final_: Map<String, Value>,
    overdue_s: f64,
    retry: bool,
    now: &Time,
) -> Result<(), HandlerError> {
    let schedules = st.schedules();
    let sid = s.id.clone();
    let status = final_
        .get("status")
        .map(ferb_py::value::py_str)
        .unwrap_or_else(|| "None".into());
    let ok_ = match status.as_str() {
        "done" => Some(true),
        "done_unverified" | "cancelled" => None,
        _ => Some(false),
    };
    schedules.ran(&sid, ok_).map_err(sched_err)?;
    if status == "failed" && !retry && s.watch == "none" {
        let regular = schedules.get(&sid).map_err(sched_err)?;
        let due = seconds(now, SCHEDULE_RETRY_S);
        if let Some(r) = regular {
            if r.state == "active" && r.next_run.is_none_or(|n| n > due) {
                book(st, s, &due, &format!("{SCHEDULE_RETRY}{sid}"), None)?;
                st.journal(
                    "schedule.retry_queued",
                    json!({"schedule_id": sid, "at": py::iso(&due)}),
                    None,
                )?;
            }
        }
    }
    let text_of = |k: &str| {
        final_
            .get(k)
            .filter(|v| ferb_py::value::truthy(v))
            .map(ferb_py::value::py_str)
    };
    let mut answer = text_of("answer")
        .or_else(|| text_of("summary"))
        .unwrap_or_default();
    let told_before = s.last_reported_met == Some(true);
    let due = seconds(now, -overdue_s);
    let reported = final_.get("watch_met").filter(|v| !v.is_null());
    if s.watch != "none" && FINISHED.contains(&status.as_str()) {
        if let Some(r) = reported {
            schedules
                .reported(&sid, ferb_py::value::truthy(r))
                .map_err(sched_err)?;
        }
    }
    let met = s.watch != "none" && watch_met(&status, reported.map(ferb_py::value::truthy));
    let superseded = s.watch == "until_met" && watch_ended(st, &sid)?;
    let alert = met && !told_before && !superseded;
    let task_id = final_
        .get("task_id")
        .map(ferb_py::value::py_str)
        .unwrap_or_else(|| "None".into());
    let ended = met
        && !superseded
        && s.watch == "until_met"
        && status == "done"
        && end_watch(st, &sid, &task_id)?;
    if ended {
        let line = format!("🗓 Stopped checking: {}", label(s));
        answer = format!("{answer}\n\n{line}");
        record_stop_line(st, &task_id, &line);
    }
    let expired = !ended && !retry && schedule_expired(st, &sid)?;
    if expired {
        let current = schedules.get(&sid).map_err(sched_err)?;
        let until = current.and_then(|c| c.until).unwrap_or(*now);
        let line = ended_line(st, &label(s), &until);
        answer = format!("{answer}\n\n{line}");
        record_stop_line(st, &task_id, &line);
    }
    if overdue_s > 60.0 {
        answer = format!("{answer}\n(ran late)");
    }
    let mut cards = vec![run_card(st, &sid, &due)?];
    cards.extend(
        final_
            .get("cards")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|c| c.is_object())
            .cloned(),
    );
    let mut result = final_.clone();
    result.insert("goal".into(), json!(s.request));
    result.insert("name".into(), json!(label(s)));
    result.insert("answer".into(), json!(answer));
    result.insert("cards".into(), Value::Array(cards));
    st.owner.task_result(result, false, alert).await;
    posted(st, &sid, &label(s), &final_, &answer)
}

/// `_posted` (ORCH-588): a scheduled run's result FERB posted in the owner's chat, journaled with its task answer's
/// origin and stored owner values (GATE-519).
fn posted(
    st: &State,
    sid: &str,
    name: &str,
    final_: &Map<String, Value>,
    answer: &str,
) -> Result<(), HandlerError> {
    let task_id = final_
        .get("task_id")
        .filter(|v| ferb_py::value::truthy(v))
        .map(ferb_py::value::py_str)
        .unwrap_or_default();
    let rows = if task_id.is_empty() {
        Vec::new()
    } else {
        st.journal
            .of_kind("task.answer", "task_id", &task_id, None)
            .map_err(super::learning_error)?
    };
    let (made, stored) = match rows.last() {
        Some(r) => (r.origin.clone(), r.data.get("provenance").cloned()),
        None => ("inferred".to_owned(), None),
    };
    let status = final_
        .get("status")
        .filter(|v| ferb_py::value::truthy(v))
        .map(ferb_py::value::py_str)
        .unwrap_or_default();
    let mut data = json!({"schedule_id": sid, "task_id": task_id, "name": name, "status": status,
                          "text": st.redact_text(answer)});
    if let Some(p) = stored.filter(Value::is_object) {
        data["provenance"] = p;
    }
    st.journal("schedule.posted", data, Some(&made))?;
    Ok(())
}

/// `_run_card`: a scheduled run's reminder card, from the schedule's row as it stands after the run.
fn run_card(st: &State, sid: &str, due: &Time) -> Result<Value, HandlerError> {
    let s = st
        .schedules()
        .get(sid)
        .map_err(sched_err)?
        .ok_or_else(|| sched_err("the schedule is gone"))?;
    let active = s.state == "active";
    let mut card = json!({"type": "reminder", "schedule_id": sid, "title": label(&s), "kind": "schedule",
                          "due": py::iso(due)});
    if s.rule.repeat != "none" || s.timing().is_some() {
        card["repeat"] = json!(s.when_said());
    }
    if let (Some(n), true) = (s.next_run.as_ref(), active) {
        card["next_run"] = json!(py::iso(n));
    }
    if let (Some(u), true) = (s.until.as_ref(), active) {
        card["until"] = json!(py::iso(u));
    }
    Ok(card)
}

fn of_kind(st: &State, kind: &str, sid: &str) -> Result<bool, HandlerError> {
    Ok(!st
        .journal
        .of_kind(kind, "schedule_id", sid, None)
        .map_err(super::learning_error)?
        .is_empty())
}

/// `_schedule_expired`: its end left it no more runs and nothing restarted it since (ORCH-320).
fn schedule_expired(st: &State, sid: &str) -> Result<bool, HandlerError> {
    let s = st.schedules().get(sid).map_err(sched_err)?;
    Ok(s.is_some_and(|s| s.state == "done") && of_kind(st, "schedule.ended", sid)?)
}

/// `_watch_ended`: a one-shot watch already ended by one of its checks.
fn watch_ended(st: &State, sid: &str) -> Result<bool, HandlerError> {
    of_kind(st, "schedule.met", sid)
}

/// `_record_stop_line`: the stop line in the ending task's recorded outcome as well (review M3).
fn record_stop_line(st: &State, task_id: &str, line: &str) {
    let Some(status) = st.tasks.task_column(task_id, "status") else {
        return;
    };
    let raw = st
        .tasks
        .task_column(task_id, "outcome_json")
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "{}".into());
    let mut outcome = ferb_py::json::loads(&raw).unwrap_or_else(|_| json!({}));
    let line = st.redact_text(line);
    let before = outcome
        .get("answer")
        .map(ferb_py::value::py_str)
        .unwrap_or_default();
    outcome["answer"] = json!(format!("{before}\n\n{line}"));
    let _ = st.tasks.set_task_status(task_id, &status, &outcome);
}

/// `_end_watch`: a one-shot watch's confirmed run found its condition met: it ends, its scope withdrawn.
fn end_watch(st: &State, sid: &str, task_id: &str) -> Result<bool, HandlerError> {
    let schedules = st.schedules();
    let Some(s) = schedules.get(sid).map_err(sched_err)? else {
        return Ok(false);
    };
    if s.state != "active" {
        return Ok(false);
    }
    if let Some(tid) = &s.timer_id {
        st.timers().cancel(tid).map_err(sched_err)?;
    }
    schedules.set_timer(sid, None, None).map_err(sched_err)?;
    schedules.set_state(sid, "done").map_err(sched_err)?;
    st.journal(
        "schedule.met",
        json!({"schedule_id": sid, "task_id": task_id}),
        None,
    )?;
    withdraw_schedule_scope(st, sid, "met")?;
    Ok(true)
}

/// `_ended_line`: the owner's line when a schedule's end leaves it no more runs (ORCH-320).
pub fn ended_line(st: &State, name: &str, until: &Time) -> String {
    format!(
        "🗓 Schedule ended: {name} (it ran {})",
        until_said(until, &st.clock.zone())
    )
}

// ---------- changes ----------

/// The standing permissions table (`ferb/policy/standing.py`): `schedule:<id>`'s scopes.
fn standing(st: &State) -> ferb_policy::stores::StandingPermissions<'_> {
    ferb_policy::stores::StandingPermissions { db: st.db() }
}

/// `_withdraw_schedule_scope` (D30): a cancelled or paused schedule's recorded scope stops covering its runs at once.
pub fn withdraw_schedule_scope(st: &State, sid: &str, action: &str) -> Result<(), HandlerError> {
    let approval = format!("{}{sid}", ferb_policy::standing::SCHEDULE_GRANT);
    let tasks = standing(st).revoke_approval(&approval).map_err(sched_err)?;
    if !tasks.is_empty() {
        st.journal(
            "standing.revoked",
            json!({"approval_id": approval, "task_ids": tasks, "reason": format!("schedule {action}")}),
            None,
        )?;
    }
    Ok(())
}

/// `_last_run_withdrawn`: a `schedule.pause` or `schedule.met` journaled after the schedule's latest run started.
fn last_run_withdrawn(st: &State, sid: &str) -> Result<bool, HandlerError> {
    let kind = st
        .journal
        .latest_kind(
            &["schedule.run", "schedule.pause", "schedule.met"],
            "schedule_id",
            sid,
        )
        .map_err(super::learning_error)?;
    Ok(matches!(
        kind.as_deref(),
        Some("schedule.pause") | Some("schedule.met")
    ))
}

/// `_last_run_holds_scope`: a one-time schedule's last run is still under way (M9).
fn last_run_holds_scope(st: &State, sid: &str) -> Result<bool, HandlerError> {
    if st.last_runs.borrow().contains(sid) {
        return Ok(!last_run_withdrawn(st, sid)?);
    }
    standing(st)
        .held(&format!("{}{sid}", ferb_policy::standing::SCHEDULE_GRANT))
        .map_err(sched_err)
}

/// `_changeable`: an active or paused schedule; or a done one whose last run is under way, for a cancel or pause.
fn changeable(st: &State, sid: &str, action: &str) -> Result<bool, HandlerError> {
    let Some(s) = st.schedules().get(sid).map_err(sched_err)? else {
        return Ok(false);
    };
    if s.state == "cancelled" {
        return Ok(false);
    }
    Ok(s.state != "done"
        || ((action == "cancel" || action == "pause") && last_run_holds_scope(st, sid)?))
}

/// `_listed_schedules`: active and paused ones, and a done one whose last run is under way (M9).
pub fn listed_schedules(st: &State) -> Result<Vec<Schedule>, HandlerError> {
    let schedules = st.schedules();
    let mut out = schedules.all(&["active", "paused"]).map_err(sched_err)?;
    for s in schedules.all(&["done"]).map_err(sched_err)? {
        if last_run_holds_scope(st, &s.id)? {
            out.push(s);
        }
    }
    Ok(out)
}

/// `_schedule_change`: cancel, pause or resume (`with_task`: one that follows the owner's pause or resume of the
/// schedule's run, ORCH-312). The owner's line.
pub fn schedule_change(
    st: &State,
    sid: &str,
    action: &str,
    with_task: &str,
) -> Result<String, HandlerError> {
    let schedules = st.schedules();
    let s = schedules.get(sid).map_err(sched_err)?;
    let Some(s) = s.filter(|_| changeable(st, sid, action).unwrap_or(false)) else {
        return Ok(format!("There's no active schedule {sid}."));
    };
    let name = label(&s);
    if s.state == "done" {
        if action == "cancel" {
            schedules.set_state(sid, "cancelled").map_err(sched_err)?;
        }
        st.journal(
            &format!("schedule.{action}"),
            json!({"schedule_id": sid}),
            None,
        )?;
        withdraw_schedule_scope(st, sid, action)?;
        let covered = "Its run already under way keeps going, but nothing it sends is covered any more: each send \
                       gets FERB's usual checks.";
        if action == "pause" {
            return Ok(format!(
                "{name} has no future runs, so there's nothing left to pause. {covered}"
            ));
        }
        return Ok(format!("Cancelled: {name}. {covered}"));
    }
    let done_word = match action {
        "cancel" => "Cancelled",
        "pause" => "Paused",
        "resume" => "Resumed",
        // Fixed against Python (named in the tests): another action journaled `schedule.<action>` and then failed
        // with an internal error (KeyError); here it is refused before anything changes
        _ => return Err(refusal("action must be cancel, pause or resume")),
    };
    match action {
        "cancel" | "pause" => {
            schedules
                .set_state(
                    sid,
                    if action == "cancel" {
                        "cancelled"
                    } else {
                        "paused"
                    },
                )
                .map_err(sched_err)?;
            if let Some(tid) = &s.timer_id {
                st.timers().cancel(tid).map_err(sched_err)?;
            }
            withdraw_schedule_scope(st, sid, action)?;
        }
        _ => {
            schedules.set_state(sid, "active").map_err(sched_err)?;
            let now = st.now();
            let nxt = if s.rule.repeat != "none" {
                let floor = seconds(&s.first_run, -1.0);
                s.next_after(if now > floor { &now } else { &floor })
            } else {
                match s.next_run {
                    Some(n) if n > now => Ok(Some(n)),
                    _ => s.next_after(&now),
                }
            }
            .map_err(sched_err)?;
            let Some(nxt) = nxt else {
                schedules.set_state(sid, "done").map_err(sched_err)?;
                return Ok(format!("{name} has no future runs left."));
            };
            let t = book(st, &s, &nxt, "", None)?;
            schedules
                .set_timer(sid, Some(&t.id), Some(&nxt))
                .map_err(sched_err)?;
        }
    }
    let mut data = json!({"schedule_id": sid});
    if !with_task.is_empty() {
        data["with_task"] = json!(with_task);
    }
    st.journal(&format!("schedule.{action}"), data, None)?;
    Ok(format!("{done_word}: {name}"))
}

/// A change the planner wrote from the owner's words (`ScheduleChange` / `ReminderCancel`, the fields these read).
#[derive(Clone, Debug, Default)]
pub struct ChatChange {
    pub id: String,
    pub action: String,
    pub quote: String,
    pub name: String,
    /// `until`: its quote and when (only with `set_end`).
    pub until: Option<(String, ferb_clock::when::When)>,
}

fn quoted(own_words: &str, quote: &str) -> bool {
    ferb_memory::terms::quoted(own_words, quote)
}

/// `_reminder_cancel_from_chat`.
pub fn reminder_cancel_from_chat(
    st: &State,
    rc: &ChatChange,
    own_words: &str,
) -> Result<String, HandlerError> {
    if !quoted(own_words, &rc.quote) {
        return Ok(blocked::blocked(
            blocked::CHANGES,
            blocked::NOT_IN_WORDS,
            "no reminder was cancelled",
            "say which reminder to cancel and I'll cancel it",
        ));
    }
    let timers = st.timers();
    let t = timers.get(&rc.id).map_err(sched_err)?;
    let Some(t) = t.filter(|t| t.kind == "reminder" || t.kind == "timer") else {
        return Ok("I couldn't find that reminder among the pending ones.".into());
    };
    if !timers.cancel(&rc.id).map_err(sched_err)? {
        return Ok("I couldn't find that reminder among the pending ones.".into());
    }
    st.journal(
        "timer.cancelled",
        json!({"timer_id": rc.id, "by": "owner"}),
        None,
    )?;
    let what = if t.kind == "timer" {
        "timer"
    } else {
        "reminder"
    };
    Ok(format!(
        "⏰ Cancelled the {what} “{}” for {}.",
        timer_label(&t),
        py::fmt(&local(st, &t.fire_at), "%a %d %b %H:%M")
    ))
}

/// `_schedule_change_from_chat`.
pub fn schedule_change_from_chat(
    st: &State,
    ch: &ChatChange,
    own_words: &str,
) -> Result<String, HandlerError> {
    if !quoted(own_words, &ch.quote) {
        return Ok(blocked::blocked(
            blocked::CHANGES,
            blocked::NOT_IN_WORDS,
            "no schedule was changed",
            "say which schedule should change and how",
        ));
    }
    let renamed = ch.name.split_whitespace().collect::<Vec<_>>().join(" ");
    let schedules = st.schedules();
    if !renamed.is_empty()
        && ch.action != "cancel"
        && schedules.get(&ch.id).map_err(sched_err)?.is_some()
    {
        schedules.set_name(&ch.id, &renamed).map_err(sched_err)?;
        st.journal(
            "schedule.renamed",
            json!({"schedule_id": ch.id, "name": renamed}),
            None,
        )?;
    }
    if (ch.action == "set_end") != ch.until.is_some() {
        st.journal(
            "schedule.refused",
            json!({"schedule_id": ch.id, "why": "set_end needs an end, and only set_end takes one"}),
            None,
        )?;
        return Ok(
            "I didn't change that schedule — could you say again what should change?".into(),
        );
    }
    if let (true, Some((quote, when))) = (ch.action == "set_end", ch.until.as_ref()) {
        if !quoted(own_words, quote) {
            return Ok(blocked::blocked(
                blocked::END_DATE,
                "the end date words I quoted from you aren't in what you wrote",
                "that schedule's end didn't change",
                "say when it should stop",
            ));
        }
        return schedule_set_end(st, &ch.id, when);
    }
    if !changeable(st, &ch.id, &ch.action)? {
        return Ok("There's no active schedule like that to change.".into());
    }
    schedule_change(st, &ch.id, &ch.action, "")
}

/// `_schedule_set_end` (ORCH-320): an active or paused schedule gets the end the owner's words give it.
pub fn schedule_set_end(
    st: &State,
    sid: &str,
    ends: &ferb_clock::when::When,
) -> Result<String, HandlerError> {
    let schedules = st.schedules();
    let Some(s) = schedules
        .get(sid)
        .map_err(sched_err)?
        .filter(|s| s.state == "active" || s.state == "paused")
    else {
        return Ok("There's no active schedule like that to change.".into());
    };
    let until = match ferb_clock::when::end_at(ends, &st.now(), &st.clock.zone()) {
        Ok(u) => u,
        Err(e) => return Ok(format!("I didn't change that schedule: {e}.")),
    };
    schedules.set_until(sid, &until).map_err(sched_err)?;
    st.journal(
        "schedule.set_end",
        json!({"schedule_id": sid, "until": py::iso(&until)}),
        None,
    )?;
    if s.state == "active" && s.next_run.is_some_and(|n| n > until) {
        if let Some(tid) = &s.timer_id {
            st.timers().cancel(tid).map_err(sched_err)?;
        }
        schedules.set_timer(sid, None, None).map_err(sched_err)?;
        schedules.set_state(sid, "done").map_err(sched_err)?;
        st.journal(
            "schedule.ended",
            json!({"schedule_id": sid, "until": py::iso(&until)}),
            None,
        )?;
        withdraw_schedule_scope(st, sid, "ended")?;
        return Ok(ended_line(st, &label(&s), &until));
    }
    Ok(format!(
        "🗓 {} now runs {}.",
        label(&s),
        until_said(&until, &st.clock.zone())
    ))
}

/// `_schedule_live`: whether a schedule's runs may still hold its recorded scope.
pub fn schedule_live(st: &State, sid: &str) -> Result<bool, HandlerError> {
    let s = st.schedules().get(sid).map_err(sched_err)?;
    Ok(match s {
        Some(s) => s.state == "active" || (s.state == "done" && !last_run_withdrawn(st, sid)?),
        None => false,
    })
}

// ---------- what FERB's records say (the planner's list, the card) ----------

/// `_schedule_rows` (T-1066): the owner's current schedules as FERB's records hold them.
pub fn schedule_rows(st: &State) -> Result<Vec<Map<String, Value>>, HandlerError> {
    let mut rows = Vec::new();
    for s in listed_schedules(st)? {
        let state = if s.state == "done" {
            "done; its last run is under way".to_owned()
        } else {
            s.state.clone()
        };
        let next = s.next_run.filter(|_| s.state == "active");
        let watch = watch_said(&s.watch);
        rows.push(crate::shared::object(json!({
            "id": s.id, "name": label(&s), "repeat": s.when_said(), "state": state,
            "next_run": iso_or_null(next.as_ref()), "until": iso_or_null(s.until.as_ref()),
            "watch": if watch.is_empty() { Value::Null } else { json!(watch) },
            "request": s.request, "failures": s.failures})));
    }
    Ok(rows)
}

/// `_schedule_last_runs` (ORCH-592): each listed schedule's latest finished run as the owner was told it.
pub fn schedule_last_runs(st: &State) -> Result<Map<String, Value>, HandlerError> {
    let results = ferb_store::task_results::TaskResults::new(st.db());
    let mut out = Map::new();
    for s in listed_schedules(st)? {
        let Some(row) = results.latest_of_schedule(&s.id).map_err(sched_err)? else {
            continue;
        };
        let task_id = ferb_py::value::py_str(row.get("task_id").unwrap_or(&Value::Null));
        let answers = st
            .journal
            .of_kind("task.answer", "task_id", &task_id, None)
            .map_err(super::learning_error)?;
        let answer = row
            .get("answer")
            .filter(|v| ferb_py::value::truthy(v))
            .map(ferb_py::value::py_str)
            .unwrap_or_default();
        let first = ferb_orchestrator::answer_marks::headline(&answer);
        let cut: String = first.chars().take(LAST_LINE).collect();
        out.insert(
            s.id.clone(),
            json!({"at": row.get("at"), "status": row.get("status"),
                   "first_line": st.redact_text(&cut),
                   "origin": answers.last().map_or("inferred".to_owned(), |a| a.origin.clone())}),
        );
    }
    Ok(out)
}

/// `_last_run_origins` (ORCH-592).
pub fn last_run_origins(st: &State) -> Result<BTreeSet<String>, HandlerError> {
    Ok(schedule_last_runs(st)?
        .values()
        .flat_map(|r| ferb_sanitize::origin::split(r["origin"].as_str()))
        .collect())
}

fn local_said(st: &State, iso: &Value, pattern: &str) -> String {
    py::parse(&ferb_py::value::py_str(iso))
        .map(|t| py::fmt(&local(st, &t), pattern))
        .unwrap_or_default()
}

/// `_schedule_facts` (T-1066): the planner's list of the owner's schedules, marked as the truth to answer from;
/// `last_runs` (ORCH-592, the planner's turn only) adds each one's last run.
pub fn schedule_facts(st: &State, last_runs: bool) -> Result<Vec<String>, HandlerError> {
    let rows = schedule_rows(st)?;
    let ran = if last_runs {
        schedule_last_runs(st)?
    } else {
        Map::new()
    };
    let mut states: Vec<(String, usize)> = Vec::new();
    for r in &rows {
        let state = ferb_py::value::py_str(&r["state"]);
        match states.iter_mut().find(|(s, _)| *s == state) {
            Some((_, n)) => *n += 1,
            None => states.push((state, 1)),
        }
    }
    let count = if rows.is_empty() {
        "none".to_owned()
    } else {
        format!(
            "{} ({})",
            rows.len(),
            states
                .iter()
                .map(|(s, n)| format!("{n} {s}"))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    let mut lines = vec![format!(
        "The owner's schedules, from FERB's records (the truth: answer anything about schedules, how many there \
         are and when they run, only from this list, never from the conversation): {count}"
    )];
    for r in &rows {
        let nxt = if r["next_run"].is_null() {
            "no next run booked".to_owned()
        } else {
            format!(
                "next run {}",
                local_said(st, &r["next_run"], "%a %d %b %Y %H:%M")
            )
        };
        let mut extra = String::new();
        if !r["until"].is_null() {
            if let Some(u) = py::parse(&ferb_py::value::py_str(&r["until"])) {
                extra.push_str(&format!(", {}", until_said(&u, &st.clock.zone())));
            }
        }
        if !r["watch"].is_null() {
            extra.push_str(&format!(", {}", ferb_py::value::py_str(&r["watch"])));
        }
        let mut last = String::new();
        if last_runs {
            let failures = r["failures"].as_i64().unwrap_or(0);
            let failed = if failures != 0 {
                format!(", {failures} failed in a row")
            } else {
                String::new()
            };
            last = match ran.get(&ferb_py::value::py_str(&r["id"])) {
                None => "; last run: none yet".to_owned(),
                Some(run) => format!(
                    "; last run {}, {}{failed}, its answer's first line (model-written, data): {}",
                    local_said(st, &run["at"], "%a %d %b %H:%M"),
                    ferb_py::value::py_str(&run["status"]),
                    ferb_py::json::dumps_utf8(&run["first_line"])
                ),
            };
        }
        lines.push(format!(
            "- Schedule {} “{}” ({}): runs {}{extra}; {nxt}; instructions: {}{last}",
            ferb_py::value::py_str(&r["id"]),
            ferb_py::value::py_str(&r["name"]),
            ferb_py::value::py_str(&r["state"]),
            ferb_py::value::py_str(&r["repeat"]),
            ferb_py::value::py_str(&r["request"]),
        ));
    }
    Ok(lines)
}

/// `_schedules_card` (T-1066): the owner's schedules as a card, with the one line of code it stands for.
pub fn schedules_card(st: &State) -> Result<(String, Value), HandlerError> {
    let rows = schedule_rows(st)?;
    let line = if rows.is_empty() {
        "🗓 You have no schedules.".to_owned()
    } else {
        format!(
            "🗓 Your schedules ({}): {}",
            rows.len(),
            rows.iter()
                .map(|r| {
                    let state = ferb_py::value::py_str(&r["state"]);
                    format!(
                        "{} — {}{}",
                        ferb_py::value::py_str(&r["name"]),
                        ferb_py::value::py_str(&r["repeat"]),
                        if state == "active" {
                            String::new()
                        } else {
                            format!(" ({state})")
                        }
                    )
                })
                .collect::<Vec<_>>()
                .join("; ")
        )
    };
    let shown: Vec<Value> = rows
        .iter()
        .map(|r| {
            Value::Object(
                r.iter()
                    .filter(|(k, v)| *k != "request" && *k != "failures" && !v.is_null())
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
            )
        })
        .collect();
    Ok((
        line.clone(),
        json!({"type": "schedules", "count": rows.len(), "line": line, "schedules": shown}),
    ))
}

/// `_schedule_last_result` (L34 Phase E): a schedule's latest finished run as the owner was told it.
fn schedule_last_result(st: &State, sid: &str) -> Result<Value, HandlerError> {
    let row = ferb_store::task_results::TaskResults::new(st.db())
        .latest_of_schedule(sid)
        .map_err(sched_err)?;
    Ok(match row {
        Some(r) => json!({"at": r.get("at"), "status": r.get("status"),
                          "answer": st.redact_text(&ferb_py::value::py_str(r.get("answer").unwrap_or(&Value::Null)))}),
        None => Value::Null,
    })
}

// ---------- the handlers ----------

pub(crate) fn handlers() -> Vec<(&'static str, Handler)> {
    vec![
        ("schedules.list", |s, p| Box::pin(schedules_list(s, p))),
        ("schedules.change", |s, p| Box::pin(schedules_change(s, p))),
        ("reminders.list", |s, p| Box::pin(reminders_list(s, p))),
        ("reminders.cancel", |s, p| Box::pin(reminders_cancel(s, p))),
        ("timers.repeat", |s, p| Box::pin(timers_repeat(s, p))),
        ("timers.states", |s, p| Box::pin(timers_states(s, p))),
    ]
}

async fn schedules_list(st: Rc<State>, _p: Params) -> HandlerResult {
    let mut out = Vec::new();
    for s in listed_schedules(&st)? {
        out.push(json!({"id": s.id, "request": s.request, "name": label(&s), "rule": s.when_said(),
                        "state": s.state, "next_run": iso_or_null(s.next_run.as_ref()), "failures": s.failures,
                        "until": iso_or_null(s.until.as_ref()), "watch": s.watch,
                        "last_result": schedule_last_result(&st, &s.id)?}));
    }
    ok(json!({"schedules": out}))
}

/// `schedules.change {id, action}`. Fixed against Python (named in the tests): a missing id or action was an internal
/// error (KeyError); here it is a refusal.
async fn schedules_change(st: Rc<State>, params: Params) -> HandlerResult {
    let (Some(id), Some(action)) = (params.get("id"), params.get("action")) else {
        return Err(refusal(
            "schedules.change needs a schedule's id and an action: cancel, pause or resume",
        ));
    };
    let text = schedule_change(
        &st,
        &ferb_py::value::py_str(id),
        &ferb_py::value::py_str(action),
        "",
    )?;
    ok(json!({"text": text}))
}

async fn reminders_list(st: Rc<State>, _p: Params) -> HandlerResult {
    let pending = st
        .timers_plain()
        .pending(&["reminder", "timer"])
        .map_err(sched_err)?;
    let out: Vec<Value> = pending
        .iter()
        .map(|t| {
            let mut row = json!({"id": t.id, "kind": t.kind, "at": py::iso(&t.fire_at), "message": t.message,
                                 "name": timer_label(t)});
            if let (true, Some(c)) = (t.kind == "timer", t.created_at.as_ref()) {
                row["started"] = json!(py::iso(c));
            }
            row
        })
        .collect();
    ok(json!({"pending": out}))
}

/// `_timer_row`: a timer as `timers.repeat` returns it.
fn timer_row(st: &State, t: &Timer) -> Result<Value, HandlerError> {
    let of = st.timers_plain().repeat_of(&t.id).map_err(sched_err)?;
    let mut row = json!({"timer": t.id, "id": t.id, "kind": t.kind, "at": py::iso(&t.fire_at),
                         "name": timer_label(t), "started": iso_or_null(t.created_at.as_ref()),
                         "message": t.message, "state": shown_state(&t.state)});
    if let Some(of) = of {
        row["repeat_of"] = json!(of);
    }
    Ok(row)
}

/// `timers.repeat {id}` (APP-762, DMN-548): the same timer again, for the same duration, from now; a fired timer
/// repeats at most once (a second call returns that repeat); a running or cancelled one is refused.
async fn timers_repeat(st: Rc<State>, params: Params) -> HandlerResult {
    let timers = st.timers();
    let shown = ferb_py::value::py_str(params.get("id").unwrap_or(&Value::Null));
    let t = timers.get(&text(&params, "id")).map_err(sched_err)?;
    let Some((t, created)) = t.and_then(|t| {
        let c = t.created_at?;
        (t.kind == "timer" && t.fire_at > c).then_some((t, c))
    }) else {
        return Err(refusal(format!("no timer {shown} to repeat")));
    };
    if let Some(made) = timers.repeated_by(&t.id).map_err(sched_err)? {
        if let Some(again) = timers.get(&made).map_err(sched_err)? {
            return ok(timer_row(&st, &again)?);
        }
    }
    if t.state != "delivered" {
        return Err(refusal(if t.state == "cancelled" {
            "that timer was cancelled, so there's nothing to repeat"
        } else {
            "that timer is still running"
        }));
    }
    let span = py::us_between(&t.fire_at, &created);
    let at = py::plus_us(&st.now(), span).map_err(|e| sched_err(e.0))?;
    let again = timers
        .add(
            "timer",
            &at,
            &t.message,
            None,
            None,
            false,
            None,
            &t.name,
            Some(&t.id),
        )
        .map_err(sched_err)?;
    st.timer_wake.notify_waiters();
    st.journal(
        "timer.repeated",
        json!({"timer_id": again.id, "of": t.id, "by": "owner"}),
        None,
    )?;
    ok(timer_row(&st, &again)?)
}

/// `timers.states {ids}` (DMN-548): what FERB's rows say of the timers and reminders the app shows.
async fn timers_states(st: Rc<State>, params: Params) -> HandlerResult {
    let ids: Vec<String> = match params.get("ids") {
        Some(Value::Array(a)) if a.iter().all(Value::is_string) => {
            a.iter().map(ferb_py::value::py_str).collect()
        }
        _ => return Err(refusal("ids must be a list of timer ids")),
    };
    let timers = st.timers_plain();
    let mut out = Map::new();
    let mut seen = BTreeSet::new();
    for tid in ids {
        if !seen.insert(tid.clone()) {
            continue;
        }
        let Some(t) = timers.get(&tid).map_err(sched_err)? else {
            continue;
        };
        if t.kind != "reminder" && t.kind != "timer" {
            continue;
        }
        let mut row = json!({"kind": t.kind, "state": shown_state(&t.state)});
        if let Some(made) = timers.repeated_by(&t.id).map_err(sched_err)? {
            row["repeated_by"] = json!(made);
        }
        out.insert(t.id.clone(), row);
    }
    ok(json!({"timers": out}))
}

/// `reminders.cancel {id}`. Fixed against Python (named in the tests): a missing id was an internal error
/// (KeyError); here it is a refusal.
async fn reminders_cancel(st: Rc<State>, params: Params) -> HandlerResult {
    let Some(id) = params.get("id") else {
        return Err(refusal("reminders.cancel needs a reminder's id"));
    };
    let id = ferb_py::value::py_str(id);
    let timers = st.timers();
    let t = timers.get(&id).map_err(sched_err)?;
    match t {
        Some(t) if t.kind != "sleep" && timers.cancel(&t.id).map_err(sched_err)? => {
            ok(json!({"cancelled": t.id}))
        }
        _ => Err(refusal(format!("no pending reminder {id}"))),
    }
}
