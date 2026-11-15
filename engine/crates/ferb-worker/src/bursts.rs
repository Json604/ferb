//! A tree's live line in Python's bursts. Python's chat stream reads a turn's queue and the bus's in two tasks, and
//! each task, once woken, takes everything queued for it: so what one piece of work tells between two of its
//! suspensions reaches the chat as one burst, grouped by queue (`_chat_send`; ferb-supervisor's chat.rs
//! `as_python_wakes`). A Rust worker thread can read the line while the work that tells it is half-way through a
//! burst (a worker's tool call runs on its connection's task, beside the chat's), so where the reader happens to cut
//! must not decide the order.
//!
//! [`per_poll`] runs a producer (a tree's work, a worker's tool call) so that what one poll tells a tree's line, from
//! one suspension to the next, goes onto the line as one message ([`BURST`]); the line's reader hands it on as one
//! burst, whenever it reads it. Outside such a producer an event goes onto the line at once.

use std::cell::RefCell;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use serde_json::{Map, Value};
use tokio::sync::mpsc;

/// A tree's live line (`Runs::open_live`).
pub type Line = mpsc::UnboundedSender<Map<String, Value>>;

/// The key of one poll's events on a tree's line, in the order they were told: one burst.
pub const BURST: &str = "\u{0}burst";

tokio::task_local! {
    /// What the producer being polled told, by line, until its poll ends.
    static HELD: RefCell<Vec<(Line, Map<String, Value>)>>;
}

/// An event onto a tree's line: held until the producer's poll ends when one is being polled, else sent now.
pub(crate) fn tell(line: &Line, event: Map<String, Value>) {
    let mut event = Some(event);
    let held = HELD
        .try_with(|held| {
            if let Some(ev) = event.take() {
                held.borrow_mut().push((line.clone(), ev));
            }
        })
        .is_ok();
    if !held {
        if let Some(ev) = event {
            let _ = line.send(ev);
        }
    }
}

/// `fut`, with what each of its polls tells a tree's line sent as one burst per line when the poll ends.
pub fn per_poll<F: Future>(fut: F) -> PerPoll<F> {
    PerPoll {
        inner: Box::pin(fut),
    }
}

/// See [`per_poll`].
pub struct PerPoll<F: Future> {
    inner: Pin<Box<F>>,
}

impl<F: Future> Future for PerPoll<F> {
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<F::Output> {
        let this = self.get_mut();
        let (out, held) = HELD.sync_scope(RefCell::new(Vec::new()), || {
            let out = this.inner.as_mut().poll(cx);
            (out, HELD.with(RefCell::take))
        });
        // each line's events in the order told, one message per line
        let mut lines: Vec<(Line, Vec<Value>)> = Vec::new();
        for (line, ev) in held {
            match lines.iter_mut().find(|(l, _)| l.same_channel(&line)) {
                Some((_, events)) => events.push(Value::Object(ev)),
                None => lines.push((line, vec![Value::Object(ev)])),
            }
        }
        for (line, events) in lines {
            let mut burst = Map::new();
            burst.insert(BURST.into(), Value::Array(events));
            let _ = line.send(burst);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ev(n: i64) -> Map<String, Value> {
        let mut m = Map::new();
        m.insert("n".into(), json!(n));
        m
    }

    #[tokio::test]
    async fn one_poll_is_one_message_and_outside_a_producer_each_event_goes_at_once() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        tell(&tx, ev(0));
        assert_eq!(rx.try_recv().unwrap(), ev(0));
        let line = tx.clone();
        per_poll(async move {
            tell(&line, ev(1));
            tell(&line, ev(2));
            tokio::task::yield_now().await;
            tell(&line, ev(3));
        })
        .await;
        let first = rx.try_recv().unwrap();
        assert_eq!(first[BURST], json!([{"n": 1}, {"n": 2}]));
        let second = rx.try_recv().unwrap();
        assert_eq!(second[BURST], json!([{"n": 3}]));
        assert!(rx.try_recv().is_err());
    }
}
