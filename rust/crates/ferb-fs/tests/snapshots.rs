//! Snapshot requests that overlap or are given up (tests/test_recovery.py
//! test_a_cancelled_wait_still_records_the_snapshot_for_the_next_request): one attempt is shared, and a request
//! dropped while its snapshot is being taken leaves the snapshot to finish and be recorded for the next request.
//! The sequential behavior is checked against Python's fixture in tests/tools.rs (case `snapshots`).
//! decisions: HAND-206, HAND-216

use std::sync::{Arc, Mutex};

use chrono::{DateTime, FixedOffset};
use ferb_core::interfaces::{BoxFuture, Clock, IdSource};
use ferb_fs::recovery::{Snapshots, Take};
use rusqlite::Connection;
use serde_json::{json, Value};

struct At(DateTime<FixedOffset>);
impl Clock for At {
    fn now(&self) -> DateTime<FixedOffset> {
        self.0
    }
    fn monotonic(&self) -> f64 {
        0.0
    }
    fn sleep_until(&self, _: DateTime<FixedOffset>) -> BoxFuture<'_, ()> {
        Box::pin(async {})
    }
}

struct Seq(Mutex<u32>);
impl IdSource for Seq {
    fn new_id(&self) -> String {
        let mut n = self.0.lock().unwrap();
        *n += 1;
        format!("s-{n}")
    }
}

type Gate = Arc<(Mutex<bool>, std::sync::Condvar)>;

/// A snapshot taker that says it started (`entered`), then waits for the gate.
fn service(
    dir: &tempfile::TempDir,
    gate: Gate,
    n: Arc<Mutex<u32>>,
    entered: Arc<Mutex<bool>>,
) -> Snapshots {
    let path = dir.path().join("ferb.db");
    ferb_store::Db::open(&path).unwrap().migrate().unwrap();
    let db = Arc::new(Mutex::new(Connection::open(&path).unwrap()));
    let take: Take = Arc::new(move || {
        *entered.lock().unwrap() = true;
        let (lock, cv) = &*gate;
        let mut open = lock.lock().unwrap();
        while !*open {
            open = cv.wait(open).unwrap();
        }
        *n.lock().unwrap() += 1;
        Ok(Some("com.apple.TimeMachine.2026-03-04-050607.local".into()))
    });
    let clock = Arc::new(At(DateTime::parse_from_rfc3339(
        "2026-03-04T05:06:07+00:00",
    )
    .unwrap()));
    Snapshots::new(
        Some(take),
        db,
        clock,
        Arc::new(Seq(Mutex::new(0))),
        Arc::new(|v: &Value| v.clone()),
    )
}

async fn until(flag: &Arc<Mutex<bool>>) {
    while !*flag.lock().unwrap() {
        tokio::task::yield_now().await;
    }
}

fn open(gate: &Gate) {
    *gate.0.lock().unwrap() = true;
    gate.1.notify_all();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn overlapping_requests_share_one_attempt() {
    let dir = tempfile::tempdir().unwrap();
    let gate = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
    let n = Arc::new(Mutex::new(0));
    let entered = Arc::new(Mutex::new(false));
    let s = Arc::new(service(&dir, gate.clone(), n.clone(), entered.clone()));
    let first = tokio::spawn({
        let s = s.clone();
        async move { s.request(Some("t1"), "a", false).await }
    });
    until(&entered).await;
    let second = tokio::spawn({
        let s = s.clone();
        async move { s.request(Some("t2"), "b", false).await }
    });
    tokio::task::yield_now().await;
    open(&gate);
    let (a, b) = (first.await.unwrap(), second.await.unwrap());
    assert_eq!(*n.lock().unwrap(), 1);
    assert_eq!(a.get("reused"), None);
    assert_eq!(b.get("reused"), Some(&json!(true)));
    assert_eq!(a["record"], b["record"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cancelled_wait_still_records_the_snapshot_for_the_next_request() {
    let dir = tempfile::tempdir().unwrap();
    let gate = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
    let n = Arc::new(Mutex::new(0));
    let entered = Arc::new(Mutex::new(false));
    let s = Arc::new(service(&dir, gate.clone(), n.clone(), entered.clone()));
    let waiting = tokio::spawn({
        let s = s.clone();
        async move { s.request(Some("task-1"), "bulk delete", false).await }
    });
    until(&entered).await;
    waiting.abort();
    assert!(waiting.await.unwrap_err().is_cancelled());
    open(&gate);
    // the snapshot goes on without its asker; the next request shares or reuses it
    let next = s.request(Some("task-2"), "bulk delete", false).await;
    assert_eq!(*n.lock().unwrap(), 1);
    assert_eq!(next.get("reused"), Some(&json!(true)));
    let conn = Connection::open(dir.path().join("ferb.db")).unwrap();
    let rows: i64 = conn
        .query_row(
            "SELECT count(*) FROM recovery WHERE kind = 'snapshot'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(rows, 1);
}
