//! The native layer (macOS only): Apple framework calls behind the logic's traits. Every blocking call here drains
//! its own autorelease pool (ferb/device/autorelease.py: each unit of work on a long-lived thread). Run on a real Mac
//! through `ferbd` (rust/JOURNAL.md 45 and 47): the app list, launch and open in the background, AX reads, a window
//! capture, Vision, process-directed typing into a background TextEdit and the safe quit. Not yet run for real: the
//! pointer routes (CGEvent and SkyLight), foreground input, recordings, audio capture.

pub mod apps;
pub mod audio;
pub mod ax;
pub mod capture;
pub mod cf;
pub mod events;
pub mod paint;
pub mod pasteboard;
pub mod permissions;
pub mod vision;
pub mod windows;

/// autorelease.py `drained`: `f` with one autorelease pool around it.
pub fn drained<T>(f: impl FnOnce() -> T) -> T {
    objc2::rc::autoreleasepool(|_| f())
}
