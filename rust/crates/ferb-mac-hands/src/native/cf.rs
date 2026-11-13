//! CoreFoundation values as the logic reads them.

use objc2_application_services::{AXUIElement, AXValue, AXValueType};
#[allow(deprecated)]
use objc2_core_foundation::{
    CFArray, CFBoolean, CFDictionary, CFNumber, CFRange, CFRetained, CFString, CFType,
    CFURLGetString, CFURLPathStyle, CGPoint, CGSize, Type, CFURL,
};
use std::ffi::c_void;
use std::ptr::NonNull;

use crate::ax::{AxValue, El};

/// An AXUIElement held by the daemon.
pub struct AxEl(pub CFRetained<AXUIElement>);

// SAFETY: AXUIElementRef is a CoreFoundation object: retain and release are thread-safe, and the Accessibility API
// may be called with it from any thread (the Python build calls it from worker threads the same way).
unsafe impl Send for AxEl {}
// SAFETY: as above; nothing in an AXUIElementRef is mutated through a shared reference on our side.
unsafe impl Sync for AxEl {}

pub fn el_of(e: CFRetained<AXUIElement>) -> El {
    El::new(AxEl(e))
}

pub fn ax_of(e: &El) -> Option<&AXUIElement> {
    e.get::<AxEl>().map(|a| &*a.0)
}

/// One attribute value as PyObjC would hand it over.
pub fn value_of(v: &CFType) -> AxValue {
    if let Some(s) = v.downcast_ref::<CFString>() {
        return AxValue::Str(s.to_string());
    }
    if let Some(b) = v.downcast_ref::<CFBoolean>() {
        return AxValue::Bool(b.as_bool());
    }
    if let Some(n) = v.downcast_ref::<CFNumber>() {
        if n.is_float_type() {
            return n.as_f64().map(AxValue::Float).unwrap_or(AxValue::None);
        }
        return n.as_i64().map(AxValue::Int).unwrap_or(AxValue::None);
    }
    if let Some(e) = v.downcast_ref::<AXUIElement>() {
        return AxValue::El(el_of(e.retain()));
    }
    if let Some(a) = v.downcast_ref::<CFArray>() {
        let mut out = vec![];
        for i in 0..a.count() {
            // SAFETY: `i` is within the array's count; the item is a CF object the array keeps alive meanwhile.
            let item = unsafe { a.value_at_index(i) } as *const CFType;
            if let Some(item) = NonNull::new(item as *mut CFType) {
                // SAFETY: a non-null pointer to a CF object owned by the array.
                out.push(value_of(unsafe { item.as_ref() }));
            }
        }
        return AxValue::List(out);
    }
    if let Some(u) = v.downcast_ref::<CFURL>() {
        #[allow(deprecated)]
        let absolute = u
            .absolute_url()
            .and_then(|a| CFURLGetString(&a).map(|s| s.to_string()))
            .unwrap_or_default();
        let path = if absolute.starts_with("file:") {
            u.file_system_path(CFURLPathStyle::CFURLPOSIXPathStyle)
                .map(|p| p.to_string())
        } else {
            None
        };
        return AxValue::Url { absolute, path };
    }
    if let Some(x) = v.downcast_ref::<AXValue>() {
        // SAFETY: AXValueGetType reads the value's own type tag.
        let t = unsafe { x.r#type() };
        if t == AXValueType::CGPoint {
            let mut p = CGPoint { x: 0.0, y: 0.0 };
            // SAFETY: the type is CGPoint, so a CGPoint-sized out buffer is what AXValueGetValue writes.
            if unsafe { x.value(t, NonNull::from(&mut p).cast::<c_void>()) } {
                return AxValue::Point(p.x, p.y);
            }
        } else if t == AXValueType::CGSize {
            let mut s = CGSize {
                width: 0.0,
                height: 0.0,
            };
            // SAFETY: the type is CGSize, so a CGSize-sized out buffer.
            if unsafe { x.value(t, NonNull::from(&mut s).cast::<c_void>()) } {
                return AxValue::Size(s.width, s.height);
            }
        } else if t == AXValueType::CFRange {
            let mut r = CFRange {
                location: 0,
                length: 0,
            };
            // SAFETY: the type is CFRange, so a CFRange-sized out buffer.
            if unsafe { x.value(t, NonNull::from(&mut r).cast::<c_void>()) } {
                return AxValue::Range(r.location as i64, r.length as i64);
            }
        }
        return AxValue::None;
    }
    AxValue::None
}

/// A dictionary of string keys, read by key.
pub fn dict_get(d: &CFDictionary, key: &CFString) -> Option<CFRetained<CFType>> {
    // SAFETY: the window server's and the session's dictionaries hold CFString keys and CF object values.
    let typed: &CFDictionary<CFString, CFType> = unsafe { d.cast_unchecked() };
    typed.get(key)
}

pub fn num(v: Option<CFRetained<CFType>>) -> Option<f64> {
    let v = v?;
    let n = v.downcast_ref::<CFNumber>()?;
    n.as_f64()
}

pub fn text(v: Option<CFRetained<CFType>>) -> Option<String> {
    let v = v?;
    v.downcast_ref::<CFString>().map(|s| s.to_string())
}

pub fn boolean(v: Option<CFRetained<CFType>>) -> Option<bool> {
    let v = v?;
    if let Some(b) = v.downcast_ref::<CFBoolean>() {
        return Some(b.as_bool());
    }
    v.downcast_ref::<CFNumber>()
        .and_then(|n| n.as_i64())
        .map(|n| n != 0)
}
