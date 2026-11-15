//! The watchers' and trail's daemon side: the sensing pause, its status, the owner's exclusions, the watcher and tap
//! records. Port of `ferb/daemon/areas/sensing.py`. Must not record what the owner excluded.
//!
//! RPCs: `sense.pause`, `sense.resume`, `sense.status`. The watcher manager is ferb-sense's real [`Sense`]; the
//! owner-input tap's state, perception's status and the folder problems come through [`Deps`].
//!
//! Not ported here: `_build_sense` (it constructs the trail, perception and every watcher: the daemon composition)
//! and the attention flow (`_attention_arrived`, `_attention_turns`, `_consider_attention`), which only Jev's live
//! `trail.attention` answers start: Jev is off (jev.rs), so it never runs; it waits for the supervisor port's
//! `attention` call.

use std::sync::{Arc, Mutex};

use ferb_rpc::{HandlerError, HandlerResult, Server};
use ferb_sense::exclusions::{Exclusions, DEFAULT_EXCLUDE_PATHS};
use ferb_sense::manager::Sense;
use serde_json::{json, Map, Value};

use super::core::{ok, Host};

/// SENSE-406: what the journal says while the owner-input tap is down.
pub const OWNER_INPUT_UNKNOWN: &str =
    "FERB can't tell whether you're using the Mac, so it counts you as using it: \
                                       no typing or clicking in front of you until it can";

pub struct Deps {
    /// The watcher manager (`self.sense`); None before it is built.
    pub sense: Option<Arc<Sense>>,
    /// `self.owner_input.running`.
    pub owner_input_running: Arc<dyn Fn() -> bool + Send + Sync>,
    /// `self.watch_problems`: the watch folders `_watch_folders` skipped.
    pub watch_problems: Arc<Mutex<Vec<String>>>,
    /// `self.perception.status()`; None when there is no perception (`{}`).
    pub perception_status: Option<Arc<dyn Fn() -> Result<Value, String> + Send + Sync>>,
}

pub fn register(server: &Server, _host: &Host, deps: &Deps) {
    let sense = deps.sense.clone();
    server.register_unary("sense.pause", move |_s, _p: Map<String, Value>| {
        let sense = sense.clone();
        async move {
            let sense = sense.ok_or_else(not_built)?;
            sense.pause().await.map_err(os_error)?;
            ok(json!({"paused": true}))
        }
    });
    let sense = deps.sense.clone();
    server.register_unary("sense.resume", move |_s, _p: Map<String, Value>| {
        let sense = sense.clone();
        async move {
            let sense = sense.ok_or_else(not_built)?;
            sense.resume().await.map_err(os_error)?;
            ok(json!({"paused": false, "watchers": sense.running()}))
        }
    });
    let (sense, running, problems, perception) = (
        deps.sense.clone(),
        deps.owner_input_running.clone(),
        deps.watch_problems.clone(),
        deps.perception_status.clone(),
    );
    server.register_unary("sense.status", move |_s, _p: Map<String, Value>| {
        let (sense, running, problems, perception) = (
            sense.clone(),
            running.clone(),
            problems.clone(),
            perception.clone(),
        );
        async move {
            status(
                sense.as_deref(),
                running(),
                &problems.lock().unwrap(),
                perception,
            )
        }
    });
}

/// Python's `assert self.sense is not None`.
fn not_built() -> HandlerError {
    HandlerError::fault("AssertionError", "")
}

fn os_error(e: std::io::Error) -> HandlerError {
    HandlerError::fault(
        match e.kind() {
            std::io::ErrorKind::PermissionDenied => "PermissionError",
            std::io::ErrorKind::NotFound => "FileNotFoundError",
            _ => "OSError",
        },
        e.to_string(),
    )
}

type PerceptionStatus = Option<Arc<dyn Fn() -> Result<Value, String> + Send + Sync>>;

/// `_sense_status`.
fn status(
    sense: Option<&Sense>,
    owner_input: bool,
    problems: &[String],
    perception: PerceptionStatus,
) -> HandlerResult {
    let paused = sense.is_some_and(Sense::paused);
    let failed: Map<String, Value> = sense
        .map(|s| s.failed().into_iter().map(|(k, v)| (k, json!(v))).collect())
        .unwrap_or_default();
    // SENSE-406: why the owner-input tap is down ("" while it runs, or while sensing is paused)
    let why = match sense {
        Some(s) if !s.paused() && !owner_input => s.tap_down(),
        _ => String::new(),
    };
    let perception = match perception {
        Some(f) => f().map_err(|e| HandlerError::fault("DatabaseError", e))?,
        None => json!({}),
    };
    ok(
        json!({"paused": paused, "watchers": sense.map(Sense::running).unwrap_or_default(),
              "failed": failed, "owner_input": owner_input, "folder_problems": problems,
              "owner_input_why": why, "perception": perception}),
    )
}

/// `_exclusions`: the `[privacy]` lists (the owner may edit them while FERB runs; config re-reads on a change).
pub fn exclusions(h: &Host) -> Result<Exclusions, HandlerError> {
    let apps: Vec<String> = h
        .config
        .exclude_apps()
        .map_err(|e| super::core::refusal(e.to_string()))?
        .into_iter()
        .collect();
    let list = |v: Option<Value>, default: &[&str]| -> Vec<String> {
        match v {
            Some(Value::Array(a)) => a.iter().map(ferb_py::value::py_str).collect(),
            Some(_) => vec![],
            None => default.iter().map(|s| s.to_string()).collect(),
        }
    };
    let domains = list(h.get("privacy", "exclude_domains")?, &[]);
    let paths = list(h.get("privacy", "exclude_paths")?, &DEFAULT_EXCLUDE_PATHS);
    Ok(Exclusions::build(&apps, &domains, &paths))
}

/// `_watch_folders`: the `[sense] watch_folders` FERB may watch; each skipped one journaled `sense.folder_skipped`.
pub fn watch_folders(
    h: &Host,
    user_home: &str,
    problems_out: &Mutex<Vec<String>>,
) -> Result<Vec<String>, HandlerError> {
    let folders: Vec<String> = match h.get("sense", "watch_folders")? {
        Some(Value::Array(a)) => a.iter().map(ferb_py::value::py_str).collect(),
        _ => vec![],
    };
    let (ok, problems) =
        ferb_sense::watchers::files::valid_folders(&folders, &exclusions(h)?, user_home);
    for p in &problems {
        h.journal("sense.folder_skipped", json!({"why": p}), None)?;
    }
    *problems_out.lock().unwrap() = problems;
    Ok(ok)
}

/// `_tap_changed` (SENSE-406): the owner-input tap went down (`why`) or came back up.
pub fn tap_changed(h: &Host, why: &str) -> Result<(), HandlerError> {
    if why.is_empty() {
        h.journal("owner_input.started", json!({}), None)
    } else {
        h.journal(
            "owner_input.unavailable",
            json!({"why": why, "effect": OWNER_INPUT_UNKNOWN}),
            None,
        )
    }
}

/// `_watcher_failed`.
pub fn watcher_failed(h: &Host, source: &str, why: &str) -> Result<(), HandlerError> {
    h.journal(
        "sense.watcher_failed",
        json!({"source": source, "error": why}),
        None,
    )
}

/// `_on_pause`: a pause forgets the owner's tabs the hub knew (`forget_owner_tabs`); journaled and published.
pub fn on_pause(h: &Host, paused: bool, forget_owner_tabs: &dyn Fn()) -> Result<(), HandlerError> {
    if paused {
        forget_owner_tabs();
    }
    let kind = if paused {
        "sense.paused"
    } else {
        "sense.resumed"
    };
    h.journal(kind, json!({}), None)?;
    h.publish(kind, json!({}));
    Ok(())
}

/// `_tab_sound` (HAND-207, HAND-213): a tab that isn't FERB's started or stopped sound, or was muted or unmuted:
/// the change handed to the disturbance sentinel, without a mute the owner changed themselves. Returns (tab,
/// change) for `DisturbanceSentinel.sound_changed`, or None for one of FERB's own tabs.
#[allow(clippy::too_many_arguments)]
pub fn tab_sound(
    browser: &str,
    tab_id: i64,
    ferb: bool,
    changed: &[String],
    audible: Option<bool>,
    muted: Option<bool>,
    muted_reason: Option<&str>,
    task: Option<String>,
) -> Option<(Value, Value)> {
    if ferb {
        return None;
    }
    let owners_mute = muted_reason == Some("user");
    let mut change = Map::new();
    for (k, v) in [("audible", audible), ("muted", muted)] {
        if changed.iter().any(|c| c == k) && !(k == "muted" && owners_mute) {
            if let Some(v) = v {
                change.insert(k.into(), json!({"before": !v, "after": v}));
            }
        }
    }
    let mut tab = json!({"browser": browser, "tab_id": tab_id});
    if let Some(t) = task.filter(|t| !t.is_empty()) {
        tab["task"] = json!(t);
    }
    Some((tab, Value::Object(change)))
}
