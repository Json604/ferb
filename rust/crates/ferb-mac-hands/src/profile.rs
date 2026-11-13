//! Per-app observations of attempted mechanisms (ferb/act/mac/profile.py): evidence for agents; they never choose a
//! method or authorize a fallback. One JSON file per app under ~/.ferb/device/apps, written whole and renamed.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::{json, Map, Value};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Mechanism {
    pub landed: i64,
    pub failed: i64,
    pub streak: i64,
    pub attempted_at: Option<String>,
    pub last: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct AppProfile {
    pub bundle: String,
    /// In insertion order (a Python dict).
    pub mechanisms: Vec<(String, Mechanism)>,
    pub updated: String,
    pub seeded_from: Option<String>,
}

/// The clock profiles stamp with (ISO seconds, UTC).
pub type Now = Arc<dyn Fn() -> String + Send + Sync>;

pub struct Profiles {
    dir: Option<PathBuf>,
    mem: Mutex<HashMap<String, AppProfile>>,
    pub diagnostics: Mutex<HashMap<String, Vec<Map<String, Value>>>>,
    now: Now,
}

pub fn now_utc() -> String {
    chrono::Utc::now()
        .format("%Y-%m-%dT%H:%M:%S+00:00")
        .to_string()
}

fn mech_json(m: &Mechanism) -> Value {
    json!({"landed": m.landed, "failed": m.failed, "streak": m.streak, "attempted_at": m.attempted_at, "last": m.last})
}

impl Profiles {
    pub fn new(dir: Option<&Path>, now: Now) -> Self {
        Profiles {
            dir: dir.map(Path::to_path_buf),
            mem: Mutex::new(HashMap::new()),
            diagnostics: Mutex::new(HashMap::new()),
            now,
        }
    }

    pub fn memory() -> Self {
        Self::new(None, Arc::new(now_utc))
    }

    fn path(&self, bundle: &str) -> Option<PathBuf> {
        let dir = self.dir.as_ref()?;
        let safe: String = bundle
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        Some(dir.join(format!("{safe}.json")))
    }

    fn diagnose(&self, bundle: &str, op: &str, path: &Path, err: String) {
        let mut d = Map::new();
        d.insert("operation".into(), json!(op));
        d.insert("path".into(), json!(path.to_string_lossy()));
        d.insert("error".into(), json!(err));
        self.diagnostics
            .lock()
            .expect("lock")
            .entry(bundle.into())
            .or_default()
            .push(d);
    }

    fn load(&self, bundle: &str) -> Option<AppProfile> {
        let path = self.path(bundle)?;
        if !path.exists() {
            return None;
        }
        let parsed = std::fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|t| ferb_py::json::loads(&t))
            .and_then(|raw| {
                let mut mechs = vec![];
                if let Some(Value::Object(ms)) = raw.get("mechanisms") {
                    for (k, d) in ms {
                        let d = d.as_object().ok_or("not a mapping")?;
                        let int = |f: &str| -> Result<i64, String> {
                            match d.get(f) {
                                None => Ok(0),
                                Some(v) => v
                                    .as_i64()
                                    .or_else(|| v.as_f64().map(|x| x.trunc() as i64))
                                    .ok_or("bad int".into()),
                            }
                        };
                        let text = |f: &str| d.get(f).and_then(Value::as_str).map(String::from);
                        mechs.push((
                            k.clone(),
                            Mechanism {
                                landed: int("landed")?,
                                failed: int("failed")?,
                                streak: int("streak")?,
                                attempted_at: text("attempted_at"),
                                last: text("last"),
                            },
                        ));
                    }
                }
                let updated = raw
                    .get("updated")
                    .map(|v| {
                        if ferb_py::value::truthy(v) {
                            ferb_py::value::py_str(v)
                        } else {
                            String::new()
                        }
                    })
                    .unwrap_or_default();
                let seeded = raw
                    .get("seeded_from")
                    .and_then(Value::as_str)
                    .map(String::from);
                Ok(AppProfile {
                    bundle: bundle.into(),
                    mechanisms: mechs,
                    updated,
                    seeded_from: seeded,
                })
            });
        match parsed {
            Ok(p) => Some(p),
            Err(e) => {
                self.diagnose(bundle, "load", &path, e);
                None
            }
        }
    }

    fn with<T>(&self, bundle: &str, f: impl FnOnce(&mut AppProfile) -> T) -> T {
        let loaded = {
            let mem = self.mem.lock().expect("lock");
            mem.contains_key(bundle)
        };
        if !loaded {
            let p = self.load(bundle).unwrap_or_else(|| AppProfile {
                bundle: bundle.into(),
                ..Default::default()
            });
            self.mem
                .lock()
                .expect("lock")
                .entry(bundle.into())
                .or_insert(p);
        }
        let mut mem = self.mem.lock().expect("lock");
        f(mem.get_mut(bundle).expect("loaded"))
    }

    /// "landed" / "failed" (the last outcome) or None when never tried.
    pub fn status(&self, bundle: &str, mechanism: &str) -> Option<String> {
        self.with(bundle, |p| {
            p.mechanisms
                .iter()
                .find(|m| m.0 == mechanism)
                .and_then(|m| m.1.last.clone())
        })
    }

    pub fn record(&self, bundle: &str, mechanism: &str, landed: bool) {
        let now = &self.now;
        self.with(bundle, |p| {
            if !p.mechanisms.iter().any(|m| m.0 == mechanism) {
                p.mechanisms.push((mechanism.into(), Mechanism::default()));
            }
            let m = &mut p
                .mechanisms
                .iter_mut()
                .find(|m| m.0 == mechanism)
                .expect("added")
                .1;
            m.attempted_at = Some(now());
            if landed {
                m.landed += 1;
                m.streak = 0;
                m.last = Some("landed".into());
            } else {
                m.failed += 1;
                m.streak += 1;
                m.last = Some("failed".into());
            }
            p.updated = now();
        });
        self.save(bundle);
    }

    pub fn as_dict(&self, bundle: &str) -> Value {
        self.with(bundle, |_| ());
        let diags = self
            .diagnostics
            .lock()
            .expect("lock")
            .get(bundle)
            .cloned()
            .unwrap_or_default();
        self.with(bundle, |p| {
            let mut mechs = Map::new();
            for (k, m) in &p.mechanisms {
                mechs.insert(k.clone(), mech_json(m));
            }
            json!({"bundle": p.bundle, "updated": p.updated, "seeded_from": p.seeded_from, "mechanisms": mechs,
                   "diagnostics": diags})
        })
    }

    fn save(&self, bundle: &str) {
        let Some(path) = self.path(bundle) else {
            return;
        };
        let text = ferb_py::json::dumps_with(
            &self.as_dict(bundle),
            ferb_py::json::Style {
                ascii: true,
                sort_keys: false,
                indent: Some(1),
            },
        );
        let result = (|| -> std::io::Result<()> {
            std::fs::create_dir_all(path.parent().expect("a folder"))?;
            let tmp = path.with_extension("json.tmp");
            std::fs::write(&tmp, text)?;
            std::fs::rename(&tmp, &path)
        })();
        if let Err(e) = result {
            self.diagnose(bundle, "save", &path, e.to_string());
        }
    }
}
