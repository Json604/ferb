//! The C functions perception calls, as Apple's headers declare them (HIServices/AXUIElement.h, AXObserver.h,
//! CoreGraphics/CGEvent.h, CGSession.h, CGDisplayConfiguration.h, CGWindow.h, CoreServices/FSEvents.h,
//! HIToolbox/CarbonEvents.h, libproc.h).

#![allow(non_upper_case_globals, non_snake_case)]

use std::ffi::c_void;

use core_foundation_sys::array::CFArrayRef;
use core_foundation_sys::base::{CFAllocatorRef, CFIndex, CFTypeID, CFTypeRef};
use core_foundation_sys::dictionary::CFDictionaryRef;
use core_foundation_sys::mach_port::CFMachPortRef;
use core_foundation_sys::runloop::{CFRunLoopRef, CFRunLoopSourceRef};
use core_foundation_sys::string::CFStringRef;

pub type AXUIElementRef = *const c_void;
pub type AXObserverRef = *const c_void;
pub type AXError = i32;
pub type AXObserverCallback = unsafe extern "C" fn(
    observer: AXObserverRef,
    element: AXUIElementRef,
    notification: CFStringRef,
    refcon: *mut c_void,
);

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    pub fn AXUIElementCreateSystemWide() -> AXUIElementRef;
    /// Whether this process is trusted for Accessibility; never prompts (a TCC preflight).
    pub fn AXIsProcessTrusted() -> u8;
    pub fn AXUIElementCreateApplication(pid: libc::pid_t) -> AXUIElementRef;
    pub fn AXUIElementCopyAttributeValue(
        el: AXUIElementRef,
        attr: CFStringRef,
        value: *mut CFTypeRef,
    ) -> AXError;
    pub fn AXUIElementGetPid(el: AXUIElementRef, pid: *mut libc::pid_t) -> AXError;
    pub fn AXUIElementSetMessagingTimeout(el: AXUIElementRef, timeout: f32) -> AXError;
    pub fn AXUIElementGetTypeID() -> CFTypeID;
    pub fn AXObserverCreate(
        pid: libc::pid_t,
        callback: AXObserverCallback,
        out: *mut AXObserverRef,
    ) -> AXError;
    pub fn AXObserverAddNotification(
        observer: AXObserverRef,
        element: AXUIElementRef,
        notification: CFStringRef,
        refcon: *mut c_void,
    ) -> AXError;
    pub fn AXObserverGetRunLoopSource(observer: AXObserverRef) -> CFRunLoopSourceRef;
    pub fn AXValueGetValue(value: CFTypeRef, kind: u32, out: *mut c_void) -> u8;
    pub fn AXValueGetTypeID() -> CFTypeID;
}

/// AXValueType: a CGPoint and a CGSize.
pub const kAXValueCGPointType: u32 = 1;
pub const kAXValueCGSizeType: u32 = 2;

pub type CGEventRef = *const c_void;
pub type CGEventTapProxy = *const c_void;
pub type CGEventTapCallBack = unsafe extern "C" fn(
    proxy: CGEventTapProxy,
    kind: u32,
    event: CGEventRef,
    user: *mut c_void,
) -> CGEventRef;

/// CGEventTapLocation, CGEventTapPlacement, CGEventTapOptions, CGEventField, CGEventType values.
pub const kCGSessionEventTap: u32 = 1;
pub const kCGHeadInsertEventTap: u32 = 0;
pub const kCGEventTapOptionListenOnly: u32 = 1;
pub const kCGEventSourceUserData: u32 = 42;
pub const kCGEventLeftMouseDown: u32 = 1;
pub const kCGEventLeftMouseUp: u32 = 2;
pub const kCGEventRightMouseDown: u32 = 3;
pub const kCGEventMouseMoved: u32 = 5;
pub const kCGEventLeftMouseDragged: u32 = 6;
pub const kCGEventKeyDown: u32 = 10;
pub const kCGEventKeyUp: u32 = 11;
pub const kCGEventFlagsChanged: u32 = 12;
pub const kCGEventScrollWheel: u32 = 22;
pub const kCGEventTapDisabledByTimeout: u32 = 0xFFFF_FFFE;
pub const kCGEventTapDisabledByUserInput: u32 = 0xFFFF_FFFF;
/// CGWindowListOption.
pub const kCGWindowListOptionOnScreenOnly: u32 = 1 << 0;
pub const kCGWindowListExcludeDesktopElements: u32 = 1 << 4;

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    pub fn CGEventTapCreate(
        tap: u32,
        place: u32,
        options: u32,
        events_of_interest: u64,
        callback: CGEventTapCallBack,
        user_info: *mut c_void,
    ) -> CFMachPortRef;
    pub fn CGEventTapEnable(tap: CFMachPortRef, enable: bool);
    pub fn CGEventGetIntegerValueField(event: CGEventRef, field: u32) -> i64;
    pub fn CGEventGetFlags(event: CGEventRef) -> u64;
    pub fn CGEventKeyboardGetUnicodeString(
        event: CGEventRef,
        max: usize,
        actual: *mut usize,
        chars: *mut u16,
    );
    pub fn CGSessionCopyCurrentDictionary() -> CFDictionaryRef;
    pub fn CGMainDisplayID() -> u32;
    pub fn CGDisplayIsAsleep(display: u32) -> u32;
    pub fn CGWindowListCopyWindowInfo(option: u32, relative_to: u32) -> CFArrayRef;
    pub static kCGWindowNumber: CFStringRef;
    pub static kCGWindowOwnerPID: CFStringRef;
    pub static kCGWindowBounds: CFStringRef;
    pub static kCGWindowName: CFStringRef;
    pub static kCGWindowLayer: CFStringRef;
    pub static kCGWindowAlpha: CFStringRef;
}

pub type FSEventStreamRef = *mut c_void;
pub type FSEventStreamCallback = unsafe extern "C" fn(
    stream: FSEventStreamRef,
    info: *mut c_void,
    count: usize,
    paths: *mut c_void,
    flags: *const u32,
    ids: *const u64,
);

#[repr(C)]
pub struct FSEventStreamContext {
    pub version: CFIndex,
    pub info: *mut c_void,
    pub retain: *const c_void,
    pub release: *const c_void,
    pub copy_description: *const c_void,
}

pub const kFSEventStreamEventIdSinceNow: u64 = 0xFFFF_FFFF_FFFF_FFFF;
pub const kFSEventStreamCreateFlagUseCFTypes: u32 = 0x1;
pub const kFSEventStreamCreateFlagNoDefer: u32 = 0x2;
pub const kFSEventStreamCreateFlagFileEvents: u32 = 0x10;

#[link(name = "CoreServices", kind = "framework")]
unsafe extern "C" {
    pub fn FSEventStreamCreate(
        allocator: CFAllocatorRef,
        callback: FSEventStreamCallback,
        context: *mut FSEventStreamContext,
        paths: CFArrayRef,
        since_when: u64,
        latency: f64,
        flags: u32,
    ) -> FSEventStreamRef;
    pub fn FSEventStreamScheduleWithRunLoop(
        stream: FSEventStreamRef,
        run_loop: CFRunLoopRef,
        mode: CFStringRef,
    );
    pub fn FSEventStreamStart(stream: FSEventStreamRef) -> u8;
    pub fn FSEventStreamStop(stream: FSEventStreamRef);
    pub fn FSEventStreamInvalidate(stream: FSEventStreamRef);
    pub fn FSEventStreamRelease(stream: FSEventStreamRef);
}

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    /// Secure Event Input is on (a password field or a terminal's Secure Keyboard Entry).
    pub fn IsSecureEventInputEnabled() -> u8;
}
