//! The Mac's own answers for [`super::Mac`]: TCC reads and prompts (`ferb-mac-hands`' permissions), the
//! pasteboard's access state (`ferb-sense`, never its contents but the owner's one requested read), Automation, a
//! fresh process's grants (this program's `grant-probe` mode) and the passive self-test (`selftest.py`: the Dock's
//! Accessibility tree and a listen-only event tap, neither of which posts input).

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use ferb_core::interfaces::BoxFuture;
use serde_json::{json, Map, Value};

/// The real Mac. `click` is the Mac driver's `report_click_through`, set once the driver is composed (no driver: no
/// FERB app's report is recorded, as Python without `self.mac`).
#[derive(Default)]
pub struct RealMac {
    pub click: OnceLock<ClickThrough>,
}

/// `MacDriver::report_click_through(pid, windows)`.
pub type ClickThrough = Arc<dyn Fn(i32, &[i64]) -> bool + Send + Sync>;

impl super::Mac for RealMac {
    fn grants(&self) -> Map<String, Value> {
        crate::basics::object(ferb_mac_hands::native::permissions::check_all())
    }

    fn request(&self, permission: &str) -> String {
        ferb_mac_hands::native::permissions::request(permission)
            .map(|s| s.value())
            .unwrap_or("unknown")
            .to_string()
    }

    fn clipboard_access(&self) -> Option<String> {
        use ferb_sense::watchers::clipboard::Pasteboard;
        let b = ferb_sense::mac::system::GeneralPasteboard;
        Some(ferb_sense::watchers::clipboard::access(b.access_behavior()).to_string())
    }

    fn clipboard_request(&self) -> String {
        ferb_sense::mac::system::request_access().to_string()
    }

    fn automation(&self, bundle_id: &str, ask: bool) -> String {
        ferb_mac_hands::native::permissions::automation(bundle_id, ask)
            .value()
            .to_string()
    }

    fn selftest(&self) -> Vec<Value> {
        ferb_device::selftest::run_all(&mut Probes)
            .into_iter()
            .map(|r| json!({"name": r.name, "ok": r.ok, "detail": r.detail}))
            .collect()
    }

    fn fresh_grants(&self) -> BoxFuture<'static, Map<String, Value>> {
        Box::pin(async {
            // `_fresh_grants`: this program started now as `grant-probe`; one that doesn't answer in 2 s is killed
            // by its real path (crate::host): inside FERB.app the agent, whose `grant-probe` mode it allows
            let Some(exe) = crate::host::real_exe() else {
                return Map::new();
            };
            let child = tokio::process::Command::new(exe)
                .arg("grant-probe")
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .kill_on_drop(true)
                .spawn();
            let Ok(child) = child else {
                return Map::new();
            };
            match tokio::time::timeout(Duration::from_secs(2), child.wait_with_output()).await {
                Ok(Ok(out)) => ferb_mac_hands::permissions::fresh_answer(&out.stdout),
                _ => Map::new(),
            }
        })
    }

    fn running(&self) -> Vec<String> {
        ferb_mac_hands::native::apps::running()
            .into_iter()
            .map(|a| a.bundle_id)
            .collect()
    }

    fn click_through(&self, pid: i32, windows: &[i64]) -> bool {
        self.click.get().is_some_and(|f| f(pid, windows))
    }
}

/// selftest.py's two probes on the real Mac.
struct Probes;

#[allow(non_upper_case_globals)]
mod ffi {
    use std::ffi::c_void;
    pub type CFTypeRef = *const c_void;
    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        pub fn AXUIElementCreateApplication(pid: i32) -> CFTypeRef;
        pub fn AXUIElementCopyAttributeValue(
            element: CFTypeRef,
            attribute: CFTypeRef,
            value: *mut CFTypeRef,
        ) -> i32;
        pub fn CGEventTapCreate(
            tap: u32,
            place: u32,
            options: u32,
            events_of_interest: u64,
            callback: extern "C" fn(
                *const c_void,
                u32,
                *const c_void,
                *mut c_void,
            ) -> *const c_void,
            user_info: *mut c_void,
        ) -> CFTypeRef;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        pub fn CFRelease(cf: CFTypeRef);
        pub fn CFArrayGetCount(array: CFTypeRef) -> isize;
        pub fn CFGetTypeID(cf: CFTypeRef) -> usize;
        pub fn CFArrayGetTypeID() -> usize;
        pub fn CFMachPortInvalidate(port: CFTypeRef);
        pub fn CFStringCreateWithBytes(
            alloc: CFTypeRef,
            bytes: *const u8,
            len: isize,
            encoding: u32,
            external: u8,
        ) -> CFTypeRef;
    }
    pub const kCFStringEncodingUTF8: u32 = 0x0800_0100;
    pub const kCGSessionEventTap: u32 = 1;
    pub const kCGHeadInsertEventTap: u32 = 0;
    pub const kCGEventTapOptionListenOnly: u32 = 1;
    pub const kCGEventMouseMoved: u32 = 5;
}

extern "C" fn pass(
    _proxy: *const std::ffi::c_void,
    _kind: u32,
    event: *const std::ffi::c_void,
    _refcon: *mut std::ffi::c_void,
) -> *const std::ffi::c_void {
    event
}

impl ferb_device::selftest::Probes for Probes {
    fn dock_children(&mut self) -> Result<usize, Option<i32>> {
        let Some(dock) = ferb_mac_hands::native::apps::running()
            .into_iter()
            .find(|a| a.bundle_id == "com.apple.dock")
        else {
            return Err(None);
        };
        let name = b"AXChildren";
        // SAFETY: plain CF calls on objects this function creates and releases once each; the attribute's value is
        // checked to be an array before it is counted.
        unsafe {
            let element = ffi::AXUIElementCreateApplication(dock.pid);
            if element.is_null() {
                return Err(None);
            }
            let attribute = ffi::CFStringCreateWithBytes(
                std::ptr::null(),
                name.as_ptr(),
                name.len() as isize,
                ffi::kCFStringEncodingUTF8,
                0,
            );
            let mut value: ffi::CFTypeRef = std::ptr::null();
            let err = ffi::AXUIElementCopyAttributeValue(element, attribute, &mut value);
            ffi::CFRelease(attribute);
            ffi::CFRelease(element);
            if err != 0 {
                return Err(Some(err));
            }
            let n = if !value.is_null() && ffi::CFGetTypeID(value) == ffi::CFArrayGetTypeID() {
                ffi::CFArrayGetCount(value).max(0) as usize
            } else {
                0
            };
            if !value.is_null() {
                ffi::CFRelease(value);
            }
            Ok(n)
        }
    }

    fn listen_only_tap(&mut self) -> bool {
        // SAFETY: a listen-only tap for mouse moves is created and at once invalidated and released; it is never
        // added to a run loop, so its callback never runs and no input is seen or posted.
        unsafe {
            let tap = ffi::CGEventTapCreate(
                ffi::kCGSessionEventTap,
                ffi::kCGHeadInsertEventTap,
                ffi::kCGEventTapOptionListenOnly,
                1u64 << ffi::kCGEventMouseMoved,
                pass,
                std::ptr::null_mut(),
            );
            if tap.is_null() {
                return false;
            }
            ffi::CFMachPortInvalidate(tap);
            ffi::CFRelease(tap);
            true
        }
    }
}
