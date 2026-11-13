//! The native layer (macOS only): Apple framework calls behind the logic's traits. Every blocking call here drains
//! its own autorelease pool (ferb/device/autorelease.py: each unit of work on a long-lived thread). Unverified beyond
//! compiling and the read-only `#[ignore]`d tests: nothing here has run against the owner's apps yet.

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
