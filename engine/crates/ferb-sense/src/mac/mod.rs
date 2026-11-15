//! The macOS sources of perception: each watcher's native side, through `objc2` (NSWorkspace, NSRunningApplication,
//! NSPasteboard, the notification centers) and raw C calls where Python called the C APIs (Accessibility,
//! CoreGraphics' event tap, session and window list, FSEvents, Carbon's secure-input check, libproc).
//!
//! Every thread FERB runs here services its own run loop and drains one autorelease pool per turn and per callback
//! (`device/autorelease.py`). Nothing here activates an app, moves the cursor, posts input, writes the pasteboard or
//! asks macOS for a permission (rule 10; SENSE-404, SENSE-405): a missing permission reads as "unavailable".

pub mod apps;
pub mod ffi;
pub mod streams;
pub mod system;

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use core_foundation_sys::base::{CFGetTypeID, CFRelease, CFTypeRef};
use core_foundation_sys::number::{
    kCFNumberFloat64Type, kCFNumberSInt64Type, CFBooleanGetTypeID, CFBooleanGetValue,
    CFNumberGetTypeID, CFNumberGetValue, CFNumberIsFloatType,
};
use core_foundation_sys::runloop::{
    kCFRunLoopDefaultMode, CFRunLoopGetCurrent, CFRunLoopRef, CFRunLoopRunInMode, CFRunLoopStop,
};
use core_foundation_sys::string::{
    kCFStringEncodingUTF8, CFStringCreateWithBytes, CFStringGetBytes, CFStringGetLength,
    CFStringGetTypeID, CFStringRef,
};
use serde_json::Value;

/// One autorelease pool around `f` (device/autorelease.py `pool`).
pub fn pool<T>(f: impl FnOnce() -> T) -> T {
    objc2::rc::autoreleasepool(|_| f())
}

/// A CoreFoundation object FERB owns (created or copied): released when dropped.
pub struct Owned(pub CFTypeRef);

// SAFETY: CoreFoundation objects are reference counted with atomic counts; FERB only reads the immutable ones it
// moves between threads (strings, numbers, dictionaries the system handed over).
unsafe impl Send for Owned {}
// SAFETY: as above; no method mutates the object.
unsafe impl Sync for Owned {}

impl Owned {
    /// None for a null pointer.
    pub fn new(p: CFTypeRef) -> Option<Owned> {
        (!p.is_null()).then_some(Owned(p))
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY: the pointer is non-null and owned by this value (Create/Copy rule), released once.
        unsafe { CFRelease(self.0) }
    }
}

/// A CFString made from Rust text (owned).
pub fn cfstr(s: &str) -> Owned {
    // SAFETY: the bytes are valid UTF-8 of the given length; a null allocator means the default one.
    let p = unsafe {
        CFStringCreateWithBytes(
            std::ptr::null(),
            s.as_ptr(),
            s.len() as isize,
            kCFStringEncodingUTF8,
            0,
        )
    };
    Owned(p as CFTypeRef)
}

/// A CFString's text.
///
/// # Safety
/// `s` must be a valid CFString.
pub unsafe fn string_of(s: CFStringRef) -> String {
    // SAFETY: the caller guarantees `s` is a valid CFString; the first call measures, the second fills the buffer.
    unsafe {
        let len = CFStringGetLength(s);
        let mut needed: isize = 0;
        CFStringGetBytes(
            s,
            core_foundation_sys::base::CFRange {
                location: 0,
                length: len,
            },
            kCFStringEncodingUTF8,
            b'?',
            0,
            std::ptr::null_mut(),
            0,
            &mut needed,
        );
        let mut buf = vec![0u8; needed.max(0) as usize];
        CFStringGetBytes(
            s,
            core_foundation_sys::base::CFRange {
                location: 0,
                length: len,
            },
            kCFStringEncodingUTF8,
            b'?',
            0,
            buf.as_mut_ptr(),
            needed,
            &mut needed,
        );
        String::from_utf8_lossy(&buf).into_owned()
    }
}

/// A CF scalar (string, number or boolean) as JSON; None for anything else.
///
/// # Safety
/// `v` must be null or a valid CF object.
pub unsafe fn scalar(v: CFTypeRef) -> Option<Value> {
    if v.is_null() {
        return None;
    }
    // SAFETY: the caller guarantees `v` is a valid CF object; each accessor is called only for its own type.
    unsafe {
        let t = CFGetTypeID(v);
        if t == CFStringGetTypeID() {
            return Some(Value::from(string_of(v as CFStringRef)));
        }
        if t == CFBooleanGetTypeID() {
            return Some(Value::Bool(CFBooleanGetValue(v as _)));
        }
        if t == CFNumberGetTypeID() {
            if CFNumberIsFloatType(v as _) != 0 {
                let mut f: f64 = 0.0;
                CFNumberGetValue(
                    v as _,
                    kCFNumberFloat64Type,
                    &mut f as *mut f64 as *mut c_void,
                );
                return Some(Value::from(f));
            }
            let mut i: i64 = 0;
            CFNumberGetValue(
                v as _,
                kCFNumberSInt64Type,
                &mut i as *mut i64 as *mut c_void,
            );
            return Some(Value::from(i));
        }
    }
    None
}

/// device/autorelease.py `native_loop`: until `done`, each turn runs `step`, then pumps this thread's run loop for
/// `interval_s` (where its sources' callbacks run), both inside one autorelease pool; a run loop with nothing to
/// wait on returns at once, so the rest of the turn is waited out.
pub fn native_loop(done: &AtomicBool, interval_s: f64, mut step: impl FnMut()) {
    // this thread's run loop, so its handle can wake it when told to end (a turn waits up to `interval_s`)
    let key = done as *const AtomicBool as usize;
    // SAFETY: CFRunLoopGetCurrent returns this thread's run loop (owned by the thread); it is retained while listed.
    unsafe {
        let rl = CFRunLoopGetCurrent();
        core_foundation_sys::base::CFRetain(rl as CFTypeRef);
        running_loops().insert(key, rl as usize);
    }
    native_turns(done, interval_s, &mut step);
    if let Some(rl) = running_loops().remove(&key) {
        // SAFETY: the reference retained above, released once.
        unsafe { CFRelease(rl as CFTypeRef) };
    }
}

/// The run loops of the threads inside [`native_loop`] now, by their `done` flag's address.
fn running_loops() -> std::sync::MutexGuard<'static, std::collections::HashMap<usize, usize>> {
    static LOOPS: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<usize, usize>>> =
        std::sync::OnceLock::new();
    LOOPS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Wakes the run loop of the [`native_loop`] whose flag is `done`, so a turn waiting on it ends now.
fn wake(done: &AtomicBool) {
    let key = done as *const AtomicBool as usize;
    let loops = running_loops();
    if let Some(rl) = loops.get(&key) {
        // SAFETY: a run loop retained while listed (the lock is held, so it can't be released meanwhile);
        // CFRunLoopStop may be called from any thread.
        unsafe { CFRunLoopStop(*rl as CFRunLoopRef) };
    }
}

fn native_turns(done: &AtomicBool, interval_s: f64, step: &mut impl FnMut()) {
    while !done.load(Ordering::SeqCst) {
        let started = Instant::now();
        let stop = pool(|| {
            step();
            if done.load(Ordering::SeqCst) {
                return true;
            }
            // SAFETY: runs this thread's own run loop in the default mode for a bounded time.
            unsafe { CFRunLoopRunInMode(kCFRunLoopDefaultMode, interval_s, 0) };
            false
        });
        if stop {
            break;
        }
        let left = Duration::from_secs_f64(interval_s).saturating_sub(started.elapsed());
        if !left.is_zero() && !done.load(Ordering::SeqCst) {
            std::thread::sleep(left.min(Duration::from_millis(100)));
        }
    }
}

/// A thread of FERB's own that ends when its handle is dropped (it is told and its run loop woken, then joined if it
/// ends within 200 ms; otherwise it ends on its own once its step returns).
pub struct NativeThread {
    pub done: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl NativeThread {
    pub fn spawn(name: &str, body: impl FnOnce(Arc<AtomicBool>) + Send + 'static) -> NativeThread {
        let done = Arc::new(AtomicBool::new(false));
        let d = done.clone();
        let handle = std::thread::Builder::new()
            .name(name.to_string())
            .spawn(move || body(d))
            .ok();
        NativeThread { done, handle }
    }
}

impl Drop for NativeThread {
    fn drop(&mut self) {
        self.done.store(true, Ordering::SeqCst);
        // its run loop's turn ends now instead of at the turn's end (a stop never waits out a slice)
        wake(&self.done);
        if let Some(h) = self.handle.take() {
            // Python's threads are never joined (daemon threads that end on their flag); here a thread that ends
            // within a moment is joined, and one still inside a long step (an Accessibility walk of an app that
            // answers slowly) is left to end on its flag, so a stop never waits on it
            let deadline = Instant::now() + Duration::from_millis(200);
            while !h.is_finished() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            if h.is_finished() {
                let _ = h.join();
            }
        }
    }
}

#[cfg(test)]
mod wake_tests {
    /// A thread turning its run loop in long slices (presence's 5 s) ends at once when its handle is dropped: a
    /// daemon's stop never waits out a slice (journal 57: the stop took 3 s on presence's and the tap's slices).
    #[test]
    fn a_native_thread_ends_at_once_when_dropped() {
        let t = super::NativeThread::spawn("ferb-test-loop", |done| {
            super::native_loop(&done, 5.0, || {});
        });
        std::thread::sleep(std::time::Duration::from_millis(300));
        let began = std::time::Instant::now();
        drop(t);
        let took = began.elapsed();
        assert!(
            took < std::time::Duration::from_millis(500),
            "took {took:?}"
        );
    }
}
