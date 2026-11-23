//! The Mac's side of `ferb_core::power` (kb T-1274): IOKit's power assertions as leases, and the power facts the
//! insomnia area reads (battery, AC, lid, heat, the screen's lock), and the owner's "Lock my Mac".
//!
//! Nothing here needs root or changes a setting: a power assertion is what `caffeinate` takes (macOS lets it go when the
//! lease ends or this process dies), the facts are reads, and `pmset displaysleepnow` asks the display to sleep as the
//! display-idle timer would (no `pmset` write).

use std::collections::HashMap;
use std::ffi::{c_char, c_void, CString};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use ferb_core::power::helper::{self, Request};
use ferb_core::power::{
    HelperState, Hold, KeepAwake, LockMac, PowerEvent, PowerFacts, PowerHandler, PowerReading,
    PowerWatch, RootHelper, Thermal,
};

type CFTypeRef = *const c_void;
type CFStringRef = *const c_void;
type CFIndex = isize;
type IOReturn = i32;
type IOPMAssertionID = u32;
type MachPort = u32;

const UTF8: u32 = 0x0800_0100;
const NUMBER_SINT64: CFIndex = 4;
const IO_RETURN_SUCCESS: IOReturn = 0;

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFStringCreateWithCString(alloc: CFTypeRef, s: *const c_char, encoding: u32) -> CFStringRef;
    fn CFRelease(cf: CFTypeRef);
    fn CFGetTypeID(cf: CFTypeRef) -> usize;
    fn CFStringGetTypeID() -> usize;
    fn CFNumberGetTypeID() -> usize;
    fn CFBooleanGetTypeID() -> usize;
    fn CFBooleanGetValue(b: CFTypeRef) -> u8;
    fn CFNumberGetValue(n: CFTypeRef, kind: CFIndex, out: *mut c_void) -> u8;
    fn CFStringGetCString(s: CFStringRef, buf: *mut c_char, size: CFIndex, encoding: u32) -> u8;
    fn CFDictionaryGetValue(d: CFTypeRef, key: CFTypeRef) -> CFTypeRef;
    fn CFDictionaryGetTypeID() -> usize;
    fn CFArrayGetCount(a: CFTypeRef) -> CFIndex;
    fn CFArrayGetValueAtIndex(a: CFTypeRef, i: CFIndex) -> CFTypeRef;
}

#[link(name = "IOKit", kind = "framework")]
extern "C" {
    fn IOPMAssertionCreateWithDescription(
        kind: CFStringRef,
        name: CFStringRef,
        details: CFStringRef,
        reason: CFStringRef,
        bundle: CFStringRef,
        timeout: f64,
        timeout_action: CFStringRef,
        out: *mut IOPMAssertionID,
    ) -> IOReturn;
    fn IOPMAssertionRelease(id: IOPMAssertionID) -> IOReturn;
    fn IOPSCopyPowerSourcesInfo() -> CFTypeRef;
    fn IOPSCopyPowerSourcesList(blob: CFTypeRef) -> CFTypeRef;
    fn IOPSGetPowerSourceDescription(blob: CFTypeRef, ps: CFTypeRef) -> CFTypeRef;
    fn IOServiceMatching(name: *const c_char) -> CFTypeRef;
    fn IOServiceGetMatchingService(main: MachPort, matching: CFTypeRef) -> u32;
    fn IORegistryEntryCreateCFProperty(
        entry: u32,
        key: CFStringRef,
        alloc: CFTypeRef,
        opts: u32,
    ) -> CFTypeRef;
    fn IOObjectRelease(obj: u32) -> i32;
    /// IOPMLibPrivate.h: the system's capabilities now (kIOPMCapability* bits); 0 when powerd can't be asked.
    fn IOPMConnectionGetSystemCapabilities() -> u32;
    /// IOPMLibPrivate.h: the power settings in force, one dictionary per power source ("AC Power", "Battery Power").
    fn IOPMCopyActivePMPreferences() -> CFTypeRef;
}

/// kIOPMCapabilityCPU and kIOPMCapabilityVideo (IOPMLibPrivate.h).
const CAPABILITY_CPU: u32 = 0x1;
const CAPABILITY_VIDEO: u32 = 0x2;

/// A dark wake from the system's capabilities: the processor runs without graphics. None when nothing was read.
pub fn dark_wake_of(capabilities: u32) -> Option<bool> {
    (capabilities & CAPABILITY_CPU != 0).then_some(capabilities & CAPABILITY_VIDEO == 0)
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGSessionCopyCurrentDictionary() -> CFTypeRef;
}

extern "C" {
    fn notify_register_check(name: *const c_char, token: *mut i32) -> u32;
    fn notify_get_state(token: i32, state: *mut u64) -> u32;
    fn notify_cancel(token: i32) -> u32;
}

/// A CF object this code owns (Create/Copy rule), released on drop.
struct Owned(CFTypeRef);

impl Owned {
    fn new(p: CFTypeRef) -> Option<Owned> {
        (!p.is_null()).then_some(Owned(p))
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY: the pointer is a CF object this code owns (checked non-null at creation), released once.
        unsafe { CFRelease(self.0) }
    }
}

fn cfstr(s: &str) -> Option<Owned> {
    let c = CString::new(s).ok()?;
    // SAFETY: a NUL-terminated UTF-8 string; the result follows the Create rule.
    Owned::new(unsafe { CFStringCreateWithCString(std::ptr::null(), c.as_ptr(), UTF8) })
}

fn is(p: CFTypeRef, type_id: usize) -> bool {
    // SAFETY: `p` is a valid CF object (non-null, checked by the callers).
    !p.is_null() && unsafe { CFGetTypeID(p) } == type_id
}

fn text(p: CFTypeRef) -> Option<String> {
    // SAFETY: a type-id read.
    if !is(p, unsafe { CFStringGetTypeID() }) {
        return None;
    }
    let mut buf = [0 as c_char; 128];
    // SAFETY: the buffer's size is the one passed; `p` is a CFString.
    if unsafe { CFStringGetCString(p, buf.as_mut_ptr(), buf.len() as CFIndex, UTF8) } == 0 {
        return None;
    }
    // SAFETY: CFStringGetCString wrote a NUL-terminated string into the buffer.
    let c = unsafe { std::ffi::CStr::from_ptr(buf.as_ptr()) };
    Some(c.to_string_lossy().into_owned())
}

fn number(p: CFTypeRef) -> Option<i64> {
    // SAFETY: a type-id read.
    if !is(p, unsafe { CFNumberGetTypeID() }) {
        return None;
    }
    let mut out: i64 = 0;
    // SAFETY: `p` is a CFNumber; the out pointer holds an i64 as kCFNumberSInt64Type asks.
    let ok = unsafe { CFNumberGetValue(p, NUMBER_SINT64, (&mut out as *mut i64).cast()) };
    (ok != 0).then_some(out)
}

fn boolean(p: CFTypeRef) -> Option<bool> {
    // SAFETY: a type-id read, then a CFBoolean's value.
    if !is(p, unsafe { CFBooleanGetTypeID() }) {
        return None;
    }
    // SAFETY: `p` is a CFBoolean.
    Some(unsafe { CFBooleanGetValue(p) } != 0)
}

fn get(dict: CFTypeRef, key: &str) -> CFTypeRef {
    let Some(k) = cfstr(key) else {
        return std::ptr::null();
    };
    // SAFETY: `dict` is a CFDictionary (the callers' Get/Copy results); the value is borrowed (Get rule).
    unsafe { CFDictionaryGetValue(dict, k.0) }
}

/// IOKit's power assertions, one per [`Hold`], each renewed by taking a new one and letting the old one go.
#[derive(Default)]
pub struct Assertions {
    held: Mutex<HashMap<Hold, IOPMAssertionID>>,
}

impl Assertions {
    pub fn new() -> Assertions {
        Assertions::default()
    }

    fn kind(hold: Hold) -> &'static str {
        match hold {
            Hold::IdleSleep => "PreventUserIdleSystemSleep",
            Hold::SystemSleep => "PreventSystemSleep",
            Hold::Display => "PreventUserIdleDisplaySleep",
        }
    }
}

impl KeepAwake for Assertions {
    fn hold(&self, hold: Hold, reason: &str, lease_s: u64) -> Result<(), String> {
        let (Some(kind), Some(name), Some(why), Some(action)) = (
            cfstr(Self::kind(hold)),
            cfstr("FERB"),
            cfstr(reason),
            cfstr("TimeoutActionRelease"),
        ) else {
            return Err("couldn't make the assertion's words".into());
        };
        let mut id: IOPMAssertionID = 0;
        // SAFETY: every string is a live CFString; the out pointer is a u32; the lease is in seconds.
        let r = unsafe {
            IOPMAssertionCreateWithDescription(
                kind.0,
                name.0,
                why.0,
                why.0,
                std::ptr::null(),
                lease_s as f64,
                action.0,
                &mut id,
            )
        };
        if r != IO_RETURN_SUCCESS {
            return Err(format!(
                "macOS refused the power assertion (IOReturn {r:#x})"
            ));
        }
        let old = self
            .held
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(hold, id);
        if let Some(old) = old {
            // SAFETY: an assertion id this process took and hasn't released.
            unsafe { IOPMAssertionRelease(old) };
        }
        Ok(())
    }

    fn release(&self, hold: Hold) {
        let old = self
            .held
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&hold);
        if let Some(old) = old {
            // SAFETY: an assertion id this process took and hasn't released.
            unsafe { IOPMAssertionRelease(old) };
        }
    }
}

/// The Mac's power facts, read now.
#[derive(Default)]
pub struct Facts;

impl Facts {
    fn battery() -> (Option<i64>, Option<bool>) {
        // SAFETY: Copy-rule results, released by `Owned`; the description is Get-rule (owned by the blob).
        let Some(blob) = Owned::new(unsafe { IOPSCopyPowerSourcesInfo() }) else {
            return (None, None);
        };
        // SAFETY: `blob` is the power sources blob.
        let Some(list) = Owned::new(unsafe { IOPSCopyPowerSourcesList(blob.0) }) else {
            return (None, None);
        };
        // SAFETY: `list` is a CFArray.
        let n = unsafe { CFArrayGetCount(list.0) };
        for i in 0..n {
            // SAFETY: an index inside the array; the item is borrowed.
            let ps = unsafe { CFArrayGetValueAtIndex(list.0, i) };
            // SAFETY: the blob and one of its sources; the description is borrowed.
            let desc = unsafe { IOPSGetPowerSourceDescription(blob.0, ps) };
            if desc.is_null() || text(get(desc, "Type")).as_deref() != Some("InternalBattery") {
                continue;
            }
            let current = number(get(desc, "Current Capacity"));
            let max = number(get(desc, "Max Capacity"))
                .filter(|m| *m > 0)
                .unwrap_or(100);
            let percent =
                current.map(|c| ((c * 100) as f64 / max as f64).round().clamp(0.0, 100.0) as i64);
            let state = text(get(desc, "Power Source State"));
            return (percent, state.map(|s| s == "Battery Power"));
        }
        (None, Some(false))
    }

    /// Power Nap's setting on power and on battery (`DarkWakeBackgroundTasks` of the settings in force).
    fn power_nap() -> (Option<bool>, Option<bool>) {
        // SAFETY: a Copy-rule dictionary (or NULL), released by `Owned`; its values are borrowed.
        let Some(prefs) = Owned::new(unsafe { IOPMCopyActivePMPreferences() }) else {
            return (None, None);
        };
        let of = |source: &str| {
            let d = get(prefs.0, source);
            // SAFETY: a type-id read before the dictionary is used as one.
            if d.is_null() || unsafe { CFGetTypeID(d) } != unsafe { CFDictionaryGetTypeID() } {
                return None;
            }
            let v = get(d, "DarkWakeBackgroundTasks");
            number(v).map(|n| n != 0).or_else(|| boolean(v))
        };
        (of("AC Power"), of("Battery Power"))
    }

    fn lid_closed() -> Option<bool> {
        let name = CString::new("IOPMrootDomain").ok()?;
        // SAFETY: a matching dictionary (consumed by IOServiceGetMatchingService); the main port 0 is the default.
        let entry = unsafe { IOServiceGetMatchingService(0, IOServiceMatching(name.as_ptr())) };
        if entry == 0 {
            return None;
        }
        let key = cfstr("AppleClamshellState");
        // SAFETY: a registry entry this code holds; the property follows the Create rule.
        let prop = key.and_then(|k| {
            Owned::new(unsafe { IORegistryEntryCreateCFProperty(entry, k.0, std::ptr::null(), 0) })
        });
        // SAFETY: the entry from IOServiceGetMatchingService, released once.
        unsafe { IOObjectRelease(entry) };
        boolean(prop?.0)
    }

    fn thermal() -> Option<Thermal> {
        let name = CString::new("com.apple.system.thermalpressurelevel").ok()?;
        let mut token: i32 = 0;
        // SAFETY: a NUL-terminated name and an out pointer for the token.
        if unsafe { notify_register_check(name.as_ptr(), &mut token) } != 0 {
            return None;
        }
        let mut state: u64 = 0;
        // SAFETY: the token registered just now.
        let ok = unsafe { notify_get_state(token, &mut state) } == 0;
        // SAFETY: the same token, cancelled once.
        unsafe { notify_cancel(token) };
        ok.then(|| Thermal::from_pressure(state))
    }
}

/// `CGSSessionScreenIsLocked` of the login session (as FERB's device facts read it).
pub fn screen_locked() -> Option<bool> {
    // SAFETY: Copy-rule result, released by `Owned`.
    let session = Owned::new(unsafe { CGSessionCopyCurrentDictionary() })?;
    Some(boolean(get(session.0, "CGSSessionScreenIsLocked")).unwrap_or(false))
}

impl PowerFacts for Facts {
    fn read(&self) -> PowerReading {
        let (battery, on_battery) = Facts::battery();
        let (power_nap_ac, power_nap_battery) = Facts::power_nap();
        PowerReading {
            battery,
            on_battery,
            lid_closed: Facts::lid_closed(),
            thermal: Facts::thermal(),
            screen_locked: screen_locked(),
            // SAFETY: a plain query of powerd (no arguments, no ownership).
            dark_wake: dark_wake_of(unsafe { IOPMConnectionGetSystemCapabilities() }),
            power_nap_ac,
            power_nap_battery,
        }
    }
}

type IoConnect = u32;

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    static kCFRunLoopDefaultMode: CFStringRef;
    fn CFRunLoopGetCurrent() -> CFTypeRef;
    fn CFRunLoopAddSource(rl: CFTypeRef, source: CFTypeRef, mode: CFStringRef);
    fn CFRunLoopRun();
}

#[link(name = "IOKit", kind = "framework")]
extern "C" {
    fn IOPSNotificationCreateRunLoopSource(
        callback: extern "C" fn(*mut c_void),
        context: *mut c_void,
    ) -> CFTypeRef;
    fn IORegisterForSystemPower(
        refcon: *mut c_void,
        port: *mut *mut c_void,
        callback: extern "C" fn(*mut c_void, u32, u32, *mut c_void),
        notifier: *mut u32,
    ) -> IoConnect;
    fn IOAllowPowerChange(kernel_port: IoConnect, notification_id: isize) -> IOReturn;
    fn IONotificationPortCreate(main: MachPort) -> *mut c_void;
    fn IONotificationPortGetRunLoopSource(port: *mut c_void) -> CFTypeRef;
    fn IOServiceAddInterestNotification(
        port: *mut c_void,
        service: u32,
        interest: *const c_char,
        callback: extern "C" fn(*mut c_void, u32, u32, *mut c_void),
        refcon: *mut c_void,
        notification: *mut u32,
    ) -> i32;
}

const MSG_CAN_SYSTEM_SLEEP: u32 = 0xe000_0270;
const MSG_SYSTEM_WILL_SLEEP: u32 = 0xe000_0280;
const MSG_SYSTEM_HAS_POWERED_ON: u32 = 0xe000_0300;
/// kIOPMMessageClamshellStateChange (iokit_family_msg(sub_iokit_powermanagement, 0x100)).
const MSG_CLAMSHELL: u32 = 0xe003_4100;
/// kIOMessageSystemCapabilityChange (iokit_common_msg(0x340)): a dark wake becoming a full one, and back.
const MSG_CAPABILITY_CHANGE: u32 = 0xe000_0340;

/// What the watch's callbacks reach (leaked: the watch lasts the process's life).
struct WatchState {
    handler: PowerHandler,
    root_port: std::sync::atomic::AtomicU32,
}

extern "C" fn on_power_source(ctx: *mut c_void) {
    // SAFETY: `ctx` is the leaked WatchState given at registration.
    let st = unsafe { &*(ctx as *const WatchState) };
    (st.handler)(PowerEvent::Changed);
}

extern "C" fn on_system_power(
    ctx: *mut c_void,
    _service: u32,
    message: u32,
    argument: *mut c_void,
) {
    // SAFETY: `ctx` is the leaked WatchState given at registration.
    let st = unsafe { &*(ctx as *const WatchState) };
    let port = st.root_port.load(std::sync::atomic::Ordering::SeqCst);
    match message {
        MSG_CAN_SYSTEM_SLEEP => {
            // SAFETY: the root port IORegisterForSystemPower gave; the argument is the notification's id.
            unsafe { IOAllowPowerChange(port, argument as isize) };
        }
        MSG_SYSTEM_WILL_SLEEP => {
            let wait = (st.handler)(PowerEvent::WillSleep).min(20_000);
            if wait > 0 {
                std::thread::sleep(Duration::from_millis(wait));
            }
            // SAFETY: as above; macOS waits for this answer (at most 30 s).
            unsafe { IOAllowPowerChange(port, argument as isize) };
        }
        MSG_SYSTEM_HAS_POWERED_ON => {
            (st.handler)(PowerEvent::Woke);
        }
        _ => {}
    }
}

extern "C" fn on_root_domain(
    ctx: *mut c_void,
    _service: u32,
    message: u32,
    _argument: *mut c_void,
) {
    if message == MSG_CLAMSHELL || message == MSG_CAPABILITY_CHANGE {
        // SAFETY: `ctx` is the leaked WatchState given at registration.
        let st = unsafe { &*(ctx as *const WatchState) };
        (st.handler)(PowerEvent::Changed);
    }
}

/// The Mac's power watch (DMN-613): the power source (IOPS), the lid (IOPMrootDomain's clamshell message) and sleep
/// and wake (IORegisterForSystemPower), each told at once on a run-loop thread of its own.
#[derive(Default)]
pub struct Watch;

impl PowerWatch for Watch {
    fn start(&self, handler: PowerHandler) -> bool {
        let (tx, rx) = std::sync::mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("ferb-power-watch".into())
            .spawn(move || {
                let st: &'static WatchState = Box::leak(Box::new(WatchState {
                    handler,
                    root_port: std::sync::atomic::AtomicU32::new(0),
                }));
                let ctx = st as *const WatchState as *mut c_void;
                // SAFETY: this thread's run loop; every source below is retained by the run loop once added.
                let ok = unsafe {
                    let rl = CFRunLoopGetCurrent();
                    let ps = IOPSNotificationCreateRunLoopSource(on_power_source, ctx);
                    if !ps.is_null() {
                        CFRunLoopAddSource(rl, ps, kCFRunLoopDefaultMode);
                    }
                    let mut port: *mut c_void = std::ptr::null_mut();
                    let mut notifier: u32 = 0;
                    let root =
                        IORegisterForSystemPower(ctx, &mut port, on_system_power, &mut notifier);
                    if root != 0 && !port.is_null() {
                        st.root_port
                            .store(root, std::sync::atomic::Ordering::SeqCst);
                        CFRunLoopAddSource(
                            rl,
                            IONotificationPortGetRunLoopSource(port),
                            kCFRunLoopDefaultMode,
                        );
                    }
                    let name = CString::new("IOPMrootDomain").unwrap_or_default();
                    let domain = IOServiceGetMatchingService(0, IOServiceMatching(name.as_ptr()));
                    let iport = IONotificationPortCreate(0);
                    if domain != 0 && !iport.is_null() {
                        let interest = CString::new("IOGeneralInterest").unwrap_or_default();
                        let mut n: u32 = 0;
                        IOServiceAddInterestNotification(
                            iport,
                            domain,
                            interest.as_ptr(),
                            on_root_domain,
                            ctx,
                            &mut n,
                        );
                        CFRunLoopAddSource(
                            rl,
                            IONotificationPortGetRunLoopSource(iport),
                            kCFRunLoopDefaultMode,
                        );
                    }
                    !ps.is_null() || root != 0
                };
                let _ = tx.send(ok);
                if ok {
                    // SAFETY: runs this thread's run loop for the process's life.
                    unsafe { CFRunLoopRun() };
                }
            });
        spawned.is_ok() && rx.recv_timeout(Duration::from_secs(5)).unwrap_or(false)
    }
}

/// The owner's "Lock my Mac": `pmset displaysleepnow` (as the user, no root, no setting changed), then the lock read
/// back for up to two seconds (0.13 s on the owner's Mac, x3 experiment 9).
#[derive(Default)]
pub struct DisplaySleepLock;

impl LockMac for DisplaySleepLock {
    fn lock_now(&self) -> Result<bool, String> {
        let status = std::process::Command::new("/usr/bin/pmset")
            .arg("displaysleepnow")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map_err(|e| e.to_string())?;
        if !status.success() {
            return Err(format!("pmset displaysleepnow ended with {status}"));
        }
        let until = Instant::now() + Duration::from_secs(2);
        loop {
            if screen_locked() == Some(true) {
                return Ok(true);
            }
            if Instant::now() >= until {
                return Ok(false);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

/// The daemon's side of FERB's root helper (APP-986): one short connection per request to its socket. A helper that
/// isn't registered, approved or running answers nothing: [`HelperState::Absent`].
pub struct HelperClient {
    socket: std::path::PathBuf,
}

impl HelperClient {
    pub fn system() -> HelperClient {
        HelperClient::at(helper::SOCKET)
    }

    pub fn at(socket: impl Into<std::path::PathBuf>) -> HelperClient {
        HelperClient {
            socket: socket.into(),
        }
    }

    fn ask(&self, req: &Request) -> Result<serde_json::Value, String> {
        use std::io::{BufRead, BufReader, Write};
        let mut s = std::os::unix::net::UnixStream::connect(&self.socket).map_err(|_| {
            "FERB's helper isn't running (it needs your OK in System Settings)".to_string()
        })?;
        let _ = s.set_read_timeout(Some(Duration::from_secs(3)));
        let _ = s.set_write_timeout(Some(Duration::from_secs(3)));
        s.write_all(req.to_line().as_bytes())
            .map_err(|e| e.to_string())?;
        let mut line = String::new();
        BufReader::new(&s)
            .read_line(&mut line)
            .map_err(|e| e.to_string())?;
        let v: serde_json::Value =
            serde_json::from_str(line.trim()).map_err(|_| "no answer".to_string())?;
        if v["ok"] == true {
            Ok(v)
        } else {
            Err(v["error"].as_str().unwrap_or("refused").to_owned())
        }
    }
}

impl RootHelper for HelperClient {
    fn state(&self) -> HelperState {
        match self.ask(&Request::Status) {
            Ok(_) => HelperState::Ready,
            Err(_) => HelperState::Absent,
        }
    }
    fn lease(&self, seconds: u64) -> Result<(), String> {
        self.ask(&Request::Lease { seconds }).map(|_| ())
    }
    fn release(&self) -> Result<(), String> {
        self.ask(&Request::Release).map(|_| ())
    }
    fn wakes(&self) -> Result<Vec<i64>, String> {
        let v = self.ask(&Request::Wakes)?;
        Ok(v["wakes"]
            .as_array()
            .map(|a| a.iter().filter_map(serde_json::Value::as_i64).collect())
            .unwrap_or_default())
    }
    fn wake(&self, at: i64) -> Result<(), String> {
        self.ask(&Request::Wake { at }).map(|_| ())
    }
    fn unwake(&self, at: i64) -> Result<(), String> {
        self.ask(&Request::Unwake { at }).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reads only: nothing is held, nothing locked (a hold would keep the developer's Mac awake for its lease). What
    /// is checked holds in any state the Mac is in (TEST-508): awake or in a dark wake, display on or asleep, on a Mac
    /// with a battery or without one. How a capability reading becomes `dark_wake` is the next test.
    #[test]
    fn the_facts_read_without_changing_anything() {
        let r = Facts.read();
        eprintln!("power facts: {r:?}");
        if let Some(b) = r.battery {
            assert!((0..=100).contains(&b));
        }
        // `dark_wake` is not checked: it was asserted to be a full wake ("whoever runs this test is at an awake Mac"),
        // which a gate run left alone at night needn't be. The settings in force always have a power-adapter half;
        // a battery half only on a Mac with a battery, so that one is not asserted either.
        assert!(r.power_nap_ac.is_some(), "{r:?}");
        assert!(Assertions::new().held.lock().unwrap().is_empty());
        Assertions::new().release(Hold::Display); // nothing held: nothing happens
    }

    /// A dark wake is the processor without graphics (IOPMLibPrivate.h's bits); nothing read is unknown.
    #[test]
    fn a_dark_wake_is_the_processor_without_graphics() {
        assert_eq!(dark_wake_of(0x1f), Some(false));
        assert_eq!(dark_wake_of(0x19), Some(true));
        assert_eq!(dark_wake_of(0x1), Some(true));
        assert_eq!(dark_wake_of(0), None);
    }

    /// A helper that isn't there is Absent, and every request says so (a socket in a temp folder: nothing listens).
    // decision: APP-986
    #[test]
    fn an_absent_helper_is_said_plainly() {
        let dir = std::env::temp_dir().join(format!("ferb-helper-client-{}", std::process::id()));
        let c = HelperClient::at(dir.join("none.sock"));
        assert_eq!(c.state(), HelperState::Absent);
        assert!(c.lease(60).unwrap_err().contains("System Settings"));
    }
}
