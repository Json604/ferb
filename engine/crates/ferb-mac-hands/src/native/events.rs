//! The events FERB posts (input.py `_post`, window_input.py): each CGEvent created from a private source, marked
//! with FERB's mark (the owner-input watcher tells FERB's events from the owner's by it), and posted to one process
//! (CGEventPostToPid), to one window of one process through the optional SkyLight route, or, only on the foreground
//! route the owner said Yes to, to the HID tap. Never posted from a test.

use std::ffi::{c_void, CString};
use std::sync::OnceLock;

use objc2_core_foundation::{CFRetained, CGPoint};
use objc2_core_graphics::{
    CGEvent, CGEventField, CGEventFlags, CGEventSource, CGEventSourceStateID, CGEventTapLocation,
    CGEventType, CGMouseButton, CGScrollEventUnit,
};

use crate::error::{HandError, Result};
use crate::input::Events;
use crate::keys::{Button, Event, Route};

/// sense/watchers/owner_input.py FERB_EVENT_MARK: "FERB".
pub const FERB_EVENT_MARK: i64 = 0x4645_5242;

type PostToPid = unsafe extern "C" fn(i32, *const c_void);
type SetField = unsafe extern "C" fn(*const c_void, u32, i64);
type SetLocation = unsafe extern "C" fn(*const c_void, f64, f64);

/// window_input.py's SkyLight transport: present only when every symbol resolves (private ABI).
#[derive(Clone, Copy)]
pub struct WindowRoute {
    post_to_pid: PostToPid,
    set_field: SetField,
    set_location: SetLocation,
}

// SAFETY: plain function pointers into a system framework that stays loaded for the process's life.
unsafe impl Send for WindowRoute {}
// SAFETY: as above.
unsafe impl Sync for WindowRoute {}

/// window_input.py:29: the route, or None when SkyLight or any of its three symbols is missing.
pub fn load_route() -> Option<WindowRoute> {
    static ROUTE: OnceLock<Option<WindowRoute>> = OnceLock::new();
    *ROUTE.get_or_init(|| {
        let path =
            CString::new("/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight").ok()?;
        // SAFETY: dlopen with a valid C string; the handle is kept for the process's life (never dlclosed).
        let lib = unsafe { libc::dlopen(path.as_ptr(), libc::RTLD_LAZY) };
        if lib.is_null() {
            return None;
        }
        let sym = |name: &str| -> Option<*mut c_void> {
            let c = CString::new(name).ok()?;
            // SAFETY: a valid handle and C string.
            let p = unsafe { libc::dlsym(lib, c.as_ptr()) };
            (!p.is_null()).then_some(p)
        };
        let (a, b, c) = (
            sym("SLEventPostToPid")?,
            sym("SLEventSetIntegerValueField")?,
            sym("SLEventSetWindowLocation")?,
        );
        // SAFETY: the symbols' signatures as Cua's MIT-licensed implementation and window_input.py declare them.
        unsafe {
            Some(WindowRoute {
                post_to_pid: std::mem::transmute::<*mut c_void, PostToPid>(a),
                set_field: std::mem::transmute::<*mut c_void, SetField>(b),
                set_location: std::mem::transmute::<*mut c_void, SetLocation>(c),
            })
        }
    })
}

/// CGEvents created and posted for real.
pub struct NativeEvents {
    pub route: Option<WindowRoute>,
}

impl NativeEvents {
    pub fn new() -> Self {
        NativeEvents {
            route: load_route(),
        }
    }
}

impl Default for NativeEvents {
    fn default() -> Self {
        Self::new()
    }
}

fn source() -> Option<CFRetained<CGEventSource>> {
    CGEventSource::new(CGEventSourceStateID::Private)
}

fn mouse(
    kind: CGEventType,
    at: (f64, f64),
    button: CGMouseButton,
    clicks: i64,
) -> Option<CFRetained<CGEvent>> {
    let s = source();
    let ev = CGEvent::new_mouse_event(s.as_deref(), kind, CGPoint { x: at.0, y: at.1 }, button)?;
    CGEvent::set_integer_value_field(Some(&ev), CGEventField::MouseEventClickState, clicks);
    Some(ev)
}

fn build(event: &Event) -> Option<CFRetained<CGEvent>> {
    match event {
        Event::Moved { at } => mouse(CGEventType::MouseMoved, *at, CGMouseButton::Left, 1),
        Event::Dragged { at } => mouse(CGEventType::LeftMouseDragged, *at, CGMouseButton::Left, 1),
        Event::Down { button, at, clicks } => match button {
            Button::Left => mouse(
                CGEventType::LeftMouseDown,
                *at,
                CGMouseButton::Left,
                *clicks,
            ),
            Button::Right => mouse(
                CGEventType::RightMouseDown,
                *at,
                CGMouseButton::Right,
                *clicks,
            ),
        },
        Event::Up { button, at, clicks } => match button {
            Button::Left => mouse(CGEventType::LeftMouseUp, *at, CGMouseButton::Left, *clicks),
            Button::Right => mouse(
                CGEventType::RightMouseUp,
                *at,
                CGMouseButton::Right,
                *clicks,
            ),
        },
        Event::Wheel {
            pixels,
            vertical,
            horizontal,
            at,
        } => {
            let s = source();
            let unit = if *pixels {
                CGScrollEventUnit::Pixel
            } else {
                CGScrollEventUnit::Line
            };
            let ev =
                CGEvent::new_scroll_wheel_event2(s.as_deref(), unit, 2, *vertical, *horizontal, 0)?;
            CGEvent::set_location(Some(&ev), CGPoint { x: at.0, y: at.1 });
            Some(ev)
        }
        Event::Char { ch, down } => {
            let s = source();
            let ev = CGEvent::new_keyboard_event(s.as_deref(), 0, *down)?;
            let mut units = [0u16; 2];
            // ENG-6: the UTF-16 length (Python passed the code-point count, sending half of an emoji)
            let n = ch.encode_utf16(&mut units).len();
            // SAFETY: `units` holds `n` UTF-16 code units for the call's duration.
            unsafe { CGEvent::keyboard_set_unicode_string(Some(&ev), n as _, units.as_ptr()) };
            Some(ev)
        }
        Event::Key { code, flags, down } => {
            let s = source();
            let ev = CGEvent::new_keyboard_event(s.as_deref(), *code, *down)?;
            CGEvent::set_flags(Some(&ev), CGEventFlags(*flags));
            Some(ev)
        }
    }
}

impl Events for NativeEvents {
    fn post(&self, event: &Event, route: &Route) -> Result<()> {
        objc2::rc::autoreleasepool(|_| {
            let ev =
                build(event).ok_or_else(|| HandError::Other("couldn't create the event".into()))?;
            CGEvent::set_integer_value_field(
                Some(&ev),
                CGEventField::EventSourceUserData,
                FERB_EVENT_MARK,
            );
            match route {
                Route::Hid => CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&ev)),
                Route::Process(pid) => CGEvent::post_to_pid(*pid, Some(&ev)),
                Route::Window { pid, window, local } => match &self.route {
                    Some(r) => {
                        let ptr = CFRetained::as_ptr(&ev).as_ptr() as *const c_void;
                        // SAFETY: a live CGEventRef and SkyLight's routing fields (window_input.py:21-26): owner pid
                        // (40), window number (51, 91, 92), FERB's mark (42); then its window-local point; posted once.
                        unsafe {
                            for (field, value) in [
                                (40u32, *pid as i64),
                                (51, *window as i64),
                                (91, *window as i64),
                                (92, *window as i64),
                                (42, FERB_EVENT_MARK),
                            ] {
                                (r.set_field)(ptr, field, value);
                            }
                            (r.set_location)(ptr, local.0, local.1);
                            (r.post_to_pid)(*pid, ptr);
                        }
                    }
                    None => CGEvent::post_to_pid(*pid, Some(&ev)),
                },
            }
            Ok(())
        })
    }
}
