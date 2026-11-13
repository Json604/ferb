//! The watchers that live on run-loop threads of FERB's own: FSEvents (files, app installs), the listen-only input
//! tap, the Mac's presence notifications, NSWorkspace's app lifecycle notifications and the Accessibility observers.
//! Each thread drains an autorelease pool per turn and per callback; each callback catches its own panics, so
//! nothing unwinds into the system's C frames; nothing here posts, suppresses or replays an event.

use std::collections::{HashMap, HashSet};
use std::ffi::c_void;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use block2::RcBlock;
use core_foundation_sys::array::{
    kCFTypeArrayCallBacks, CFArrayCreate, CFArrayGetCount, CFArrayGetValueAtIndex, CFArrayRef,
};
use core_foundation_sys::base::{CFRelease, CFRetain, CFTypeRef};
use core_foundation_sys::mach_port::{
    CFMachPortCreateRunLoopSource, CFMachPortInvalidate, CFMachPortRef,
};
use core_foundation_sys::runloop::{
    kCFRunLoopCommonModes, kCFRunLoopDefaultMode, CFRunLoopAddSource, CFRunLoopGetCurrent,
    CFRunLoopRemoveSource,
};
use core_foundation_sys::string::CFStringRef;
use ferb_core::interfaces::BoxFuture;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2_app_kit::{NSRunningApplication, NSWorkspace};
use objc2_foundation::{
    NSDistributedNotificationCenter, NSNotification, NSNotificationCenter, NSObjectProtocol,
    NSString,
};

use super::apps::{notification_name, Ax};
use super::ffi::*;
use super::{cfstr, native_loop, pool, string_of, NativeThread, Owned};
use crate::front_window::FERB_APPS;
use crate::manager::{Emit, Stop, Watcher};
use crate::observation::Redact;
use crate::watchers::accessibility::{
    accessibility_observation, element_facts, subscription_attached, subscription_unavailable,
    APP_EVENTS, ELEMENT_EVENTS,
};
use crate::watchers::base::{FsEvents, OnFsEvent};
use crate::watchers::owner_input::{KeyEvent, TapBackend, TapSink, FERB_EVENT_MARK};
use crate::watchers::presence::{MacPresence, PresenceNative, DISTRIBUTED, POLL_S, WORKSPACE};

// ---------------------------------------------------------------- FSEvents

/// base.py:81 `FsStream`: file-level FSEvents on a thread of their own.
pub struct MacFsEvents {
    pub latency_s: f64,
}

impl Default for MacFsEvents {
    fn default() -> Self {
        MacFsEvents { latency_s: 0.3 }
    }
}

unsafe extern "C" fn fs_callback(
    _stream: FSEventStreamRef,
    info: *mut c_void,
    count: usize,
    paths: *mut c_void,
    flags: *const u32,
    _ids: *const u64,
) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        pool(|| {
            // SAFETY: `info` is the OnFsEvent this stream was created with, alive until the stream is released;
            // with kFSEventStreamCreateFlagUseCFTypes `paths` is a CFArray of `count` CFStrings and `flags` holds
            // `count` values.
            unsafe {
                let on_event = &*(info as *const OnFsEvent);
                let paths = paths as CFArrayRef;
                for i in 0..count.min(CFArrayGetCount(paths).max(0) as usize) {
                    let p = string_of(CFArrayGetValueAtIndex(paths, i as isize) as CFStringRef);
                    on_event(&p, *flags.add(i));
                }
            }
        })
    }));
}

impl FsEvents for MacFsEvents {
    fn start(&self, folders: &[String], on_event: OnFsEvent) -> Option<Box<dyn Send + Sync>> {
        let (ready, started) = mpsc::channel::<bool>();
        let (folders, latency) = (folders.to_vec(), self.latency_s);
        let thread = NativeThread::spawn("ferb-fsevents", move |done| {
            let info = Box::into_raw(Box::new(on_event));
            let strings: Vec<Owned> = folders.iter().map(|f| cfstr(f)).collect();
            let ptrs: Vec<*const c_void> = strings.iter().map(|s| s.0).collect();
            // SAFETY: an array of valid CFStrings, retained by the array (kCFTypeArrayCallBacks).
            let array = Owned(unsafe {
                CFArrayCreate(
                    std::ptr::null(),
                    ptrs.as_ptr(),
                    ptrs.len() as isize,
                    &kCFTypeArrayCallBacks,
                )
            } as CFTypeRef);
            let mut ctx = FSEventStreamContext {
                version: 0,
                info: info as *mut c_void,
                retain: std::ptr::null(),
                release: std::ptr::null(),
                copy_description: std::ptr::null(),
            };
            // SAFETY: the callback matches FSEventStreamCallback; the context is copied by the call; `info` lives
            // until after the stream is released below.
            let stream = unsafe {
                FSEventStreamCreate(
                    std::ptr::null(),
                    fs_callback,
                    &mut ctx,
                    array.0 as CFArrayRef,
                    kFSEventStreamEventIdSinceNow,
                    latency,
                    kFSEventStreamCreateFlagUseCFTypes
                        | kFSEventStreamCreateFlagFileEvents
                        | kFSEventStreamCreateFlagNoDefer,
                )
            };
            let mut ok = false;
            if !stream.is_null() {
                // SAFETY: a valid stream scheduled on this thread's own run loop.
                unsafe {
                    FSEventStreamScheduleWithRunLoop(
                        stream,
                        CFRunLoopGetCurrent(),
                        kCFRunLoopDefaultMode,
                    );
                    ok = FSEventStreamStart(stream) != 0;
                }
            }
            let _ = ready.send(ok);
            if ok {
                native_loop(&done, 1.0, || {});
            }
            if !stream.is_null() {
                // SAFETY: the stream is ours; stopped, invalidated and released once, on its own thread.
                unsafe {
                    if ok {
                        FSEventStreamStop(stream);
                    }
                    FSEventStreamInvalidate(stream);
                    FSEventStreamRelease(stream);
                }
            }
            // SAFETY: the stream is gone, so no callback can use `info` any more.
            drop(unsafe { Box::from_raw(info) });
        });
        match started.recv_timeout(Duration::from_secs(5)) {
            Ok(true) => Some(Box::new(thread)),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------- the owner-input tap

struct TapContext {
    sink: Arc<dyn TapSink>,
    port: AtomicPtr<c_void>,
}

struct CgKey(CGEventRef);

impl KeyEvent for CgKey {
    fn flags(&self) -> u64 {
        // SAFETY: the event is valid during the tap's callback, where this key lives.
        unsafe { CGEventGetFlags(self.0) }
    }

    /// text_input.py:93 `key_text`: the whole text, the buffer grown until it fits; a lone surrogate gives "".
    fn text(&self) -> String {
        let mut size = 4usize;
        loop {
            let mut buf = vec![0u16; size];
            let mut n = 0usize;
            // SAFETY: the buffer holds `size` UTF-16 units; the call writes at most that many and says how many.
            unsafe { CGEventKeyboardGetUnicodeString(self.0, size, &mut n, buf.as_mut_ptr()) };
            if n < size {
                return String::from_utf16(&buf[..n]).unwrap_or_default();
            }
            size *= 2;
            if size > 1 << 16 {
                return String::new();
            }
        }
    }
}

unsafe extern "C" fn tap_callback(
    _proxy: CGEventTapProxy,
    kind: u32,
    event: CGEventRef,
    user: *mut c_void,
) -> CGEventRef {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        pool(|| {
            // SAFETY: `user` is the TapContext this tap was created with, alive until the tap is torn down.
            let ctx = unsafe { &*(user as *const TapContext) };
            if kind == kCGEventTapDisabledByTimeout || kind == kCGEventTapDisabledByUserInput {
                let port = ctx.port.load(Ordering::SeqCst);
                if ctx.sink.wanted() && !port.is_null() {
                    // SAFETY: the port is the tap's own, still valid while the context is.
                    unsafe { CGEventTapEnable(port as CFMachPortRef, true) }; // the system turned it off: back on
                }
                return;
            }
            // SAFETY: a valid event during the callback.
            let marked = unsafe { CGEventGetIntegerValueField(event, kCGEventSourceUserData) }
                == FERB_EVENT_MARK;
            if kind == kCGEventKeyDown {
                ctx.sink.event(marked, Some(&CgKey(event)));
            } else {
                ctx.sink.event(marked, None);
            }
        })
    }));
    event // listen-only: the event goes on untouched
}

/// owner_input.py:50: a listen-only `CGEventTap` on its own run-loop thread.
#[derive(Default)]
pub struct MacTap {
    thread: Mutex<Option<NativeThread>>,
}

impl TapBackend for MacTap {
    fn create(&self, sink: Arc<dyn TapSink>) -> bool {
        let kinds = [
            kCGEventMouseMoved,
            kCGEventLeftMouseDown,
            kCGEventLeftMouseUp,
            kCGEventRightMouseDown,
            kCGEventLeftMouseDragged,
            kCGEventKeyDown,
            kCGEventKeyUp,
            kCGEventScrollWheel,
            kCGEventFlagsChanged,
        ];
        let mask = kinds.iter().fold(0u64, |m, k| m | (1u64 << k));
        let (ready, created) = mpsc::channel::<bool>();
        let thread = NativeThread::spawn("ferb-owner-input", move |done| {
            let ctx = Box::into_raw(Box::new(TapContext {
                sink,
                port: AtomicPtr::new(std::ptr::null_mut()),
            }));
            // SAFETY: a listen-only session tap; the callback matches CGEventTapCallBack; `ctx` outlives the tap.
            let port = unsafe {
                CGEventTapCreate(
                    kCGSessionEventTap,
                    kCGHeadInsertEventTap,
                    kCGEventTapOptionListenOnly,
                    mask,
                    tap_callback,
                    ctx as *mut c_void,
                )
            };
            if port.is_null() {
                let _ = ready.send(false);
                // SAFETY: no tap was made, so nothing else holds `ctx`.
                drop(unsafe { Box::from_raw(ctx) });
                return;
            }
            // SAFETY: `ctx` is valid (just made).
            unsafe { (*ctx).port.store(port as *mut c_void, Ordering::SeqCst) };
            // SAFETY: the port is valid; its run-loop source is added to this thread's run loop and released by us.
            let source = unsafe { CFMachPortCreateRunLoopSource(std::ptr::null(), port, 0) };
            unsafe {
                CFRunLoopAddSource(CFRunLoopGetCurrent(), source, kCFRunLoopCommonModes);
                CGEventTapEnable(port, true);
            }
            let _ = ready.send(true);
            native_loop(&done, 1.0, || {});
            // SAFETY: the tap is ours: disabled, its source removed, invalidated and released on its own thread;
            // after that no callback can use `ctx`.
            unsafe {
                CGEventTapEnable(port, false);
                CFRunLoopRemoveSource(CFRunLoopGetCurrent(), source, kCFRunLoopCommonModes);
                CFMachPortInvalidate(port);
                CFRelease(source as CFTypeRef);
                CFRelease(port as CFTypeRef);
                drop(Box::from_raw(ctx));
            }
        });
        let ok = created
            .recv_timeout(Duration::from_secs(5))
            .unwrap_or(false);
        if ok {
            *self
                .thread
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(thread);
        }
        ok
    }

    fn destroy(&self) {
        self.thread
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
    }
}

// ---------------------------------------------------------------- notifications

type Token = Retained<ProtocolObject<dyn NSObjectProtocol>>;

fn workspace_name(name: &str) -> Retained<NSString> {
    NSString::from_str(name) // AppKit's NSWorkspace notification names are their own identifiers
}

/// Observe `name` on `center` with `f` (run on the posting thread, inside a pool, never unwinding).
fn observe(
    center: &NSNotificationCenter,
    name: &str,
    f: impl Fn(&NSNotification) + 'static,
) -> Token {
    let block = RcBlock::new(move |note: NonNull<NSNotification>| {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            // SAFETY: the center passes a valid notification for the duration of the call.
            pool(|| f(unsafe { note.as_ref() }))
        }));
    });
    let n = workspace_name(name);
    // SAFETY: a nil object and queue (delivered on the posting thread); the block lives as long as the observer.
    unsafe { center.addObserverForName_object_queue_usingBlock(Some(&n), None, None, &block) }
}

fn forget(center: &NSNotificationCenter, tokens: Vec<Token>) {
    for t in tokens {
        let obj: &AnyObject = t.as_ref();
        // SAFETY: each token was returned by this center's addObserverForName.
        unsafe { center.removeObserver(obj) };
    }
}

/// presence.py:132 `_watch`: the notifications and the periodic read, on FERB's own run-loop thread.
pub struct MacPresenceNative;

impl PresenceNative for MacPresenceNative {
    fn watch(&self, presence: Arc<MacPresence>) -> Option<Box<dyn Send + Sync>> {
        Some(Box::new(NativeThread::spawn(
            "ferb-presence",
            move |done| {
                let workspace = pool(|| NSWorkspace::sharedWorkspace().notificationCenter());
                let distributed = pool(NSDistributedNotificationCenter::defaultCenter);
                let mut ws_tokens = Vec::new();
                let mut dn_tokens = Vec::new();
                for (name, flag, value) in WORKSPACE {
                    let p = presence.clone();
                    ws_tokens.push(observe(&workspace, name, move |_| p.changed(flag, value)));
                }
                for (name, flag, value) in DISTRIBUTED {
                    let p = presence.clone();
                    dn_tokens.push(observe(&distributed, name, move |_| p.changed(flag, value)));
                }
                let p = presence.clone();
                native_loop(&done, POLL_S, move || {
                    let _ = catch_unwind(AssertUnwindSafe(|| p.read()));
                });
                forget(&workspace, ws_tokens);
                forget(&distributed, dn_tokens);
            },
        )))
    }
}

/// workspace.py: NSWorkspace lifecycle notifications for every application.
pub struct WorkspaceWatcher {
    pub redact: Arc<dyn Redact>,
    pub excluded: Arc<dyn Fn(&str, &str) -> bool + Send + Sync>,
}

fn app_facts(app: &NSRunningApplication) -> (String, String, i64) {
    (
        app.bundleIdentifier()
            .map(|s| s.to_string())
            .unwrap_or_default(),
        app.localizedName()
            .map(|s| s.to_string())
            .unwrap_or_default(),
        i64::from(app.processIdentifier()),
    )
}

impl Watcher for WorkspaceWatcher {
    fn source(&self) -> &str {
        "workspace"
    }

    fn run(self: Arc<Self>, emit: Emit, stop: Stop) -> BoxFuture<'static, Result<(), String>> {
        Box::pin(async move {
            let me = self.clone();
            let thread = NativeThread::spawn("ferb-workspace", move |done| {
                let center = pool(|| NSWorkspace::sharedWorkspace().notificationCenter());
                let mut tokens = Vec::new();
                for name in crate::watchers::workspace::EVENTS {
                    let (me, emit, done) = (me.clone(), emit.clone(), done.clone());
                    tokens.push(observe(&center, name, move |note| {
                        let key: &AnyObject =
                            (unsafe { objc2_app_kit::NSWorkspaceApplicationKey }).as_ref();
                        let app = note
                            .userInfo()
                            .and_then(|info| info.objectForKey(key))
                            .and_then(|o| o.downcast::<NSRunningApplication>().ok())
                            .map(|a| app_facts(&a));
                        let o = crate::watchers::workspace::observation(
                            &note.name().to_string(),
                            app.as_ref().map(|(b, n, p)| (b.as_str(), n.as_str(), *p)),
                            me.excluded.as_ref(),
                            me.redact.as_ref(),
                        );
                        if let Some(o) = o {
                            if !done.load(Ordering::SeqCst) {
                                emit(o);
                            }
                        }
                    }));
                }
                native_loop(&done, 0.5, || {});
                forget(&center, tokens);
            });
            stop.wait().await;
            drop(thread);
            Ok(())
        })
    }
}

// ---------------------------------------------------------------- Accessibility observers

/// accessibility.py:44: system-wide AX event subscriptions, one observer per app on one run-loop thread.
pub struct AccessibilityWatcher {
    pub redact: Arc<dyn Redact>,
    pub excluded: Arc<dyn Fn(&str, &str) -> bool + Send + Sync>,
    /// `act/mac/browsers.py` `is_browser` (a browser's events keep no title: SENSE-5)
    pub is_browser: Arc<dyn Fn(&str) -> bool + Send + Sync>,
}

struct AppObserver {
    bundle: String,
    name: String,
    pid: i64,
    root: Ax,
    registered: Mutex<Vec<Ax>>,
    watcher: Arc<AccessibilityWatcher>,
    emit: Emit,
    done: Arc<AtomicBool>,
}

impl AppObserver {
    fn watch_element(&self, observer: AXObserverRef, element: Option<Ax>) {
        let Some(el) = element else {
            return;
        };
        let mut reg = self
            .registered
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if reg.iter().any(|old| old.same(&el)) {
            return;
        }
        for event in ELEMENT_EVENTS {
            let n = cfstr(event);
            // SAFETY: a valid observer and element; the refcon is this AppObserver, alive while the observer is.
            unsafe {
                AXObserverAddNotification(
                    observer,
                    el.raw(),
                    n.0 as CFStringRef,
                    self as *const AppObserver as *mut c_void,
                )
            };
        }
        reg.push(el);
    }
}

unsafe extern "C" fn ax_callback(
    observer: AXObserverRef,
    element: AXUIElementRef,
    notification: CFStringRef,
    refcon: *mut c_void,
) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        pool(|| {
            // SAFETY: the refcon is the AppObserver registered with this observer, alive until its source is removed.
            let app = unsafe { &*(refcon as *const AppObserver) };
            if app.done.load(Ordering::SeqCst) || (app.watcher.excluded)(&app.bundle, &app.name) {
                return;
            }
            // SAFETY: the notification name is a valid CFString for the call; the element is retained for our use.
            let kind = unsafe { notification_name(notification) };
            let Some(el) = Ax::owned(unsafe { CFRetain(element as CFTypeRef) } as AXUIElementRef)
            else {
                return;
            };
            if let Some(facts) = element_facts(&kind, &el) {
                let browser = (app.watcher.is_browser)(&app.bundle);
                (app.emit)(accessibility_observation(
                    &app.bundle,
                    &app.name,
                    app.pid,
                    &kind,
                    &facts,
                    app.watcher.redact.as_ref(),
                    browser,
                ));
            }
            if kind == "AXUIElementDestroyed" {
                app.registered
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .retain(|old| !old.same(&el));
                return;
            }
            app.watch_element(observer, app.root.element("AXFocusedUIElement"));
            for w in app.root.elements("AXWindows") {
                app.watch_element(observer, Some(w));
            }
        })
    }));
}

struct Attached {
    observer: Owned,
    /// kept alive while the observer is: it is the observer's refcon
    #[allow(dead_code)]
    app: Box<AppObserver>,
}

impl Watcher for AccessibilityWatcher {
    fn source(&self) -> &str {
        "accessibility"
    }

    fn run(self: Arc<Self>, emit: Emit, stop: Stop) -> BoxFuture<'static, Result<(), String>> {
        Box::pin(async move {
            let me = self.clone();
            let thread = NativeThread::spawn("ferb-ax-observers", move |done| {
                let mut observers: HashMap<i64, Attached> = HashMap::new();
                let mut failed: HashSet<i64> = HashSet::new();
                // SAFETY: this thread's own run loop.
                let runloop = unsafe { CFRunLoopGetCurrent() };
                let remove = |a: &Attached| {
                    // SAFETY: the observer's source was added to this run loop.
                    unsafe {
                        CFRunLoopRemoveSource(
                            runloop,
                            AXObserverGetRunLoopSource(a.observer.0 as AXObserverRef),
                            kCFRunLoopDefaultMode,
                        )
                    };
                };
                native_loop(&done, 1.0, || {
                    // the app list read each second drains with its turn
                    let apps = NSWorkspace::sharedWorkspace().runningApplications();
                    let running: Vec<(String, String, i64)> =
                        apps.iter().map(|a| app_facts(&a)).collect();
                    let pids: HashSet<i64> = running.iter().map(|a| a.2).collect();
                    let gone: Vec<i64> = observers
                        .keys()
                        .filter(|p| !pids.contains(p))
                        .copied()
                        .collect();
                    for pid in gone {
                        if let Some(a) = observers.remove(&pid) {
                            remove(&a);
                        }
                    }
                    failed.retain(|p| pids.contains(p));
                    for (bundle, name, pid) in running {
                        if observers.contains_key(&pid) || failed.contains(&pid) {
                            continue;
                        }
                        if bundle.is_empty() || (me.excluded)(&bundle, &name) {
                            continue;
                        }
                        if FERB_APPS.contains(&ferb_sanitize::parity::casefold(&bundle).as_str()) {
                            continue; // rule 17 (owner, 2026-10-25): no observer in FERB's own apps
                        }
                        let Some(root) = Ax::app(pid) else {
                            continue;
                        };
                        let mut observer: AXObserverRef = std::ptr::null();
                        // SAFETY: the callback matches AXObserverCallback; `observer` receives an owned observer.
                        let code = unsafe {
                            AXObserverCreate(pid as libc::pid_t, ax_callback, &mut observer)
                        };
                        if code != 0 || observer.is_null() {
                            failed.insert(pid);
                            emit(subscription_unavailable(
                                &bundle,
                                pid,
                                i64::from(code),
                                me.redact.as_ref(),
                            ));
                            continue;
                        }
                        let app = Box::new(AppObserver {
                            bundle: bundle.clone(),
                            name,
                            pid,
                            root,
                            registered: Mutex::new(Vec::new()),
                            watcher: me.clone(),
                            emit: emit.clone(),
                            done: done.clone(),
                        });
                        let mut unsupported = Vec::new();
                        for event in APP_EVENTS {
                            let n = cfstr(event);
                            // SAFETY: valid observer and element; the refcon is the boxed AppObserver kept below.
                            let status = unsafe {
                                AXObserverAddNotification(
                                    observer,
                                    app.root.raw(),
                                    n.0 as CFStringRef,
                                    &*app as *const AppObserver as *mut c_void,
                                )
                            };
                            if status != 0 {
                                unsupported.push((event.to_string(), i64::from(status)));
                            }
                        }
                        app.watch_element(observer, app.root.element("AXFocusedUIElement"));
                        for w in app.root.elements("AXWindows") {
                            app.watch_element(observer, Some(w));
                        }
                        // SAFETY: the observer's source joins this thread's run loop.
                        unsafe {
                            CFRunLoopAddSource(
                                runloop,
                                AXObserverGetRunLoopSource(observer),
                                kCFRunLoopDefaultMode,
                            )
                        };
                        observers.insert(
                            pid,
                            Attached {
                                observer: Owned(observer as CFTypeRef),
                                app,
                            },
                        );
                        emit(subscription_attached(
                            &bundle,
                            pid,
                            &unsupported,
                            me.redact.as_ref(),
                        ));
                    }
                });
                for a in observers.values() {
                    remove(a);
                }
            });
            stop.wait().await;
            drop(thread);
            Ok(())
        })
    }
}
