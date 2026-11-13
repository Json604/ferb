//! One exact window, captured into memory (ferb/act/mac/screenshot.py): the window list rules over what the window
//! server lists, and the capture's order of checks (minimized refused by name, a locked or sleeping screen said, one
//! capture at a time, one quiet retry of "Failed to start stream": HAND-405, HAND-437). The CGWindowList read and
//! ScreenCaptureKit are `native::windows` and `native::capture`.

use std::sync::Mutex;

use crate::ax::Rect;
use crate::driver::Window;
use crate::error::{HandError, Result};

pub const MAX_EDGE: u32 = 1568;
pub const JPEG_QUALITY: f64 = 0.8;
pub const TIMEOUT_S: f64 = 10.0;
/// SCStreamErrorFailedToStart.
pub const START_FAILED: i64 = -3811;
pub const RETRY_AFTER_S: f64 = 0.3;

/// The window list as Quartz gives it, front to back: one entry per window, `None` when the list is NULL.
pub trait WindowList: Send + Sync {
    /// `everywhere`: every Space too (kCGWindowListOptionAll); else on screen only. Desktop elements excluded.
    fn list(&self, everywhere: bool) -> Option<Vec<Window>>;
}

fn near(a: &Rect, b: &Rect) -> bool {
    (a.0 - b.0).abs() <= 2.0
        && (a.1 - b.1).abs() <= 2.0
        && (a.2 - b.2).abs() <= 2.0
        && (a.3 - b.3).abs() <= 2.0
}

fn words(s: &str) -> String {
    s.split(ferb_sanitize::parity::is_space)
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn holds(b: &Rect, x: f64, y: f64) -> bool {
    b.0 <= x && x <= b.0 + b.2 && b.1 <= y && y <= b.1 + b.3
}

/// screenshot.py:47 (L33): the frontmost normal window at the rect's centre is the target.
pub fn window_on_screen(
    list: &dyn WindowList,
    pid: i32,
    rect: Rect,
    like: Option<Rect>,
    title: Option<&str>,
    window: Option<i64>,
) -> bool {
    let (cx, cy) = (rect.0 + rect.2 / 2.0, rect.1 + rect.3 / 2.0);
    let normal: Vec<Window> = list
        .list(false)
        .unwrap_or_default()
        .into_iter()
        .filter(|w| w.layer == 0 && w.alpha > 0.0)
        .collect();
    let target = |w: &Window| {
        w.pid == pid
            && window.is_none_or(|id| w.id == id)
            && like.is_none_or(|l| {
                near(&w.bounds, &l) && title.is_none_or(|t| words(&w.title) == words(t))
            })
    };
    if like.is_some() && normal.iter().filter(|w| target(w)).count() != 1 {
        return false;
    }
    normal
        .iter()
        .find(|w| holds(&w.bounds, cx, cy))
        .is_some_and(target)
}

/// screenshot.py:89: the unique process-owned normal window at exact bounds (a title breaks a tie).
pub fn front_window(
    list: &dyn WindowList,
    pid: i32,
    like: Option<Rect>,
    title: Option<&str>,
) -> Option<Window> {
    let mut mine: Vec<Window> = list
        .list(true)
        .unwrap_or_default()
        .into_iter()
        .filter(|w| w.pid == pid && w.layer == 0 && w.alpha > 0.0)
        .collect();
    if let Some(l) = like {
        mine.retain(|w| near(&w.bounds, &l));
    }
    if mine.len() > 1 {
        if let Some(t) = title.filter(|t| !t.is_empty()) {
            mine.retain(|w| words(&w.title) == words(t));
        }
    }
    if mine.len() == 1 {
        mine.pop()
    } else {
        None
    }
}

/// screenshot.py:104 (L12): on-screen normal windows, front to back.
pub fn normal_windows(list: &dyn WindowList) -> Vec<Window> {
    list.list(false)
        .unwrap_or_default()
        .into_iter()
        .filter(|w| w.layer == 0 && w.alpha > 0.0 && w.bounds.2 > 0.0 && w.bounds.3 > 0.0)
        .collect()
}

pub fn on_screen(list: &dyn WindowList, id: i64) -> bool {
    list.list(false)
        .unwrap_or_default()
        .iter()
        .any(|w| w.id == id)
}

pub fn window(list: &dyn WindowList, id: i64) -> Option<Window> {
    list.list(true)
        .unwrap_or_default()
        .into_iter()
        .find(|w| w.id == id)
}

/// screenshot.py:126: every on-screen window at this point, all levels; None when the list can't be read.
pub fn windows_at(list: &dyn WindowList, x: f64, y: f64) -> Option<Vec<Window>> {
    let all = list.list(false)?;
    Some(
        all.into_iter()
            .filter(|w| {
                w.bounds.0 <= x
                    && x < w.bounds.0 + w.bounds.2
                    && w.bounds.1 <= y
                    && y < w.bounds.1 + w.bounds.3
            })
            .collect(),
    )
}

/// screenshot.py:141.
pub fn window_at(list: &dyn WindowList, x: f64, y: f64, max_layer: i64) -> Option<Window> {
    list.list(false).unwrap_or_default().into_iter().find(|w| {
        w.alpha > 0.0
            && w.layer <= max_layer
            && w.bounds.0 <= x
            && x < w.bounds.0 + w.bounds.2
            && w.bounds.1 <= y
            && y < w.bounds.1 + w.bounds.3
    })
}

/// The pieces one capture needs (`native::capture`).
pub trait Capturer: Send + Sync {
    /// AXMinimized for the one Accessibility window at these bounds (None: can't tell).
    fn minimized(&self, pid: i32, bounds: Rect) -> Option<bool>;
    /// "locked" or "asleep", else None (also when it can't be told).
    fn screen_off(&self) -> Option<String>;
    /// One ScreenCaptureKit capture: JPEG bytes and the image size.
    fn capture_once(&self, w: &Window, max_edge: Option<u32>) -> Result<(Vec<u8>, i64, i64)>;
    fn sleep(&self, seconds: f64);
}

/// The one-at-a-time capture lock of this process.
pub struct CaptureLock(pub Mutex<()>);

pub static CAPTURING: CaptureLock = CaptureLock(Mutex::new(()));

/// screenshot.py:200: JPEG bytes of exactly this window and the image size.
pub fn capture(
    c: &dyn Capturer,
    w: &Window,
    max_edge: Option<u32>,
    preview: bool,
) -> Result<(Vec<u8>, i64, i64)> {
    if c.minimized(w.pid, w.bounds) == Some(true) {
        return Err(HandError::Refused("that window is minimized, and FERB doesn't bring windows back on its own (it works in the background): it can be captured once it's shown again".into()));
    }
    if let Some(off) = c.screen_off() {
        return Err(HandError::ScreenOff(off));
    }
    let _guard = if preview {
        match CAPTURING.0.try_lock() {
            Ok(g) => g,
            Err(std::sync::TryLockError::WouldBlock) => {
                return Err(HandError::CaptureBusy(
                    "another capture is under way".into(),
                ))
            }
            Err(std::sync::TryLockError::Poisoned(p)) => p.into_inner(),
        }
    } else {
        CAPTURING.0.lock().unwrap_or_else(|p| p.into_inner())
    };
    match c.capture_once(w, max_edge) {
        Err(HandError::Capture {
            code: Some(START_FAILED),
            ..
        }) => {}
        other => return other,
    }
    c.sleep(RETRY_AFTER_S);
    match c.capture_once(w, max_edge) {
        Err(
            e @ HandError::Capture {
                code: Some(START_FAILED),
                ..
            },
        ) => match c.screen_off() {
            Some(off) => Err(HandError::ScreenOff(off)),
            None => Err(e),
        },
        other => other,
    }
}

/// screenshot.py:173: an NSError kept whole enough to diagnose.
pub fn capture_error(
    text: &str,
    domain: &str,
    code: i64,
    reasons: &[String],
    underlying: Option<(String, i64)>,
) -> HandError {
    let mut parts = vec![format!("{domain} {code}")];
    parts.extend(reasons.iter().filter(|r| !r.is_empty()).cloned());
    if let Some((d, c)) = underlying {
        parts.push(format!("underlying {d} {c}"));
    }
    HandError::Capture {
        message: format!("{text} ({})", parts.join("; ")),
        domain: domain.into(),
        code: Some(code),
    }
}

/// The size the capture is configured to (screenshot.py:242-245): the window in pixels at the filter's point scale,
/// scaled down so its long side is at most `max_edge`.
pub fn capture_size(bounds: Rect, scale: f64, max_edge: Option<u32>) -> (i64, i64) {
    let (fw, fh) = (bounds.2 * scale, bounds.3 * scale);
    let k = match max_edge {
        Some(m) => 1.0f64.min(m as f64 / fw.max(fh)),
        None => 1.0,
    };
    (((fw * k) as i64).max(1), ((fh * k) as i64).max(1))
}
