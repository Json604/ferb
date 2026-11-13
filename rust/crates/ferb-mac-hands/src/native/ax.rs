//! Accessibility on the real Mac: AXUIElement reads and actions as an `ax::AxSource`, and the driver's `Ax` over
//! `ax::Ax` (accessibility.py's functions). Every call drains its own autorelease pool (autorelease.py). The
//! messaging timeout is HAND-221's 2 s, set on the system-wide element once and on each app element.

use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use objc2::rc::autoreleasepool;
use objc2_application_services::{AXError, AXUIElement, AXValue, AXValueType};
use objc2_core_foundation::{
    CFBoolean, CFEqual, CFHash, CFRange, CFRetained, CFString, CFType, Type,
};

use super::cf::{ax_of, el_of, value_of};
use super::windows::NativeWindows;
use crate::ax::{
    self as axm, AxNode, AxSource, AxTree, AxValue, CgWindow, El, Rect, ScrollPosition,
};
use crate::driver::{Ax, Facts};
use crate::error::{HandError, Result};
use crate::shots::WindowList;

static TIMEOUT_SET: AtomicBool = AtomicBool::new(false);

/// The real Accessibility source.
pub struct NativeAx {
    started: Instant,
    windows: NativeWindows,
}

impl Default for NativeAx {
    fn default() -> Self {
        NativeAx {
            started: Instant::now(),
            windows: NativeWindows,
        }
    }
}

fn cfstr(s: &str) -> CFRetained<CFString> {
    CFString::from_str(s)
}

impl NativeAx {
    /// One attribute with its AX error code (0 on success).
    pub fn raw_attr(&self, el: &El, name: &str) -> (i32, AxValue) {
        let Some(e) = ax_of(el) else {
            return (-25201, AxValue::None);
        };
        autoreleasepool(|_| {
            let mut out: *const CFType = std::ptr::null();
            // SAFETY: a valid out pointer; on success a +1 reference we take over.
            let err = unsafe { e.copy_attribute_value(&cfstr(name), NonNull::from(&mut out)) };
            match NonNull::new(out as *mut CFType) {
                // SAFETY: the Copy rule.
                Some(p) if err == AXError::Success => {
                    (0, value_of(&*unsafe { CFRetained::<CFType>::from_raw(p) }))
                }
                _ => (err.0, AxValue::None),
            }
        })
    }
}

impl AxSource for NativeAx {
    fn system(&self) -> El {
        // SAFETY: AXUIElementCreateSystemWide has no preconditions.
        let el = unsafe { AXUIElement::new_system_wide() };
        if !TIMEOUT_SET.swap(true, Ordering::SeqCst) {
            // SAFETY: a valid element; a positive timeout.
            unsafe { el.set_messaging_timeout(axm::AX_MESSAGE_TIMEOUT_S as f32) };
        }
        el_of(el)
    }

    fn app(&self, pid: i32) -> El {
        if !TIMEOUT_SET.load(Ordering::SeqCst) {
            self.system();
        }
        // SAFETY: AXUIElementCreateApplication takes any pid; a pid with no app gives an element whose reads fail.
        let el = unsafe { AXUIElement::new_application(pid) };
        // SAFETY: a valid element; a positive timeout.
        unsafe { el.set_messaging_timeout(axm::AX_MESSAGE_TIMEOUT_S as f32) };
        el_of(el)
    }

    fn attr(&self, el: &El, name: &str) -> AxValue {
        let Some(e) = ax_of(el) else {
            return AxValue::None;
        };
        autoreleasepool(|_| {
            let mut out: *const CFType = std::ptr::null();
            // SAFETY: `out` is a valid out pointer; on success it holds a +1 reference we take over below.
            let err = unsafe { e.copy_attribute_value(&cfstr(name), NonNull::from(&mut out)) };
            if err != AXError::Success {
                return AxValue::None;
            }
            match NonNull::new(out as *mut CFType) {
                // SAFETY: the Copy rule: the caller owns the returned reference.
                Some(p) => value_of(&*unsafe { CFRetained::<CFType>::from_raw(p) }),
                None => AxValue::None,
            }
        })
    }

    fn actions(&self, el: &El) -> Vec<String> {
        let Some(e) = ax_of(el) else { return vec![] };
        autoreleasepool(|_| {
            let mut out: *const objc2_core_foundation::CFArray = std::ptr::null();
            // SAFETY: a valid out pointer; on success a +1 CFArray of CFStrings.
            let err = unsafe { e.copy_action_names(NonNull::from(&mut out)) };
            if err != AXError::Success {
                return vec![];
            }
            let Some(p) = NonNull::new(out as *mut objc2_core_foundation::CFArray) else {
                return vec![];
            };
            // SAFETY: the Copy rule: we own it.
            let arr = unsafe { CFRetained::from_raw(p) };
            match value_of(arr.as_ref()) {
                AxValue::List(items) => items
                    .into_iter()
                    .filter_map(|i| i.as_str().map(String::from))
                    .collect(),
                _ => vec![],
            }
        })
    }

    fn perform(&self, el: &El, action: &str) -> i32 {
        let Some(e) = ax_of(el) else { return -25201 };
        // SAFETY: a valid element and action name.
        autoreleasepool(|_| unsafe { e.perform_action(&cfstr(action)) }.0)
    }

    fn set(&self, el: &El, name: &str, value: AxValue) -> i32 {
        let Some(e) = ax_of(el) else { return -25201 };
        autoreleasepool(|_| {
            let v: CFRetained<CFType> = match value {
                AxValue::Bool(b) => CFBoolean::new(b).retain().into(),
                AxValue::Str(s) => CFString::from_str(&s).into(),
                AxValue::Range(a, b) => {
                    let r = CFRange {
                        location: a as isize,
                        length: b as isize,
                    };
                    // SAFETY: a CFRange-typed value from a CFRange's bytes.
                    match unsafe {
                        AXValue::new(AXValueType::CFRange, NonNull::from(&r).cast::<c_void>())
                    } {
                        Some(v) => v.into(),
                        None => return -25201,
                    }
                }
                _ => return -25201,
            };
            // SAFETY: a valid element, attribute name and CF value.
            unsafe { e.set_attribute_value(&cfstr(name), &v) }.0
        })
    }

    fn element_at(&self, root: &El, x: f64, y: f64) -> Option<El> {
        let r = ax_of(root)?;
        autoreleasepool(|_| {
            let mut out: *const AXUIElement = std::ptr::null();
            // SAFETY: a valid out pointer; on success a +1 element.
            let err =
                unsafe { r.copy_element_at_position(x as f32, y as f32, NonNull::from(&mut out)) };
            if err != AXError::Success {
                return None;
            }
            // SAFETY: the Copy rule.
            NonNull::new(out as *mut AXUIElement).map(|p| el_of(unsafe { CFRetained::from_raw(p) }))
        })
    }

    fn pid_of(&self, el: &El) -> Option<i32> {
        let e = ax_of(el)?;
        let mut pid: libc::pid_t = 0;
        // SAFETY: a valid element and out pointer.
        let err = unsafe { e.pid(NonNull::from(&mut pid)) };
        (err == AXError::Success).then_some(pid)
    }

    fn same(&self, a: &El, b: &El) -> bool {
        match (ax_of(a), ax_of(b)) {
            (Some(x), Some(y)) => CFEqual(Some(x.as_ref()), Some(y.as_ref())),
            _ => false,
        }
    }

    fn hash(&self, el: &El) -> u64 {
        ax_of(el)
            .map(|e| CFHash(Some(e.as_ref())) as u64)
            .unwrap_or(0)
    }

    fn cg_windows(&self, pid: i32) -> Vec<CgWindow> {
        let on: Vec<i64> = self
            .windows
            .list(false)
            .unwrap_or_default()
            .into_iter()
            .map(|w| w.id)
            .collect();
        self.windows
            .list_all(true)
            .unwrap_or_default()
            .into_iter()
            .filter(|w| w.pid == pid && w.layer == 0 && w.bounds.2 > 0.0 && w.bounds.3 > 0.0)
            .map(|w| CgWindow {
                id: w.id,
                title: w.title.clone(),
                onscreen: on.contains(&w.id),
                bounds: w.bounds,
            })
            .collect()
    }

    fn monotonic(&self) -> f64 {
        self.started.elapsed().as_secs_f64()
    }

    fn sleep(&self, seconds: f64) {
        std::thread::sleep(std::time::Duration::from_secs_f64(seconds.max(0.0)));
    }
}

/// The driver's Accessibility on the real Mac.
pub struct AxDriver {
    pub ax: axm::Ax<NativeAx>,
    /// confirm.browser_page (the policy crate's), for `page_url`.
    pub browser_page: fn(Option<&str>) -> bool,
}

impl AxDriver {
    pub fn new(browser_page: fn(Option<&str>) -> bool) -> Self {
        AxDriver {
            ax: axm::Ax::new(NativeAx::default()),
            browser_page,
        }
    }
}

fn no_answer(e: axm::NoAnswer) -> HandError {
    HandError::NoAnswer(e)
}

impl Ax for AxDriver {
    fn read_tree(
        &self,
        pid: i32,
        browser: bool,
        window: Option<&El>,
        offset: usize,
        budget_s: Option<f64>,
        masks_only: bool,
    ) -> AxTree {
        self.ax
            .read_tree(pid, browser, window, offset, budget_s, masks_only)
    }
    fn expose_tree(&self, pid: i32) -> bool {
        self.ax.expose_tree(pid)
    }
    fn app_window(&self, pid: i32) -> Option<El> {
        self.ax.app_window(pid)
    }
    fn ax_windows(&self, pid: i32) -> Option<Vec<El>> {
        Some(self.ax.ax_windows(pid))
    }
    fn windows(&self, pid: i32) -> Vec<Facts> {
        self.ax.windows(pid, None)
    }
    fn window_named(&self, pid: i32, which: &str) -> Option<El> {
        self.ax.window_named(pid, which)
    }
    fn frame(&self, el: &El) -> Option<Rect> {
        self.ax.frame(Some(el))
    }
    fn window_with_frame(
        &self,
        pid: i32,
        bounds: Rect,
        title: Option<&str>,
        current: Option<&dyn Fn() -> String>,
    ) -> Option<El> {
        self.ax.window_with_frame(pid, bounds, title, current)
    }
    fn element_window(&self, el: &El) -> Option<El> {
        self.ax.element_window(el)
    }
    fn window_title(&self, win: &El) -> String {
        self.ax.window_title(Some(win))
    }
    fn web_areas_of(&self, win: &El) -> Option<Vec<Rect>> {
        self.ax.web_areas_of(win)
    }
    fn hit(&self, x: f64, y: f64) -> Option<(i32, Option<Rect>)> {
        self.ax.hit(x, y)
    }
    fn hit_element(&self, pid: i32, x: f64, y: f64) -> Option<El> {
        self.ax.hit_element(pid, x, y)
    }
    fn parent_of(&self, el: &El) -> Option<El> {
        self.ax.parent_of(el)
    }
    fn role_of(&self, el: &El) -> String {
        self.ax.role_of(el)
    }
    fn subrole_of(&self, el: &El) -> String {
        self.ax.subrole_of(el)
    }
    fn secure_element(&self, el: &El) -> bool {
        self.ax.secure_element(Some(el))
    }
    fn focused_element(&self, pid: i32) -> Option<El> {
        self.ax.focused_element(pid)
    }
    fn same(&self, a: &El, b: &El) -> bool {
        self.ax.same(Some(a), Some(b))
    }
    fn surface_facts(&self, el: &El) -> Option<Vec<Facts>> {
        Some(self.ax.surface_facts(Some(el)))
    }
    fn window_facts(&self, el: &El) -> Option<Vec<Option<Facts>>> {
        Some(self.ax.window_facts(el))
    }
    fn active_editor(&self, pid: i32) -> Option<El> {
        self.ax.active_editor(pid)
    }
    fn selected_text(&self, el: &El) -> Option<String> {
        self.ax.selected_text(el)
    }
    fn document_of(&self, el: &El) -> Option<String> {
        self.ax.document_of(el)
    }
    fn place_of(&self, el: &El) -> (String, String) {
        self.ax.place_of(el)
    }
    fn file_of(&self, el: &El) -> Option<String> {
        self.ax.file_of(el)
    }
    fn selected_items(&self, pid: i32) -> Vec<El> {
        self.ax.selected_items(pid)
    }
    fn page_url(&self, el: &El) -> Option<Option<String>> {
        Some(self.ax.page_url(el, &|a| (self.browser_page)(a)))
    }
    fn menu_item_shortcut(&self, pid: i32, path: &[String]) -> Option<(String, i64)> {
        self.ax.menu_item_shortcut(pid, path)
    }
    fn item_shortcut(&self, el: &El) -> Option<(String, i64)> {
        self.ax.item_shortcut(el)
    }
    fn press(&self, el: &El) -> Result<bool> {
        self.ax.perform(Some(el), "AXPress").map_err(no_answer)
    }
    fn perform(&self, el: &El, action: &str) -> Result<bool> {
        self.ax.perform(Some(el), action).map_err(no_answer)
    }
    fn cancel(&self, el: &El) -> Result<bool> {
        self.ax.perform(Some(el), "AXCancel").map_err(no_answer)
    }
    fn focus(&self, el: &El) -> bool {
        self.ax.focus(el)
    }
    fn has_focus(&self, pid: i32, el: &El) -> Option<bool> {
        self.ax.has_focus(pid, el)
    }
    fn set_value(&self, el: &El, text: &str) -> Result<bool> {
        self.ax.set_value(el, text).map_err(no_answer)
    }
    fn value(&self, el: &El) -> Option<String> {
        self.ax.value(el)
    }
    fn selected_range(&self, el: &El) -> Option<(i64, i64)> {
        self.ax.selected_range(el)
    }
    fn set_selected_range(&self, el: &El, start: i64, length: i64) -> bool {
        self.ax.set_selected_range(el, start, length)
    }
    fn set_selected_text(&self, el: &El, text: &str) -> bool {
        self.ax.set_selected_text(el, text)
    }
    fn menus(&self, pid: i32) -> Vec<serde_json::Value> {
        self.ax.menus(pid)
    }
    fn press_menu(&self, pid: i32, path: &[String]) -> Result<bool> {
        self.ax.press_menu(pid, path).map_err(no_answer)
    }
    fn menu_item_for_shortcut(
        &self,
        pid: i32,
        key: &str,
        mods: &[String],
    ) -> Option<(El, String, bool)> {
        self.ax.menu_item_for_shortcut(pid, key, mods)
    }
    fn center(&self, n: &AxNode) -> (f64, f64) {
        axm::center(n)
    }
    fn raise_window(&self, pid: i32, bounds: Rect) -> bool {
        self.ax.raise_window(pid, bounds)
    }
    fn scroll_area_of(&self, el: &El) -> Option<El> {
        self.ax.scroll_area_of(el)
    }
    fn scroll_area_in(&self, win: &El) -> Option<El> {
        self.ax.scroll_area_in(win)
    }
    fn scroll_content(&self, area: &El) -> Option<El> {
        self.ax.scroll_content(area)
    }
    fn scroll_bar(&self, area: &El, vertical: bool) -> Option<El> {
        self.ax.scroll_bar(area, vertical)
    }
    fn scroll_position(&self, area: Option<&El>, content: Option<&El>) -> ScrollPosition {
        self.ax.scroll_position(area, content)
    }
    fn popup_menu(&self, el: &El, pid: i32) -> Option<El> {
        self.ax.popup_menu(el, pid)
    }
    fn menu_items(&self, menu: &El) -> Vec<(El, String)> {
        self.ax.menu_items(menu)
    }
}

/// launched.py `unsaved`: a window marked edited; None when FERB can't tell.
pub fn unsaved(pid: i32) -> Option<bool> {
    const NO_VALUE: i32 = -25212;
    if !crate::native::permissions::ax_trusted(false) {
        return None;
    }
    let src = NativeAx::default();
    let app = src.app(pid);
    let list = match src.raw_attr(&app, "AXWindows") {
        (NO_VALUE, _) => return Some(false),
        (0, AxValue::List(l)) => l,
        (0, _) => return Some(false),
        _ => return None,
    };
    let mut unknown = false;
    for w in list.iter().filter_map(AxValue::as_el) {
        let (err, edited) = src.raw_attr(&w, "AXEdited");
        if err == 0 && edited.truthy() {
            return Some(true);
        }
        if err != 0 {
            let (derr, doc) = src.raw_attr(&w, "AXDocument");
            if derr == 0 && doc.truthy() {
                unknown = true;
            }
        }
    }
    if unknown {
        None
    } else {
        Some(false)
    }
}
