//! Apps, processes, windows and Accessibility elements as the watchers read them: the frontmost app
//! (Accessibility's system-wide focused application, read fresh), an app's name and bundle id
//! (NSRunningApplication), a process's parent (libproc), the on-screen normal windows (the window server's list), an
//! app's sole window title and main window frame, and the focused element for text capture. Read-only, always.

use std::ffi::c_void;
use std::sync::Once;

use core_foundation_sys::array::{
    CFArrayGetCount, CFArrayGetTypeID, CFArrayGetValueAtIndex, CFArrayRef,
};
use core_foundation_sys::base::{CFEqual, CFGetTypeID, CFHash, CFRetain, CFTypeRef};
use core_foundation_sys::dictionary::{CFDictionaryGetValue, CFDictionaryRef};
use core_foundation_sys::string::CFStringRef;
use objc2_app_kit::NSRunningApplication;
use serde_json::Value;

use super::ffi::*;
use super::{cfstr, pool, scalar, string_of, Owned};
use crate::front_window::Window;
use crate::observation::Dict;
use crate::watchers::accessibility::AxElement;
use crate::watchers::base::Focused;
use crate::watchers::text_input::{FieldReads, FocusedElement};

/// accessibility.py:35: how long one Accessibility message may take (an app that hangs can't stall a watcher).
pub const AX_MESSAGE_TIMEOUT_S: f32 = 2.0;
/// accessibility.py:780: ancestors read at most for the terminal check.
pub const SURFACE_DEPTH: usize = 60;
/// accessibility.py:777.
pub const SURFACE_ATTRS: [(&str, &str); 8] = [
    ("role", "AXRole"),
    ("subrole", "AXSubrole"),
    ("role_description", "AXRoleDescription"),
    ("identifier", "AXIdentifier"),
    ("dom_identifier", "AXDOMIdentifier"),
    ("dom_classes", "AXDOMClassList"),
    ("title", "AXTitle"),
    ("description", "AXDescription"),
];

static SYSTEM_TIMEOUT: Once = Once::new();

/// How long a reading of Accessibility trust serves before it is read again (the watchers poll every 0.5 s).
pub const TRUST_FRESH_S: f64 = 2.0;
static TRUST: std::sync::Mutex<Option<(std::time::Instant, bool)>> = std::sync::Mutex::new(None);

/// Whether FERB is trusted for Accessibility (`AXIsProcessTrusted`, a preflight that never prompts), read at most
/// every [`TRUST_FRESH_S`]. Fixed against Python: base.py `focused_app` and the window-title watcher called the
/// Accessibility API from the daemon's first second, before the owner's Allow all; macOS answers an untrusted
/// process's first such call by adding FERB to Accessibility switched off and showing its own "would like to control
/// this computer" alert, outside FERB's setup (live 2026-11-13: during the browser pairing). Untrusted, every element
/// here is None, as an unreadable one already was; the watchers start reading once the owner allows it.
pub fn trusted() -> bool {
    trusted_with(&TRUST, std::time::Instant::now(), || {
        // SAFETY: AXIsProcessTrusted takes no arguments and only asks TCC; it never prompts.
        unsafe { AXIsProcessTrusted() != 0 }
    })
}

/// [`trusted`] over a given cache, clock and reading (tests).
pub fn trusted_with(
    cache: &std::sync::Mutex<Option<(std::time::Instant, bool)>>,
    now: std::time::Instant,
    read: impl FnOnce() -> bool,
) -> bool {
    let mut held = cache
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some((at, verdict)) = *held {
        if now.saturating_duration_since(at).as_secs_f64() < TRUST_FRESH_S {
            return verdict;
        }
    }
    let verdict = read();
    *held = Some((now, verdict));
    verdict
}

/// An Accessibility element FERB holds (retained), read one attribute at a time.
pub struct Ax(Owned);

impl Ax {
    /// Takes ownership of an element the caller created or copied.
    pub fn owned(p: AXUIElementRef) -> Option<Ax> {
        Owned::new(p as CFTypeRef).map(Ax)
    }

    /// The system-wide element (with FERB's messaging timeout set on it once); None while FERB isn't trusted for
    /// Accessibility ([`trusted`]: no call reaches the API that would make macOS prompt).
    pub fn system() -> Option<Ax> {
        if !trusted() {
            return None;
        }
        // SAFETY: creating the system-wide element has no preconditions; it is owned (Create rule).
        let el = Ax::owned(unsafe { AXUIElementCreateSystemWide() })?;
        SYSTEM_TIMEOUT.call_once(|| {
            // SAFETY: a valid element; the timeout applies to messages sent through it.
            unsafe { AXUIElementSetMessagingTimeout(el.raw(), AX_MESSAGE_TIMEOUT_S) };
        });
        Some(el)
    }

    /// An app's element, with the messaging timeout set; None while FERB isn't trusted for Accessibility.
    pub fn app(pid: i64) -> Option<Ax> {
        Ax::system()?;
        // SAFETY: any pid is accepted (a stale one gives an element whose reads fail); owned (Create rule).
        let el = Ax::owned(unsafe { AXUIElementCreateApplication(pid as libc::pid_t) })?;
        // SAFETY: a valid element.
        unsafe { AXUIElementSetMessagingTimeout(el.raw(), AX_MESSAGE_TIMEOUT_S) };
        Some(el)
    }

    pub fn raw(&self) -> AXUIElementRef {
        self.0 .0 as AXUIElementRef
    }

    /// An attribute's value as FERB owns it (null-free), or None when it can't be read.
    pub fn copy(&self, name: &str) -> Option<Owned> {
        let key = cfstr(name);
        let mut out: CFTypeRef = std::ptr::null();
        // SAFETY: a valid element and attribute name; on success `out` is a copied (owned) value.
        let err =
            unsafe { AXUIElementCopyAttributeValue(self.raw(), key.0 as CFStringRef, &mut out) };
        if err != 0 {
            return None;
        }
        Owned::new(out)
    }

    /// An attribute that is itself an element.
    pub fn element(&self, name: &str) -> Option<Ax> {
        let v = self.copy(name)?;
        // SAFETY: `v` is a valid CF object; its type is checked before it is used as an element.
        if unsafe { CFGetTypeID(v.0) } != unsafe { AXUIElementGetTypeID() } {
            return None;
        }
        Some(Ax(v))
    }

    /// An attribute that is a list of elements.
    pub fn elements(&self, name: &str) -> Vec<Ax> {
        let Some(v) = self.copy(name) else {
            return Vec::new();
        };
        // SAFETY: `v` is a valid CF object; its type is checked; each item is retained before it is kept.
        unsafe {
            if CFGetTypeID(v.0) != CFArrayGetTypeID() {
                return Vec::new();
            }
            let a = v.0 as CFArrayRef;
            (0..CFArrayGetCount(a))
                .filter_map(|i| {
                    let item = CFArrayGetValueAtIndex(a, i);
                    if item.is_null() || CFGetTypeID(item) != AXUIElementGetTypeID() {
                        return None;
                    }
                    Some(Ax(Owned(CFRetain(item))))
                })
                .collect()
        }
    }

    /// The element's process.
    pub fn pid(&self) -> Option<i64> {
        let mut pid: libc::pid_t = 0;
        // SAFETY: a valid element and a writable pid.
        (unsafe { AXUIElementGetPid(self.raw(), &mut pid) } == 0).then_some(i64::from(pid))
    }

    /// `CFEqual`: the same element.
    pub fn same(&self, other: &Ax) -> bool {
        // SAFETY: both are valid CF objects.
        unsafe { CFEqual(self.0 .0, other.0 .0) != 0 }
    }

    /// A list of strings (`_strings`): one string as itself, anything else as none.
    fn strings(&self, name: &str) -> Vec<String> {
        let Some(v) = self.copy(name) else {
            return Vec::new();
        };
        // SAFETY: `v` is valid; each item's type is checked before it is read.
        unsafe {
            if let Some(Value::String(s)) = scalar(v.0) {
                return vec![s];
            }
            if CFGetTypeID(v.0) != CFArrayGetTypeID() {
                return Vec::new();
            }
            let a = v.0 as CFArrayRef;
            (0..CFArrayGetCount(a))
                .filter_map(|i| match scalar(CFArrayGetValueAtIndex(a, i)) {
                    Some(Value::String(s)) => Some(s),
                    Some(other) => Some(ferb_py::value::py_str(&other)),
                    None => None,
                })
                .collect()
        }
    }

    /// A frame attribute pair (AXPosition, AXSize) as (x, y, w, h).
    pub fn frame(&self) -> Option<(f64, f64, f64, f64)> {
        let (pos, size) = (self.copy("AXPosition")?, self.copy("AXSize")?);
        let mut p = [0f64; 2];
        let mut s = [0f64; 2];
        // SAFETY: AXValueGetValue checks the value's type and writes a CGPoint / CGSize (two doubles) when it
        // matches; the buffers are that size.
        let ok = unsafe {
            AXValueGetValue(pos.0, kAXValueCGPointType, p.as_mut_ptr() as *mut c_void) != 0
                && AXValueGetValue(size.0, kAXValueCGSizeType, s.as_mut_ptr() as *mut c_void) != 0
        };
        ok.then_some((p[0], p[1], s[0], s[1]))
    }
}

impl AxElement for Ax {
    fn attr(&self, name: &str) -> Option<Value> {
        let v = self.copy(name)?;
        // SAFETY: `v` is a valid CF object.
        unsafe { scalar(v.0) }
    }

    fn window(&self) -> Option<Box<dyn AxElement + '_>> {
        self.element("AXWindow")
            .map(|w| Box::new(w) as Box<dyn AxElement>)
    }

    fn hash_id(&self) -> String {
        // SAFETY: a valid CF object.
        unsafe { CFHash(self.0 .0) }.to_string()
    }
}

impl FocusedElement for Ax {
    /// accessibility.py:795 `surface_facts`: the element's structural facts and label, then each ancestor's up to the
    /// app, never a value.
    fn surface_facts(&self) -> Vec<Dict> {
        let mut chain = Vec::new();
        let mut el: Option<Ax> = Some(Ax(Owned(
            // SAFETY: retaining a valid element for the walk's own reference.
            unsafe { CFRetain(self.0 .0) },
        )));
        while let Some(e) = el {
            if chain.len() >= SURFACE_DEPTH {
                break;
            }
            let mut facts = Dict::new();
            for (key, name) in SURFACE_ATTRS {
                if key == "dom_classes" {
                    let list = e.strings(name);
                    if !list.is_empty() {
                        facts.insert(key.into(), Value::from(list));
                    }
                } else if let Some(v) = e.attr(name) {
                    let text = ferb_py::value::py_str(&v);
                    if !text.is_empty() {
                        facts.insert(key.into(), Value::from(text));
                    }
                }
            }
            let top = facts.get("role") == Some(&Value::from("AXApplication"));
            chain.push(facts);
            if top {
                break;
            }
            el = e.element("AXParent");
        }
        chain
    }
}

/// base.py:34 `focused_app`: the frontmost app as Accessibility's system-wide focused application, read fresh.
pub fn focused_app() -> Option<Focused> {
    pool(|| {
        let app = Ax::system()?.element("AXFocusedApplication")?;
        let pid = app.pid()?;
        let (name, bundle) = app_of(pid)?;
        (!bundle.is_empty()).then_some(Focused {
            pid,
            bundle_id: bundle,
            name,
        })
    })
}

/// front_window.py:159 `app_of`: a process's app name and bundle id (NSRunningApplication); None when it isn't an app.
pub fn app_of(pid: i64) -> Option<(String, String)> {
    pool(|| {
        let app =
            NSRunningApplication::runningApplicationWithProcessIdentifier(pid as libc::pid_t)?;
        let name = app
            .localizedName()
            .map(|s| s.to_string())
            .unwrap_or_default();
        let bundle = app
            .bundleIdentifier()
            .map(|s| s.to_string())
            .unwrap_or_default();
        Some((name, bundle))
    })
}

/// front_window.py:113 `parent_pid`: a process's parent as the kernel reports it, or None.
pub fn parent_pid(pid: i64) -> Option<i64> {
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
    // SAFETY: the buffer is a proc_bsdinfo of the size passed; the kernel writes at most that many bytes.
    let n = unsafe {
        libc::proc_pidinfo(
            pid as i32,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr() as *mut c_void,
            size,
        )
    };
    if n < size {
        return None;
    }
    // SAFETY: the kernel filled the whole struct (n == size).
    Some(i64::from(unsafe { info.assume_init() }.pbi_ppid))
}

/// accessibility.py:253 `ax_windows`: every window Accessibility hands over for an app, deduplicated.
fn ax_windows(pid: i64) -> Vec<Ax> {
    let Some(app) = Ax::app(pid) else {
        return Vec::new();
    };
    let mut candidates: Vec<Ax> = Vec::new();
    candidates.extend(app.element("AXFocusedWindow"));
    candidates.extend(app.element("AXMainWindow"));
    candidates.extend(
        app.element("AXFocusedUIElement")
            .and_then(|f| f.element("AXWindow")),
    );
    candidates.extend(app.elements("AXWindows"));
    let mut out: Vec<Ax> = Vec::new();
    for w in candidates {
        let role = w
            .attr("AXRole")
            .map(|v| ferb_py::value::py_str(&v))
            .unwrap_or_default();
        if (role == "AXWindow" || role == "AXSheet") && !out.iter().any(|x| x.same(&w)) {
            out.push(w);
        }
    }
    out
}

/// base.py:49 `focused_title`: the title of the app's sole reachable window (accessibility.py:247 `app_window`).
pub fn focused_title(pid: i64) -> Option<String> {
    pool(|| {
        let wins = ax_windows(pid);
        if wins.len() != 1 {
            return None;
        }
        match wins[0].attr("AXTitle") {
            Some(v) if ferb_py::value::truthy(&v) => Some(ferb_py::value::py_str(&v)),
            _ => None,
        }
    })
}

/// accessibility.py:265 `main_window_frame`: the frame of the app's main window, else its focused window.
pub fn main_window_frame(pid: i64) -> Option<(f64, f64, f64, f64)> {
    pool(|| {
        let app = Ax::app(pid)?;
        app.element("AXMainWindow")
            .or_else(|| app.element("AXFocusedWindow"))?
            .frame()
    })
}

/// screenshot.py `normal_windows`: on-screen normal windows (layer 0, not transparent, with an area), front to back.
pub fn normal_windows() -> Vec<Window> {
    pool(|| {
        // SAFETY: a read-only query of the window server; the array is owned (Copy rule).
        let Some(list) = Owned::new(unsafe {
            CGWindowListCopyWindowInfo(
                kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements,
                0,
            )
        } as CFTypeRef) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        // SAFETY: the list is a CFArray of CFDictionaries; each value is read with its key and checked by `scalar`.
        unsafe {
            let a = list.0 as CFArrayRef;
            for i in 0..CFArrayGetCount(a) {
                let d = CFArrayGetValueAtIndex(a, i) as CFDictionaryRef;
                let get = |k: CFStringRef| scalar(CFDictionaryGetValue(d, k as *const c_void));
                let num = |k: CFStringRef, default: f64| {
                    get(k).and_then(|v| v.as_f64()).unwrap_or(default)
                };
                let layer = num(kCGWindowLayer, 1.0) as i64;
                let alpha = num(kCGWindowAlpha, 1.0);
                let b =
                    CFDictionaryGetValue(d, kCGWindowBounds as *const c_void) as CFDictionaryRef;
                let bound = |key: &str| {
                    if b.is_null() {
                        return 0.0;
                    }
                    let k = cfstr(key);
                    scalar(CFDictionaryGetValue(b, k.0))
                        .and_then(|v| v.as_f64())
                        .unwrap_or(0.0)
                };
                let bounds = (bound("X"), bound("Y"), bound("Width"), bound("Height"));
                if layer != 0 || alpha <= 0.0 || bounds.2 <= 0.0 || bounds.3 <= 0.0 {
                    continue;
                }
                out.push(Window {
                    id: num(kCGWindowNumber, 0.0) as i64,
                    pid: num(kCGWindowOwnerPID, -1.0) as i64,
                    bounds,
                    title: match get(kCGWindowName) {
                        Some(Value::String(s)) => s,
                        _ => String::new(),
                    },
                    layer,
                    alpha,
                });
            }
        }
        out
    })
}

/// The reads text capture makes on this Mac.
pub struct MacFieldReads;

impl FieldReads for MacFieldReads {
    /// text_input.py:56: Carbon's `IsSecureEventInputEnabled` (always linked on macOS, so never "unknown" here).
    fn secure_input(&self) -> bool {
        // SAFETY: a read-only query with no arguments.
        unsafe { IsSecureEventInputEnabled() != 0 }
    }

    fn focused_element(&self, pid: i64) -> Option<Box<dyn FocusedElement>> {
        Ax::app(pid)?
            .element("AXFocusedUIElement")
            .map(|e| Box::new(e) as Box<dyn FocusedElement>)
    }
}

/// Reads the CFString of an AX notification name.
///
/// # Safety
/// `s` must be a valid CFString.
pub unsafe fn notification_name(s: CFStringRef) -> String {
    // SAFETY: as the caller guarantees.
    unsafe { string_of(s) }
}
