//! `_macos_events` (main.py:642): macOS's event system gets a turn on the main thread a few times a second. Without
//! it the running-app list never refreshes in the daemon (an app launched after the daemon started stays "not
//! running" for FERB: the owner's WhatsApp report, 2026-10-01, and this port's real-machine check, where TextEdit
//! opened by mac.open was then "not running"). Python's asyncio loop runs on the main thread and turns the run loop
//! there; here the daemon's async work runs on a thread of its own and the main thread turns the run loop.

/// MACOS_TICK_S (areas/shared.py:18).
pub const MACOS_TICK_S: f64 = 0.2;

/// Runs `work` on a thread named `name` while the main thread gives macOS's run loop a zero-wait turn every
/// MACOS_TICK_S (each turn in its own autorelease pool, DMN-26); returns what `work` returns.
pub fn run<T: Send + 'static>(name: &str, work: impl FnOnce() -> T + Send + 'static) -> T {
    let job = std::thread::Builder::new()
        .name(name.to_string())
        .spawn(work)
        .expect("the daemon's thread");
    while !job.is_finished() {
        #[cfg(target_os = "macos")]
        objc2::rc::autoreleasepool(|_| {
            // SAFETY: the exported mode name; this is the main thread's run loop (we are on the main thread).
            let mode = unsafe { objc2_core_foundation::kCFRunLoopDefaultMode };
            objc2_core_foundation::CFRunLoop::run_in_mode(mode, 0.0, true);
        });
        std::thread::sleep(std::time::Duration::from_secs_f64(MACOS_TICK_S));
    }
    match job.join() {
        Ok(v) => v,
        Err(e) => std::panic::resume_unwind(e),
    }
}
