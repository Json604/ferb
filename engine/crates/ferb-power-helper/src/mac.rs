//! The helper's Mac side: IOKit's sleep switch and scheduled wakes (root), the lease's marker file, and the caller's
//! signature (only ferbd, signed by the helper's own certificate).

use std::ffi::{c_char, c_void, CString};
use std::path::PathBuf;

use ferb_platform_macos::security::{own_leaf_sha1, Requirement, SecurityFramework};
use ferb_rpc::app_peer::{AuditToken, Security};

use crate::{helper, Marker, RootPower};

type CFTypeRef = *const c_void;
type IOReturn = i32;

const UTF8: u32 = 0x0800_0100;
/// Seconds between 1970 and 2001 (CFAbsoluteTime's origin).
const CF_EPOCH: f64 = 978_307_200.0;
/// The id FERB's wakes are scheduled under (`scheduledby`): only these are listed or cancelled.
pub const WAKE_OWNER: &str = "com.ferb.FERB";

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    static kCFBooleanTrue: CFTypeRef;
    static kCFBooleanFalse: CFTypeRef;
    fn CFStringCreateWithCString(alloc: CFTypeRef, s: *const c_char, encoding: u32) -> CFTypeRef;
    fn CFRelease(cf: CFTypeRef);
    fn CFDateCreate(alloc: CFTypeRef, at: f64) -> CFTypeRef;
    fn CFDateGetAbsoluteTime(date: CFTypeRef) -> f64;
    fn CFArrayGetCount(a: CFTypeRef) -> isize;
    fn CFArrayGetValueAtIndex(a: CFTypeRef, i: isize) -> CFTypeRef;
    fn CFDictionaryGetValue(d: CFTypeRef, key: CFTypeRef) -> CFTypeRef;
    fn CFEqual(a: CFTypeRef, b: CFTypeRef) -> u8;
    fn CFGetTypeID(cf: CFTypeRef) -> usize;
    fn CFDateGetTypeID() -> usize;
}

#[link(name = "IOKit", kind = "framework")]
extern "C" {
    fn IOPMSetSystemPowerSetting(key: CFTypeRef, value: CFTypeRef) -> IOReturn;
    fn IOPMSchedulePowerEvent(time: CFTypeRef, id: CFTypeRef, kind: CFTypeRef) -> IOReturn;
    fn IOPMCancelScheduledPowerEvent(time: CFTypeRef, id: CFTypeRef, kind: CFTypeRef) -> IOReturn;
    fn IOPMCopyScheduledPowerEvents() -> CFTypeRef;
}

struct Owned(CFTypeRef);

impl Owned {
    fn new(p: CFTypeRef) -> Option<Owned> {
        (!p.is_null()).then_some(Owned(p))
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY: a CF object this code owns, released once.
        unsafe { CFRelease(self.0) }
    }
}

fn cfstr(s: &str) -> Result<Owned, String> {
    let c = CString::new(s).map_err(|e| e.to_string())?;
    // SAFETY: a NUL-terminated UTF-8 string; Create rule.
    Owned::new(unsafe { CFStringCreateWithCString(std::ptr::null(), c.as_ptr(), UTF8) })
        .ok_or_else(|| "couldn't make a string".to_string())
}

fn date(at: i64) -> Result<Owned, String> {
    // SAFETY: a plain value; Create rule.
    Owned::new(unsafe { CFDateCreate(std::ptr::null(), at as f64 - CF_EPOCH) })
        .ok_or_else(|| "couldn't make a date".to_string())
}

fn io(r: IOReturn, what: &str) -> Result<(), String> {
    if r == 0 {
        Ok(())
    } else {
        Err(format!("macOS refused {what} (IOReturn {r:#x})"))
    }
}

/// IOKit as root.
pub struct Iokit;

impl RootPower for Iokit {
    fn set_sleep_disabled(&self, on: bool) -> Result<(), String> {
        let key = cfstr("SleepDisabled")?;
        // SAFETY: the framework's boolean constants; a live key string; root (the program checks it at start).
        let r = unsafe {
            IOPMSetSystemPowerSetting(key.0, if on { kCFBooleanTrue } else { kCFBooleanFalse })
        };
        io(r, "the sleep switch")
    }

    fn scheduled(&self) -> Result<Vec<i64>, String> {
        // SAFETY: Copy rule; null when nothing is scheduled.
        let Some(all) = Owned::new(unsafe { IOPMCopyScheduledPowerEvents() }) else {
            return Ok(Vec::new());
        };
        let (by, time, owner) = (cfstr("scheduledby")?, cfstr("time")?, cfstr(WAKE_OWNER)?);
        let mut out = Vec::new();
        // SAFETY: a CFArray of CFDictionaries (IOPMLib.h); every value read is borrowed.
        unsafe {
            for i in 0..CFArrayGetCount(all.0) {
                let ev = CFArrayGetValueAtIndex(all.0, i);
                let who = CFDictionaryGetValue(ev, by.0);
                let at = CFDictionaryGetValue(ev, time.0);
                if who.is_null() || at.is_null() || CFEqual(who, owner.0) == 0 {
                    continue;
                }
                if CFGetTypeID(at) == CFDateGetTypeID() {
                    out.push((CFDateGetAbsoluteTime(at) + CF_EPOCH).round() as i64);
                }
            }
        }
        out.sort_unstable();
        Ok(out)
    }

    fn schedule(&self, at: i64) -> Result<(), String> {
        let (t, id, kind) = (date(at)?, cfstr(WAKE_OWNER)?, cfstr("wake")?);
        // SAFETY: live CF objects; root.
        io(
            unsafe { IOPMSchedulePowerEvent(t.0, id.0, kind.0) },
            "the wake",
        )
    }

    fn cancel(&self, at: i64) -> Result<(), String> {
        let (t, id, kind) = (date(at)?, cfstr(WAKE_OWNER)?, cfstr("wake")?);
        // SAFETY: live CF objects; root. Only an event scheduled under FERB's own id matches.
        io(
            unsafe { IOPMCancelScheduledPowerEvent(t.0, id.0, kind.0) },
            "cancelling the wake",
        )
    }
}

/// The lease's marker: a root-owned file in /var/db.
pub struct FileMarker(pub PathBuf);

impl FileMarker {
    pub fn system() -> FileMarker {
        FileMarker(PathBuf::from("/var/db/com.ferb.power.lease"))
    }
}

impl Marker for FileMarker {
    fn exists(&self) -> bool {
        self.0.exists()
    }
    fn set(&self, on: bool) -> Result<(), String> {
        if on {
            std::fs::write(&self.0, b"FERB holds the sleep switch\n").map_err(|e| e.to_string())
        } else {
            match std::fs::remove_file(&self.0) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(e.to_string()),
            }
        }
    }
}

/// The only caller the helper answers: ferbd's identifier, signed by the helper's own leaf certificate. `None` for an
/// ad-hoc-signed helper (a development build): then nobody is answered.
pub fn caller_requirement_text(leaf_sha1: Option<&str>) -> Option<String> {
    let leaf = leaf_sha1.filter(|h| h.len() == 40 && h.bytes().all(|b| b.is_ascii_hexdigit()))?;
    Some(format!(
        r#"identifier "{}" and certificate leaf = H"{}""#,
        helper::CALLER_ID,
        leaf
    ))
}

/// The caller check, made once at start from this program's own signature.
pub struct Callers {
    security: SecurityFramework,
    requirement: Option<Requirement>,
}

impl Callers {
    pub fn new() -> Callers {
        let security = SecurityFramework;
        let requirement = caller_requirement_text(own_leaf_sha1().as_deref())
            .and_then(|t| security.requirement(&t));
        Callers {
            security,
            requirement,
        }
    }

    /// Whether the process behind this audit token is ferbd, signed as the helper is.
    pub fn allowed(&self, token: Option<&AuditToken>) -> bool {
        match (token, &self.requirement) {
            (Some(t), Some(r)) => self.security.satisfies(t, r),
            _ => false,
        }
    }
}

impl Default for Callers {
    fn default() -> Self {
        Callers::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // decision: APP-986
    // decision: DMN-610
    #[test]
    fn only_ferbd_signed_by_the_helper_s_own_certificate_is_answered() {
        let leaf = "0123456789ABCDEF0123456789ABCDEF01234567";
        assert_eq!(
            caller_requirement_text(Some(leaf)).unwrap(),
            format!(r#"identifier "com.ferb.daemon" and certificate leaf = H"{leaf}""#)
        );
        assert_eq!(caller_requirement_text(None), None, "ad-hoc: nobody");
        assert_eq!(caller_requirement_text(Some("\" or anchor apple")), None);
        // this test program is not ferbd: refused (its own audit token, through the real Security framework)
        let callers = Callers {
            security: SecurityFramework,
            requirement: SecurityFramework
                .requirement(&caller_requirement_text(Some(leaf)).unwrap()),
        };
        let (a, _b) = std::os::unix::net::UnixStream::pair().unwrap();
        let token = ferb_platform_macos::peer::peer_token(&a);
        assert!(token.is_some());
        assert!(!callers.allowed(token.as_ref()));
        assert!(!callers.allowed(None));
        assert!(!Callers {
            security: SecurityFramework,
            requirement: None
        }
        .allowed(token.as_ref()));
    }

    #[test]
    fn the_marker_file_comes_and_goes() {
        let dir = std::env::temp_dir().join(format!("ferb-power-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let m = FileMarker(dir.join("lease"));
        assert!(!m.exists());
        m.set(true).unwrap();
        assert!(m.exists());
        m.set(false).unwrap();
        m.set(false).unwrap();
        assert!(!m.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
