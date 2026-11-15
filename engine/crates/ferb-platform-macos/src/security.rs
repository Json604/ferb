//! The few Security and CoreFoundation calls the FERB.app check needs. Port of `app_peer.py:36-101`
//! (`_Security`, there through ctypes). Everything that can't be made or checked is a "no".

use std::ffi::{c_char, c_void, CString};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::ptr;

use ferb_rpc::app_peer::{AuditToken, Security};

type CFTypeRef = *const c_void;
type OSStatus = i32;

const UTF8: u32 = 0x0800_0100; // kCFStringEncodingUTF8
const NO_FLAGS: u32 = 0; // kSecCSDefaultFlags
const SINT32: isize = 3; // kCFNumberSInt32Type

/// A CoreFoundation structure this code only ever takes the address of.
#[repr(C)]
struct Opaque {
    _private: [u8; 0],
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    static kCFTypeDictionaryKeyCallBacks: Opaque;
    static kCFTypeDictionaryValueCallBacks: Opaque;

    fn CFDataCreate(allocator: CFTypeRef, bytes: *const u8, length: isize) -> CFTypeRef;
    fn CFNumberCreate(allocator: CFTypeRef, kind: isize, value: *const c_void) -> CFTypeRef;
    fn CFDictionaryCreate(
        allocator: CFTypeRef,
        keys: *const CFTypeRef,
        values: *const CFTypeRef,
        count: isize,
        key_callbacks: *const Opaque,
        value_callbacks: *const Opaque,
    ) -> CFTypeRef;
    fn CFStringCreateWithCString(
        allocator: CFTypeRef,
        text: *const c_char,
        encoding: u32,
    ) -> CFTypeRef;
    fn CFURLCreateFromFileSystemRepresentation(
        allocator: CFTypeRef,
        path: *const u8,
        length: isize,
        is_directory: u8,
    ) -> CFTypeRef;
    fn CFRelease(object: CFTypeRef);
}

#[link(name = "Security", kind = "framework")]
extern "C" {
    static kSecGuestAttributeAudit: CFTypeRef;
    static kSecGuestAttributePid: CFTypeRef;

    fn SecCodeCopyGuestWithAttributes(
        host: CFTypeRef,
        attributes: CFTypeRef,
        flags: u32,
        guest: *mut CFTypeRef,
    ) -> OSStatus;
    fn SecRequirementCreateWithString(
        text: CFTypeRef,
        flags: u32,
        requirement: *mut CFTypeRef,
    ) -> OSStatus;
    fn SecCodeCheckValidity(code: CFTypeRef, flags: u32, requirement: CFTypeRef) -> OSStatus;
    fn SecCodeCopyStaticCode(code: CFTypeRef, flags: u32, static_code: *mut CFTypeRef) -> OSStatus;
    fn SecStaticCodeCheckValidity(
        static_code: CFTypeRef,
        flags: u32,
        requirement: CFTypeRef,
    ) -> OSStatus;
    fn SecStaticCodeCreateWithPath(path: CFTypeRef, flags: u32, code: *mut CFTypeRef) -> OSStatus;
    fn SecCodeCopyDesignatedRequirement(
        code: CFTypeRef,
        flags: u32,
        requirement: *mut CFTypeRef,
    ) -> OSStatus;
}

/// An object this code owns one reference to (a Create or Copy result), released when dropped.
struct Owned(CFTypeRef);

impl Owned {
    /// `None` for the null a failed Create returns.
    fn new(object: CFTypeRef) -> Option<Owned> {
        // not `then_some`: that would build (and drop, releasing null) an `Owned` for the null too
        if object.is_null() {
            None
        } else {
            Some(Owned(object))
        }
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY: `self.0` is non-null (`Owned::new`) and this is the one reference a Create/Copy call gave
        // us, released exactly once.
        unsafe { CFRelease(self.0) }
    }
}

/// A compiled code requirement (`SecRequirementRef`).
pub struct Requirement(Owned);

// SAFETY: a SecRequirement is immutable once created, and CoreFoundation's retain/release are thread-safe; the
// object is only read (SecCodeCheckValidity) and released, so it may move to and be used from another thread.
unsafe impl Send for Requirement {}
// SAFETY: as above; nothing mutates the requirement through a shared reference.
unsafe impl Sync for Requirement {}

/// The macOS Security framework.
#[derive(Clone, Copy, Debug, Default)]
pub struct SecurityFramework;

impl Security for SecurityFramework {
    type Requirement = Requirement;

    // app_peer.py:67-73
    fn requirement(&self, text: &str) -> Option<Requirement> {
        let text = CString::new(text).ok()?;
        // SAFETY: `text` is a NUL-terminated string that lives across the call; a null allocator is the
        // default one. The result is null or an owned CFString.
        let string =
            Owned::new(unsafe { CFStringCreateWithCString(ptr::null(), text.as_ptr(), UTF8) })?;
        let mut requirement: CFTypeRef = ptr::null();
        // SAFETY: `string` is a live CFString and `requirement` a valid place for one pointer. On success the
        // framework stores an owned SecRequirement there.
        let status =
            unsafe { SecRequirementCreateWithString(string.0, NO_FLAGS, &mut requirement) };
        let requirement = Owned::new(requirement)?; // owned from here whatever the status says
        (status == 0).then_some(Requirement(requirement))
    }

    // app_peer.py:75-87
    fn designated(&self, bundle: &Path) -> Option<Requirement> {
        let raw = bundle.as_os_str().as_bytes();
        // SAFETY: `raw` is `raw.len()` readable bytes; the call copies them. The result is null or an owned
        // CFURL.
        let url = Owned::new(unsafe {
            CFURLCreateFromFileSystemRepresentation(
                ptr::null(),
                raw.as_ptr(),
                raw.len() as isize,
                1,
            )
        })?;
        let mut code: CFTypeRef = ptr::null();
        // SAFETY: `url` is a live CFURL and `code` a valid place for one pointer; on success it receives an
        // owned SecStaticCode.
        let status = unsafe { SecStaticCodeCreateWithPath(url.0, NO_FLAGS, &mut code) };
        let code = Owned::new(code)?;
        if status != 0 {
            return None;
        }
        let mut requirement: CFTypeRef = ptr::null();
        // SAFETY: `code` is a live SecStaticCode and `requirement` a valid place for one pointer; on success
        // it receives an owned SecRequirement.
        let status =
            unsafe { SecCodeCopyDesignatedRequirement(code.0, NO_FLAGS, &mut requirement) };
        let requirement = Owned::new(requirement)?;
        (status == 0).then_some(Requirement(requirement))
    }

    // app_peer.py:89-101
    fn satisfies(&self, token: &AuditToken, requirement: &Requirement) -> bool {
        // SAFETY: `token` is `token.len()` readable bytes, copied by the call. The result is null or an owned
        // CFData.
        let Some(data) =
            Owned::new(unsafe { CFDataCreate(ptr::null(), token.as_ptr(), token.len() as isize) })
        else {
            return false;
        };
        // SAFETY: reading a constant the Security framework exports; it is a CFString that lives as long as
        // the framework is loaded.
        let Some(code) = guest(unsafe { kSecGuestAttributeAudit }, &data) else {
            return false;
        };
        // SAFETY: `code` is a live SecCode and `requirement` a live SecRequirement, both only read.
        unsafe { SecCodeCheckValidity(code.0, NO_FLAGS, requirement.0 .0) == 0 }
    }
}

/// Whether the process `pid` runs code `requirement` accepts. Two checks, both with `--strict`'s validation
/// (kSecCSStrictValidate), as `codesign --verify --strict` checks the file:
/// - the running code as the kernel loaded it (SecCodeCheckValidity on the guest, "secure against attempts to
///   modify the file system source", SecCode.h): its code directory, so its hash, is the one the pin names;
/// - then every page of that code's file as it is now (SecStaticCodeCheckValidity on the guest's static
///   code), so a file whose pages were changed under an intact signature is refused before it is given
///   anything. A page changed after this is the kernel's to refuse as it is loaded (on this Mac an invalid
///   page kills the process).
///
/// The caller must be the unreaped parent of `pid`, so the number cannot name another process.
pub(crate) fn running_code_satisfies(pid: libc::pid_t, requirement: &Requirement) -> bool {
    const STRICT: u32 = 1 << 4; // kSecCSStrictValidate
    let pid: i32 = pid;
    // SAFETY: `pid` is a live i32 the call copies; kCFNumberSInt32Type says how to read it. The result is null
    // or an owned CFNumber.
    let Some(number) = Owned::new(unsafe {
        CFNumberCreate(ptr::null(), SINT32, (&raw const pid).cast::<c_void>())
    }) else {
        return false;
    };
    // SAFETY: reading a constant the Security framework exports, as above.
    let Some(code) = guest(unsafe { kSecGuestAttributePid }, &number) else {
        return false;
    };
    // SAFETY: `code` is a live SecCode and `requirement` a live SecRequirement, both only read.
    if unsafe { SecCodeCheckValidity(code.0, STRICT, requirement.0 .0) } != 0 {
        return false;
    }
    let mut on_disk: CFTypeRef = ptr::null();
    // SAFETY: `code` is a live SecCode and `on_disk` a valid place for one pointer; on success it receives an
    // owned SecStaticCode for the running code's file.
    let status = unsafe { SecCodeCopyStaticCode(code.0, NO_FLAGS, &mut on_disk) };
    let Some(on_disk) = Owned::new(on_disk) else {
        return false;
    };
    // SAFETY: `on_disk` is a live SecStaticCode and `requirement` a live SecRequirement, both only read.
    status == 0 && unsafe { SecStaticCodeCheckValidity(on_disk.0, STRICT, requirement.0 .0) } == 0
}

/// The running code the guest attribute `key` with `value` names.
fn guest(key: CFTypeRef, value: &Owned) -> Option<Owned> {
    let keys = [key];
    let values = [value.0];
    // SAFETY: `keys` and `values` each hold one valid CF object and the count is 1; the callback
    // structures are CoreFoundation's own constants for CF-typed keys and values, so the dictionary
    // retains what it holds. The result is null or an owned CFDictionary.
    let attributes = Owned::new(unsafe {
        CFDictionaryCreate(
            ptr::null(),
            keys.as_ptr(),
            values.as_ptr(),
            1,
            &raw const kCFTypeDictionaryKeyCallBacks,
            &raw const kCFTypeDictionaryValueCallBacks,
        )
    })?;
    let mut code: CFTypeRef = ptr::null();
    // SAFETY: a null host means the system itself; `attributes` is a live CFDictionary and `code` a valid
    // place for one pointer; on success it receives an owned SecCode for the process the attribute names.
    let status =
        unsafe { SecCodeCopyGuestWithAttributes(ptr::null(), attributes.0, NO_FLAGS, &mut code) };
    let code = Owned::new(code)?;
    (status == 0).then_some(code)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::peer::peer_token;
    use ferb_rpc::app_peer::DEV_REQUIREMENT;

    fn own_token() -> AuditToken {
        let (a, _b) = std::os::unix::net::UnixStream::pair().unwrap();
        peer_token(&a).unwrap()
    }

    #[test]
    fn a_requirement_compiles_or_it_is_none() {
        assert!(SecurityFramework.requirement(DEV_REQUIREMENT).is_some());
        assert!(SecurityFramework
            .requirement("this is not a requirement")
            .is_none());
        assert!(SecurityFramework
            .requirement("identifier \"a\0b\"")
            .is_none());
    }

    #[test]
    fn a_signed_bundle_has_a_designated_requirement_and_a_missing_one_has_none() {
        assert!(SecurityFramework
            .designated(Path::new("/System/Applications/Calculator.app"))
            .is_some());
        assert!(SecurityFramework
            .designated(Path::new("/nonexistent/FERB.app"))
            .is_none());
    }

    // decision: DMN-402
    #[test]
    fn this_test_process_is_not_an_apple_app_and_a_made_up_token_is_nobody() {
        let apple = SecurityFramework.requirement("anchor apple").unwrap();
        assert!(!SecurityFramework.satisfies(&own_token(), &apple));
        let calculator = SecurityFramework
            .designated(Path::new("/System/Applications/Calculator.app"))
            .unwrap();
        assert!(!SecurityFramework.satisfies(&own_token(), &calculator));
        let dev = SecurityFramework.requirement(DEV_REQUIREMENT).unwrap();
        assert!(!SecurityFramework.satisfies(&own_token(), &dev));
        assert!(!SecurityFramework.satisfies(&[0u8; 32], &dev));
        assert!(!SecurityFramework.satisfies(&[0xff; 32], &dev));
    }
}
