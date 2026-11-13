//! App installs and removals (`ferb/sense/watchers/app_installs.py`): FSEvents on the app folders; each change reads
//! that app only (Info.plist), updates the device profile, then is recorded (actor unknown: an installer can't be
//! attributed). A newly found app is never launched.

use std::sync::Arc;

use ferb_core::interfaces::BoxFuture;
use serde_json::Value;

use super::base::FsEvents;
use super::files::change_of;
use crate::manager::{Emit, Stop, Watcher};
use crate::observation::{dict, Observation};
use crate::paths;

/// device/profile.py:17 `AppInfo` (the parts recorded here).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppInfo {
    pub bundle_id: String,
    pub name: String,
    pub version: String,
    pub path: String,
    pub scriptable: bool,
}

/// app_installs.py:16, under `home`.
pub fn app_folders(home: &str) -> Vec<String> {
    vec![
        "/Applications".into(),
        "/Applications/Utilities".into(),
        format!("{}/Applications", home.trim_end_matches('/')),
    ]
}

/// `Path(p).suffix` and `Path(p).stem` of a path's last part.
fn suffix_stem(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(i) if i > 0 && i + 1 < name.len() => (&name[i..], &name[..i]),
        _ => ("", name),
    }
}

/// (change, app info, path): the device profile and `device.changed`.
pub type OnChange = Arc<dyn Fn(&str, Option<&AppInfo>, &str) + Send + Sync>;

/// app_installs.py:19.
pub struct AppInstallsWatcher {
    on_change: OnChange,
    /// the app folders that exist
    pub folders: Vec<String>,
    stream: Arc<dyn FsEvents>,
    /// `device/scan.py` `read_app`: an app bundle's Info.plist facts.
    read_app: Arc<dyn Fn(&str) -> Option<AppInfo> + Send + Sync>,
}

impl AppInstallsWatcher {
    pub fn new(
        on_change: OnChange,
        folders: &[String],
        stream: Arc<dyn FsEvents>,
        read_app: Arc<dyn Fn(&str) -> Option<AppInfo> + Send + Sync>,
    ) -> AppInstallsWatcher {
        AppInstallsWatcher {
            on_change,
            folders: folders
                .iter()
                .filter(|f| std::fs::metadata(f).is_ok_and(|m| m.is_dir()))
                .map(|f| paths::normal(f))
                .collect(),
            stream,
            read_app,
        }
    }

    /// app_installs.py:28 `observe`: only top-level app bundles, not files inside them.
    pub fn observe(&self, path: &str, flags: u32) -> Option<(Observation, Option<AppInfo>)> {
        let n = paths::normal(path);
        let (parent, name) = match n.rfind('/') {
            Some(0) => ("/".to_string(), &n[1..]),
            Some(i) => (n[..i].to_string(), &n[i + 1..]),
            None => (".".to_string(), n.as_str()),
        };
        let (suffix, stem) = suffix_stem(name);
        if suffix != ".app" || !self.folders.contains(&parent) {
            return None;
        }
        match change_of(path, flags)? {
            "created" | "renamed_in" => {
                let info = (self.read_app)(&n)?;
                let o = Observation::by(
                    "app_install",
                    dict([
                        ("change", Value::from("installed")),
                        ("app", Value::from(info.name.clone())),
                        ("bundle_id", Value::from(info.bundle_id.clone())),
                        ("path", Value::from(path)),
                    ]),
                    "unknown",
                );
                Some((o, Some(info)))
            }
            "removed" | "renamed_out" => Some((
                Observation::by(
                    "app_install",
                    dict([
                        ("change", Value::from("removed")),
                        ("app", Value::from(stem)),
                        ("path", Value::from(path)),
                    ]),
                    "unknown",
                ),
                None,
            )),
            _ => None,
        }
    }

    /// app_installs.py:43: the device profile and `device.changed` first, then the trail (which may drop it).
    pub fn deliver(&self, emit: &Emit, o: Observation, info: Option<AppInfo>) {
        let change = crate::observation::text_or_empty(o.data.get("change"));
        let path = crate::observation::text_or_empty(o.data.get("path"));
        (self.on_change)(&change, info.as_ref(), &path);
        emit(o);
    }
}

impl Watcher for AppInstallsWatcher {
    fn source(&self) -> &str {
        "app_install"
    }

    fn run(self: Arc<Self>, emit: Emit, stop: Stop) -> BoxFuture<'static, Result<(), String>> {
        Box::pin(async move {
            if self.folders.is_empty() {
                stop.wait().await;
                return Ok(());
            }
            let me = self.clone();
            let on_event = Arc::new(move |path: &str, flags: u32| {
                if let Some((o, info)) = me.observe(path, flags) {
                    me.deliver(&emit, o, info);
                }
            });
            let Some(stream) = self.stream.start(&self.folders, on_event) else {
                return Err("RuntimeError: app installation event stream failed to start".into());
            };
            stop.wait().await;
            drop(stream);
            Ok(())
        })
    }
}
