//! Local facts about how the owner uses this Mac (ferb/device/persona.py, MEM-13): installed apps, their use
//! (Spotlight's counts through `mdls`, else the owner's own focus events), the browsers and their profiles. Code only
//! gathers; a model interprets them. `persona.json` in FERB's home keeps what FERB saw.

use std::collections::BTreeSet;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use crate::plistlib;
use crate::profile::AppInfo;

pub const PERSONA_KEY: &str = "setup-persona";
pub const PERSONA_FILE: &str = "persona.json";
pub const TOP_APPS: usize = 25;
pub const OLD_KEYS: [&str; 2] = ["persona_pending", "persona_reviewed"];

#[derive(Debug, thiserror::Error)]
pub enum PersonaError {
    #[error("{0}")]
    Config(#[from] ferb_core::config::ConfigError),
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("persona.json isn't JSON: {0}")]
    Json(String),
}

/// persona.py:151: what FERB saw on this Mac and, from older builds, the proposal and whether it was reviewed.
pub struct PersonaState {
    pub home: PathBuf,
    pub path: PathBuf,
}

impl PersonaState {
    pub fn new(home: &Path) -> Self {
        PersonaState {
            home: home.to_path_buf(),
            path: home.join(PERSONA_FILE),
        }
    }

    fn read(&self) -> Result<Map<String, Value>, PersonaError> {
        let config = ferb_core::config::Config::new(&self.home);
        if self.path.exists() {
            check_owner_only(&self.path)?;
            let v: Value = serde_json::from_str(&std::fs::read_to_string(&self.path)?)
                .map_err(|e| PersonaError::Json(e.to_string()))?;
            return Ok(match v {
                Value::Object(m) => m,
                _ => Map::new(),
            });
        }
        let mut state = Map::new();
        for (key, old) in [
            ("pending", "persona_pending"),
            ("reviewed", "persona_reviewed"),
        ] {
            if let Some(v) = config.get("setup", old)? {
                state.insert(key.into(), v);
            }
        }
        if !state.is_empty() {
            self.write(&state)?;
            config.remove("setup", &OLD_KEYS)?;
        }
        Ok(state)
    }

    fn write(&self, state: &Map<String, Value>) -> Result<(), PersonaError> {
        let text = ferb_py::json::dumps_utf8(&Value::Object(state.clone()));
        ferb_core::config::write_private(&self.home, &self.path, &text)?;
        Ok(())
    }

    fn change(&self, key: &str, value: Value) -> Result<(), PersonaError> {
        let mut state = self.read()?;
        state.insert(key.into(), value);
        self.write(&state)
    }

    fn object(&self, key: &str) -> Result<Map<String, Value>, PersonaError> {
        Ok(match self.read()?.get(key) {
            Some(Value::Object(m)) => m.clone(),
            _ => Map::new(),
        })
    }

    pub fn pending(&self) -> Result<Map<String, Value>, PersonaError> {
        self.object("pending")
    }
    pub fn set_pending(&self, proposal: Map<String, Value>) -> Result<(), PersonaError> {
        self.change("pending", Value::Object(proposal))
    }
    pub fn reviewed(&self) -> Result<bool, PersonaError> {
        Ok(self.read()?.get("reviewed") == Some(&Value::Bool(true)))
    }
    pub fn set_reviewed(&self, reviewed: bool) -> Result<(), PersonaError> {
        self.change("reviewed", Value::Bool(reviewed))
    }
    pub fn seen(&self) -> Result<Map<String, Value>, PersonaError> {
        self.object("seen")
    }
    pub fn set_seen(&self, seen: Map<String, Value>) -> Result<(), PersonaError> {
        self.change("seen", Value::Object(seen))
    }
}

/// config.py `_check_owner_only`.
fn check_owner_only(path: &Path) -> Result<(), PersonaError> {
    let st = std::fs::metadata(path)?;
    // SAFETY: getuid has no preconditions and cannot fail.
    if st.uid() != unsafe { libc::getuid() } {
        return Err(ferb_core::config::ConfigError::Insecure(format!(
            "{} is not owned by this user",
            path.display()
        ))
        .into());
    }
    let mode = st.mode() & 0o7777;
    if mode & 0o077 != 0 {
        return Err(ferb_core::config::ConfigError::Insecure(format!(
            "{} has mode 0o{mode:o}; others must have no access",
            path.display()
        ))
        .into());
    }
    Ok(())
}

/// persona.py:215: `mdls` asked for each app's path, use count and last use.
pub fn usage_command(apps: &[AppInfo]) -> Vec<String> {
    let mut v: Vec<String> = [
        "/usr/bin/mdls",
        "-plist",
        "-",
        "-name",
        "kMDItemPath",
        "-name",
        "kMDItemUseCount",
        "-name",
        "kMDItemLastUsedDate",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    v.extend(apps.iter().map(|a| a.path.clone()));
    v
}

#[derive(PartialEq, Eq)]
enum Identity {
    Inode(u64, u64),
    Path(PathBuf),
}

/// persona.py:220: (device, inode), or the absolute path when it can't be read.
fn path_identity(path: &str) -> Identity {
    match std::fs::metadata(path) {
        Ok(st) => Identity::Inode(st.dev(), st.ino()),
        Err(_) => Identity::Path(absolute(Path::new(path))),
    }
}

/// `os.path.abspath`: made absolute and normalized, links not followed.
fn absolute(path: &Path) -> PathBuf {
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(path)
    };
    let mut out = PathBuf::from("/");
    for c in joined.components() {
        match c {
            std::path::Component::Normal(n) => out.push(n),
            std::path::Component::ParentDir => {
                out.pop();
            }
            _ => {}
        }
    }
    out
}

/// Python's `str()` of a plist value as `mdls` writes it (a date as `YYYY-MM-DD HH:MM:SS`).
fn mdls_str(v: &plist::Value) -> String {
    match v {
        plist::Value::Date(d) => {
            let t: std::time::SystemTime = (*d).into();
            let dt: chrono::DateTime<chrono::Utc> = t.into();
            let micros = dt.timestamp_subsec_micros();
            if micros == 0 {
                dt.format("%Y-%m-%d %H:%M:%S").to_string()
            } else {
                format!("{}.{micros:06}", dt.format("%Y-%m-%d %H:%M:%S"))
            }
        }
        other => plistlib::py_str(other),
    }
}

/// persona.py:228: one row per app, most used first.
pub fn usage_rows(apps: &[AppInfo], raw: &[u8]) -> Vec<Map<String, Value>> {
    let records: Vec<plist::Dictionary> = match plistlib::loads(raw) {
        Some(plist::Value::Array(a)) => a.into_iter().filter_map(|v| v.into_dictionary()).collect(),
        Some(plist::Value::Dictionary(d)) => vec![d],
        _ => Vec::new(),
    };
    let by_path: Vec<(Identity, &plist::Dictionary)> = records
        .iter()
        .filter_map(|r| {
            r.get("kMDItemPath")
                .and_then(|p| p.as_string())
                .map(|p| (path_identity(p), r))
        })
        .collect();
    let mut rows: Vec<Map<String, Value>> = apps
        .iter()
        .map(|app| {
            let me = path_identity(&app.path);
            // a later record for the same path wins, as in a dict
            let record = by_path
                .iter()
                .rev()
                .find(|(id, _)| *id == me)
                .map(|(_, r)| *r);
            let count = record.and_then(|r| r.get("kMDItemUseCount"));
            let uses = match count {
                Some(plist::Value::Integer(i)) => {
                    i.as_signed().filter(|n| *n >= 0).map(|n| json!(n))
                }
                _ => None,
            };
            let last = record
                .and_then(|r| r.get("kMDItemLastUsedDate"))
                .filter(|v| plistlib::truthy(v))
                .map(mdls_str)
                .unwrap_or_else(|| "unknown".into());
            let mut row = Map::new();
            row.insert("bundle_id".into(), json!(app.bundle_id));
            row.insert("app".into(), json!(app.name));
            row.insert("uses".into(), uses.unwrap_or(Value::Null));
            row.insert("last_used".into(), json!(last));
            row
        })
        .collect();
    rows.sort_by_key(|r| -r["uses"].as_i64().unwrap_or(0));
    rows
}

/// Runs `mdls`: its stdout bytes, Err when it couldn't run.
pub type Mdls<'a> = dyn FnMut(&[String]) -> Result<Vec<u8>, ()> + 'a;
/// A persona row (app, use count, last use, source).
pub type Row = Map<String, Value>;

/// persona.py:204: `mdls` run through `run` (its stdout bytes; Err: it couldn't run).
pub fn app_usage(apps: &[AppInfo], run: &mut Mdls<'_>) -> Vec<Map<String, Value>> {
    if apps.is_empty() {
        return Vec::new();
    }
    let raw = run(&usage_command(apps)).unwrap_or_default();
    usage_rows(apps, &raw)
}

/// persona.py:275: focus events fill in where Spotlight has no count.
pub fn with_activity(
    rows: &[Map<String, Value>],
    activity: &[Map<String, Value>],
) -> Vec<Map<String, Value>> {
    rows.iter()
        .map(|row| {
            let mut item = row.clone();
            let focused = activity
                .iter()
                .rev()
                .find(|a| a.get("bundle_id") == row.get("bundle_id"));
            match (item.get("uses").is_none_or(Value::is_null), focused) {
                (true, Some(f)) => {
                    item.insert("focus_events".into(), f["focus_events"].clone());
                    item.insert("source".into(), json!("owner_activity"));
                    item.insert("last_used".into(), f["last_used"].clone());
                }
                _ => {
                    item.insert("source".into(), json!("spotlight"));
                }
            }
            item
        })
        .collect()
}

fn use_of(r: &Map<String, Value>) -> f64 {
    for k in ["uses", "focus_events"] {
        if let Some(v) = r.get(k) {
            if ferb_py::value::truthy(v) {
                return v.as_f64().unwrap_or(0.0);
            }
        }
    }
    0.0
}

/// persona.py:290.
pub fn mac_facts(
    rows: &[Map<String, Value>],
    scanned_at: &str,
    browsers: &[String],
    profiles: &[Value],
    profiles_read: bool,
) -> Value {
    let used: Vec<&Map<String, Value>> = rows.iter().filter(|r| use_of(r) > 0.0).collect();
    let mut ranked = used.clone();
    ranked.sort_by(|a, b| {
        use_of(b)
            .partial_cmp(&use_of(a))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let apps: Vec<Value> = ranked
        .iter()
        .take(TOP_APPS)
        .map(|r| {
            let mut m = Map::new();
            for k in ["app", "bundle_id", "uses", "focus_events", "last_used"] {
                if let Some(v) = r.get(k).filter(|v| !v.is_null()) {
                    m.insert(k.into(), v.clone());
                }
            }
            Value::Object(m)
        })
        .collect();
    json!({"scanned_at": scanned_at, "installed_apps": rows.len(), "apps_with_use": used.len(), "apps": apps,
           "browsers": browsers, "profiles": profiles, "profiles_read": profiles_read})
}

/// persona.py:304: what makes a refresh worth interpreting again.
pub fn facts_fingerprint(facts: &Value) -> String {
    let empty = Vec::new();
    let apps = facts
        .get("apps")
        .and_then(Value::as_array)
        .unwrap_or(&empty);
    let mut app_keys: Vec<String> = apps
        .iter()
        .take(10)
        .map(|a| {
            let id = a
                .get("bundle_id")
                .filter(|v| ferb_py::value::truthy(v))
                .or_else(|| a.get("app"));
            id.map(ferb_py::value::py_str)
                .unwrap_or_else(|| "None".into())
        })
        .collect();
    app_keys.sort();
    let mut browsers: Vec<Value> = facts
        .get("browsers")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    browsers.sort_by_key(ferb_py::value::py_str);
    let mut profiles: Vec<String> = facts
        .get("profiles")
        .and_then(Value::as_array)
        .unwrap_or(&empty)
        .iter()
        .map(|p| {
            let f = |k: &str| {
                p.get(k)
                    .map(ferb_py::value::py_str)
                    .unwrap_or_else(|| "None".into())
            };
            format!(
                "{}|{}|{}|{}",
                f("browser"),
                f("name"),
                f("email"),
                f("last_used")
            )
        })
        .collect();
    profiles.sort();
    let key = json!({"apps": app_keys, "browsers": browsers, "profiles": profiles});
    let text = ferb_py::json::dumps_with(
        &key,
        ferb_py::json::Style {
            ascii: true,
            sort_keys: true,
            indent: None,
        },
    );
    hex::encode(Sha256::digest(text.as_bytes()))[..16].to_string()
}

/// persona.py:333 (the daemon's `setup.persona`), without its cancellation: scan, save, usage, and the browsers'
/// profiles only once FERB may read other apps' data (DMN-507).
pub struct Discover<'a> {
    pub scan: &'a mut dyn FnMut() -> crate::profile::DeviceProfile,
    pub usage: &'a mut dyn FnMut(&[AppInfo]) -> Vec<Row>,
    pub allowed: &'a mut dyn FnMut() -> bool,
    pub profiles: &'a mut dyn FnMut() -> Result<Vec<Value>, String>,
}

pub fn discover_persona(
    home: &Path,
    activity: &[Map<String, Value>],
    excluded: &BTreeSet<String>,
    seams: Discover<'_>,
) -> std::io::Result<Value> {
    let profile = (seams.scan)();
    crate::profile::ProfileStore::new(home).save(&profile, "mac profile scan")?;
    let apps: Vec<AppInfo> = profile
        .apps
        .iter()
        .filter(|a| !excluded.contains(&a.bundle_id))
        .cloned()
        .collect();
    let rows = (seams.usage)(&apps);
    let readable = (seams.allowed)();
    let found = if readable {
        (seams.profiles)().unwrap_or_default()
    } else {
        Vec::new()
    };
    let browsers: Vec<String> = profile
        .browsers
        .iter()
        .filter(|b| !excluded.contains(&b.bundle_id))
        .map(|b| b.name.clone())
        .collect();
    Ok(mac_facts(
        &with_activity(&rows, activity),
        &profile.scanned_at,
        &browsers,
        &found,
        readable,
    ))
}
