//! File events in the folders the owner lists in `[sense] watch_folders` (`ferb/sense/watchers/files.py`): path and
//! kind of change, never contents. A folder must exist, be a directory under the owner's home and not be excluded.
//! `.DS_Store` and `.git` internals are noise; repeats for one path within a second coalesce.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use ferb_core::interfaces::BoxFuture;
use serde_json::Value;

use super::base::{FsEvents, CREATED, IS_DIR, MODIFIED, REMOVED, RENAMED};
use crate::exclusions::Exclusions;
use crate::manager::{Emit, Stop, Watcher};
use crate::observation::{dict, Observation};
use crate::paths;

/// files.py:18.
pub const COALESCE_S: f64 = 1.0;

/// files.py:21 `valid_folders`: (folders to watch, problems with the others). A folder behind a link loop is
/// reported as missing (Python raised RuntimeError there: see `paths`).
pub fn valid_folders(
    folders: &[String],
    exclusions: &Exclusions,
    home: &str,
) -> (Vec<String>, Vec<String>) {
    let home = paths::resolve(home).unwrap_or_else(|_| paths::normal(home));
    let (mut ok, mut problems) = (Vec::new(), Vec::new());
    for f in folders {
        let p = match paths::resolve_strict(&paths::expanduser(f)) {
            Ok(p) => p,
            Err(_) => {
                problems.push(format!("{f}: doesn't exist"));
                continue;
            }
        };
        if !std::fs::metadata(&p).is_ok_and(|m| m.is_dir()) {
            problems.push(format!("{f}: not a folder"));
        } else if !paths::within(&p, &home) {
            problems.push(format!("{f}: not inside your home folder"));
        } else if exclusions.path(Some(&p)) {
            problems.push(format!("{f}: excluded ([privacy] exclude_paths)"));
        } else {
            ok.push(p);
        }
    }
    (ok, problems)
}

/// files.py:43 `change_of`.
pub fn change_of(path: &str, flags: u32) -> Option<&'static str> {
    if flags & REMOVED != 0 && !paths::lexists(path) {
        return Some("removed");
    }
    if flags & RENAMED != 0 {
        return Some(if paths::lexists(path) {
            "renamed_in"
        } else {
            "renamed_out"
        });
    }
    if flags & CREATED != 0 {
        return Some("created");
    }
    if flags & MODIFIED != 0 {
        return Some("modified");
    }
    None
}

/// files.py:55 `noise`.
pub fn noise(path: &str) -> bool {
    let parts = paths::parts(path);
    let last = parts.last().map(String::as_str).unwrap_or("");
    last == ".DS_Store"
        || parts[..parts.len().saturating_sub(1)]
            .iter()
            .any(|p| p == ".git")
        || last == ".git"
}

struct Seen {
    recent: HashMap<(String, String), f64>,
    known: HashSet<String>,
}

/// files.py:60.
pub struct FilesWatcher {
    folders: Arc<dyn Fn() -> Vec<String> + Send + Sync>,
    stream: Arc<dyn FsEvents>,
    now: Arc<dyn Fn() -> f64 + Send + Sync>,
    seen: Mutex<Seen>,
}

impl FilesWatcher {
    /// `now`: monotonic seconds.
    pub fn new(
        folders: Arc<dyn Fn() -> Vec<String> + Send + Sync>,
        stream: Arc<dyn FsEvents>,
        now: Arc<dyn Fn() -> f64 + Send + Sync>,
    ) -> FilesWatcher {
        FilesWatcher {
            folders,
            stream,
            now,
            seen: Mutex::new(Seen {
                recent: HashMap::new(),
                known: HashSet::new(),
            }),
        }
    }

    /// files.py:69 `observe`.
    pub fn observe(&self, path: &str, flags: u32) -> Option<Observation> {
        if noise(path) {
            return None;
        }
        let mut change = change_of(path, flags)?;
        let mut s = self
            .seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if change == "created" && s.known.contains(path) {
            change = "modified"; // FSEvents keeps a file's flags cumulative: it existed already
        }
        if change == "removed" || change == "renamed_out" {
            s.known.remove(path);
        } else {
            s.known.insert(path.to_string());
        }
        let key = (path.to_string(), change.to_string());
        let t = (self.now)();
        if t - s.recent.get(&key).copied().unwrap_or(-COALESCE_S * 2.0) < COALESCE_S {
            return None;
        }
        s.recent.insert(key, t);
        if s.recent.len() > 5000 {
            s.recent.retain(|_, v| t - *v < COALESCE_S);
        }
        if s.known.len() > 20000 {
            s.known.clear();
        }
        Some(Observation::new(
            "file",
            dict([
                ("path", Value::from(path)),
                ("change", Value::from(change)),
                ("is_dir", Value::Bool(flags & IS_DIR != 0)),
            ]),
        ))
    }
}

impl Watcher for FilesWatcher {
    fn source(&self) -> &str {
        "file"
    }

    fn run(self: Arc<Self>, emit: Emit, stop: Stop) -> BoxFuture<'static, Result<(), String>> {
        Box::pin(async move {
            let folders = (self.folders)();
            if folders.is_empty() {
                stop.wait().await;
                return Ok(());
            }
            let me = self.clone();
            let on_event = Arc::new(move |path: &str, flags: u32| {
                if let Some(o) = me.observe(path, flags) {
                    emit(o);
                }
            });
            let Some(stream) = self.stream.start(&folders, on_event) else {
                return Err("RuntimeError: file event stream failed to start".into());
            };
            stop.wait().await;
            drop(stream);
            Ok(())
        })
    }
}
