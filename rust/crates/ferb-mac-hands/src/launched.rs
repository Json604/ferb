//! The apps FERB launches as task resources (ferb/act/mac/launched.py, L39, HAND-200): quit at the task tree's
//! release unless the owner used it, it may hold unsaved changes, or FERB can't tell. Never forced: quitting asks the
//! app to quit, as ⌘Q does. Matched by pid and bundle id, so a relaunch by the owner is never quit.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::{json, Map, Value};

use crate::apps::RunningApp;
use crate::error::{HandError, Result};
use crate::input::off;

pub const KEPT_OWNER: &str = "you used it during the task";
pub const KEPT_UNSAVED: &str = "it has unsaved changes";
pub const KEPT_UNKNOWN: &str = "FERB couldn't tell whether it has unsaved changes";
pub const KEPT_REFUSED: &str = "it didn't accept FERB's request to quit";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launched {
    pub bundle_id: String,
    pub name: String,
    pub pid: i32,
    pub at: String,
}

/// The native side: running apps, whether a window is edited (AXEdited; None can't tell), quit, hide.
pub trait AppControl: Send + Sync {
    fn running(&self) -> Vec<RunningApp>;
    fn unsaved(&self, pid: i32) -> Option<bool>;
    fn quit(&self, pid: i32, bundle_id: &str) -> bool;
    fn hide(&self, pid: i32, bundle_id: &str) -> bool;
}

pub type OwnerUsed = Arc<dyn Fn(&str, &Launched) -> bool + Send + Sync>;

fn tree(task_id: &str) -> &str {
    task_id.split('.').next().unwrap_or(task_id)
}

pub struct LaunchedApps {
    control: Arc<dyn AppControl>,
    owner_used: OwnerUsed,
    now: Arc<dyn Fn() -> String + Send + Sync>,
    by_tree: Mutex<HashMap<String, Vec<Launched>>>,
}

impl LaunchedApps {
    pub fn new(
        control: Arc<dyn AppControl>,
        owner_used: OwnerUsed,
        now: Arc<dyn Fn() -> String + Send + Sync>,
    ) -> Self {
        LaunchedApps {
            control,
            owner_used,
            now,
            by_tree: Mutex::new(HashMap::new()),
        }
    }

    pub fn launched(&self, task_id: &str, bundle_id: &str, name: &str, pid: i32) -> Launched {
        let app = Launched {
            bundle_id: bundle_id.into(),
            name: name.into(),
            pid,
            at: (self.now)(),
        };
        self.add(task_id, app.clone());
        app
    }

    pub fn add(&self, task_id: &str, app: Launched) {
        self.by_tree
            .lock()
            .expect("lock")
            .entry(tree(task_id).into())
            .or_default()
            .push(app);
    }

    pub fn of(&self, task_id: &str) -> Vec<Launched> {
        self.by_tree
            .lock()
            .expect("lock")
            .get(tree(task_id))
            .cloned()
            .unwrap_or_default()
    }

    fn own(&self, task_id: &str, app: &RunningApp, verb: &str) -> Result<Launched> {
        self.of(task_id).into_iter().find(|x| x.pid == app.pid && x.bundle_id == app.bundle_id).ok_or_else(|| {
            HandError::Refused(format!(
                "{} isn't an app FERB opened for this task, so FERB doesn't {verb} it: the owner's apps stay as they \
are (HAND-200)",
                app.name
            ))
        })
    }

    /// ⌘Q the safe way: only an app this tree launched, with no unsaved changes.
    pub async fn quit_now(&self, task_id: &str, app: &RunningApp) -> Result<String> {
        let mine = self.own(task_id, app, "quit")?;
        let c = self.control.clone();
        let pid = mine.pid;
        let state = off(move || c.unsaved(pid)).await;
        if state != Some(false) {
            let why = if state == Some(true) {
                KEPT_UNSAVED
            } else {
                KEPT_UNKNOWN
            };
            return Err(HandError::Refused(format!(
                "FERB didn't quit {}: {why}",
                mine.name
            )));
        }
        let c = self.control.clone();
        let bid = mine.bundle_id.clone();
        if !off(move || c.quit(pid, &bid)).await {
            return Err(HandError::Refused(format!(
                "{} didn't accept FERB's request to quit",
                mine.name
            )));
        }
        if let Some(list) = self.by_tree.lock().expect("lock").get_mut(tree(task_id)) {
            if let Some(i) = list.iter().position(|x| *x == mine) {
                list.remove(i);
            }
        }
        Ok(format!(
            "asked {} to quit (FERB opened it for this task; it shows no unsaved changes)",
            mine.name
        ))
    }

    pub async fn hide_now(&self, task_id: &str, app: &RunningApp) -> Result<String> {
        let mine = self.own(task_id, app, "hide")?;
        let c = self.control.clone();
        let (pid, bid) = (mine.pid, mine.bundle_id.clone());
        if !off(move || c.hide(pid, &bid)).await {
            return Err(HandError::Refused(format!(
                "{} didn't hide when FERB asked",
                mine.name
            )));
        }
        Ok(format!("hid {} (FERB opened it for this task)", mine.name))
    }

    /// launched.py:153: at the tree's own task's release (`<tree>.worker`), each app the tree launched, quit or left
    /// running with why.
    pub async fn release(&self, task_id: &str) -> Vec<Value> {
        let (t, rest) = match task_id.split_once('.') {
            Some((t, r)) => (t, r),
            None => (task_id, ""),
        };
        if rest != "worker" {
            return vec![];
        }
        let apps = self
            .by_tree
            .lock()
            .expect("lock")
            .remove(t)
            .unwrap_or_default();
        let mut out = vec![];
        for app in apps {
            let mut record = Map::new();
            record.insert("resource".into(), json!("app"));
            record.insert("app".into(), json!(app.name));
            record.insert("bundle_id".into(), json!(app.bundle_id));
            let with = |extra: Value| {
                let mut r = record.clone();
                r.extend(extra.as_object().expect("an object").clone());
                Value::Object(r)
            };
            let c = self.control.clone();
            let now: Vec<RunningApp> = off(move || c.running())
                .await
                .into_iter()
                .filter(|a| a.pid == app.pid && a.bundle_id == app.bundle_id)
                .collect();
            if now.is_empty() {
                out.push(with(
                    json!({"released": true, "closed": false, "note": "it had already quit"}),
                ));
                continue;
            }
            if now[0].active || (self.owner_used)(task_id, &app) {
                out.push(with(
                    json!({"released": true, "closed": false, "left_running": KEPT_OWNER}),
                ));
                continue;
            }
            let c = self.control.clone();
            let pid = app.pid;
            let state = off(move || c.unsaved(pid)).await;
            if state != Some(false) {
                let why = if state == Some(true) {
                    KEPT_UNSAVED
                } else {
                    KEPT_UNKNOWN
                };
                out.push(with(
                    json!({"released": true, "closed": false, "left_running": why}),
                ));
                continue;
            }
            let c = self.control.clone();
            let bid = app.bundle_id.clone();
            let asked = off(move || c.quit(pid, &bid)).await;
            out.push(with(if asked {
                json!({"released": true, "closed": true})
            } else {
                json!({"released": false, "closed": false, "left_running": KEPT_REFUSED,
                       "error": format!("{} didn't accept FERB's request to quit", app.name)})
            }));
        }
        out
    }
}

/// launched.py:188: the owner's line for each app FERB launched and left running.
pub fn left_running(resources: &[Value]) -> Vec<String> {
    resources
        .iter()
        .filter(|r| {
            r.get("resource") == Some(&json!("app"))
                && r.get("left_running").is_some_and(ferb_py::value::truthy)
        })
        .map(|r| {
            format!(
                "{} is still open: FERB opened it for this task and left it running because {}.",
                ferb_py::value::py_str(&r["app"]),
                ferb_py::value::py_str(&r["left_running"])
            )
        })
        .collect()
}
