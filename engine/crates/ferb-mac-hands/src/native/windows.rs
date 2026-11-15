//! The window server's list (CGWindowListCopyWindowInfo), the displays, and whether the screen is locked or asleep
//! (screenshot.py's Quartz reads). Read-only: nothing here changes a window.

use objc2::rc::autoreleasepool;
use objc2_core_foundation::{CFDictionary, CFRetained, CFString, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{
    kCGWindowAlpha, kCGWindowBounds, kCGWindowLayer, kCGWindowName, kCGWindowNumber,
    kCGWindowOwnerName, kCGWindowOwnerPID, CGDisplayIsAsleep, CGGetDisplaysWithPoint,
    CGGetOnlineDisplayList, CGPreflightScreenCaptureAccess, CGRectMakeWithDictionaryRepresentation,
    CGSessionCopyCurrentDictionary, CGWindowListCopyWindowInfo, CGWindowListOption,
};

use super::cf::{boolean, dict_get, num, text};
use crate::ax::Rect;
use crate::driver::Window;
use crate::shots::WindowList;

/// The window server's list on this Mac.
#[derive(Clone, Copy, Default)]
pub struct NativeWindows;

/// One window's facts with its owner's name (permissions.py's Screen Recording probe reads names).
pub struct Listed {
    pub window: Window,
    pub owner_name: String,
}

impl NativeWindows {
    fn read(&self, everywhere: bool, exclude_desktop: bool) -> Option<Vec<Listed>> {
        autoreleasepool(|_| {
            let mut opts = if everywhere {
                CGWindowListOption::OptionAll
            } else {
                CGWindowListOption::OptionOnScreenOnly
            };
            if exclude_desktop {
                opts |= CGWindowListOption::ExcludeDesktopElements;
            }
            let list = CGWindowListCopyWindowInfo(opts, 0)?;
            let mut out = vec![];
            for i in 0..list.count() {
                // SAFETY: `i` is below the count; each item is a CFDictionary the list keeps alive while we read it.
                let item = unsafe { list.value_at_index(i) } as *const CFDictionary;
                if item.is_null() {
                    continue;
                }
                // SAFETY: a non-null CFDictionary pointer owned by `list`, which outlives this loop body.
                let d: &CFDictionary = unsafe { &*item };
                // SAFETY: the window server's keys are CFStrings exported by CoreGraphics.
                let (k_num, k_pid, k_bounds, k_name, k_layer, k_alpha, k_owner) = unsafe {
                    (
                        kCGWindowNumber,
                        kCGWindowOwnerPID,
                        kCGWindowBounds,
                        kCGWindowName,
                        kCGWindowLayer,
                        kCGWindowAlpha,
                        kCGWindowOwnerName,
                    )
                };
                let Some(id) = num(dict_get(d, k_num)) else {
                    continue;
                };
                let mut rect = CGRect {
                    origin: CGPoint { x: 0.0, y: 0.0 },
                    size: CGSize {
                        width: 0.0,
                        height: 0.0,
                    },
                };
                if let Some(b) = dict_get(d, k_bounds) {
                    let bd: Option<CFRetained<CFDictionary>> = b.downcast::<CFDictionary>().ok();
                    // SAFETY: `rect` is a valid out pointer; the dictionary is the window server's bounds dictionary.
                    unsafe { CGRectMakeWithDictionaryRepresentation(bd.as_deref(), &mut rect) };
                }
                out.push(Listed {
                    window: Window {
                        id: id as i64,
                        pid: num(dict_get(d, k_pid)).map(|p| p as i32).unwrap_or(-1),
                        bounds: (
                            rect.origin.x,
                            rect.origin.y,
                            rect.size.width,
                            rect.size.height,
                        ),
                        title: text(dict_get(d, k_name)).unwrap_or_default(),
                        layer: num(dict_get(d, k_layer)).map(|l| l as i64).unwrap_or(0),
                        alpha: num(dict_get(d, k_alpha)).unwrap_or(1.0),
                        layer_known: num(dict_get(d, k_layer)).is_some(),
                    },
                    owner_name: text(dict_get(d, k_owner)).unwrap_or_default(),
                });
            }
            Some(out)
        })
    }

    /// Every window, desktop elements included (permissions' probe and accessibility.py `cg_windows`).
    pub fn list_all(&self, everywhere: bool) -> Option<Vec<Window>> {
        self.read(everywhere, false)
            .map(|l| l.into_iter().map(|x| x.window).collect())
    }

    pub fn listed(&self, everywhere: bool) -> Option<Vec<Listed>> {
        self.read(everywhere, false)
    }
}

impl WindowList for NativeWindows {
    fn list(&self, everywhere: bool) -> Option<Vec<Window>> {
        self.read(everywhere, true)
            .map(|l| l.into_iter().map(|x| x.window).collect())
    }
}

/// point.py:49: the display holding the rect's centre.
pub fn display_of(rect: Rect) -> Option<u32> {
    let p = CGPoint {
        x: rect.0 + rect.2 / 2.0,
        y: rect.1 + rect.3 / 2.0,
    };
    let (mut id, mut count) = (0u32, 0u32);
    // SAFETY: one display id slot and a count slot, both valid for writes.
    let err = unsafe { CGGetDisplaysWithPoint(p, 1, &mut id, &mut count) };
    (err.0 == 0 && count > 0).then_some(id)
}

/// screenshot.py:157 (HAND-437): "locked", "asleep", or None (also when it can't be told).
pub fn screen_off() -> Option<String> {
    autoreleasepool(|_| {
        if let Some(info) = CGSessionCopyCurrentDictionary() {
            if boolean(dict_get(
                &info,
                &CFString::from_static_str("CGSSessionScreenIsLocked"),
            )) == Some(true)
            {
                return Some("locked".to_string());
            }
        }
        let mut ids = [0u32; 16];
        let mut count = 0u32;
        // SAFETY: room for 16 display ids and a count.
        let err = unsafe { CGGetOnlineDisplayList(16, ids.as_mut_ptr(), &mut count) };
        if err.0 == 0
            && count > 0
            && ids[..count.min(16) as usize]
                .iter()
                .all(|d| CGDisplayIsAsleep(*d))
        {
            return Some("asleep".to_string());
        }
        None
    })
}

/// permissions.py:52: another process's window title is listed only with Screen Recording.
pub fn other_app_window_title_readable(me: i32) -> Option<bool> {
    title_readable_in(me, &NativeWindows.listed(true).unwrap_or_default())
}

/// permissions.py:52-62 over a window list: true when another app's normal window shows a title, false when such
/// windows exist and none has one, None when there is none to judge by. A window the window server lists without a
/// layer counts as layer 0, as Python's `w.get("kCGWindowLayer", 0)` does.
pub fn title_readable_in(me: i32, windows: &[Listed]) -> Option<bool> {
    let mut seen = false;
    for w in windows {
        if w.window.pid == me
            || w.owner_name == "Dock"
            || w.owner_name == "WindowServer"
            || w.window.layer != 0
        {
            continue;
        }
        if !w.window.title.is_empty() {
            return Some(true);
        }
        seen = true;
    }
    if seen {
        Some(false)
    } else {
        None
    }
}

pub fn preflight_screen_capture() -> bool {
    CGPreflightScreenCaptureAccess()
}

#[cfg(test)]
mod tests {
    use super::{title_readable_in, Listed};
    use crate::driver::Window;

    fn w(pid: i32, owner: &str, title: &str, layer: Option<i64>) -> Listed {
        Listed {
            window: Window {
                id: 1,
                pid,
                bounds: (0.0, 0.0, 10.0, 10.0),
                title: title.into(),
                layer: layer.unwrap_or(0),
                alpha: 1.0,
                layer_known: layer.is_some(),
            },
            owner_name: owner.into(),
        }
    }

    /// permissions.py:52-62: a window the window server lists without a layer is judged as a normal one (Python's
    /// `kCGWindowLayer` default 0); FERB's own, the Dock's, WindowServer's and status items are never judged.
    #[test]
    fn a_window_with_no_layer_counts_as_normal() {
        assert_eq!(
            title_readable_in(7, &[w(9, "Notes", "", None)]),
            Some(false)
        );
        assert_eq!(
            title_readable_in(7, &[w(9, "Notes", "Shopping", None)]),
            Some(true)
        );
        assert_eq!(
            title_readable_in(
                7,
                &[
                    w(7, "FERB", "mine", Some(0)),
                    w(9, "Dock", "Dock", Some(0)),
                    w(9, "Clock", "status", Some(25)),
                ]
            ),
            None
        );
    }
}
