//! Shared watcher plumbing (`ferb/sense/watchers/base.py`): the poll loop, the focused app as the system reports it,
//! and the FSEvents seam. Blocking reads run on worker threads, each inside an autorelease pool of its own.

use std::sync::Arc;

use crate::manager::Stop;

/// base.py:28 `Focused`: the frontmost app (Accessibility's system-wide focused application).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Focused {
    pub pid: i64,
    pub bundle_id: String,
    pub name: String,
}

/// The frontmost app, read fresh (None: none, or not an app with a bundle id).
pub type ReadFocused = Arc<dyn Fn() -> Option<Focused> + Send + Sync>;

/// base.py:56: only a run of this many failed reads stops a polling watcher.
pub const MAX_FAILED_READS: u32 = 20;

/// One poll step: `Err` is why the read failed (`"Type: message"`).
pub type Step = Arc<dyn Fn() -> Result<(), String> + Send + Sync>;

/// `drained(step)` (device/autorelease.py): the step inside one autorelease pool on macOS.
pub fn drained<T>(f: impl FnOnce() -> T) -> T {
    #[cfg(target_os = "macos")]
    {
        crate::mac::pool(f)
    }
    #[cfg(not(target_os = "macos"))]
    {
        f()
    }
}

/// base.py:59 `poll`: run `step` on a worker thread every interval until stopped. One failed read is skipped; only a
/// run of `MAX_FAILED_READS` failures stops the watcher (and doctor shows why).
pub async fn poll(interval_s: f64, stop: &Stop, step: Step) -> Result<(), String> {
    let mut failures = 0;
    while !stop.is_set() {
        let s = step.clone();
        let done = tokio::task::spawn_blocking(move || drained(|| s()))
            .await
            .unwrap_or_else(|e| Err(format!("RuntimeError: {e}")));
        match done {
            Ok(()) => failures = 0,
            Err(why) => {
                failures += 1;
                if failures >= MAX_FAILED_READS {
                    return Err(why);
                }
            }
        }
        stop.wait_for(interval_s).await;
    }
    Ok(())
}

/// base.py:78: FSEvents item flags.
pub const CREATED: u32 = 0x100;
pub const REMOVED: u32 = 0x200;
pub const RENAMED: u32 = 0x800;
pub const MODIFIED: u32 = 0x1000;
pub const IS_DIR: u32 = 0x20000;

/// `on_event(path, flags)`, called on the stream's own thread.
pub type OnFsEvent = Arc<dyn Fn(&str, u32) + Send + Sync>;

/// base.py:81 `FsStream`: file-level FSEvents for some folders. `start` gives a handle that stops the stream when
/// dropped, or None when the stream didn't start.
pub trait FsEvents: Send + Sync {
    fn start(&self, folders: &[String], on_event: OnFsEvent) -> Option<Box<dyn Send + Sync>>;
}
