//! The daemon's published events: each one is redacted once, then handed to its consumers in a fixed order. Port of
//! `ferb/daemon/events.py`. Must not change an event's data beyond redaction, or let a consumer see it unredacted.

use std::collections::BTreeSet;
use std::sync::{Arc, RwLock};

use serde_json::{Map, Value};

pub type Redact = Arc<dyn Fn(&Value) -> Value + Send + Sync>;
pub type Consumer = Arc<dyn Fn(&str, Map<String, Value>) + Send + Sync>;

/// events.py `Events`.
pub struct Events {
    /// Read per event: the daemon may still swap its redactor at start.
    redact: Box<dyn Fn() -> Redact + Send + Sync>,
    consumers: RwLock<Vec<(Option<BTreeSet<String>>, Consumer)>>,
}

impl Events {
    pub fn new(redact: impl Fn() -> Redact + Send + Sync + 'static) -> Events {
        Events {
            redact: Box::new(redact),
            consumers: RwLock::new(Vec::new()),
        }
    }

    /// `consumer(topic, redacted data)` for these topics (None: every topic), after the ones added before it.
    pub fn on(&self, topics: Option<&[&str]>, consumer: Consumer) {
        let topics = topics.map(|t| t.iter().map(|s| s.to_string()).collect());
        self.consumers.write().unwrap().push((topics, consumer));
    }

    pub fn publish(&self, topic: &str, data: &Map<String, Value>) {
        let redacted = (self.redact)()(&Value::Object(data.clone()));
        let redacted = match redacted {
            Value::Object(o) => o,
            _ => Map::new(),
        };
        let consumers = self.consumers.read().unwrap().clone();
        for (topics, consumer) in consumers {
            if topics.as_ref().is_none_or(|t| t.contains(topic)) {
                // a shallow copy each: one consumer adding a key can't leak to the next
                consumer(topic, redacted.clone());
            }
        }
    }
}

impl super::core::Bus for Events {
    fn publish(&self, topic: &str, data: Value) {
        match data {
            Value::Object(o) => Events::publish(self, topic, &o),
            _ => Events::publish(self, topic, &Map::new()),
        }
    }
}
