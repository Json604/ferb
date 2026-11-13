//! NSWorkspace lifecycle notifications for every application (`ferb/sense/watchers/workspace.py`). No application is
//! launched by this source. The observer thread is [`crate::mac`]'s; this is what each notification records.

use serde_json::Value;

use crate::observation::{dict, Observation, Redact};

/// workspace.py:38: the notifications observed.
pub const EVENTS: [&str; 8] = [
    "NSWorkspaceDidLaunchApplicationNotification",
    "NSWorkspaceDidTerminateApplicationNotification",
    "NSWorkspaceDidActivateApplicationNotification",
    "NSWorkspaceDidDeactivateApplicationNotification",
    "NSWorkspaceWillSleepNotification",
    "NSWorkspaceDidWakeNotification",
    "NSWorkspaceDidMountNotification",
    "NSWorkspaceDidUnmountNotification",
];

/// workspace.py:26 `observed`: (notification name, its app as (bundle id, name, pid)) to an observation; None for
/// an app on the exclusion list.
pub fn observation(
    name: &str,
    app: Option<(&str, &str, i64)>,
    excluded: &dyn Fn(&str, &str) -> bool,
    redact: &dyn Redact,
) -> Option<Observation> {
    let mut data = dict([("event", Value::from(name))]);
    if let Some((bundle, app_name, pid)) = app {
        if excluded(bundle, app_name) {
            return None;
        }
        data.insert("bundle_id".into(), Value::from(bundle));
        data.insert("app".into(), Value::from(app_name));
        data.insert("pid".into(), Value::from(pid));
    }
    Some(Observation::by("workspace", redact.dict(&data), "unknown"))
}
