//! `_sleep_until`'s durable sleep (areas/schedules.py:200), public for the daemon program: the timer loop's pass
//! wakes it when its "sleep" timer is due; a cancelled wait cancels its timer; either way no sleeper is left.

mod a_support;

use std::sync::{Arc, Mutex};

use a_support::{Daemon, Seq};
use ferb_clock::clock::{Clock, FakeClock};
use ferb_daemon::areas_a::schedules;

async fn timers(d: &Daemon) -> Vec<(String, String)> {
    d.on(|st| {
        Box::pin(async move {
            let conn = st.db().conn();
            let mut stmt = conn
                .prepare("SELECT kind, state FROM timers ORDER BY seq")
                .unwrap();
            let rows = stmt
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap()
                .collect::<Result<Vec<(String, String)>, _>>()
                .unwrap();
            rows
        })
    })
    .await
}

#[tokio::test]
async fn a_sleep_wakes_when_its_timer_fires_and_a_cancelled_one_cancels_its_timer() {
    let tmp = tempfile::tempdir().unwrap();
    let d = Daemon::start(tmp.path(), "01", "id", FakeClock::new(None), Seq::new("id")).await;
    // woken by the timer loop's pass once due
    let at = ferb_clock::py::iso(&(d.clock.now() + chrono::Duration::seconds(60)));
    let woke: Arc<Mutex<Option<Option<String>>>> = Arc::default();
    {
        let woke = woke.clone();
        d.on(move |st| {
            Box::pin(async move {
                let at = ferb_clock::py::parse(&at).unwrap();
                tokio::task::spawn_local(async move {
                    let got =
                        schedules::sleep_until(&st, "task-1", &at, std::future::pending()).await;
                    *woke.lock().unwrap() = Some(got);
                });
                tokio::task::yield_now().await;
            })
        })
        .await;
    }
    assert_eq!(timers(&d).await, [("sleep".into(), "pending".into())]);
    assert!(
        woke.lock().unwrap().is_none(),
        "asleep until the timer is due"
    );
    d.clock.advance(61.0);
    let fired = d
        .on(|st| Box::pin(async move { schedules::tick(&st).await.unwrap() }))
        .await;
    assert_eq!(fired, 1, "the sleep timer was handed over");
    for _ in 0..100 {
        if woke.lock().unwrap().is_some() {
            break;
        }
        d.on(|_| Box::pin(async { tokio::task::yield_now().await }))
            .await;
    }
    assert!(
        matches!(&*woke.lock().unwrap(), Some(Some(_))),
        "woken with its timer's id"
    );
    assert_ne!(timers(&d).await[0].1, "pending");

    // cancelled: the wait ends at once and its timer is cancelled
    let at = ferb_clock::py::iso(&(d.clock.now() + chrono::Duration::seconds(600)));
    let ended = d
        .on(move |st| {
            Box::pin(async move {
                let at = ferb_clock::py::parse(&at).unwrap();
                schedules::sleep_until(&st, "task-2", &at, async {}).await
            })
        })
        .await;
    assert_eq!(ended, None);
    assert_eq!(timers(&d).await[1], ("sleep".into(), "cancelled".into()));
    d.close().await;
}
