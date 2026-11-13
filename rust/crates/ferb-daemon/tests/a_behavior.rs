//! Group A's behavior no recording shows: the scheduled run's crash window (fixed against Python with ferb-clock's
//! claim and handoff), a run skipped while the previous one is under way, a timer's delivery told once.

mod a_support;

use std::sync::atomic::Ordering;

use a_support::{Daemon, Seq};
use ferb_clock::clock::FakeClock;
use ferb_daemon::areas_a::schedules;
use serde_json::{json, Value};

async fn once(d: &Daemon, at: &str) -> String {
    let at = at.to_owned();
    d.on(move |st| {
        Box::pin(async move {
            let rule = ferb_clock::schedules::Rule::from_json(
                json!({"repeat": "none", "tz": "UTC"}).as_object().unwrap(),
            );
            let first = ferb_clock::py::parse(&at).unwrap();
            let s = st
                .schedules()
                .add(
                    "Check the invented page",
                    "check it",
                    &serde_json::Map::new(),
                    &rule,
                    &first,
                    "none",
                    None,
                    "",
                )
                .unwrap();
            let t = schedules::book(&st, &s, &first, "", None).unwrap();
            st.schedules()
                .set_timer(&s.id, Some(&t.id), Some(&first))
                .unwrap();
            s.id
        })
    })
    .await
}

async fn tick(d: &Daemon) -> usize {
    let n = d
        .on(|st| Box::pin(async move { schedules::tick(&st).await.unwrap() }))
        .await;
    d.on(|st| Box::pin(async move { schedules::settle_runs(&st).await }))
        .await;
    n
}

fn kinds(calls: &[Value]) -> Vec<String> {
    calls
        .iter()
        .map(|c| c.as_object().unwrap().keys().next().unwrap().clone())
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_run_whose_task_was_never_recorded_is_handed_over_again_and_runs_once() {
    let tmp = tempfile::tempdir().unwrap();
    let d = Daemon::start(tmp.path(), "01", "id", FakeClock::new(None), Seq::new("id")).await;
    let sid = once(&d, "2026-01-01T09:05:00+00:00").await;
    d.stand.fail_create.store(1, Ordering::SeqCst);
    d.clock.advance(300.0);
    assert_eq!(tick(&d).await, 1);
    // nothing recorded: the timer is still claimed, not handled, and no run started
    let timers = d.table("timers").await;
    assert_eq!(timers[0][7], "claimed");
    assert!(d.table("timer_deliveries").await.is_empty());
    assert_eq!(kinds(&d.stand.calls.take()), ["create_scheduled"]);
    // the next pass hands it over again: the run is recorded and runs, once
    assert_eq!(tick(&d).await, 1);
    assert_eq!(
        kinds(&d.stand.calls.take()),
        ["create_scheduled", "run_scheduled", "task_result"]
    );
    assert_eq!(d.table("timers").await[0][7], "delivered");
    assert_eq!(d.table("timer_deliveries").await.len(), 1);
    assert_eq!(tick(&d).await, 0);
    let s = d.table("schedules").await;
    assert_eq!(s[0][1], json!(sid));
    assert_eq!(s[0][8], "done");
    d.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_run_while_the_previous_one_is_under_way_is_skipped_and_told() {
    let tmp = tempfile::tempdir().unwrap();
    let d = Daemon::start(tmp.path(), "01", "id", FakeClock::new(None), Seq::new("id")).await;
    let sid = once(&d, "2026-01-01T09:05:00+00:00").await;
    d.stand.running.lock().unwrap().insert(sid.clone());
    d.clock.advance(300.0);
    assert_eq!(tick(&d).await, 1);
    let calls = d.stand.calls.take();
    assert_eq!(kinds(&calls), ["task_result"]);
    assert_eq!(
        calls[0]["task_result"]["answer"],
        "“Check the invented page” was due, but the previous run is still under way, so it didn't run."
    );
    let journal = d.table("journal").await;
    let skipped: Vec<&Vec<Value>> = journal
        .iter()
        .filter(|r| r[3] == "schedule.skipped")
        .collect();
    assert_eq!(skipped.len(), 1);
    assert_eq!(
        skipped[0][4],
        json!(format!("{{\"schedule_id\": \"{sid}\", \"why\": \"the previous run is still under way\", \"retry\": false}}"))
    );
    assert_eq!(d.table("timers").await[0][7], "delivered");
    d.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_reminder_is_delivered_once_even_when_handed_over_again() {
    let tmp = tempfile::tempdir().unwrap();
    let d = Daemon::start(tmp.path(), "01", "id", FakeClock::new(None), Seq::new("id")).await;
    d.sql(vec![(
        "INSERT INTO timers (id, kind, fire_at, message, state, created_at) VALUES ('tm-1', 'reminder', \
         '2026-01-01T09:01:00+00:00', 'Call the invented plumber', 'pending', '2026-01-01T09:00:00+00:00')"
            .into(),
        vec![],
    )])
    .await;
    d.clock.advance(120.0);
    assert_eq!(tick(&d).await, 1);
    let calls = d.stand.calls.take();
    assert_eq!(
        calls[0]["notify"]["text"],
        "Call the invented plumber (due 1 min ago)"
    );
    // a crash after the delivery's record and before its acknowledgement: handed over again, told no more
    d.sql(vec![(
        "UPDATE timers SET state = 'claimed' WHERE id = 'tm-1'".into(),
        vec![],
    )])
    .await;
    assert_eq!(tick(&d).await, 1);
    assert!(d.stand.calls.take().is_empty());
    let delivered = d
        .table("journal")
        .await
        .into_iter()
        .filter(|r| r[3] == "timer.delivered")
        .count();
    assert_eq!(delivered, 1);
    d.close().await;
}
