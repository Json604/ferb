//! `_Fresh` (areas/shared.py:142; T-1124): a slow read kept FRESH_S, refreshed behind the kept value.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ferb_clock::clock::FakeClock;
use ferb_daemon::shared::{Fresh, FRESH_S};

/// The first read waits; within FRESH_S the kept value is served without a read; after it the kept value is served at
/// once and one refresh runs in the background; a salt is a value of its own.
// decision: DMN-600
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_value_is_kept_ten_seconds_then_refreshed_behind() {
    let clock = FakeClock::new(None);
    let fresh = Fresh::new(FRESH_S, Arc::new(clock.clone()));
    let reads = Arc::new(AtomicU64::new(0));
    let read = {
        let reads = reads.clone();
        move || reads.fetch_add(1, Ordering::SeqCst) + 1
    };
    assert_eq!(fresh.get("permissions", "", read.clone()), 1);
    clock.advance(9.0);
    assert_eq!(fresh.get("permissions", "", read.clone()), 1);
    assert_eq!(
        reads.load(Ordering::SeqCst),
        1,
        "no read within the ten seconds"
    );
    clock.advance(2.0);
    assert_eq!(
        fresh.get("permissions", "", read.clone()),
        1,
        "the kept value at once"
    );
    let until = Instant::now() + Duration::from_secs(5);
    while fresh.get("permissions", "", read.clone()) != 2 {
        assert!(Instant::now() < until, "refreshed behind");
        tokio::task::yield_now().await;
    }
    assert_eq!(reads.load(Ordering::SeqCst), 2, "one refresh");
    assert_eq!(
        fresh.get("permissions", "other", read.clone()),
        3,
        "a salt is its own value"
    );
}

/// A refresh that panics leaves the kept value, and the next stale read refreshes again.
// decision: DMN-600
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refresh_that_fails_keeps_the_value() {
    let clock = FakeClock::new(None);
    let fresh = Fresh::new(FRESH_S, Arc::new(clock.clone()));
    let calls = Arc::new(AtomicU64::new(0));
    let read = {
        let calls = calls.clone();
        move || {
            if calls.fetch_add(1, Ordering::SeqCst) == 1 {
                panic!("TCC read failed");
            }
            7u32
        }
    };
    assert_eq!(fresh.get("k", "", read.clone()), 7);
    clock.advance(20.0);
    assert_eq!(fresh.get("k", "", read.clone()), 7);
    let until = Instant::now() + Duration::from_secs(5);
    // the failed refresh frees the value: a later stale read starts another, which keeps the third read's 7
    while calls.load(Ordering::SeqCst) < 3 {
        assert!(Instant::now() < until, "refreshed again after the failure");
        assert_eq!(fresh.get("k", "", read.clone()), 7, "the kept value stays");
        tokio::task::yield_now().await;
    }
}
