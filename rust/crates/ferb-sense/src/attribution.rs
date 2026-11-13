//! Who caused an observation (`ferb/sense/trail.py:46` `Attribution`). FERB's own actions report here as they happen
//! (the Mac driver's app switches and actions in an app, file tools, shell processes); tab ownership and the identity
//! profile come in the observation's hints. It never claims a correlated action caused an event beyond these facts.

use std::collections::HashMap;
use std::sync::Mutex;

use serde_json::Value;

use crate::observation::{truthy, Observation};
use crate::paths;

/// trail.py:34: a focus change this soon after FERB switched apps is FERB's.
pub const FERB_ACTIVATION_S: f64 = 3.0;
/// trail.py:35: a file event this soon after a FERB file tool touched the path is FERB's.
pub const FERB_FILE_S: f64 = 10.0;

/// "owner", "ferb" or "unknown" with the task, if any.
pub type Decision = (String, Option<String>);

type Cwds = Box<dyn Fn() -> Vec<(String, String)> + Send + Sync>;

#[derive(Default)]
struct State {
    apps: HashMap<String, (Option<String>, f64)>,
    paths: HashMap<String, (Option<String>, f64)>,
    acting: HashMap<String, Vec<Option<String>>>,
}

/// trail.py:46.
pub struct Attribution {
    now: Box<dyn Fn() -> f64 + Send + Sync>,
    state: Mutex<State>,
    /// (cwd, task) of FERB's running commands.
    pub shell_cwds: Mutex<Cwds>,
}

/// While alive, a FERB action runs in the app (trail.py:57 `acting`); dropping it ends that (no timer).
pub struct Acting<'a> {
    attribution: &'a Attribution,
    bundle_id: String,
    task_id: Option<String>,
}

impl Drop for Acting<'_> {
    fn drop(&mut self) {
        self.attribution.end_acting(&self.bundle_id, &self.task_id);
    }
}

/// [`Acting`] that holds its attribution itself: what a caller keeps beyond a borrow (the Mac driver's action).
pub struct ActingOwned {
    attribution: std::sync::Arc<Attribution>,
    bundle_id: String,
    task_id: Option<String>,
}

impl Drop for ActingOwned {
    fn drop(&mut self) {
        self.attribution.end_acting(&self.bundle_id, &self.task_id);
    }
}

fn hint_str(v: Option<&Value>) -> Option<String> {
    match v {
        Some(Value::Null) | None => None,
        Some(Value::String(s)) => Some(s.clone()),
        Some(other) => Some(ferb_py::value::py_str(other)),
    }
}

impl Attribution {
    /// `now`: monotonic seconds (Python's `time.monotonic`).
    pub fn new(now: impl Fn() -> f64 + Send + Sync + 'static) -> Attribution {
        Attribution {
            now: Box::new(now),
            state: Mutex::new(State::default()),
            shell_cwds: Mutex::new(Box::new(Vec::new)),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// trail.py:57: P4, while a FERB action runs in this app, events in it are FERB's.
    /// [`Attribution::acting`] as an owned guard.
    pub fn acting_owned(
        self: &std::sync::Arc<Self>,
        bundle_id: &str,
        task_id: Option<&str>,
    ) -> ActingOwned {
        self.begin_acting(bundle_id, task_id);
        ActingOwned {
            attribution: self.clone(),
            bundle_id: bundle_id.to_string(),
            task_id: task_id.map(str::to_string),
        }
    }

    fn end_acting(&self, bundle_id: &str, task_id: &Option<String>) {
        let mut s = self.lock();
        if let Some(tasks) = s.acting.get_mut(bundle_id) {
            if let Some(i) = tasks.iter().position(|t| t == task_id) {
                tasks.remove(i);
            }
            if tasks.is_empty() {
                s.acting.remove(bundle_id);
            }
        }
    }

    fn begin_acting(&self, bundle_id: &str, task_id: Option<&str>) {
        self.lock()
            .acting
            .entry(bundle_id.to_string())
            .or_default()
            .push(task_id.map(str::to_string));
    }

    pub fn acting(&self, bundle_id: &str, task_id: Option<&str>) -> Acting<'_> {
        self.begin_acting(bundle_id, task_id);
        Acting {
            attribution: self,
            bundle_id: bundle_id.to_string(),
            task_id: task_id.map(str::to_string),
        }
    }

    /// The tasks with an action running in each app, in order (for inspection).
    pub fn acting_now(&self) -> HashMap<String, Vec<Option<String>>> {
        self.lock().acting.clone()
    }

    /// trail.py:69.
    pub fn ferb_activated(&self, bundle_id: &str, task_id: Option<&str>) {
        let until = (self.now)() + FERB_ACTIVATION_S;
        self.lock()
            .apps
            .insert(bundle_id.to_string(), (task_id.map(str::to_string), until));
    }

    /// trail.py:72: the path as given and as resolved (FSEvents reports real paths, /private/var/...).
    pub fn ferb_touched(&self, path: &str, task_id: Option<&str>) {
        let now = (self.now)();
        let p = paths::expanduser(path);
        let mut forms = vec![p.clone()];
        if let Ok(real) = paths::resolve(&p) {
            if real != p {
                forms.push(real);
            }
        }
        let mut s = self.lock();
        s.paths.retain(|_, v| v.1 >= now);
        for form in forms {
            s.paths
                .insert(form, (task_id.map(str::to_string), now + FERB_FILE_S));
        }
    }

    /// The paths FERB touched lately, with their task and until when (for inspection).
    pub fn touched(&self) -> Vec<(String, Option<String>, f64)> {
        let mut out: Vec<_> = self
            .lock()
            .paths
            .iter()
            .map(|(k, v)| (k.clone(), v.0.clone(), v.1))
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    /// trail.py:79.
    pub fn decide(&self, o: &Observation) -> Decision {
        let h = &o.hints;
        let now = (self.now)();
        if let Some(Value::String(a)) = h.get("actor") {
            if a == "owner" || a == "ferb" {
                return (a.clone(), hint_str(h.get("task_id")));
            }
        }
        if truthy(h.get("ferb_identity")) {
            let task = if truthy(h.get("task_id")) {
                h.get("task_id")
            } else {
                h.get("tab_task")
            };
            return ("ferb".into(), hint_str(task));
        }
        if truthy(h.get("tab_task")) {
            return ("ferb".into(), hint_str(h.get("tab_task")));
        }
        let bundle = o
            .data
            .get("bundle_id")
            .filter(|v| ferb_py::value::truthy(v))
            .map(ferb_py::value::py_str);
        {
            let s = self.lock();
            if let Some(b) = &bundle {
                if o.source != "notification" {
                    if let Some(acting) = s.acting.get(b).filter(|a| !a.is_empty()) {
                        return ("ferb".into(), acting.last().cloned().flatten());
                    }
                }
            }
            if matches!(h.get("actor"), Some(Value::String(a)) if a == "unknown") {
                return ("unknown".into(), hint_str(h.get("task_id")));
            }
            if let Some(b) = &bundle {
                if let Some(hit) = s.apps.get(b) {
                    if hit.1 >= now {
                        return ("ferb".into(), hit.0.clone());
                    }
                }
            }
        }
        if let Some(path) = o.data.get("path").filter(|v| ferb_py::value::truthy(v)) {
            let path = ferb_py::value::py_str(path);
            if let Some(hit) = self.lock().paths.get(&path) {
                if hit.1 >= now {
                    return ("ferb".into(), hit.0.clone());
                }
            }
            let cwds = (self
                .shell_cwds
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner))();
            for (cwd, _task) in cwds {
                if path == cwd || path.starts_with(&format!("{}/", cwd.trim_end_matches('/'))) {
                    return ("unknown".into(), None);
                }
            }
        }
        let actor = match h.get("default_actor") {
            Some(v) => ferb_py::value::py_str(v),
            None => "owner".into(),
        };
        (actor, None)
    }
}
