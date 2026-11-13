//! (c) The timer and schedule stores and the `time.*` tools against what the Python stores did
//! (`rust/fixtures/clock/timers.json`, `schedules_store.json`, `tools.json`): every call's return or refusal, what
//! was reported, and the tables' rows after it, text columns byte for byte. Temporary folders only.
//! decisions: DMN-547, DMN-548, ORCH-590 (as ORCH-590), ORCH-511, ORCH-542, TEST-508

#![cfg(feature = "store")]

mod common;

use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;
use std::future::Future;
use std::rc::Rc;

use common::{error, fixture, opt_time, time, tool_ctx, Diffs, YieldOnce};
use ferb_clock::clock::{Clock, FakeClock};
use ferb_clock::py::{self, Time};
use ferb_clock::schedule_store::{RunClaim, Schedules, StoreError};
use ferb_clock::schedules::Rule;
use ferb_clock::timers::{mark_handled, Delivery, Presence, Timer, Timers};
use ferb_clock::tools;
use ferb_clock::zone::Zone;
use ferb_core::interfaces::BoxFuture;
use ferb_core::types::{Identity, ToolContext};
use ferb_store::Db;
use rusqlite::types::Value as Sql;
use serde_json::{json, Map, Value};
use tempfile::TempDir;

fn store() -> (TempDir, Db) {
    let dir = TempDir::new().unwrap();
    let db = Db::open(dir.path().join("ferb.db")).unwrap();
    db.migrate().unwrap();
    (dir, db)
}

fn dump(db: &Db, tables: &[&str]) -> Value {
    let mut out = Map::new();
    for t in tables {
        let mut st = db
            .conn()
            .prepare(&format!("SELECT * FROM {t} ORDER BY rowid"))
            .unwrap();
        let cols: Vec<String> = st.column_names().iter().map(|c| c.to_string()).collect();
        let rows: Vec<Value> = st
            .query_map([], |r| {
                let mut m = Map::new();
                for (i, c) in cols.iter().enumerate() {
                    let v = match r.get::<_, Sql>(i)? {
                        Sql::Null => Value::Null,
                        Sql::Integer(n) => json!(n),
                        Sql::Real(f) => json!(f),
                        Sql::Text(s) => json!(s),
                        Sql::Blob(b) => json!(b),
                    };
                    m.insert(c.clone(), v);
                }
                Ok(Value::Object(m))
            })
            .unwrap()
            .map(Result::unwrap)
            .collect();
        out.insert(t.to_string(), Value::Array(rows));
    }
    Value::Object(out)
}

fn timer_json(t: &Timer) -> Value {
    json!({"id": t.id, "kind": t.kind, "fire_at": py::iso(&t.fire_at), "message": t.message, "task_id": t.task_id,
           "request_id": t.request_id, "state": t.state, "overdue_s": t.overdue_s, "name": t.name,
           "created_at": t.created_at.map(|c| py::iso(&c))})
}

fn seq(prefix: &'static str) -> impl Fn() -> String {
    let n = Cell::new(0);
    move || {
        n.set(n.get() + 1);
        format!("{prefix}-{:06}", n.get())
    }
}

const TIMER_TABLES: [&str; 5] = [
    "timers",
    "timer_when_active",
    "timer_names",
    "timer_repeats",
    "timer_deliveries",
];

#[derive(Clone, Default)]
struct Mac(Rc<RefCell<(bool, Option<Time>)>>);

impl Presence for Mac {
    fn owner_away(&self) -> bool {
        self.0.borrow().0
    }
    fn present_since(&self) -> Option<Time> {
        self.0.borrow().1
    }
}

/// A naive time the Rust type can't hold: Python refused it with "a timer needs a time zone".
fn naive(v: &Value) -> bool {
    v.as_str()
        .is_some_and(|s| matches!(py::parse_maybe_naive(s), Ok(None)))
}

fn block_on<F: std::future::Future>(f: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap()
        .block_on(f)
}

#[test]
fn timer_scenarios_match_python() {
    let mut d = Diffs::default();
    for scenario in fixture("timers.json").as_array().unwrap() {
        let (_dir, db) = store();
        let clock = FakeClock::new(Some(time(&scenario["start"])));
        let tokens = Cell::new(0u64);
        let changes: Rc<RefCell<Vec<Value>>> = Rc::default();
        let errors: Rc<RefCell<Vec<Value>>> = Rc::default();
        let telling = Rc::new(Cell::new(false));
        let failing = Rc::new(Cell::new(false));
        let mac = Mac::default();
        let mut timers = Timers::new(&db, &clock, seq("t")).tokens(move || {
            tokens.set(tokens.get() + 1);
            format!("{:016x}", tokens.get())
        });
        timers.set_presence(mac.clone());
        {
            let (changes, telling, failing) = (changes.clone(), telling.clone(), failing.clone());
            timers.on_change = Some(Box::new(move |t, state| {
                if !telling.get() {
                    return Ok(());
                }
                changes.borrow_mut().push(json!([t.id, t.kind, state]));
                if failing.get() {
                    Err("publish failed".into())
                } else {
                    Ok(())
                }
            }));
            let errors = errors.clone();
            timers.on_error = Some(Box::new(move |t, e| {
                errors
                    .borrow_mut()
                    .push(json!([t.map(|t| t.id.clone()), e]));
            }));
        }
        let name = scenario["name"].as_str().unwrap();
        for (i, step) in scenario["steps"].as_array().unwrap().iter().enumerate() {
            let s = &step["step"];
            let op = s["op"].as_str().unwrap();
            let what = format!("{name} step {i} {s}");
            let res: Result<Value, String> = match op {
                "add" if naive(&s["fire_at"]) || naive(&s["return_after"]) => {
                    // the Rust type is always aware: Python's runtime refusal is a type here
                    d.check(
                        &what,
                        &json!({"error": "ValueError", "message": "a timer needs a time zone"}),
                        &json!({"error": step["error"], "message": step["message"]}),
                    );
                    continue;
                }
                "add" => timers
                    .add(
                        s["kind"].as_str().unwrap(),
                        &time(&s["fire_at"]),
                        s["message"].as_str().unwrap_or(""),
                        s["task_id"].as_str(),
                        s["request_id"].as_str(),
                        s["when_active"].as_bool().unwrap_or(false),
                        opt_time(&s["return_after"]).as_ref(),
                        s["name"].as_str().unwrap_or(""),
                        s["repeat_of"].as_str(),
                    )
                    .map(|t| timer_json(&t))
                    .map_err(|e| e.to_string()),
                "get" => timers
                    .get(s["id"].as_str().unwrap())
                    .map(|t| t.as_ref().map_or(Value::Null, timer_json))
                    .map_err(|e| e.to_string()),
                "pending" => {
                    let kinds: Vec<&str> = s
                        .get("kinds")
                        .and_then(Value::as_array)
                        .map(|a| a.iter().map(|k| k.as_str().unwrap()).collect())
                        .unwrap_or_else(|| vec!["reminder", "timer"]);
                    timers
                        .pending(&kinds)
                        .map(|ts| json!(ts.iter().map(timer_json).collect::<Vec<_>>()))
                        .map_err(|e| e.to_string())
                }
                "cancel" => timers
                    .cancel(s["id"].as_str().unwrap())
                    .map(|b| json!(b))
                    .map_err(|e| e.to_string()),
                "cancel_task" => timers
                    .cancel_task(s["task_id"].as_str().unwrap())
                    .map(|n| json!(n))
                    .map_err(|e| e.to_string()),
                "cancel_all_sleeps" => timers
                    .cancel_all_sleeps()
                    .map(|n| json!(n))
                    .map_err(|e| e.to_string()),
                "when_active" => timers
                    .when_active(s["id"].as_str().unwrap())
                    .map(|b| json!(b))
                    .map_err(|e| e.to_string()),
                "next_due_in" => timers
                    .next_due_in()
                    .map(|f| json!(f))
                    .map_err(|e| e.to_string()),
                "claim_due" => timers
                    .claim_due()
                    .map(|ts| json!(ts.iter().map(timer_json).collect::<Vec<_>>()))
                    .map_err(|e| e.to_string()),
                "ack" => timers
                    .ack(s["id"].as_str().unwrap())
                    .map(|_| Value::Null)
                    .map_err(|e| e.to_string()),
                "advance" => {
                    clock.advance(s["s"].as_f64().unwrap());
                    Ok(Value::Null)
                }
                "mark_handled" => mark_handled(db.conn(), s["id"].as_str().unwrap(), &clock.now())
                    .map(|b| json!(b))
                    .map_err(|e| e.to_string()),
                "delivered" => timers
                    .delivered(s["id"].as_str().unwrap())
                    .map(|b| json!(b))
                    .map_err(|e| e.to_string()),
                "tick" => {
                    let fail: BTreeSet<String> = s["fail"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|x| x.as_str().unwrap().to_string())
                        .collect();
                    block_on(timers.tick(|t: Timer| {
                        let refuse = fail.contains(&t.id);
                        async move {
                            if refuse {
                                Err(format!("consumer refused {}", t.id))
                            } else {
                                Ok(Delivery::Done)
                            }
                        }
                    }))
                    .map(|n| json!(n))
                    .map_err(|e| e.to_string())
                }
                "tick_error" => {
                    db.conn()
                        .execute_batch("ALTER TABLE timer_when_active RENAME TO twa_gone")
                        .unwrap();
                    let out = match block_on(timers.tick(|_t: Timer| async { Ok(Delivery::Done) }))
                    {
                        Ok(_) => Value::Null,
                        Err(e) => {
                            timers.report(None, &e.to_string());
                            json!("reported")
                        }
                    };
                    db.conn()
                        .execute_batch("ALTER TABLE twa_gone RENAME TO timer_when_active")
                        .unwrap();
                    Ok(out)
                }
                "presence" => {
                    *mac.0.borrow_mut() = (s["away"].as_bool().unwrap(), opt_time(&s["since"]));
                    Ok(Value::Null)
                }
                "on_change" => {
                    telling.set(true);
                    failing.set(s["fail"].as_bool().unwrap());
                    Ok(Value::Null)
                }
                "repeated_by" => timers
                    .repeated_by(s["id"].as_str().unwrap())
                    .map(|x| json!(x))
                    .map_err(|e| e.to_string()),
                "repeat_of" => timers
                    .repeat_of(s["id"].as_str().unwrap())
                    .map(|x| json!(x))
                    .map_err(|e| e.to_string()),
                other => panic!("unknown op {other}"),
            };
            match res {
                Ok(ret) => d.check(&format!("{what} ret"), &step["ret"], &ret),
                Err(e) => d.check(&format!("{what} error"), &json!(step["message"]), &json!(e)),
            }
            d.check(
                &format!("{what} changes"),
                &step["changes"],
                &json!(changes.borrow_mut().split_off(0)),
            );
            d.check(
                &format!("{what} errors"),
                &step["errors"],
                &json!(errors.borrow_mut().split_off(0)),
            );
            d.check(
                &format!("{what} rows"),
                &step["rows"],
                &dump(&db, &TIMER_TABLES),
            );
        }
    }
    d.finish("timers");
}

#[test]
fn schedule_store_rows_match_python() {
    let mut d = Diffs::default();
    let zone = Zone::named("Asia/Kolkata").unwrap();
    for scenario in fixture("schedules_store.json").as_array().unwrap() {
        let (_dir, db) = store();
        let clock = FakeClock::new(Some(py::parse("2026-11-13T09:00:00+05:30").unwrap()));
        let store = Schedules::new(&db, &clock, seq("s"), zone);
        for (i, step) in scenario["steps"].as_array().unwrap().iter().enumerate() {
            let s = &step["step"];
            let what = format!("store step {i} {s}");
            let id = s["id"].as_str().unwrap_or("");
            let sched = |x: &ferb_clock::schedules::Schedule| {
                json!({"id": x.id, "request": x.request, "own_words": x.own_words, "targets": x.targets,
                       "rule": Value::Object(x.rule.to_json()), "next_run": x.next_run.map(|t| py::iso(&t)),
                       "timer_id": x.timer_id, "state": x.state, "failures": x.failures,
                       "first_run": py::iso(&x.first_run), "watch": x.watch,
                       "last_reported_met": x.last_reported_met, "until": x.until.map(|t| py::iso(&t)),
                       "name": x.name, "when_said": x.when_said()})
            };
            let res: Result<Value, StoreError> = match s["op"].as_str().unwrap() {
                "add" => store
                    .add(
                        s["request"].as_str().unwrap(),
                        s["own_words"].as_str().unwrap(),
                        s["targets"].as_object().unwrap(),
                        &Rule::from_json(s["rule"].as_object().unwrap()),
                        &time(&s["first"]),
                        s["watch"].as_str().unwrap(),
                        opt_time(&s["until"]).as_ref(),
                        s["name"].as_str().unwrap(),
                    )
                    .map(|x| json!(x.id)),
                "set_timer" => store
                    .set_timer(
                        id,
                        s["timer_id"].as_str(),
                        opt_time(&s["next_run"]).as_ref(),
                    )
                    .map(|_| Value::Null),
                "set_state" => store
                    .set_state(id, s["state"].as_str().unwrap())
                    .map(|_| Value::Null),
                "reported" => store
                    .reported(id, s["met"].as_bool().unwrap())
                    .map(|_| Value::Null),
                "ran" => store.ran(id, s["ok"].as_bool()).map(|n| json!(n)),
                "edit" => {
                    let rule = s
                        .get("rule")
                        .map(|r| Rule::from_json(r.as_object().unwrap()));
                    let first = opt_time(s.get("first").unwrap_or(&Value::Null));
                    store
                        .edit(
                            id,
                            s["request"].as_str(),
                            rule.as_ref().zip(first.as_ref()),
                            s.get("targets").and_then(Value::as_object),
                            s["own_words"].as_str().unwrap_or(""),
                        )
                        .map(|_| Value::Null)
                }
                "set_until" => store.set_until(id, &time(&s["until"])).map(|_| Value::Null),
                "set_name" => store
                    .set_name(id, s["name"].as_str().unwrap())
                    .map(|_| Value::Null),
                "get" => store.get(id).map(|x| x.as_ref().map_or(Value::Null, sched)),
                "all" => {
                    let states: Vec<&str> = s
                        .get("states")
                        .and_then(Value::as_array)
                        .map(|a| a.iter().map(|k| k.as_str().unwrap()).collect())
                        .unwrap_or_else(|| vec!["active", "paused"]);
                    store
                        .all(&states)
                        .map(|xs| json!(xs.iter().map(sched).collect::<Vec<_>>()))
                }
                other => panic!("unknown op {other}"),
            };
            match res {
                Ok(ret) => d.check(&format!("{what} ret"), &step["ret"], &ret),
                Err(StoreError::Schedule(e)) => d.check(
                    &format!("{what} error"),
                    &error(
                        step["error"].as_str().unwrap_or(""),
                        step["message"].as_str().unwrap_or(""),
                    ),
                    &error(e.kind(), e.message()),
                ),
                // sqlite3's own errors (an IntegrityError): the same words from the same SQLite
                Err(e) => d.check(
                    &format!("{what} db error"),
                    &step["message"],
                    &json!(e.to_string()),
                ),
            }
            d.check(
                &format!("{what} rows"),
                &step["rows"],
                &dump(&db, &["schedules", "schedule_ends", "schedule_names"]),
            );
            clock.advance(61.5);
        }
    }
    d.finish("schedule store");
}

struct Sleep<'a> {
    clock: &'a FakeClock,
    slept: std::sync::Mutex<Vec<Value>>,
}

impl tools::Sleeper for Sleep<'_> {
    fn sleep<'a>(
        &'a self,
        worker: &'a str,
        task: &'a str,
        at: &'a Time,
        span: Option<&'a str>,
    ) -> BoxFuture<'a, ()> {
        self.slept
            .lock()
            .unwrap()
            .push(json!([worker, task, py::iso(at), span]));
        self.clock.set_wall(*at);
        Box::pin(async {})
    }
}

fn ctx() -> ToolContext {
    tool_ctx(Identity::Ferb, &["time"], "/w", Some("sp1"))
}

#[test]
fn the_time_tools_answer_as_python_does() {
    let mut d = Diffs::default();
    let (_dir, db) = store();
    let zone = Zone::named("Asia/Kolkata").unwrap();
    let clock = FakeClock::new(Some(py::parse("2026-11-13T09:00:00+05:30").unwrap()));
    let timers = Timers::new(&db, &clock, seq("t"));
    let limit = py::plus_us(&clock.now(), 4 * 3600 * py::US_PER_SECOND).unwrap();
    let sleeper = Sleep {
        clock: &clock,
        slept: std::sync::Mutex::new(vec![]),
    };
    let c = ctx();
    let cases = fixture("tools.json");
    for case in cases.as_array().unwrap() {
        if let Some(desc) = case.get("descriptions") {
            for (tool, text) in desc.as_object().unwrap() {
                d.check(
                    &format!("description {tool}"),
                    text,
                    &json!(tools::description(tool)),
                );
            }
            continue;
        }
        let args = &case["args"];
        let now = clock.now();
        d.check("now", &case["now"], &json!(py::iso(&now)));
        let book: Option<&dyn tools::TimerBook> = if case["content"]["error"]
            == json!("timers aren't available")
            && case["tool"] == "time.remind_at"
        {
            None
        } else {
            Some(&timers)
        };
        let res = match case["tool"].as_str().unwrap() {
            "time.remind_at" => tools::remind_at(book, args, &now, &zone, &c),
            "time.set_timer" => tools::set_timer(book, args, &now, &zone, &c),
            _ => block_on(tools::sleep_until(
                Some(&sleeper),
                args,
                &|| clock.now(),
                Some(limit),
                &zone,
                &c,
            )),
        };
        let what = format!("{} {args}", case["tool"]);
        d.check(
            &format!("{what} content"),
            &case["content"],
            &Value::Object(res.content),
        );
        d.check(
            &format!("{what} is_error"),
            &case["is_error"],
            &json!(res.is_error),
        );
        d.check(
            &format!("{what} slept"),
            &case["slept"],
            &json!(sleeper.slept.lock().unwrap().split_off(0)),
        );
        d.check(
            &format!("{what} rows"),
            &case["rows"],
            &dump(&db, &["timers"]),
        );
    }
    d.finish("tools");
}

// ---------- the crash window (fixed against Python) ----------

fn daily_schedule(store: &Schedules<'_>, first: &Time) -> String {
    let zone = Zone::named("Asia/Kolkata").unwrap();
    let rule = Rule::build("daily", first, None, None, &zone, false).unwrap();
    store
        .add(
            "check the slots",
            "check the slots daily",
            &Map::new(),
            &rule,
            first,
            "none",
            None,
            "",
        )
        .unwrap()
        .id
}

/// decision: ORCH-511. Fixed against Python (`ferb/daemon/areas/schedules.py:659-687`): Python marks a schedule's
/// timer handled, books the next run, and only then creates the run's task; a crash in between lost that run.
/// With the claim and the handoff, a crash before the task exists leaves the timer claimed and unhandled: the
/// next start hands it over again, the run happens (late), and the next run is not booked twice.
#[test]
fn a_crash_between_booking_the_next_run_and_creating_the_task_loses_no_run() {
    let (_dir, db) = store();
    let clock = FakeClock::new(Some(py::parse("2026-11-13T09:00:00+05:30").unwrap()));
    let zone = Zone::named("Asia/Kolkata").unwrap();
    db.conn()
        .execute_batch("CREATE TABLE runs (task_id TEXT PRIMARY KEY, timer_id TEXT)")
        .unwrap();
    let store = Schedules::new(&db, &clock, seq("s"), zone);
    let first = py::parse("2026-11-13T10:00:00+05:30").unwrap();
    let sid = daily_schedule(&store, &first);
    {
        let timers = Timers::new(&db, &clock, seq("a"));
        let t = timers
            .add("schedule", &first, &sid, None, None, false, None, "", None)
            .unwrap();
        store.set_timer(&sid, Some(&t.id), Some(&first)).unwrap();
    }
    clock.advance(3600.0 + 5.0);

    // the first daemon: claims the run, books the next one, then dies before the task exists
    let booked = {
        let timers = Timers::new(&db, &clock, seq("b"));
        let claimed = timers.claim_due().unwrap();
        assert_eq!(claimed.len(), 1);
        let RunClaim::Run(run) = store.claim_run(&timers, &claimed[0]).unwrap() else {
            panic!("a run to start")
        };
        assert!(!run.redelivered);
        run.booked.clone().expect("the next run booked")
    };
    assert_eq!(py::iso(&booked.0), "2026-11-14T04:30:00+00:00");

    // the next start: the claimed timer is handed over again, and its run starts
    clock.advance(120.0);
    let timers = Timers::new(&db, &clock, seq("c"));
    let again = timers.claim_due().unwrap();
    assert_eq!(again.len(), 1, "the unhandled claim comes back");
    let RunClaim::Run(run) = store.claim_run(&timers, &again[0]).unwrap() else {
        panic!("the run is still to start")
    };
    assert!(
        run.redelivered && run.booked.is_none(),
        "the next run isn't booked twice"
    );
    assert_eq!(
        run.overdue_s, 125.0,
        "late from its time, 10:00, to the next start's look"
    );
    store
        .start_run(&timers, &run, |c| -> Result<(), StoreError> {
            c.execute("INSERT INTO runs VALUES ('sc-run-1', ?)", [&run.timer_id])?;
            Ok(())
        })
        .unwrap();
    let schedule_timers: i64 = db
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM timers WHERE kind = 'schedule' AND state = 'pending'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(schedule_timers, 1, "one next run");
    assert!(timers.delivered(&run.timer_id).unwrap());
    assert_eq!(
        timers.get(&run.timer_id).unwrap().unwrap().state,
        "delivered"
    );
    // a third look: nothing to hand over, and a stale redelivery is only acknowledged
    assert!(timers.claim_due().unwrap().is_empty());
    assert_eq!(
        store.claim_run(&timers, &again[0]).unwrap(),
        RunClaim::AlreadyHandled
    );

    // the Python order, for contrast: handled first, so the same crash leaves nothing to hand over
    let lost = {
        let (_dir2, db2) = self::store();
        let clock2 = FakeClock::new(Some(py::parse("2026-11-13T09:00:00+05:30").unwrap()));
        let store2 = Schedules::new(&db2, &clock2, seq("s"), zone);
        let sid2 = daily_schedule(&store2, &first);
        let timers2 = Timers::new(&db2, &clock2, seq("d"));
        let t = timers2
            .add("schedule", &first, &sid2, None, None, false, None, "", None)
            .unwrap();
        store2.set_timer(&sid2, Some(&t.id), Some(&first)).unwrap();
        clock2.advance(3605.0);
        let claimed = timers2.claim_due().unwrap();
        assert!(mark_handled(db2.conn(), &claimed[0].id, &clock2.now()).unwrap()); // areas/schedules.py:150
        block_on(timers2.tick(|_t: Timer| async { Ok(Delivery::Done) })).unwrap(); // acknowledged
        timers2.claim_due().unwrap().is_empty() // after the crash: nothing comes back, no task exists
    };
    assert!(lost);
}

/// A failed task creation leaves the timer claimed and unhandled (handed over again), and a schedule the owner
/// paused before the next start is not run.
#[test]
fn a_failed_handoff_is_handed_over_again_and_a_paused_schedule_is_not_run() {
    let (_dir, db) = store();
    let clock = FakeClock::new(Some(py::parse("2026-11-13T09:00:00+05:30").unwrap()));
    let zone = Zone::named("Asia/Kolkata").unwrap();
    let store = Schedules::new(&db, &clock, seq("s"), zone);
    let first = py::parse("2026-11-13T10:00:00+05:30").unwrap();
    let sid = daily_schedule(&store, &first);
    let timers = Timers::new(&db, &clock, seq("a"));
    let t = timers
        .add("schedule", &first, &sid, None, None, false, None, "", None)
        .unwrap();
    store.set_timer(&sid, Some(&t.id), Some(&first)).unwrap();
    clock.advance(3605.0);
    let claimed = timers.claim_due().unwrap();
    let RunClaim::Run(run) = store.claim_run(&timers, &claimed[0]).unwrap() else {
        panic!()
    };
    let failed = store.start_run(&timers, &run, |_c| -> Result<(), StoreError> {
        Err(StoreError::BadJson("the task couldn't be made".into()))
    });
    assert!(failed.is_err());
    assert!(!timers.delivered(&t.id).unwrap());
    assert_eq!(timers.get(&t.id).unwrap().unwrap().state, "claimed");
    // paused before the next start: the redelivery is only acknowledged
    store.set_state(&sid, "paused").unwrap();
    let again = timers.claim_due().unwrap();
    assert_eq!(
        store.claim_run(&timers, &again[0]).unwrap(),
        RunClaim::NotRunnable
    );
}

/// Fixed against Python: a failure computing the next check ends Python's timer loop for good (`next_due_in` is
/// outside its `try`); here it is reported and the loop goes on.
#[test]
fn the_timer_loop_survives_a_failing_check() {
    let (_dir, db) = store();
    let clock = FakeClock::new(None);
    let reported: Rc<RefCell<Vec<String>>> = Rc::default();
    let mut timers = Timers::new(&db, &clock, seq("t"));
    {
        let reported = reported.clone();
        timers.on_error = Some(Box::new(move |_t, e| {
            reported.borrow_mut().push(e.to_string())
        }));
    }
    db.conn()
        .execute_batch("ALTER TABLE timer_when_active RENAME TO twa_gone")
        .unwrap();
    let rests = Cell::new(0);
    let run = timers.run(
        |_t: Timer| async { Ok(Delivery::Done) },
        |_s| {
            rests.set(rests.get() + 1);
            YieldOnce(false)
        },
    );
    let mut run = std::pin::pin!(run);
    let waker = std::task::Waker::noop();
    let mut cx = std::task::Context::from_waker(waker);
    for _ in 0..10 {
        assert!(run.as_mut().poll(&mut cx).is_pending());
    }
    assert!(rests.get() >= 3, "the loop went on resting");
    assert!(reported
        .borrow()
        .iter()
        .any(|e| e.contains("no such table")));
}
