//! The in-process event bus (ferb/bus.py): subscription and overflow signaling. Event loss never becomes a
//! successful outcome: critical events are never dropped (a subscriber whose queue is full of them is disconnected
//! with an explicit overflow); telemetry may be dropped, oldest first, and the subscriber is told how many.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use serde_json::json;
use tokio::sync::Notify;

use crate::interfaces::Clock;
use crate::types::Dict;
use crate::when::DateTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventClass {
    Critical,
    Telemetry,
}

impl EventClass {
    pub fn as_str(self) -> &'static str {
        match self {
            EventClass::Critical => "critical",
            EventClass::Telemetry => "telemetry",
        }
    }
}

/// bus.py:21-35. Never dropped.
pub const CRITICAL: &[&str] = &[
    "task.created",
    "task.started",
    "task.finished",
    "task.retried",
    "task.steered",
    "approval.requested",
    "approval.resolved",
    "device.changed",
    "browser.profile_paired",
    "timer.fired",
    "memory.committed",
    "timer.changed", // DMN-548: a timer set, cancelled (by any path) or fired: every client shows it
    "runtimes.changed", // DMN-580: a role's connection, model or effort, or a runtime's refusal, changed
    "memory.retired",
    "memory.expires",
    "memory.set_aside", // ORCH-317
    "memory.restored",
    "stop.engaged",
    "stop.lifted",
    "sense.paused",
    "sense.resumed",
    "sense.watcher_failed",
    "browser.debugger_detached",
    "browser.owner_stopped", // ORCH-302
    "browser.owner_took",
    "task.result",
    "task.resumed_event",
    "conversation.paused",
    "attention.notice",
    "task.paused", // ORCH-308
    "task.resumed",
    "loop.status",
    "memory.confirmed_derived",
    "memory.forgot_derived",
    "memory.rollback",
    "memory.forgot",
    "task.part", // ORCH-319
];

/// bus.py:36-44. May be dropped, oldest first.
pub const TELEMETRY: &[&str] = &[
    "tool.called",
    "tool.finished",
    "trail.event",
    "action.observed",
    "browser.connected",
    "browser.disconnected",
    "browser.profiles_changed",
    "memory.proposed",
    "secret.refused",
    "browser.dialog",
    "browser.file_chooser",
    "browser.download",
    "browser.cleanup_failed",
    "browser.connection_refused",
    "disturbance.observed",
    "memory.derived",
    "memory.digest",
    "memory.embedding_unavailable",
    "perception.attention",
    "perception.batch",
    "security.alignment",
    "memory.import.progress",
    "task.step",     // ORCH-531: a lost one is re-read: bus.dropped -> chat.status
    "task.progress", // APP-893: a lost one is replaced by the next
    "cursor.point",
];

/// The synthetic notice to a subscriber: `{"count": n}` telemetry events were dropped.
pub const DROPPED: &str = "bus.dropped";

#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    pub topic: String,
    pub at: DateTime,
    pub data: Dict,
}

impl Event {
    /// bus.py:55: critical when the topic is in CRITICAL, telemetry otherwise (the dropped notice included).
    pub fn class(&self) -> EventClass {
        if CRITICAL.contains(&self.topic.as_str()) {
            EventClass::Critical
        } else {
            EventClass::Telemetry
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BusError {
    /// bus.py:60: this subscriber fell so far behind that a critical event couldn't be queued; resubscribe and
    /// re-read.
    #[error("subscriber overflowed with critical events")]
    Overflow,
    /// bus.py:127.
    #[error("unknown bus topic '{0}'; declare it in CRITICAL or TELEMETRY first")]
    UnknownTopic(String),
}

#[derive(Default)]
struct State {
    queue: VecDeque<Event>,
    dropped: u64,
    reported_dropped: u64,
    overflowed: bool,
    closed: bool,
}

struct SubInner {
    /// None: every topic. Else exact names and `prefix.*` patterns.
    topics: Option<Vec<String>>,
    maxsize: usize,
    state: Mutex<State>,
    wakeup: Notify,
}

impl SubInner {
    /// bus.py:74.
    fn wants(&self, topic: &str) -> bool {
        match &self.topics {
            None => true,
            Some(topics) => topics
                .iter()
                .any(|t| topic == t || (t.ends_with(".*") && topic.starts_with(&t[..t.len() - 1]))),
        }
    }

    /// bus.py:78-94. True: the subscriber overflowed and must leave the bus.
    fn offer(&self, ev: &Event) -> bool {
        let mut state = self.state.lock().expect("bus subscriber lock");
        let mut overflowed = false;
        if state.queue.len() >= self.maxsize {
            if let Some(victim) = state
                .queue
                .iter()
                .position(|e| e.class() == EventClass::Telemetry)
            {
                state.queue.remove(victim); // drop the oldest telemetry
                state.dropped += 1;
                state.queue.push_back(ev.clone());
            } else if ev.class() == EventClass::Telemetry {
                state.dropped += 1; // queue is all critical: drop the new telemetry
            } else {
                state.overflowed = true; // never drop critical silently
                overflowed = true;
            }
        } else {
            state.queue.push_back(ev.clone());
        }
        drop(state);
        self.wakeup.notify_one();
        overflowed
    }
}

struct BusInner {
    clock: Arc<dyn Clock>,
    subs: Mutex<Vec<Arc<SubInner>>>,
}

impl BusInner {
    fn unsubscribe(&self, sub: &Arc<SubInner>) {
        self.subs
            .lock()
            .expect("bus lock")
            .retain(|s| !Arc::ptr_eq(s, sub));
    }
}

/// One subscriber's bounded queue (bus.py:64). Read with [`Subscription::next`]; one reader at a time.
pub struct Subscription {
    bus: Arc<BusInner>,
    inner: Arc<SubInner>,
}

impl Subscription {
    fn step(&self) -> Option<Result<Option<Event>, BusError>> {
        let mut state = self.inner.state.lock().expect("bus subscriber lock");
        if state.dropped > state.reported_dropped {
            let n = state.dropped - state.reported_dropped;
            state.reported_dropped = state.dropped;
            let mut data = Dict::new();
            data.insert("count".to_string(), json!(n));
            return Some(Ok(Some(Event {
                topic: DROPPED.to_string(),
                at: self.bus.clock.now(),
                data,
            })));
        }
        if let Some(ev) = state.queue.pop_front() {
            return Some(Ok(Some(ev)));
        }
        if state.overflowed {
            return Some(Err(BusError::Overflow));
        }
        state.closed.then_some(Ok(None))
    }

    /// bus.py:99-113: the next event, waiting for one. A count of dropped telemetry comes first, then queued
    /// events; after them `Err(Overflow)` for an overflowed subscriber, `Ok(None)` for a closed one.
    pub async fn next(&self) -> Result<Option<Event>, BusError> {
        loop {
            if let Some(found) = self.step() {
                return found;
            }
            self.inner.wakeup.notified().await;
        }
    }

    /// As [`Subscription::next`] without waiting: None when there is nothing to hand over yet.
    pub fn try_next(&self) -> Option<Result<Option<Event>, BusError>> {
        self.step()
    }

    /// Telemetry events dropped so far.
    pub fn dropped(&self) -> u64 {
        self.inner
            .state
            .lock()
            .expect("bus subscriber lock")
            .dropped
    }

    pub fn overflowed(&self) -> bool {
        self.inner
            .state
            .lock()
            .expect("bus subscriber lock")
            .overflowed
    }

    pub fn closed(&self) -> bool {
        self.inner.state.lock().expect("bus subscriber lock").closed
    }

    /// bus.py:115: leave the bus; what is already queued can still be read.
    pub fn close(&self) {
        self.inner.state.lock().expect("bus subscriber lock").closed = true;
        self.bus.unsubscribe(&self.inner);
        self.inner.wakeup.notify_one();
    }
}

#[derive(Clone)]
pub struct Bus {
    inner: Arc<BusInner>,
}

impl Bus {
    pub fn new(clock: Arc<dyn Clock>) -> Self {
        Bus {
            inner: Arc::new(BusInner {
                clock,
                subs: Mutex::new(Vec::new()),
            }),
        }
    }

    /// bus.py:126: stamp the event with the clock's time and offer it to every subscriber that wants its topic.
    pub fn publish(&self, topic: &str, data: Option<Dict>) -> Result<Event, BusError> {
        if !CRITICAL.contains(&topic) && !TELEMETRY.contains(&topic) {
            return Err(BusError::UnknownTopic(topic.to_string()));
        }
        let ev = Event {
            topic: topic.to_string(),
            at: self.inner.clock.now(),
            data: data.unwrap_or_default(),
        };
        let subs = self.inner.subs.lock().expect("bus lock").clone();
        for sub in subs {
            if sub.wants(topic) && sub.offer(&ev) {
                self.inner.unsubscribe(&sub);
            }
        }
        Ok(ev)
    }

    /// bus.py:135. `topics` None: everything; Python's default `maxsize` is [`Bus::DEFAULT_MAXSIZE`].
    pub fn subscribe(&self, topics: Option<&[&str]>, maxsize: usize) -> Subscription {
        let inner = Arc::new(SubInner {
            topics: topics.map(|t| t.iter().map(|s| s.to_string()).collect()),
            maxsize,
            state: Mutex::new(State::default()),
            wakeup: Notify::new(),
        });
        self.inner
            .subs
            .lock()
            .expect("bus lock")
            .push(inner.clone());
        Subscription {
            bus: self.inner.clone(),
            inner,
        }
    }

    pub const DEFAULT_MAXSIZE: usize = 1000;

    /// How many subscribers are on the bus.
    pub fn subscribers(&self) -> usize {
        self.inner.subs.lock().expect("bus lock").len()
    }
}
