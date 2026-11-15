//! New app bundles' security facts and later TCC and launch-job changes (`ferb/sense/watchers/software.py`). Never
//! executes an installed app. The grants and launch jobs of apps already seen are read again only when TCC or a
//! launch-job folder changed (stat calls each tick); a newly installed app is read at once.

use std::collections::HashMap;
use std::sync::Arc;

use ferb_core::interfaces::BoxFuture;
use serde_json::{Map, Value};

use crate::manager::{Emit, Stop, Watcher};
use crate::observation::{Observation, Redact};

/// The reads this watcher makes (the Mac's, or fakes).
pub trait SoftwareReads: Send + Sync {
    /// `change_stamp()`: what changes when grants or launch jobs can.
    fn stamp(&self) -> Vec<String>;
    /// `app_security_facts(path, bundle)`.
    fn security_facts(&self, bundle: &str, path: &str) -> Value;
    /// `privacy_facts(bundle, path)` (both TCC databases).
    fn privacy(&self, bundle: &str, path: &str) -> Value;
    /// `associated_jobs(path)`.
    fn jobs(&self, path: &str) -> Value;
    /// `login_items(path, bundle)`.
    fn login_items(&self, bundle: &str, path: &str) -> Value;
}

/// software.py:12.
pub struct SoftwareWatcher {
    /// (bundle id, path) of the apps installed (perception.py `installed`)
    installed: Arc<dyn Fn() -> Vec<(String, String)> + Send + Sync>,
    redact: Arc<dyn Redact>,
    reads: Arc<dyn SoftwareReads>,
    pub interval_s: f64,
}

/// `json.dumps(current, sort_keys=True)`: the fingerprint compared between ticks.
fn fingerprint(v: &Value) -> String {
    ferb_py::json::dumps_with(
        v,
        ferb_py::json::Style {
            ascii: true,
            sort_keys: true,
            indent: None,
        },
    )
}

impl SoftwareWatcher {
    pub fn new(
        installed: Arc<dyn Fn() -> Vec<(String, String)> + Send + Sync>,
        redact: Arc<dyn Redact>,
        reads: Arc<dyn SoftwareReads>,
    ) -> SoftwareWatcher {
        SoftwareWatcher {
            installed,
            redact,
            reads,
            interval_s: 1.0,
        }
    }

    /// One tick: what changed since `previous` (path to fingerprint) under the stamp `stamp`.
    pub fn tick(
        &self,
        emit: &dyn Fn(Observation),
        previous: &mut HashMap<String, String>,
        stamp: &mut Option<Vec<String>>,
        stop: &Stop,
    ) {
        let now = self.reads.stamp();
        let changed = stamp.as_ref() != Some(&now);
        *stamp = Some(now);
        for (bundle, path) in (self.installed)() {
            if stop.is_set() {
                break;
            }
            if previous.contains_key(&path) && !changed {
                continue;
            }
            let current = if !previous.contains_key(&path) {
                let facts = self
                    .redact
                    .value(&self.reads.security_facts(&bundle, &path));
                let current: Map<String, Value> = ["privacy", "launch_jobs", "login_items"]
                    .iter()
                    .map(|k| (k.to_string(), facts.get(*k).cloned().unwrap_or(Value::Null)))
                    .collect();
                if let Value::Object(f) = facts {
                    emit(Observation::by("software", f, "unknown"));
                }
                current
            } else {
                let mut c = Map::new();
                c.insert("privacy".into(), self.reads.privacy(&bundle, &path));
                c.insert("launch_jobs".into(), self.reads.jobs(&path));
                c.insert("login_items".into(), self.reads.login_items(&bundle, &path));
                match self.redact.value(&Value::Object(c)) {
                    Value::Object(m) => m,
                    _ => Map::new(),
                }
            };
            let print = fingerprint(&Value::Object(current.clone()));
            if previous.get(&path).is_some_and(|p| *p != print) {
                let mut d = Map::new();
                d.insert("bundle_id".into(), Value::from(bundle.clone()));
                d.insert("path".into(), Value::from(path.clone()));
                d.insert("change".into(), Value::from("permissions_or_persistence"));
                d.extend(current);
                emit(Observation::by("software", self.redact.dict(&d), "unknown"));
            }
            previous.insert(path, print);
        }
    }
}

impl Watcher for SoftwareWatcher {
    fn source(&self) -> &str {
        "software"
    }

    fn run(self: Arc<Self>, emit: Emit, stop: Stop) -> BoxFuture<'static, Result<(), String>> {
        Box::pin(async move {
            let mut previous = HashMap::new();
            let mut stamp = None;
            while !stop.is_set() {
                let (me, e, s) = (self.clone(), emit.clone(), stop.clone());
                let (p, st) = tokio::task::spawn_blocking(move || {
                    let (mut p, mut st) = (previous, stamp);
                    super::base::drained(|| me.tick(&|o| e(o), &mut p, &mut st, &s));
                    (p, st)
                })
                .await
                .map_err(|e| format!("RuntimeError: {e}"))?;
                (previous, stamp) = (p, st);
                stop.wait_for(self.interval_s).await;
            }
            Ok(())
        })
    }
}
