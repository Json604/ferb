//! FERB's macOS banner categories, the owner's switches for them, and FERB.app as their poster (L34 D). Port of
//! `ferb/daemon/banners.py`. Must not decide what to tell the owner: callers decide that; this only routes what they
//! hand it.
//!
//! While FERB.app holds a `notifications.subscribe` stream (macOS lets it notify), each banner goes to it and
//! FERB.app posts it with its own name and icon; otherwise the daemon posts it with osascript. The owner switches
//! categories off in Settings (`[banners]` in config.toml); chat, logs and the journal keep everything either way.

use std::sync::{Arc, Mutex};

use ferb_core::config::Config;
use ferb_rpc::HandlerError;
use serde_json::{Map, Value};
use tokio::sync::mpsc;

use super::core::refusal;

pub const SECTION: &str = "banners";
pub const CATEGORIES: [&str; 5] = ["question", "reminder", "alert", "task_finished", "loop"];
/// Banners one FERB.app connection may leave unread; past that it stopped reading, and osascript posts the rest.
pub const QUEUE_SIZE: usize = 100;

/// banners.py `Banner`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Banner {
    pub title: String,
    pub text: String,
    /// One of CATEGORIES; "" for one no switch governs.
    pub category: String,
    /// What a click opens: {"kind": "question"|"task"|"reminder", …}.
    pub target: Map<String, Value>,
    /// FERB.app posts it only while the notch isn't showing it.
    pub when_hidden: bool,
    /// The notification's id: a repost replaces it; "" for a new one.
    pub ident: String,
    /// A reminder's typed card from its row; empty for none.
    pub card: Map<String, Value>,
}

/// A category posts unless the owner set it to false; a banner with no category always posts. `get` reads
/// `[banners] <category>` (None when absent).
pub fn enabled(get: &dyn Fn(&str, &str) -> Option<Value>, category: &str) -> bool {
    !CATEGORIES.contains(&category) || get(SECTION, category) != Some(Value::Bool(false))
}

pub fn categories(get: &dyn Fn(&str, &str) -> Option<Value>) -> Map<String, Value> {
    CATEGORIES
        .iter()
        .map(|c| (c.to_string(), Value::Bool(enabled(get, c))))
        .collect()
}

/// `config.get` as banners read it: an unreadable config counts as no switch set (Python raises there; the
/// caller's own read already refused it).
pub fn getter(config: &Config) -> impl Fn(&str, &str) -> Option<Value> + '_ {
    move |section, key| config.get(section, key).ok().flatten()
}

/// The owner's switches from Settings → Notifications, checked whole before anything is written.
pub fn set_categories(
    config: &Config,
    changes: &Value,
) -> Result<Map<String, Value>, HandlerError> {
    let changes = match changes {
        Value::Object(o) if !o.is_empty() => o,
        _ => {
            return Err(refusal(format!(
                "set names the categories to switch: {}",
                CATEGORIES.join(", ")
            )))
        }
    };
    let mut unknown: Vec<String> = changes
        .keys()
        .filter(|k| !CATEGORIES.contains(&k.as_str()))
        .cloned()
        .collect();
    unknown.sort();
    if !unknown.is_empty() {
        let shown: Vec<String> = unknown.iter().map(|u| ferb_py_repr(u)).collect();
        return Err(refusal(format!(
            "unknown banner categories [{}]; they are {}",
            shown.join(", "),
            CATEGORIES.join(", ")
        )));
    }
    if !changes.values().all(Value::is_boolean) {
        return Err(refusal("each category is switched with true or false"));
    }
    for (category, on) in changes {
        config
            .set(SECTION, category, on)
            .map_err(|e| refusal(e.to_string()))?;
    }
    Ok(categories(&getter(config)))
}

/// Python's `repr` of a str inside a list (`['x']`).
fn ferb_py_repr(s: &str) -> String {
    if s.contains('\'') && !s.contains('"') {
        format!("\"{s}\"")
    } else {
        format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'"))
    }
}

type Queue = mpsc::Sender<Map<String, Value>>;

/// FERB.app's open `notifications.subscribe` streams, one queue each.
#[derive(Clone, Default)]
pub struct Posters {
    queues: Arc<Mutex<Vec<(u64, Queue)>>>,
    next: Arc<Mutex<u64>>,
}

/// One open stream's queue; closing it (dropping, or `Posters::close`) takes it off the list.
pub struct Poster {
    pub id: u64,
    pub queue: mpsc::Receiver<Map<String, Value>>,
}

impl Posters {
    pub fn connected(&self) -> bool {
        !self.queues.lock().unwrap().is_empty()
    }

    pub fn open(&self) -> Poster {
        let (tx, rx) = mpsc::channel(QUEUE_SIZE);
        let id = {
            let mut n = self.next.lock().unwrap();
            *n += 1;
            *n
        };
        self.queues.lock().unwrap().push((id, tx));
        Poster { id, queue: rx }
    }

    pub fn close(&self, id: u64) {
        self.queues.lock().unwrap().retain(|(q, _)| *q != id);
    }

    /// Hand the banner to every open poster; false when none took it (none connected, or each stopped reading).
    pub fn deliver(&self, item: &Map<String, Value>) -> bool {
        let mut took = false;
        for (_, q) in self.queues.lock().unwrap().iter() {
            if q.try_send(item.clone()).is_ok() {
                took = true;
            }
        }
        took
    }
}
