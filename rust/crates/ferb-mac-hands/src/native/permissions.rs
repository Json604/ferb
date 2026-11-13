//! TCC grant reads on the real Mac (permissions.py): Accessibility, Screen Recording, Input Monitoring (IOHID), Full
//! Disk Access (a file only it opens), Automation (AEDeterminePermissionToAutomateTarget). A check never prompts;
//! `request` and `automation(…, ask: true)` are the explicit setup calls the owner's Grant makes.

use std::ffi::c_void;
use std::path::PathBuf;

use objc2_application_services::{
    kAXTrustedCheckOptionPrompt, AXIsProcessTrustedWithOptions, AXUIElement,
};
use objc2_core_foundation::{CFBoolean, CFDictionary, CFString, CFType};

use super::windows;
use crate::permissions::{
    ae_status, confirmed, full_disk_access as fda, hid_status, Probe, Status,
};

#[link(name = "IOKit", kind = "framework")]
extern "C" {
    fn IOHIDCheckAccess(request: u32) -> u32;
    fn IOHIDRequestAccess(request: u32) -> bool;
}

#[repr(C)]
struct AEDesc {
    descriptor_type: u32,
    data_handle: *mut c_void,
}

#[link(name = "CoreServices", kind = "framework")]
extern "C" {
    fn AECreateDesc(type_code: u32, data: *const c_void, size: isize, result: *mut AEDesc) -> i16;
    fn AEDisposeDesc(desc: *mut AEDesc) -> i16;
    fn AEDeterminePermissionToAutomateTarget(
        target: *const AEDesc,
        event_class: u32,
        event_id: u32,
        ask: u8,
    ) -> i32;
}

const LISTEN_EVENT: u32 = 1; // kIOHIDRequestTypeListenEvent
const AX_API_DISABLED: i32 = -25211;

fn fourcc(s: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*s)
}

/// AXIsProcessTrustedWithOptions({kAXTrustedCheckOptionPrompt: prompt}).
pub fn ax_trusted(prompt: bool) -> bool {
    objc2::rc::autoreleasepool(|_| {
        // SAFETY: the exported option key.
        let key: &CFString = unsafe { kAXTrustedCheckOptionPrompt };
        let value = CFBoolean::new(prompt);
        let dict = CFDictionary::<CFString, CFType>::from_slices(&[key], &[value.as_ref()]);
        // SAFETY: a dictionary of the documented key and a CFBoolean value.
        unsafe { AXIsProcessTrustedWithOptions(Some(dict.as_opaque())) }
    })
}

/// permissions.py:39: a real Accessibility read of the focused app's role.
pub fn ax_read_works() -> Option<bool> {
    use crate::ax::AxSource;
    let src = super::ax::NativeAx::default();
    let system = src.system();
    let (err, focused) = src.raw_attr(&system, "AXFocusedApplication");
    if err == AX_API_DISABLED {
        return Some(false);
    }
    let focused = match (err, focused.as_el()) {
        (0, Some(f)) => f,
        _ => return None,
    };
    let (err, _) = src.raw_attr(&focused, "AXRole");
    let _ = std::marker::PhantomData::<AXUIElement>;
    if err == 0 {
        Some(true)
    } else if err == AX_API_DISABLED {
        Some(false)
    } else {
        None
    }
}

pub fn accessibility() -> Status {
    if confirmed(ax_trusted(false), ax_read_works) {
        Status::Granted
    } else {
        Status::Denied
    }
}

pub fn screen_recording() -> Status {
    let me = std::process::id() as i32;
    if confirmed(windows::preflight_screen_capture(), || {
        windows::other_app_window_title_readable(me)
    }) {
        Status::Granted
    } else {
        Status::Denied
    }
}

pub fn input_monitoring() -> Status {
    // SAFETY: IOHIDCheckAccess takes a request type and only reads TCC.
    hid_status(unsafe { IOHIDCheckAccess(LISTEN_EVENT) })
}

/// permissions.py:113: files only Full Disk Access opens, in order.
pub fn fda_probes(home: &str) -> Vec<PathBuf> {
    vec![
        PathBuf::from(home).join("Library/Application Support/com.apple.TCC/TCC.db"),
        PathBuf::from("/Library/Application Support/com.apple.TCC/TCC.db"),
    ]
}

pub fn full_disk_access() -> Status {
    let home = std::env::var("HOME").unwrap_or_default();
    fda(fda_probes(&home).into_iter().map(|p| {
        use std::io::Read;
        match std::fs::File::open(&p).and_then(|mut f| {
            let mut b = [0u8; 16];
            f.read(&mut b).map(|_| ())
        }) {
            Ok(()) => Probe::Read,
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => Probe::PermissionDenied,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Probe::NotFound,
            Err(_) => Probe::OtherError,
        }
    }))
}

/// permissions.py:167: Apple Events permission for this process -> bundle_id. `ask` shows macOS's prompt (setup or
/// the first script, after the owner's Yes); a check never prompts. Blocking: macOS can take long even without ask.
pub fn automation(bundle_id: &str, ask: bool) -> Status {
    let data = bundle_id.as_bytes();
    let mut desc = AEDesc {
        descriptor_type: 0,
        data_handle: std::ptr::null_mut(),
    };
    // SAFETY: `data` is valid for `len` bytes; `desc` is a valid out struct.
    if unsafe {
        AECreateDesc(
            fourcc(b"bund"),
            data.as_ptr().cast(),
            data.len() as isize,
            &mut desc,
        )
    } != 0
    {
        return Status::Unknown;
    }
    // SAFETY: `desc` was created above; the event class and id are the wildcard '****'.
    let rc = unsafe {
        AEDeterminePermissionToAutomateTarget(&desc, fourcc(b"****"), fourcc(b"****"), ask as u8)
    };
    // SAFETY: disposing the descriptor created above, once.
    unsafe { AEDisposeDesc(&mut desc) };
    ae_status(rc)
}

/// permissions.py:361: the owner's Grant in setup asks macOS for one grant.
pub fn request(permission: &str) -> Result<Status, String> {
    match permission {
        "accessibility" => {
            ax_trusted(true);
            Ok(accessibility())
        }
        "screen_recording" => {
            objc2_core_graphics::CGRequestScreenCaptureAccess();
            Ok(screen_recording())
        }
        "input_monitoring" => {
            // SAFETY: shows macOS's own prompt once; no other effect.
            unsafe { IOHIDRequestAccess(LISTEN_EVENT) };
            Ok(input_monitoring())
        }
        other => Err(format!(
            "'{other}' has no macOS request; add FERB in its System Settings pane"
        )),
    }
}

/// permissions.py:257.
pub fn check_all() -> serde_json::Value {
    serde_json::json!({"accessibility": accessibility().value(), "screen_recording": screen_recording().value(),
                       "input_monitoring": input_monitoring().value(), "full_disk_access": full_disk_access().value()})
}
