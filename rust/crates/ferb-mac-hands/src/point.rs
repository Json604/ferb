//! Where FERB is about to act, for its on-screen cursor (L33, `Point`), the surface a call works on for the live
//! preview (`PreviewTarget`, HAND-402), and one preview capture's own budget and phase times (DMN-406)
//! (ferb/act/point.py). Geometry only: never a label, value, URL or text.

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use serde_json::{json, Map, Value};

use crate::ax::Rect;
use crate::error::Result;

pub const POINT_TIMEOUT_S: f64 = 0.5;

#[derive(Debug, Clone, PartialEq)]
pub struct Point {
    pub kind: &'static str,
    pub rect: Rect,
    pub display: u32,
}

impl Point {
    /// point.py:30's checks: a known kind, four finite numbers, a positive width and height.
    pub fn new(kind: &'static str, rect: Rect, display: u32) -> std::result::Result<Point, String> {
        if !["click", "type", "scroll", "press", "drag"].contains(&kind) {
            return Err(format!("bad point kind {kind:?}"));
        }
        if ![rect.0, rect.1, rect.2, rect.3]
            .iter()
            .all(|v| v.is_finite())
        {
            return Err("a point's rect is four finite numbers".into());
        }
        if rect.2 <= 0.0 || rect.3 <= 0.0 {
            return Err("a point's rect needs a positive width and height".into());
        }
        Ok(Point {
            kind,
            rect,
            display,
        })
    }

    pub fn payload(&self, task_id: &str, worker_id: &str) -> Value {
        json!({"task_id": task_id, "worker_id": worker_id, "kind": self.kind,
               "rect": [self.rect.0, self.rect.1, self.rect.2, self.rect.3], "display": self.display})
    }
}

/// One masked frame (bytes, or bytes and a note on its masking).
pub type Frame = (Vec<u8>, String);
pub type FrameFn =
    Arc<dyn Fn() -> Pin<Box<dyn Future<Output = Result<Frame>> + Send>> + Send + Sync>;
pub type ResolveFn = Arc<
    dyn Fn() -> Pin<Box<dyn Future<Output = Result<(Map<String, Value>, FrameFn)>> + Send>>
        + Send
        + Sync,
>;

/// The surface one call works on (point.py:56): its scope, a short label, and how to capture it, resolved only when
/// a frame is wanted. `resolve` None: FERB works there but has no window of it to show yet.
#[derive(Clone)]
pub struct PreviewTarget {
    pub scope: String,
    pub label: String,
    pub resolve: Option<ResolveFn>,
}

impl std::fmt::Debug for PreviewTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreviewTarget")
            .field("scope", &self.scope)
            .field("label", &self.label)
            .field("resolve", &self.resolve.is_some())
            .finish()
    }
}

/// One preview capture's deadline (monotonic seconds) and its phase times (ms).
#[derive(Debug, Default)]
pub struct PreviewCtx {
    pub deadline: f64,
    pub phases: Mutex<BTreeMap<String, f64>>,
}

tokio::task_local! {
    pub static PREVIEW: Arc<PreviewCtx>;
}

/// The preview capture this task runs, if any.
pub fn preview() -> Option<Arc<PreviewCtx>> {
    PREVIEW.try_with(|p| p.clone()).ok()
}

/// point.py:84: seconds this preview capture has left (never below 0), or None outside one.
pub fn preview_left(now: f64) -> Option<f64> {
    preview().map(|p| (p.deadline - now).max(0.0))
}

/// point.py:91: adds the block's time (ms) to this preview capture's phase `name`; nothing outside one.
pub async fn preview_phase<T>(
    name: &str,
    now: &(dyn Fn() -> f64 + Send + Sync),
    fut: impl Future<Output = T>,
) -> T {
    let ctx = preview();
    let started = now();
    let out = fut.await;
    if let Some(ctx) = ctx {
        let mut phases = ctx.phases.lock().expect("lock");
        let total = phases.get(name).copied().unwrap_or(0.0) + (now() - started) * 1000.0;
        phases.insert(name.into(), (total * 10.0).round_ties_even() / 10.0);
    }
    out
}
