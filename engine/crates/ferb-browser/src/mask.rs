//! Painting a tab capture's secret fields over (decision: L13): the geometry of
//! `ferb/act/evidence/mask.py` (the extension's mask state before and after a capture, what must be
//! painted, image boxes) and `ext_bridge.painted`. Decoding, filling and encoding the JPEG is the
//! platform's ([`Images`], Quartz on the Mac).

use serde_json::Value;

use crate::error::{Error, Result};

/// x, y, w, h in CSS px or points.
pub type Rect = (f64, f64, f64, f64);
/// x, y, w, h in image pixels, top-left origin.
pub type ImageBox = (i64, i64, i64, i64);

/// The image library (`mask.size`, `mask.mask_secret_fields`): Quartz on the Mac.
pub trait Images: Send + Sync {
    /// The picture's pixel size; `Err` when it isn't a picture (Python's `ValueError`).
    fn size(&self, image: &[u8]) -> std::result::Result<(i64, i64), String>;
    /// The picture with each box filled opaque black, re-encoded; `Err` when it can't be painted.
    fn mask(&self, image: &[u8], boxes: &[ImageBox]) -> std::result::Result<Vec<u8>, String>;
}

/// One thing a page's mask state names (`PageItem`).
#[derive(Clone, Debug, PartialEq)]
pub struct PageItem {
    pub key: String,
    pub rect: Rect,
    pub paint: bool,
    pub child: Option<String>,
}

/// A page's mask state at one moment (`PageState`).
#[derive(Clone, Debug, PartialEq)]
pub struct PageState {
    pub top: String,
    pub viewport: Rect,
    pub items: Vec<PageItem>,
}

/// `_rect`: four finite numbers (never booleans).
pub fn rect(v: Option<&Value>) -> Option<Rect> {
    let a = v?.as_array()?;
    if a.len() != 4 {
        return None;
    }
    let n: Vec<f64> = a
        .iter()
        .map(|x| x.as_f64().filter(|f| f.is_finite()))
        .collect::<Option<_>>()?;
    Some((n[0], n[1], n[2], n[3]))
}

impl PageState {
    /// `PageState.parse`: the extension's report, `None` when anything in it is malformed.
    pub fn parse(raw: Option<&Value>) -> Option<PageState> {
        let raw = raw?.as_object()?;
        let top = raw.get("top")?.as_str()?.to_string();
        let items = raw.get("items")?.as_array()?;
        let viewport = rect(raw.get("viewport")).filter(|v| v.2 > 0.0 && v.3 > 0.0)?;
        let mut out = vec![];
        for i in items {
            let i = i.as_object()?;
            let rect = rect(i.get("rect"))?;
            let key = i.get("key")?.as_str()?.to_string();
            let paint = i.get("paint")?.as_bool()?;
            let child = match i.get("child") {
                None | Some(Value::Null) => None,
                Some(Value::String(s)) => Some(s.clone()),
                Some(_) => return None,
            };
            out.push(PageItem {
                key,
                rect,
                paint,
                child,
            });
        }
        Some(PageState {
            top,
            viewport,
            items: out,
        })
    }
}

fn holds(a: Rect, b: Rect) -> bool {
    a.0 <= b.0 && a.1 <= b.1 && a.0 + a.2 >= b.0 + b.2 && a.1 + a.3 >= b.1 + b.3
}

/// `page_masks`: what one capture must paint over, from the looks before and after it, and the
/// viewport the rects are in; withheld when a secret field's place is unknown.
pub fn page_masks(before: &PageState, after: &PageState) -> Result<(Vec<Rect>, Rect)> {
    if before.top != after.top || before.viewport != after.viewport {
        return Err(Error::withheld(
            "pixels withheld: the page changed during capture",
        ));
    }
    let find = |s: &PageState, k: &str| s.items.iter().rev().find(|i| i.key == k).cloned();
    let mut keys: Vec<&str> = before
        .items
        .iter()
        .chain(&after.items)
        .map(|i| i.key.as_str())
        .collect();
    keys.sort_unstable();
    keys.dedup();
    let (mut still, mut loose) = (vec![], vec![]);
    for key in keys {
        match (find(before, key), find(after, key)) {
            (Some(x), Some(y)) => {
                if x.rect != y.rect {
                    return Err(Error::withheld(
                        "pixels withheld: a secret field or frame moved during capture",
                    ));
                }
                if x.paint || y.paint || x.child != y.child {
                    still.push(x.rect);
                }
            }
            (Some(only), None) | (None, Some(only)) => {
                if only.paint {
                    loose.push(only.rect);
                }
            }
            (None, None) => {}
        }
    }
    for r in &loose {
        if !still.iter().any(|s| holds(*s, *r)) {
            return Err(Error::withheld(
                "pixels withheld: a secret field appeared or went during capture",
            ));
        }
    }
    still.extend(loose);
    Ok((still, before.viewport))
}

/// `to_image_boxes`: screen or page boxes onto the capture's pixels, rounded outward; a box outside
/// the bounds or with no area is dropped.
pub fn to_image_boxes(rects: &[Rect], bounds: Rect, size: (i64, i64)) -> Vec<ImageBox> {
    let (bx, by, bw, bh) = bounds;
    if bw <= 0.0 || bh <= 0.0 {
        return vec![];
    }
    let (kx, ky) = (size.0 as f64 / bw, size.1 as f64 / bh);
    let mut out = vec![];
    for &(x, y, w, h) in rects {
        let (left, top) = (x.max(bx), y.max(by));
        let (right, bottom) = ((x + w).min(bx + bw), (y + h).min(by + bh));
        if w <= 0.0 || h <= 0.0 || right <= left || bottom <= top {
            continue;
        }
        let (px, py) = (
            ((left - bx) * kx).floor() as i64,
            ((top - by) * ky).floor() as i64,
        );
        out.push((
            px,
            py,
            ((right - bx) * kx).ceil() as i64 - px,
            ((bottom - by) * ky).ceil() as i64 - py,
        ));
    }
    out
}

/// `painted`: a tab capture with every secret field the extension reported painted over, or
/// withheld (`Withheld`) when that can't be done.
pub fn painted(images: &dyn Images, jpeg: Vec<u8>, reply: &Value) -> Result<Vec<u8>> {
    if let Some(w) = reply.get("withheld").filter(|w| ferb_py::value::truthy(w)) {
        return Err(Error::withheld(format!(
            "pixels withheld: {}",
            ferb_py::value::py_str(w)
        )));
    }
    let (before, after) = match reply.get("mask").and_then(Value::as_object) {
        Some(m) => (
            PageState::parse(m.get("before")),
            PageState::parse(m.get("after")),
        ),
        None => (None, None),
    };
    let (Some(before), Some(after)) = (before, after) else {
        return Err(Error::withheld(
            "pixels withheld: the extension didn't say where the page's secret fields are",
        ));
    };
    let (rects, viewport) = page_masks(&before, &after)?;
    if rects.is_empty() {
        return Ok(jpeg);
    }
    let unpaintable = || Error::withheld("pixels withheld: the capture couldn't be painted over");
    let size = images.size(&jpeg).map_err(|_| unpaintable())?;
    let boxes = to_image_boxes(&rects, viewport, size);
    if boxes.is_empty() {
        return Ok(jpeg);
    }
    images.mask(&jpeg, &boxes).map_err(|_| unpaintable())
}
