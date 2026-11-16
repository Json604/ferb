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

/// One masked frame (bytes, or bytes and a note on its masking): a recording's, which is stored.
pub type Frame = (Vec<u8>, String);
pub type FrameFn =
    Arc<dyn Fn() -> Pin<Box<dyn Future<Output = Result<Frame>> + Send>> + Send + Sync>;

/// One live-preview frame as captured, unmasked, for the owner's own eyes in FERB.app only (APP-922). It lives in
/// memory only and must never reach a model, a tool result, evidence, a recording, a trace, a log, the database or
/// disk: it has no `Serialize`, no conversion into bytes or from a masked image's path, and `Debug` shows only its
/// size. Its bytes are read in one place, the `tasks.preview` answer: to FERB.app ([`OwnerOnlyFrame::for_owner_app`])
/// or to the owner's paired, unlocked phone over a direct path or his own relay ([`OwnerOnlyFrame::for_paired_phone`],
/// PHN-454); a test checks no other caller exists.
#[derive(Clone, PartialEq, Eq)]
pub struct OwnerOnlyFrame(Vec<u8>);

impl OwnerOnlyFrame {
    /// A frame a live preview's own capture took (the preview's capture paths only).
    pub fn captured(bytes: Vec<u8>) -> OwnerOnlyFrame {
        OwnerOnlyFrame(bytes)
    }

    /// The bytes, for the `tasks.preview` answer to FERB.app and nothing else.
    pub fn for_owner_app(&self) -> &[u8] {
        &self.0
    }

    /// The bytes, for the `tasks.preview` answer to the owner's own paired phone, inside the link only (PHN-454,
    /// owner: the phone's preview is unmasked too). The second and last reader: it takes a [`PairedPhoneLink`], which
    /// only the daemon's phone gate makes, for a session it holds paired, unlocked now and on a direct path or the
    /// owner's relay (never the public relay, PHN-444). A test checks no other caller or maker exists.
    pub fn for_paired_phone(&self, _link: &PairedPhoneLink) -> &[u8] {
        &self.0
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl std::fmt::Debug for OwnerOnlyFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "<owner-only preview frame, {} bytes>", self.0.len())
    }
}

/// The phone gate's word that one `tasks.preview` answer may carry its frame to a phone (PHN-454): the session's phone
/// is paired (not removed), its unlock window is open now, and the link names a direct path or the owner's relay as
/// the session's tier. Made only by ferb-daemon's phone gate (`Phones::preview_link`; a source-scan test holds that),
/// for one answer; it holds no bytes and is never kept.
#[derive(Debug)]
pub struct PairedPhoneLink {
    _made_by_the_phone_gate: (),
}

impl PairedPhoneLink {
    /// The phone gate's own maker, after its checks (ferb-daemon `phone::Phones::preview_link` only).
    pub fn after_the_phone_gate_s_checks() -> PairedPhoneLink {
        PairedPhoneLink {
            _made_by_the_phone_gate: (),
        }
    }
}

/// One preview frame and a note on how it was captured ("" mostly).
pub type PreviewFrame = (OwnerOnlyFrame, String);
pub type PreviewFrameFn =
    Arc<dyn Fn() -> Pin<Box<dyn Future<Output = Result<PreviewFrame>> + Send>> + Send + Sync>;
/// A live preview's surface resolved: its facts and its owner-only frame capture.
pub type ResolveFn = Arc<
    dyn Fn() -> Pin<Box<dyn Future<Output = Result<(Map<String, Value>, PreviewFrameFn)>> + Send>>
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
