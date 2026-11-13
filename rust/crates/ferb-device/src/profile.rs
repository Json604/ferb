//! The device profile (ferb/device/profile.py): installed apps and browsers, saved whole to
//! `<home>/device/<id>.json`, each change appended to `device/history.jsonl` (DMN-509). Plain files only.

use std::collections::BTreeMap;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppInfo {
    pub bundle_id: String,
    pub name: String,
    pub version: String,
    pub path: String,
    pub scriptable: bool,
    #[serde(default = "untested")]
    pub ax_support: String,
}

fn untested() -> String {
    "untested".into()
}

impl AppInfo {
    pub fn new(bundle_id: &str, name: &str, version: &str, path: &str, scriptable: bool) -> Self {
        AppInfo {
            bundle_id: bundle_id.into(),
            name: name.into(),
            version: version.into(),
            path: path.into(),
            scriptable,
            ax_support: untested(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowserInfo {
    pub name: String,
    pub bundle_id: String,
    pub path: String,
    #[serde(default)]
    pub drivers: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceProfile {
    pub device_id: String,
    pub os: String,
    pub os_version: String,
    pub scanned_at: String,
    pub apps: Vec<AppInfo>,
    pub browsers: Vec<BrowserInfo>,
}

impl DeviceProfile {
    pub fn app(&self, bundle_id: &str) -> Option<&AppInfo> {
        self.apps.iter().find(|a| a.bundle_id == bundle_id)
    }

    /// `json.dumps(asdict(self), indent=2, sort_keys=True)`.
    pub fn to_json(&self) -> String {
        let value = serde_json::to_value(self).expect("a profile is JSON");
        ferb_py::json::dumps_with(
            &value,
            ferb_py::json::Style {
                ascii: true,
                sort_keys: true,
                indent: Some(2),
            },
        )
    }

    pub fn from_json(text: &str) -> Result<Self, String> {
        serde_json::from_str(text).map_err(|e| e.to_string())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProfileDiff {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub changed: Vec<String>,
}

impl ProfileDiff {
    pub fn empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.changed.is_empty()
    }
}

/// profile.py:67.
pub fn diff(old: &DeviceProfile, new: &DeviceProfile) -> ProfileDiff {
    let o: BTreeMap<&str, &AppInfo> = old.apps.iter().map(|a| (a.bundle_id.as_str(), a)).collect();
    let n: BTreeMap<&str, &AppInfo> = new.apps.iter().map(|a| (a.bundle_id.as_str(), a)).collect();
    let changed = o
        .iter()
        .filter_map(|(b, a)| {
            let m = n.get(b)?;
            ((a.version.as_str(), a.scriptable, a.path.as_str())
                != (m.version.as_str(), m.scriptable, m.path.as_str()))
                .then(|| b.to_string())
        })
        .collect();
    ProfileDiff {
        added: n
            .keys()
            .filter(|b| !o.contains_key(*b))
            .map(|b| b.to_string())
            .collect(),
        removed: o
            .keys()
            .filter(|b| !n.contains_key(*b))
            .map(|b| b.to_string())
            .collect(),
        changed,
    }
}

pub struct ProfileStore {
    pub dir: PathBuf,
}

impl ProfileStore {
    pub fn new(home: &Path) -> Self {
        ProfileStore {
            dir: home.join("device"),
        }
    }

    pub fn path(&self, device_id: &str) -> PathBuf {
        self.dir.join(format!("{device_id}.json"))
    }

    pub fn history_path(&self) -> PathBuf {
        self.dir.join("history.jsonl")
    }

    /// The saved profile; None when there is none. A file that isn't a profile is an error, as in Python.
    pub fn load(&self, device_id: &str) -> std::io::Result<Option<DeviceProfile>> {
        let p = self.path(device_id);
        if !p.exists() {
            return Ok(None);
        }
        let text = std::fs::read_to_string(&p)?;
        DeviceProfile::from_json(&text)
            .map(Some)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }

    /// profile.py:90: writes the profile and appends its change; true if it changed (not only its scan time).
    pub fn save(&self, profile: &DeviceProfile, message: &str) -> std::io::Result<bool> {
        std::fs::create_dir_all(&self.dir)?;
        std::fs::set_permissions(&self.dir, std::fs::Permissions::from_mode(0o700))?;
        let old = self.load(&profile.device_id)?;
        if let Some(old) = &old {
            if same_except_time(old, profile) {
                return Ok(false);
            }
        }
        let target = self.path(&profile.device_id);
        let staged = self.dir.join(format!(".{}.json.new", profile.device_id));
        std::fs::write(&staged, format!("{}\n", profile.to_json()))?;
        std::fs::rename(&staged, &target)?;
        let d = match &old {
            Some(old) => diff(old, profile),
            None => {
                let mut added: Vec<String> =
                    profile.apps.iter().map(|a| a.bundle_id.clone()).collect();
                added.sort();
                ProfileDiff {
                    added,
                    ..ProfileDiff::default()
                }
            }
        };
        let entry = json!({"at": profile.scanned_at, "device_id": profile.device_id, "message": message,
                           "added": d.added, "removed": d.removed, "changed": d.changed});
        let line = ferb_py::json::dumps_with(
            &entry,
            ferb_py::json::Style {
                ascii: true,
                sort_keys: true,
                indent: None,
            },
        );
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.history_path())?;
        f.write_all(format!("{line}\n").as_bytes())?;
        Ok(true)
    }

    /// The messages of the recorded changes, oldest first; a line that doesn't parse is skipped.
    pub fn history(&self) -> Vec<String> {
        let Ok(text) = std::fs::read_to_string(self.history_path()) else {
            return Vec::new();
        };
        crate::py::splitlines(&text)
            .into_iter()
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .filter_map(|v| v.get("message").map(ferb_py::value::py_str))
            .collect()
    }
}

fn same_except_time(a: &DeviceProfile, b: &DeviceProfile) -> bool {
    (&a.device_id, &a.os, &a.os_version, &a.apps, &a.browsers)
        == (&b.device_id, &b.os, &b.os_version, &b.apps, &b.browsers)
}

/// profile.py:127.
pub fn summarize(profile: &DeviceProfile) -> Value {
    json!({"apps": profile.apps.len(), "scriptable": profile.apps.iter().filter(|a| a.scriptable).count(),
           "browsers": profile.browsers.iter().map(|b| b.name.clone()).collect::<Vec<_>>()})
}
