//! One window captured with ScreenCaptureKit (screenshot.py `_capture_once`): the desktop-independent window filter
//! renders only that window, never what covers it or the rest of the screen; in memory only, encoded to JPEG. A
//! completion handler owns its own copy of the shared answer slot, so it can arrive after the waiter gave up
//! (TIMEOUT_S) without anything dangling: the slot is simply never read.

use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::AnyThread;
use objc2_core_graphics::CGImage;
use objc2_foundation::{NSError, NSString};
use objc2_screen_capture_kit::{
    SCContentFilter, SCScreenshotManager, SCShareableContent, SCStreamConfiguration, SCWindow,
};

use crate::driver::Window;
use crate::error::{HandError, Result};
use crate::shots::{capture_error, capture_size, Capturer, TIMEOUT_S};

/// A value a completion handler hands over from another thread.
struct Sent<T>(T);
// SAFETY: the objects passed (SCShareableContent, CGImage, NSError) are immutable results handed from
// ScreenCaptureKit's queue to the one waiting thread; nothing else touches them meanwhile.
unsafe impl<T> Send for Sent<T> {}

type Slot<T> = Arc<(Mutex<Option<Sent<T>>>, Condvar)>;

fn wait<T>(slot: &Slot<T>) -> Result<T> {
    let (m, cv) = &**slot;
    let guard = m.lock().unwrap_or_else(|p| p.into_inner());
    let (mut guard, timeout) = cv
        .wait_timeout_while(guard, Duration::from_secs_f64(TIMEOUT_S), |v| v.is_none())
        .unwrap_or_else(|p| p.into_inner());
    if timeout.timed_out() && guard.is_none() {
        return Err(HandError::Timeout(
            "the screen capture didn't answer".into(),
        ));
    }
    Ok(guard.take().expect("answered").0)
}

fn put<T>(slot: &Slot<T>, v: T) {
    let (m, cv) = &**slot;
    *m.lock().unwrap_or_else(|p| p.into_inner()) = Some(Sent(v));
    cv.notify_all();
}

/// screenshot.py:173.
pub fn error_of(e: &NSError) -> HandError {
    let text = e.localizedDescription().to_string();
    let (domain, code) = (e.domain().to_string(), e.code() as i64);
    let info = e.userInfo();
    let mut reasons = vec![];
    for key in [
        "NSLocalizedFailureReason",
        "NSLocalizedRecoverySuggestion",
        "NSDebugDescription",
    ] {
        if let Some(v) = info.objectForKey(&NSString::from_str(key)) {
            if let Ok(s) = v.downcast::<NSString>() {
                reasons.push(s.to_string());
            }
        }
    }
    let underlying = info
        .objectForKey(&NSString::from_str("NSUnderlyingError"))
        .and_then(|u| u.downcast::<NSError>().ok())
        .map(|u| (u.domain().to_string(), u.code() as i64));
    capture_error(&text, &domain, code, &reasons, underlying)
}

/// The shareable content (every window, every Space: a window on another Space or full screen can be captured).
pub fn shareable() -> Result<Retained<SCShareableContent>> {
    let slot: Slot<std::result::Result<Retained<SCShareableContent>, HandError>> =
        Arc::new((Mutex::new(None), Condvar::new()));
    let s2 = slot.clone();
    let handler = RcBlock::new(
        move |content: *mut SCShareableContent, error: *mut NSError| {
            // SAFETY: ScreenCaptureKit passes either a valid content or a valid error for the call's duration; we retain
            // whichever is there.
            let got = unsafe {
                match (Retained::retain(content), Retained::retain(error)) {
                    (_, Some(e)) => Err(error_of(&e)),
                    (Some(c), None) => Ok(c),
                    (None, None) => Err(HandError::Other("no shareable content".into())),
                }
            };
            put(&s2, got);
        },
    );
    // SAFETY: the handler block is copied by ScreenCaptureKit and outlives this call.
    unsafe {
        SCShareableContent::getShareableContentExcludingDesktopWindows_onScreenWindowsOnly_completionHandler(true, false, &handler)
    };
    wait(&slot)?
}

/// The real capture pieces (screenshot.py), behind `shots::Capturer`.
pub struct NativeCapturer;

impl Capturer for NativeCapturer {
    fn minimized(&self, pid: i32, bounds: crate::ax::Rect) -> Option<bool> {
        crate::ax::Ax::new(super::ax::NativeAx::default()).window_minimized(pid, bounds)
    }

    fn screen_off(&self) -> Option<String> {
        super::windows::screen_off()
    }

    fn capture_once(&self, w: &Window, max_edge: Option<u32>) -> Result<(Vec<u8>, i64, i64)> {
        objc2::rc::autoreleasepool(|_| {
            let content = shareable()?;
            // SAFETY: reading the content's window list.
            let windows = unsafe { content.windows() };
            let sc: Option<Retained<SCWindow>> = windows
                .to_vec()
                .into_iter()
                .find(|x| unsafe { x.windowID() } as i64 == w.id);
            let Some(sc) = sc else {
                return Err(HandError::NoTarget(
                    "that window is no longer on screen".into(),
                ));
            };
            // SAFETY: a filter of one window, desktop independent.
            let filter = unsafe {
                SCContentFilter::initWithDesktopIndependentWindow(SCContentFilter::alloc(), &sc)
            };
            // SAFETY: plain configuration setters.
            let config = unsafe { SCStreamConfiguration::new() };
            // SAFETY: the filter's own scale.
            let scale = unsafe { filter.pointPixelScale() } as f64;
            let (width, height) =
                capture_size(w.bounds, if scale > 0.0 { scale } else { 1.0 }, max_edge);
            // SAFETY: plain configuration setters.
            unsafe {
                config.setWidth(width as usize);
                config.setHeight(height as usize);
                config.setShowsCursor(false);
            }
            let slot: Slot<
                std::result::Result<objc2_core_foundation::CFRetained<CGImage>, HandError>,
            > = Arc::new((Mutex::new(None), Condvar::new()));
            let s2 = slot.clone();
            let handler = RcBlock::new(move |image: *mut CGImage, error: *mut NSError| {
                // SAFETY: a valid image or a valid error for the call's duration; we retain whichever is there.
                let got = unsafe {
                    if let Some(e) = Retained::retain(error) {
                        Err(error_of(&e))
                    } else if let Some(img) = std::ptr::NonNull::new(image) {
                        Ok(objc2_core_foundation::CFRetained::retain(img))
                    } else {
                        Err(HandError::Other("no image".into()))
                    }
                };
                put(&s2, got);
            });
            // SAFETY: a filter and configuration made above; the block is copied by ScreenCaptureKit.
            unsafe {
                SCScreenshotManager::captureImageWithFilter_configuration_completionHandler(
                    &filter,
                    &config,
                    Some(&handler),
                )
            };
            let image = wait(&slot)??;
            Ok((super::paint::jpeg(&image)?, width, height))
        })
    }

    fn sleep(&self, seconds: f64) {
        std::thread::sleep(Duration::from_secs_f64(seconds));
    }
}

/// The driver's Shots on the real Mac.
pub struct NativeShots;

impl crate::driver::Shots for NativeShots {
    fn window(&self, id: i64) -> Option<Window> {
        crate::shots::window(&super::windows::NativeWindows, id)
    }
    fn front_window(
        &self,
        pid: i32,
        like: Option<crate::ax::Rect>,
        title: Option<&str>,
    ) -> Option<Window> {
        crate::shots::front_window(&super::windows::NativeWindows, pid, like, title)
    }
    fn on_screen(&self, id: i64) -> bool {
        crate::shots::on_screen(&super::windows::NativeWindows, id)
    }
    fn window_at(&self, x: f64, y: f64) -> Option<Window> {
        crate::shots::window_at(&super::windows::NativeWindows, x, y, 0)
    }
    fn windows_at(&self, x: f64, y: f64) -> Option<Vec<Window>> {
        crate::shots::windows_at(&super::windows::NativeWindows, x, y)
    }
    fn capture(
        &self,
        w: &Window,
        max_edge: Option<u32>,
        preview: bool,
    ) -> Result<(Vec<u8>, i64, i64)> {
        crate::shots::capture(&NativeCapturer, w, max_edge, preview)
    }
    fn window_on_screen(
        &self,
        pid: i32,
        rect: crate::ax::Rect,
        like: Option<crate::ax::Rect>,
        title: Option<&str>,
        window: Option<i64>,
    ) -> bool {
        crate::shots::window_on_screen(
            &super::windows::NativeWindows,
            pid,
            rect,
            like,
            title,
            window,
        )
    }
    fn display_of(&self, rect: crate::ax::Rect) -> Option<u32> {
        super::windows::display_of(rect)
    }
}
