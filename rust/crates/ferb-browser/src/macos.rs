//! The Mac's side of `profiles.py` and `downloads.py`: Launch Services lookups (an app by bundle id,
//! its executable, the apps that open https links: HAND-247, HAND-414, HAND-415), whether a
//! browser runs (`pgrep -f`), starting a browser detached, and the volume's capacity for important
//! usage (HAND-246). Reached through [`crate::profiles::Platform`] and a closure the daemon hands
//! [`crate::downloads::Downloads`]; nothing here opens a window or changes anything.
//!
//! AppKit and Foundation are called through the Objective-C runtime (`objc_msgSend`), each call in
//! its own autorelease pool, as Python's PyObjC calls were (`objc.autorelease_pool()`).

use std::ffi::{c_char, c_void, CStr, CString};
use std::path::{Path, PathBuf};

use crate::profiles::Platform;

type Id = *mut c_void;
type Sel = *mut c_void;

#[link(name = "objc")]
extern "C" {
    fn objc_getClass(name: *const c_char) -> Id;
    fn sel_registerName(name: *const c_char) -> Sel;
    fn objc_msgSend();
    fn objc_autoreleasePoolPush() -> *mut c_void;
    fn objc_autoreleasePoolPop(pool: *mut c_void);
}

#[link(name = "AppKit", kind = "framework")]
extern "C" {}

#[link(name = "Foundation", kind = "framework")]
extern "C" {
    static NSURLVolumeAvailableCapacityForImportantUsageKey: Id;
}

/// One autorelease pool around `f` (`objc.autorelease_pool()`).
fn pool<T>(f: impl FnOnce() -> T) -> T {
    // SAFETY: push and pop are paired on this thread; nothing autoreleased inside outlives the pop
    // (every result is copied into Rust values first).
    let p = unsafe { objc_autoreleasePoolPush() };
    let out = f();
    // SAFETY: the pool pushed above, popped once.
    unsafe { objc_autoreleasePoolPop(p) };
    out
}

fn class(name: &str) -> Id {
    let c = CString::new(name).expect("a class name");
    // SAFETY: a NUL-terminated class name; a missing class is a null pointer, checked by callers.
    unsafe { objc_getClass(c.as_ptr()) }
}

fn sel(name: &str) -> Sel {
    let c = CString::new(name).expect("a selector name");
    // SAFETY: a NUL-terminated selector name; the runtime registers it once and returns it.
    unsafe { sel_registerName(c.as_ptr()) }
}

/// `[obj sel]` returning an object.
///
/// # Safety
/// `obj` must be a live object (or nil) that answers `sel` with no argument and an object result.
unsafe fn send0(obj: Id, s: &str) -> Id {
    if obj.is_null() {
        return std::ptr::null_mut();
    }
    // SAFETY: objc_msgSend called through the exact signature of a no-argument, object-returning
    // method, which the caller guarantees.
    let f: unsafe extern "C" fn(Id, Sel) -> Id =
        unsafe { std::mem::transmute(objc_msgSend as unsafe extern "C" fn()) };
    // SAFETY: as above.
    unsafe { f(obj, sel(s)) }
}

/// `[obj sel:arg]` returning an object.
///
/// # Safety
/// `obj` must be a live object (or nil) that answers `sel` with one object argument and an object
/// result.
unsafe fn send1(obj: Id, s: &str, arg: Id) -> Id {
    if obj.is_null() {
        return std::ptr::null_mut();
    }
    // SAFETY: the one-object-argument, object-returning signature, which the caller guarantees.
    let f: unsafe extern "C" fn(Id, Sel, Id) -> Id =
        unsafe { std::mem::transmute(objc_msgSend as unsafe extern "C" fn()) };
    // SAFETY: as above.
    unsafe { f(obj, sel(s), arg) }
}

/// An `NSString` (autoreleased) from Rust text.
fn ns_string(text: &str) -> Id {
    let Ok(c) = CString::new(text) else {
        return std::ptr::null_mut();
    };
    // SAFETY: +[NSString stringWithUTF8String:] takes a NUL-terminated UTF-8 string and returns an
    // autoreleased string (nil for invalid UTF-8, which Rust text never is).
    let f: unsafe extern "C" fn(Id, Sel, *const c_char) -> Id =
        unsafe { std::mem::transmute(objc_msgSend as unsafe extern "C" fn()) };
    // SAFETY: as above; the class is Foundation's.
    unsafe { f(class("NSString"), sel("stringWithUTF8String:"), c.as_ptr()) }
}

/// Rust text from an `NSString` (`None` for nil).
fn rust_string(s: Id) -> Option<String> {
    if s.is_null() {
        return None;
    }
    // SAFETY: -[NSString UTF8String] returns a NUL-terminated buffer owned by the string (or its
    // autorelease pool), copied at once.
    let f: unsafe extern "C" fn(Id, Sel) -> *const c_char =
        unsafe { std::mem::transmute(objc_msgSend as unsafe extern "C" fn()) };
    // SAFETY: as above.
    let p = unsafe { f(s, sel("UTF8String")) };
    if p.is_null() {
        return None;
    }
    // SAFETY: a valid NUL-terminated C string, see above.
    Some(unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned())
}

fn workspace() -> Id {
    // SAFETY: +[NSWorkspace sharedWorkspace] takes no argument and returns the shared object.
    unsafe { send0(class("NSWorkspace"), "sharedWorkspace") }
}

/// `-[NSURL path]` of a file URL.
fn url_path(url: Id) -> Option<PathBuf> {
    // SAFETY: `url` is an NSURL (or nil); `path` returns an NSString.
    rust_string(unsafe { send0(url, "path") }).map(PathBuf::from)
}

/// The app bundle Launch Services knows for this bundle id
/// (`URLForApplicationWithBundleIdentifier:`).
pub fn app_url(bundle_id: &str) -> Option<PathBuf> {
    pool(|| {
        // SAFETY: the shared workspace answers this method with one NSString and returns an NSURL
        // or nil.
        let url = unsafe {
            send1(
                workspace(),
                "URLForApplicationWithBundleIdentifier:",
                ns_string(bundle_id),
            )
        };
        url_path(url)
    })
}

/// `_launch_services`: the executable of that app (`NSBundle.executableURL`).
pub fn executable(bundle_id: &str) -> Option<PathBuf> {
    pool(|| {
        // SAFETY: as in `app_url`.
        let url = unsafe {
            send1(
                workspace(),
                "URLForApplicationWithBundleIdentifier:",
                ns_string(bundle_id),
            )
        };
        if url.is_null() {
            return None;
        }
        // SAFETY: +[NSBundle bundleWithURL:] takes an NSURL and returns a bundle or nil;
        // -executableURL returns an NSURL or nil.
        let bundle = unsafe { send1(class("NSBundle"), "bundleWithURL:", url) };
        // SAFETY: as above.
        url_path(unsafe { send0(bundle, "executableURL") })
    })
}

/// `_https_handlers`: every app bundle Launch Services lists as opening https links
/// (`URLsForApplicationsToOpenURL:`, macOS 12 and later); `None` without that method.
pub fn https_handlers() -> Option<Vec<PathBuf>> {
    pool(|| {
        let ws = workspace();
        // SAFETY: -[NSObject respondsToSelector:] takes a selector and returns a BOOL.
        let responds: unsafe extern "C" fn(Id, Sel, Sel) -> bool =
            unsafe { std::mem::transmute(objc_msgSend as unsafe extern "C" fn()) };
        // SAFETY: as above, on the shared workspace.
        if ws.is_null()
            || !unsafe {
                responds(
                    ws,
                    sel("respondsToSelector:"),
                    sel("URLsForApplicationsToOpenURL:"),
                )
            }
        {
            return None;
        }
        // SAFETY: +[NSURL URLWithString:] takes an NSString and returns an NSURL.
        let probe = unsafe {
            send1(
                class("NSURL"),
                "URLWithString:",
                ns_string("https://example.com"),
            )
        };
        // SAFETY: the method checked above takes an NSURL and returns an NSArray of NSURLs.
        let urls = unsafe { send1(ws, "URLsForApplicationsToOpenURL:", probe) };
        if urls.is_null() {
            return Some(vec![]);
        }
        // SAFETY: -[NSArray count] returns an NSUInteger; -objectAtIndex: an element.
        let count: unsafe extern "C" fn(Id, Sel) -> usize =
            unsafe { std::mem::transmute(objc_msgSend as unsafe extern "C" fn()) };
        // SAFETY: as above.
        let at: unsafe extern "C" fn(Id, Sel, usize) -> Id =
            unsafe { std::mem::transmute(objc_msgSend as unsafe extern "C" fn()) };
        // SAFETY: as above.
        let n = unsafe { count(urls, sel("count")) };
        Some(
            (0..n)
                // SAFETY: an index below the array's count.
                .filter_map(|i| url_path(unsafe { at(urls, sel("objectAtIndex:"), i) }))
                .collect(),
        )
    })
}

/// `_important_usage` (HAND-246): the volume's available capacity for important usage (free plus
/// what macOS frees on demand), as Finder shows it; `None` when macOS doesn't give it.
pub fn important_usage(where_: &Path) -> Option<i64> {
    pool(|| {
        let path = ns_string(&where_.to_string_lossy());
        // SAFETY: +[NSURL fileURLWithPath:] takes an NSString and returns an NSURL.
        let url = unsafe { send1(class("NSURL"), "fileURLWithPath:", path) };
        if url.is_null() {
            return None;
        }
        let mut value: Id = std::ptr::null_mut();
        // SAFETY: -[NSURL getResourceValue:forKey:error:] takes an out-pointer, a key and a
        // nullable error out-pointer and returns a BOOL; the key is Foundation's constant.
        let get: unsafe extern "C" fn(Id, Sel, *mut Id, Id, *mut Id) -> bool =
            unsafe { std::mem::transmute(objc_msgSend as unsafe extern "C" fn()) };
        // SAFETY: as above; `value` receives an autoreleased NSNumber or nil.
        let ok = unsafe {
            get(
                url,
                sel("getResourceValue:forKey:error:"),
                &mut value,
                NSURLVolumeAvailableCapacityForImportantUsageKey,
                std::ptr::null_mut(),
            )
        };
        if !ok || value.is_null() {
            return None;
        }
        // SAFETY: -[NSNumber longLongValue] returns a long long.
        let long: unsafe extern "C" fn(Id, Sel) -> i64 =
            unsafe { std::mem::transmute(objc_msgSend as unsafe extern "C" fn()) };
        // SAFETY: as above.
        let n = unsafe { long(value, sel("longLongValue")) };
        (n > 0).then_some(n)
    })
}

/// The Mac's [`Platform`].
#[derive(Clone, Copy, Debug, Default)]
pub struct MacPlatform;

impl Platform for MacPlatform {
    fn has_app(&self, bundle_id: &str) -> bool {
        app_url(bundle_id).is_some_and(|p| crate::profiles::app_path_ok(&p))
    }

    fn locate(&self, bundle_id: &str) -> Result<Option<PathBuf>, String> {
        Ok(executable(bundle_id))
    }

    fn https_handlers(&self) -> Result<Vec<PathBuf>, String> {
        https_handlers().ok_or_else(|| "URLsForApplicationsToOpenURL: isn't available".into())
    }

    fn pgrep(&self, pattern: &str) -> bool {
        std::process::Command::new("pgrep")
            .args(["-f", pattern])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    }
}

/// `_detached`: a browser started in a session of its own, never FERB's child with a timeout, never
/// killed; its exit is collected by a thread so it leaves no zombie.
pub fn launch_detached(args: &[String]) -> std::io::Result<()> {
    use std::os::unix::process::CommandExt;
    let (program, rest) = args
        .split_first()
        .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    let mut cmd = std::process::Command::new(program);
    cmd.args(rest)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    // SAFETY: setsid is async-signal-safe and touches no memory of the parent; run between fork and
    // exec in the child (Python's start_new_session).
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = cmd.spawn()?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

/// `_browser_pid`: the browser process behind a native-host connection, the native host's parent (the
/// browser starts it, and FERB's launcher execs the relay in place). Kernel facts (LOCAL_PEERPID, then
/// libproc's `pbi_ppid`), never a name or window lookup; `None` when either can't be read or the parent
/// is launchd or the kernel.
pub fn native_host_parent(socket: std::os::fd::BorrowedFd<'_>) -> Option<i32> {
    use std::os::fd::AsRawFd;
    const SOL_LOCAL: libc::c_int = 0;
    const LOCAL_PEERPID: libc::c_int = 0x002;
    let mut host: libc::pid_t = 0;
    let mut len = std::mem::size_of::<libc::pid_t>() as libc::socklen_t;
    // SAFETY: `host` is a writable pid_t of exactly `len` bytes; the descriptor is open while borrowed.
    let status = unsafe {
        libc::getsockopt(
            socket.as_raw_fd(),
            SOL_LOCAL,
            LOCAL_PEERPID,
            (&mut host as *mut libc::pid_t).cast(),
            &mut len,
        )
    };
    if status != 0 || len as usize != std::mem::size_of::<libc::pid_t>() || host <= 0 {
        return None;
    }
    let parent = parent_pid(host)?;
    (parent > 1).then_some(parent)
}

/// A process's parent as the kernel reports it (`proc_pidinfo(PROC_PIDTBSDINFO).pbi_ppid`).
pub fn parent_pid(pid: i32) -> Option<i32> {
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    // SAFETY: the buffer is one proc_bsdinfo of the size passed; the kernel writes at most that many bytes.
    let n = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            size,
        )
    };
    if n != size {
        return None;
    }
    // SAFETY: proc_pidinfo filled the whole struct (n == size), and it was zeroed before.
    let info = unsafe { info.assume_init() };
    i32::try_from(info.pbi_ppid).ok()
}

/// A process's executable path (`proc_pidpath`).
pub fn pid_path(pid: i32) -> Option<String> {
    let mut buf = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    // SAFETY: the buffer is writable for the size passed; proc_pidpath writes a NUL-terminated path.
    let n = unsafe { libc::proc_pidpath(pid, buf.as_mut_ptr().cast(), buf.len() as u32) };
    if n <= 0 {
        return None;
    }
    buf.truncate(n as usize);
    String::from_utf8(buf).ok()
}

/// `_app_bundle` (HAND-223, HAND-438): the bundle id of the app whose executable this process runs, from
/// its path (libproc) and the outermost `.app` / `.app.bundle`'s Info.plist: kernel and file facts, never
/// a window or a name the process claims. `None` for no pid, no bundle or no readable id.
pub fn app_bundle(pid: Option<i32>) -> Option<String> {
    let pid = pid.filter(|p| *p > 1)?;
    bundle_id_of_executable(&pid_path(pid)?)
}

/// The `CFBundleIdentifier` of the outermost bundle around `executable` (XML or binary Info.plist).
pub fn bundle_id_of_executable(executable: &str) -> Option<String> {
    let bundle = crate::rules::outermost_bundle(executable)?;
    let info = plist::Value::from_file(Path::new(bundle).join("Contents/Info.plist")).ok()?;
    let id = info
        .as_dictionary()?
        .get("CFBundleIdentifier")?
        .as_string()?;
    (!id.is_empty()).then(|| id.to_string())
}

#[cfg(test)]
mod peer_tests {
    use super::*;
    use std::os::fd::AsFd;

    // decision: HAND-223
    #[test]
    fn a_native_hosts_parent_is_read_from_the_socket() {
        // the peer of one end of a pair is this process; its parent is the one that started the test
        let (a, _b) = std::os::unix::net::UnixStream::pair().unwrap();
        let parent = native_host_parent(a.as_fd());
        // SAFETY: getppid has no preconditions.
        let expected = unsafe { libc::getppid() };
        assert_eq!(parent, (expected > 1).then_some(expected));
        let file = tempfile::tempfile().unwrap();
        assert_eq!(native_host_parent(file.as_fd()), None); // not a socket: nothing
    }

    // decision: HAND-438
    #[test]
    fn the_outermost_bundles_info_plist_names_the_app() {
        let dir = tempfile::tempdir().unwrap();
        let clone = dir
            .path()
            .join("code_sign_clone.Ab12/Google Chrome.app.bundle");
        std::fs::create_dir_all(clone.join("Contents/Frameworks/Helper.app/Contents/MacOS"))
            .unwrap();
        let mut d = plist::Dictionary::new();
        d.insert("CFBundleIdentifier".into(), "com.google.Chrome".into());
        plist::Value::Dictionary(d)
            .to_file_binary(clone.join("Contents/Info.plist"))
            .unwrap();
        let exe = clone.join("Contents/Frameworks/Helper.app/Contents/MacOS/Helper");
        assert_eq!(
            bundle_id_of_executable(exe.to_str().unwrap()).as_deref(),
            Some("com.google.Chrome")
        );
        assert_eq!(bundle_id_of_executable("/usr/bin/true"), None);
        // this test binary runs from no bundle; launchd and the kernel never count
        assert_eq!(app_bundle(Some(std::process::id() as i32)), None);
        assert_eq!(app_bundle(Some(1)), None);
        assert_eq!(app_bundle(None), None);
    }
}
