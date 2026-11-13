//! The owner's front window at send (L12, desktop.py `_front_window`): ferb-sense's `front_window::capture` over this
//! Mac's parts. The window server's normal windows (layer 0, not transparent, with an area, front to back: Python's
//! `screenshot.normal_windows`, a window the server lists without a layer counting as not normal), the app of a
//! process and its parent (NSRunningApplication, `proc_pidinfo`), FERB's own processes, the owner's exclusion list,
//! the window's picture through the Mac driver (its secret fields masked in the capture), on-device redaction (Vision
//! then painting), and the evidence store under the request. DMN-520: the planner waits at most
//! `FRONT_WINDOW_WAIT_S` (ferb-supervisor's wait); a capture it stops waiting for is journaled
//! `screen.capture_stalled` with its step.
//!
//! A browser window's active tab comes from AppleScript once the owner granted Automation for that browser
//! (desktop.py `_front_tab`, the desktop area's Automation state), else from the extension's naming of the captured
//! window. The sending client's process isn't carried by the supervisor's seam: FERB.app is the only client, and
//! FERB's own processes are never the front window (`own`).

use std::sync::{Arc, Mutex};

use ferb_core::interfaces::BoxFuture;
use ferb_core::types::ModelImage;
use ferb_sense::front_window::{self as fw, Shot, ShotError, TabLookup, Window};
use ferb_sense::observation::{Dict, Redact};
use ferb_supervisor::flow::FrontCapture;
use serde_json::{json, Map, Value};

use super::tools::Parts;

struct Sources {
    parts: Parts,
    redactor: Arc<ferb_sanitize::redact::Redactor>,
    request_id: String,
    exclusions: ferb_sense::exclusions::Exclusions,
    daemon_pid: i64,
    tokens: Arc<ferb_rpc::RunTokens>,
}

impl fw::Sources for Sources {
    fn windows(&self) -> Vec<Window> {
        normal_windows()
    }
    fn app_of(&self, pid: i64) -> Option<(String, String)> {
        app_of(pid)
    }
    fn own(&self, pid: i64, bundle: &str) -> bool {
        fw::ferb_process(pid, bundle, self.daemon_pid, &parent_pid)
    }
    fn excluded(&self, bundle: &str, name: &str, url: Option<&str>) -> bool {
        self.exclusions.app(Some(bundle), Some(name)) || self.exclusions.url(url)
    }
    /// desktop.py:245 `_front_tab`.
    fn tab(
        &self,
        bundle: &str,
        w: &Window,
        after_shot: bool,
    ) -> Result<Option<(String, String)>, TabLookup> {
        if !self.parts.native() {
            return Ok(None);
        }
        let desktop = super::desktop_seams::DesktopSeams {
            parts: self.parts.clone(),
            tokens: self.tokens.clone(),
        };
        self.parts
            .automation()
            .front_tab(&desktop.host(), &desktop, bundle, w, after_shot)
    }
    fn shoot<'a>(&'a self, w: &'a Window) -> BoxFuture<'a, Result<Shot, ShotError>> {
        let mac = self.parts.mac().map(|m| m.tools.clone());
        let (pid, id) = (w.pid, w.id);
        Box::pin(async move {
            let Some(tools) = mac else {
                return Err(ShotError::Refused(
                    "FERB has no Mac driver to capture windows with".into(),
                ));
            };
            if !(tools.driver.snapshot_images)() {
                return Err(ShotError::Refused(
                    "screenshots are switched off ([mac] snapshot_image)".into(),
                ));
            }
            let shot = super::tools::mac::off_thread(move || {
                Box::pin(async move {
                    let paint = tools.paint.clone();
                    tools
                        .driver
                        .window_image(&format!("pid:{pid}"), Some(&id.to_string()), &*paint)
                        .await
                        .map(|(_, shot, jpeg)| (jpeg, shot.masking, shot.tab))
                })
            })
            .await;
            match shot {
                Some(Ok(s)) => Ok(s),
                Some(Err(ferb_mac_hands::error::HandError::Refused(m)))
                | Some(Err(ferb_mac_hands::error::HandError::Withheld(m))) => {
                    Err(ShotError::Refused(m))
                }
                Some(Err(e)) => Err(ShotError::Failed(hand_kind(&e))),
                None => Err(ShotError::Failed("RuntimeError".into())),
            }
        })
    }
    fn redact_image(&self, raw: &[u8]) -> Result<Vec<u8>, String> {
        match self.parts.mac() {
            Some(m) => m.images.redact(raw),
            None => Err(ferb_mac_hands::ocr::UNAVAILABLE.into()),
        }
    }
    fn redact(&self) -> &dyn Redact {
        self.redactor.as_ref()
    }
    fn keep<'a>(
        &'a self,
        image: &'a [u8],
        facts: &'a Dict,
        origin: &'a str,
    ) -> BoxFuture<'a, Result<String, String>> {
        Box::pin(async move {
            use ferb_gateway::call_hooks::Evidence;
            super::tools::mac::EvidenceKeeper {
                parts: self.parts.clone(),
            }
            .put(
                &self.request_id,
                "",
                "front_window",
                Some(image),
                facts.clone(),
                origin.to_string(),
            )
            .await
        })
    }
    fn client(&self) -> Option<i64> {
        crate::off::off("front.client", None)
    }
    fn parent(&self, pid: i64) -> Option<i64> {
        parent_pid(pid)
    }
    fn main_frame(&self, pid: i64) -> Option<(f64, f64, f64, f64)> {
        main_frame(pid)
    }
    fn is_browser(&self, bundle: &str) -> bool {
        ferb_mac_hands::apps::known(bundle)
    }
    fn read_sources(&self, bundle: &str) -> Vec<(String, String)> {
        // act/mac/tools.py `_sources`
        ferb_mac_hands::tools::sources_of(bundle, ferb_mac_hands::apps::known(bundle))
    }
}

/// A Mac-hands error's class name, as `error_name` gives it.
fn hand_kind(e: &ferb_mac_hands::error::HandError) -> String {
    let d = format!("{e:?}");
    d.split(['(', ' ', '{'])
        .next()
        .unwrap_or("Error")
        .to_string()
}

/// `_front_window(request_id)`: the window the planner is given, the redacted picture as the planner's and the worker's
/// image; None when the Mac's parts are off, sensing is paused, nothing but FERB is in front, or the capture failed
/// (journaled).
pub async fn at_send(
    parts: &Parts,
    tokens: &Arc<ferb_rpc::RunTokens>,
    request_id: &str,
) -> Option<FrontCapture> {
    parts.mac()?;
    if parts.home().join("PAUSED").exists() {
        return None;
    }
    let host = ferb_daemon::areas_b::core::Host {
        home: parts.home().clone(),
        config: parts.config(),
        redactor: parts.redactor(),
        journal: parts.journal(),
        bus: parts.events(),
        clock: parts.zoned(),
    };
    let exclusions = match ferb_daemon::areas_b::sensing::exclusions(&host) {
        Ok(e) => e,
        Err(e) => {
            parts.journal().note(
                "screen.capture_failed",
                json!({"request_id": request_id, "error": e.type_name}),
            );
            return None;
        }
    };
    let src = Arc::new(Sources {
        parts: parts.clone(),
        redactor: parts.redactor(),
        request_id: request_id.to_string(),
        exclusions,
        daemon_pid: i64::from(std::process::id()),
        tokens: tokens.clone(),
    });
    let stage = Arc::new(Mutex::new(Vec::<String>::new()));
    // the planner stops waiting by dropping this future (DMN-520): the step the capture was on is journaled
    struct Stalled {
        parts: Parts,
        request_id: String,
        stage: Arc<Mutex<Vec<String>>>,
        started: std::time::Instant,
        done: bool,
    }
    impl Drop for Stalled {
        fn drop(&mut self) {
            if self.done {
                return;
            }
            let at = self
                .stage
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .last()
                .cloned()
                .unwrap_or_else(|| "start".into());
            self.parts.journal().note(
                "screen.capture_stalled",
                json!({"request_id": self.request_id, "stage": at,
                       "seconds": (self.started.elapsed().as_secs_f64() * 100.0).round() / 100.0}),
            );
        }
    }
    let mut stalled = Stalled {
        parts: parts.clone(),
        request_id: request_id.to_string(),
        stage: stage.clone(),
        started: std::time::Instant::now(),
        done: false,
    };
    let (s2, st2) = (src.clone(), stage.clone());
    // the capture's futures hold what can't move between threads: a thread of its own (dropped with this future)
    let job = super::tools::mac::off_thread(move || {
        Box::pin(async move { fw::capture(s2.as_ref(), &st2).await })
    });
    let got = job.await;
    stalled.done = true;
    let front = match got {
        Some(front) => front?,
        None => {
            parts.journal().note(
                "screen.capture_failed",
                json!({"request_id": request_id, "error": "RuntimeError"}),
            );
            return None;
        }
    };
    let image = match (&front.image, &front.image_ref) {
        (Some(data), Some(r)) => Some(ModelImage::new(data.clone(), r)),
        _ => None,
    };
    let facts: Map<String, Value> = front.facts().into_iter().collect();
    Some(FrontCapture {
        window: ferb_supervisor::planner::FrontWindow {
            facts,
            image_ref: front.image_ref.clone().unwrap_or_default(),
            withheld: front.withheld.clone().unwrap_or_default(),
            masking: front.masking.clone(),
            origin: front.origin.clone(),
        },
        image,
    })
}

/// `screenshot.normal_windows`: on-screen normal windows (layer 0, not transparent, with an area), front to back.
#[cfg(target_os = "macos")]
pub fn normal_windows() -> Vec<Window> {
    use ferb_mac_hands::native::cf::{dict_get, num, text};
    use objc2_core_foundation::{CFDictionary, CFRetained, CGPoint, CGRect, CGSize};
    use objc2_core_graphics::{
        kCGWindowAlpha, kCGWindowBounds, kCGWindowLayer, kCGWindowName, kCGWindowNumber,
        kCGWindowOwnerPID, CGRectMakeWithDictionaryRepresentation, CGWindowListCopyWindowInfo,
        CGWindowListOption,
    };
    objc2::rc::autoreleasepool(|_| {
        let opts =
            CGWindowListOption::OptionOnScreenOnly | CGWindowListOption::ExcludeDesktopElements;
        let Some(list) = CGWindowListCopyWindowInfo(opts, 0) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for i in 0..list.count() {
            // SAFETY: `i` is below the count; each item is a CFDictionary the list keeps alive while it is read.
            let item = unsafe { list.value_at_index(i) } as *const CFDictionary;
            if item.is_null() {
                continue;
            }
            // SAFETY: a non-null CFDictionary pointer owned by `list`, which outlives this loop body.
            let d: &CFDictionary = unsafe { &*item };
            // SAFETY: the window server's keys are CFStrings exported by CoreGraphics.
            let (k_num, k_pid, k_bounds, k_name, k_layer, k_alpha) = unsafe {
                (
                    kCGWindowNumber,
                    kCGWindowOwnerPID,
                    kCGWindowBounds,
                    kCGWindowName,
                    kCGWindowLayer,
                    kCGWindowAlpha,
                )
            };
            let Some(id) = num(dict_get(d, k_num)) else {
                continue;
            };
            // `int(d.get("kCGWindowLayer", 1)) == 0`: a window listed without a layer is not a normal one
            let layer = num(dict_get(d, k_layer)).map_or(1, |l| l as i64);
            let alpha = num(dict_get(d, k_alpha)).unwrap_or(1.0);
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
            if layer != 0 || alpha <= 0.0 || rect.size.width <= 0.0 || rect.size.height <= 0.0 {
                continue;
            }
            out.push(Window {
                id: id as i64,
                pid: num(dict_get(d, k_pid)).map_or(-1, |p| p as i64),
                bounds: (
                    rect.origin.x,
                    rect.origin.y,
                    rect.size.width,
                    rect.size.height,
                ),
                title: text(dict_get(d, k_name)).unwrap_or_default(),
                layer,
                alpha,
            });
        }
        out
    })
}

#[cfg(not(target_os = "macos"))]
pub fn normal_windows() -> Vec<Window> {
    Vec::new()
}

#[cfg(target_os = "macos")]
fn app_of(pid: i64) -> Option<(String, String)> {
    ferb_sense::mac::apps::app_of(pid)
}

#[cfg(not(target_os = "macos"))]
fn app_of(_pid: i64) -> Option<(String, String)> {
    None
}

#[cfg(target_os = "macos")]
fn parent_pid(pid: i64) -> Option<i64> {
    ferb_sense::mac::apps::parent_pid(pid)
}

#[cfg(not(target_os = "macos"))]
fn parent_pid(_pid: i64) -> Option<i64> {
    None
}

/// `accessibility.main_window_frame`: the app's main (else focused) window's frame.
#[cfg(target_os = "macos")]
fn main_frame(pid: i64) -> Option<(f64, f64, f64, f64)> {
    let ax = ferb_mac_hands::ax::Ax::new(ferb_mac_hands::native::ax::NativeAx::default());
    ax.main_window_frame(pid as i32)
}

#[cfg(not(target_os = "macos"))]
fn main_frame(_pid: i64) -> Option<(f64, f64, f64, f64)> {
    None
}
