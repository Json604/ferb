//! The small policy modules: policy/blocked.py (the owner's line when a safety check stops a chat turn's entry,
//! ORCH-519), policy/escape.py (D31's exact-command key), policy/services.py (the planner's device facts),
//! policy/targets.py (recorded request targets) and policy/contacts.py (observed recipient metadata).

use std::collections::{BTreeSet, HashMap};

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

pub mod blocked {
    use crate::py;

    pub const REMINDERS: &str =
        "reminders and schedules are set only from words you wrote in this chat";
    pub const CHANGES: &str =
        "a reminder or schedule changes only when your own words in this message ask for it";
    pub const END_DATE: &str = "an end date comes only from words you wrote";
    pub const MEMORY: &str = "I save a memory only from words you wrote in this chat";
    pub const FORGET: &str =
        "I delete a memory only when a whole sentence or clause of yours asks for it";
    pub const CONVERSATION: &str =
        "a conversation I'm having for you changes only on your own instruction";
    pub const NOT_IN_WORDS: &str = "the words I quoted from you aren't in what you wrote";
    pub const NOT_IN_RECENT: &str =
        "the words I quoted from you aren't in this message or your recent messages in this chat";
    pub const DOESNT_SAY: &str = "the words I quoted from you don't say it";

    fn plain(text: &str) -> String {
        py::squeeze(text).trim_end_matches('.').to_string()
    }

    fn upper(text: &str) -> String {
        let mut chars = text.chars();
        match chars.next() {
            Some(c) => py::upper(&c.to_string()) + chars.as_str(),
            None => String::new(),
        }
    }

    /// `blocked`: "Blocked by a safety check: <check>. <Why>, so <outcome>. <Todo>".
    pub fn blocked(check: &str, why: &str, outcome: &str, todo: &str) -> String {
        format!(
            "Blocked by a safety check: {}. {}, so {}. {}.",
            plain(check),
            upper(&plain(why)),
            plain(outcome),
            upper(&plain(todo))
        )
    }
}

/// escape.py `escape_key`: D31 exact-command identity.
pub fn escape_key(rule: &str, command: &str, cwd: &str) -> String {
    let body = format!(
        "[{},{}]",
        ferb_py::json::dumps_utf8(command),
        ferb_py::json::dumps_utf8(cwd)
    );
    format!(
        "escape:{rule}:{}",
        hex::encode(Sha256::digest(body.as_bytes()))
    )
}

/// services.py `Environment`: installed apps and paired profiles as facts.
#[derive(Debug, Clone, Default)]
pub struct Environment {
    pub apps: Vec<(String, String)>,
    /// (browser, folder, name) or (browser, folder, name, account).
    pub paired: Vec<Vec<String>>,
}

/// services.py `summary`: the planner's device facts (stable, live).
pub fn services_summary(
    env: &Environment,
    macos: &str,
    running: &BTreeSet<String>,
) -> (String, String) {
    let profiles: Vec<Value> = env
        .paired
        .iter()
        .map(|p| {
            let mut m = Map::new();
            m.insert(
                "browser".into(),
                json!(p.first().cloned().unwrap_or_default()),
            );
            m.insert(
                "profile".into(),
                json!(p.get(1).cloned().unwrap_or_default()),
            );
            m.insert("name".into(), json!(p.get(2).cloned().unwrap_or_default()));
            if let Some(account) = p.get(3).filter(|a| !a.is_empty()) {
                m.insert("account".into(), json!(account));
            }
            Value::Object(m)
        })
        .collect();
    let stable = format!(
        "This Mac: macOS {macos}.\nPaired owner profiles: {}",
        ferb_py::json::dumps_utf8(&Value::Array(profiles))
    );
    let live = format!(
        "Running bundle IDs: {}",
        ferb_py::json::dumps(&running.iter().collect::<Vec<_>>())
    );
    (stable, live)
}

/// targets.py `Targets`: recorded request targets.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Targets {
    pub apps: BTreeSet<String>,
    pub sites: BTreeSet<String>,
    pub paths: BTreeSet<String>,
    pub recipients: BTreeSet<String>,
}

impl Targets {
    pub fn to_json(&self) -> Value {
        json!({"apps": self.apps, "sites": self.sites, "paths": self.paths, "recipients": self.recipients})
    }

    /// `Targets.from_json`: each field a list (anything else reads as empty), its items as text.
    pub fn from_json(d: &Value) -> Targets {
        let d = match d {
            Value::String(s) => ferb_py::json::loads(s).unwrap_or(Value::Null),
            other => other.clone(),
        };
        let fs = |k: &str| -> BTreeSet<String> {
            match d.get(k) {
                Some(Value::Array(items)) => items.iter().map(ferb_py::value::py_str).collect(),
                _ => BTreeSet::new(),
            }
        };
        Targets {
            apps: fs("apps"),
            sites: fs("sites"),
            paths: fs("paths"),
            recipients: fs("recipients"),
        }
    }
}

/// targets.py `Request`: the owner's words and observed targets for one request.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Request {
    pub request_id: String,
    pub words: String,
    pub targets: Targets,
    pub payload: String,
    pub asked_origins: BTreeSet<String>,
}

/// targets.py `Requests`: per-request facts, recorded when a task starts and forgotten when it ends.
#[derive(Debug, Default)]
pub struct Requests {
    by_id: HashMap<String, Request>,
}

impl Requests {
    pub fn record(&mut self, r: Request) {
        self.by_id.insert(r.request_id.clone(), r);
    }

    pub fn get(&self, request_id: &str) -> Request {
        self.by_id
            .get(request_id)
            .cloned()
            .unwrap_or_else(|| Request {
                request_id: request_id.to_string(),
                ..Request::default()
            })
    }

    pub fn forget(&mut self, request_id: &str) {
        self.by_id.remove(request_id);
    }
}

/// contacts.py `Contact`: observed recipient metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Contact {
    pub name: String,
    pub selected: String,
    pub detail: String,
}
