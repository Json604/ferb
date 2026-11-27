//! The previews ask what a task is doing (`status`, the supervisor's records, read from the database) with none of
//! their own locks held. On 2026-11-28 the owner's daemon froze: a `tasks.preview` request held the previews' state
//! while asking, and the resource sampler held the database while reading the previews' counters (an AB/BA
//! deadlock). Here `status` reads the counters itself: if a request still asked under the state lock, it would wait
//! on itself and the test fails on its time limit instead of hanging.

use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
use std::sync::{mpsc, Arc};
use std::time::Duration;

use ferb_daemon::areas_b::task_preview::{BoxFuture, PreviewClock, TaskPreviews};
use serde_json::Value;

struct Still;

impl PreviewClock for Still {
    fn monotonic(&self) -> f64 {
        100.0
    }
    fn sleep(&self, _seconds: f64) -> BoxFuture<()> {
        Box::pin(std::future::pending())
    }
}

fn previews() -> TaskPreviews {
    TaskPreviews::new(
        Arc::new(|_: &[String], _: &str| Ok(None)),
        Arc::new(|| true),
        5.0,
        Arc::new(|_: &str, _: Value| {}),
        Arc::new(Still),
    )
}

/// One request on its own thread; None when it didn't answer in time (it waits on a lock it holds).
fn answer(previews: &TaskPreviews, worker: Option<&'static str>) -> Option<Value> {
    let p = previews.clone();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let a = rt.block_on(p.frame("t1", worker, None));
        let _ = tx.send(Value::Object(a.fields));
    });
    rx.recv_timeout(Duration::from_secs(10)).ok()
}

// decision: ENG-24
#[test]
fn a_preview_asks_what_the_task_is_doing_with_none_of_its_locks_held() {
    for worker in [None, Some("t1.s1"), Some("t1.s2")] {
        let previews = previews();
        previews.declare("t1", "t1.s1", None, &[]);
        previews.declare("t1", "t1.s2", None, &[]);
        let asked = Arc::new(AtomicUsize::new(0));
        let (p, n) = (previews.clone(), asked.clone());
        previews.set_status(Arc::new(move |_root: &str, worker: &str| {
            let _ = p.snapshot(); // the counters, as the sampler reads them
            n.fetch_add(1, SeqCst);
            (!worker.is_empty()).then(|| ("asking".to_owned(), "Asking you".to_owned()))
        }));
        let got = answer(&previews, worker)
            .unwrap_or_else(|| panic!("{worker:?}: the request waited on its own lock"));
        assert_eq!(got["state"], "asking", "{worker:?}: {got}");
        assert!(asked.load(SeqCst) >= 1, "{worker:?}");
    }
}
