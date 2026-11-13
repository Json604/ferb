//! Which boxes a window capture paints over (L13): the evidence store's page and box arithmetic
//! (ferb/act/evidence/mask.py) and the driver's two-look rules (ferb/act/mac/driver.py:55-264). Must not decide
//! what is secret: the boxes come from the Accessibility walk (password fields) and the browser extension's
//! secret-field rects. A capture is never refused for what it shows; only boxes that can't be painted withhold it.

use serde_json::Value;

use crate::ax::{outermost, AxTree, Rect};

/// x, y, w, h in image pixels, top-left origin.
pub type ImgBox = (i64, i64, i64, i64);

pub const OCR_ONLY: &str =
    "masked by text recognition only (a secret it can't read as text could show)";
pub const OCR_REST: &str =
    "anything else is left to text recognition (a secret it can't read as text could show)";
pub const UNPAINTABLE: &str = "the capture couldn't be painted over; pixels withheld";
pub const EXTENSION_UNREAD: &str =
    "not paired with this window's profile, an incognito window, or a page scripts can't reach";
pub const AX_NO_AREA: &str = "Accessibility shows no page area";
pub const AX_EMPTY: &str = "Accessibility's page area is empty";
pub const OTHER_PAGE: &str =
    "the paired profile's window at this place shows another page (its active tab's title \
isn't the captured window's)";

fn page_unread(why: &str) -> String {
    format!(
        "the page's secret fields couldn't be located (neither the browser extension nor Accessibility could read the \
page this window shows: {why}); {OCR_ONLY}"
    )
}
fn window_unread() -> String {
    format!(
        "the window's secret fields couldn't be located (Accessibility couldn't read this window when it was \
captured); {OCR_ONLY}"
    )
}
fn window_half_read() -> String {
    format!(
        "Accessibility couldn't read this window in one look around the capture: the secret fields the other look \
found are painted where it saw them; {OCR_REST}"
    )
}
fn field_unplaced() -> String {
    format!("a secret field Accessibility found has no place on screen to paint; {OCR_ONLY}")
}
fn field_moved() -> String {
    format!(
        "a secret field moved, appeared or went during the capture: painted where it was before and after; {OCR_REST}"
    )
}
fn page_moved() -> String {
    format!(
        "the page changed during the capture: its secret fields are painted where they were before and after; \
{OCR_REST}"
    )
}
pub fn page_unplaced() -> String {
    format!(
        "the page's secret fields couldn't be placed on the window (Accessibility shows no page area); {OCR_ONLY}"
    )
}
fn page_part_placed() -> String {
    format!(
        "the page changed during the capture and one look's secret fields couldn't be placed on the window \
(Accessibility showed no page area): the others are painted where they were; {OCR_REST}"
    )
}
fn one_look(why: &str) -> String {
    format!(
        "the browser extension read the page in one look around the capture only ({why}): its secret fields are \
painted where it saw them; a field only the other look could show is left to text recognition (a secret it can't \
read as text could show)"
    )
}
fn ax_only(why: &str) -> String {
    format!("masked from Accessibility only: the browser extension couldn't read this page ({why})")
}

/// A capture whose secret fields couldn't be placed or painted: only its pixels are withheld.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("{0}")]
pub struct Withheld(pub String);

/// One thing a page's mask state names (mask.py:19).
#[derive(Debug, Clone, PartialEq)]
pub struct PageItem {
    pub key: String,
    pub rect: Rect,
    pub paint: bool,
    pub child: Option<String>,
}

/// A browser page's mask state at one moment (mask.py:32).
#[derive(Debug, Clone, PartialEq)]
pub struct PageState {
    pub top: String,
    pub viewport: Rect,
    pub items: Vec<PageItem>,
}

fn rect_of(v: Option<&Value>) -> Option<Rect> {
    let a = v?.as_array()?;
    if a.len() != 4 {
        return None;
    }
    let mut out = [0.0; 4];
    for (i, x) in a.iter().enumerate() {
        // bool is excluded (serde has no bool-as-number); non-finite numbers can't be in JSON
        let n = x.as_f64()?;
        if !n.is_finite() {
            return None;
        }
        out[i] = n;
    }
    Some((out[0], out[1], out[2], out[3]))
}

impl PageState {
    /// mask.py:41: the extension's report, or None when anything in it is malformed.
    pub fn parse(raw: &Value) -> Option<PageState> {
        let o = raw.as_object()?;
        let top = o.get("top")?.as_str()?;
        let items = o.get("items")?.as_array()?;
        let viewport = rect_of(o.get("viewport"))?;
        if viewport.2 <= 0.0 || viewport.3 <= 0.0 {
            return None;
        }
        let mut out = vec![];
        for i in items {
            let io = i.as_object()?;
            let rect = rect_of(io.get("rect"))?;
            let key = io.get("key")?.as_str()?;
            let paint = io.get("paint")?.as_bool()?;
            let child = match io.get("child") {
                None | Some(Value::Null) => None,
                Some(Value::String(s)) => Some(s.clone()),
                _ => return None,
            };
            out.push(PageItem {
                key: key.into(),
                rect,
                paint,
                child,
            });
        }
        Some(PageState {
            top: top.into(),
            viewport,
            items: out,
        })
    }
}

fn holds(a: &Rect, b: &Rect) -> bool {
    a.0 <= b.0 && a.1 <= b.1 && a.0 + a.2 >= b.0 + b.2 && a.1 + a.3 >= b.1 + b.3
}

/// mask.py:65: what one capture of a page must paint, from the looks before and after it.
pub fn page_masks(before: &PageState, after: &PageState) -> Result<(Vec<Rect>, Rect), Withheld> {
    if before.top != after.top || before.viewport != after.viewport {
        return Err(Withheld(
            "pixels withheld: the page changed during capture".into(),
        ));
    }
    let find = |s: &PageState, k: &str| s.items.iter().rev().find(|i| i.key == k).cloned();
    let mut keys: Vec<&str> = before
        .items
        .iter()
        .chain(after.items.iter())
        .map(|i| i.key.as_str())
        .collect();
    keys.sort();
    keys.dedup();
    let mut still = vec![];
    let mut loose = vec![];
    for key in keys {
        match (find(before, key), find(after, key)) {
            (Some(x), Some(y)) => {
                if x.rect != y.rect {
                    return Err(Withheld(
                        "pixels withheld: a secret field or frame moved during capture".into(),
                    ));
                }
                if x.paint || y.paint || x.child != y.child {
                    still.push(x.rect);
                }
            }
            (x, y) => {
                if let Some(only) = x.or(y) {
                    if only.paint {
                        loose.push(only.rect);
                    }
                }
            }
        }
    }
    for r in &loose {
        if !still.iter().any(|s| holds(s, r)) {
            return Err(Withheld(
                "pixels withheld: a secret field appeared or went during capture".into(),
            ));
        }
    }
    still.extend(loose);
    Ok((still, before.viewport))
}

/// mask.py:132: screen-point boxes -> image-pixel boxes on a capture at `size`, rounded outward.
pub fn to_image_boxes(rects: &[Rect], bounds: Rect, size: (i64, i64)) -> Vec<ImgBox> {
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
        let px = ((left - bx) * kx).floor() as i64;
        let py = ((top - by) * ky).floor() as i64;
        out.push((
            px,
            py,
            ((right - bx) * kx).ceil() as i64 - px,
            ((bottom - by) * ky).ceil() as i64 - py,
        ));
    }
    out
}

/// What the browser extension says about the page a browser window shows (driver.py:93).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WebSecrets {
    pub state: Option<PageState>,
    pub why: String,
    /// A profile didn't answer: not asked again for this capture.
    pub late: bool,
    /// The answering window's active tab (URL, title), protocol 15.
    pub tab: Option<(String, String)>,
}

impl WebSecrets {
    pub fn complete(&self) -> bool {
        self.state.is_some()
    }

    /// driver.py:107: a `window_secrets` answer; anything malformed is a page nobody could read.
    pub fn from_reply(r: &Value) -> WebSecrets {
        let state = if r.get("complete") == Some(&Value::Bool(true)) {
            r.get("state").and_then(PageState::parse)
        } else {
            None
        };
        let why = match r.get("reason") {
            Some(Value::String(s)) if !s.is_empty() => {
                format!(
                    "its profile couldn't read the page: {}",
                    s.chars().take(160).collect::<String>()
                )
            }
            _ => "its profile couldn't read the page".into(),
        };
        let tab = match r.get("tab") {
            Some(Value::Object(t)) => match (t.get("url"), t.get("title")) {
                (Some(Value::String(u)), Some(Value::String(title))) => {
                    Some((u.chars().take(4096).collect(), title.clone()))
                }
                _ => None,
            },
            _ => None,
        };
        let why = if state.is_some() { String::new() } else { why };
        WebSecrets {
            state,
            why,
            late: false,
            tab,
        }
    }
}

/// One look at a window: its AX walk and the extension's page.
pub type Look = (Option<AxTree>, Option<WebSecrets>);

/// driver.py:87.
pub fn same_title(a: &str, b: &str) -> bool {
    let words = |s: &str| {
        s.split(ferb_sanitize::parity::is_space)
            .filter(|w| !w.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    };
    words(a) == words(b)
}

fn union(groups: &[&[Rect]]) -> Vec<Rect> {
    let mut out: Vec<Rect> = vec![];
    for g in groups {
        for b in g.iter() {
            if !out.contains(b) {
                out.push(*b);
            }
        }
    }
    out
}

/// driver.py:212: the paintable secret-field boxes of a walk, and whether that is every one it found.
pub fn ax_boxes(tree: &AxTree) -> (Vec<Rect>, bool) {
    let boxes: Vec<Rect> = match &tree.secret_rects {
        Some(r) => r.clone(),
        None => tree
            .nodes
            .iter()
            .filter(|n| n.secret)
            .map(|n| n.rect)
            .collect(),
    };
    let placed: Vec<Rect> = boxes
        .iter()
        .filter(|b| b.2 > 0.0 && b.3 > 0.0)
        .copied()
        .collect();
    let whole = !(tree.secure == Some(true) && boxes.is_empty()) && placed.len() == boxes.len();
    (placed, whole)
}

fn sorted(v: &[Rect]) -> Vec<Rect> {
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    s
}

fn look_items(p: &PageState, other: Option<&PageState>) -> Vec<Rect> {
    p.items
        .iter()
        .filter(|i| {
            i.paint
                || other.is_some_and(|o| {
                    o.items
                        .iter()
                        .rev()
                        .find(|t| t.key == i.key)
                        .is_some_and(|t| t.paint || t.child != i.child)
                })
        })
        .map(|i| i.rect)
        .collect()
}

fn placed(rects: &[Rect], p: &PageState, t: &AxTree) -> (Vec<Rect>, bool) {
    match page_on_screen(rects, p.viewport, &outermost(&t.web_areas)) {
        Ok(r) => (r, true),
        Err(_) => (vec![], false),
    }
}

/// driver.py:235: the page's secret-field rects in screen points, on the AX web area that fits its viewport best.
pub fn page_on_screen(
    rects: &[Rect],
    viewport: Rect,
    areas: &[Rect],
) -> Result<Vec<Rect>, Withheld> {
    if rects.is_empty() {
        return Ok(vec![]);
    }
    let (ox, oy, vw, vh) = viewport;
    let fits: Vec<Rect> = areas
        .iter()
        .filter(|a| a.2 > 0.0 && a.3 > 0.0)
        .copied()
        .collect();
    if fits.is_empty() {
        return Err(Withheld(page_unplaced()));
    }
    let misfit = |a: &Rect| ((a.2 / vw) / (a.3 / vh)).ln().abs();
    let best = fits.iter().map(misfit).fold(f64::INFINITY, f64::min);
    let mut out = vec![];
    for a in fits.iter().filter(|a| misfit(a) == best) {
        let (ax, ay, aw, ah) = *a;
        let (kx, ky) = (aw / vw, ah / vh);
        out.extend(
            rects
                .iter()
                .map(|&(x, y, w, h)| (ax + (x - ox) * kx, ay + (y - oy) * ky, w * kx, h * ky)),
        );
    }
    Ok(out)
}

fn page_on_screen_looks(
    p1: &PageState,
    p2: &PageState,
    t1: &AxTree,
    t2: &AxTree,
) -> (Vec<Rect>, String) {
    let (a1, a2) = (outermost(&t1.web_areas), outermost(&t2.web_areas));
    let mut still = None;
    if sorted(&a1) == sorted(&a2) {
        still = page_masks(p1, p2).ok();
    }
    if let Some((rects, viewport)) = still {
        return match page_on_screen(&rects, viewport, &a2) {
            Ok(r) => (r, String::new()),
            Err(_) => (vec![], page_unplaced()),
        };
    }
    let (mut r1, ok1) = placed(&look_items(p1, Some(p2)), p1, t1);
    let (r2, ok2) = placed(&look_items(p2, Some(p1)), p2, t2);
    r1.extend(r2);
    let note = if ok1 && ok2 {
        page_moved()
    } else if ok1 || ok2 {
        page_part_placed()
    } else {
        page_unplaced()
    };
    (r1, note)
}

fn dedup_notes(notes: Vec<String>) -> String {
    let mut out: Vec<String> = vec![];
    for n in notes {
        if !out.contains(&n) {
            out.push(n);
        }
    }
    out.join("; ")
}

/// driver.py:123: the screen-point boxes to paint on one capture, from the looks before and after it, and a note
/// on how its secret fields were found when worth saying.
pub fn window_masks(before: &Look, after: &Look, browser: bool) -> (Vec<Rect>, String) {
    let ((t1, w1), (t2, w2)) = (before, after);
    let readable = |t: &Option<AxTree>| t.as_ref().is_some_and(|t| t.secure.is_some());
    if !readable(t1) || !readable(t2) {
        let read: Vec<(&AxTree, &Option<WebSecrets>)> = [(t1, w1), (t2, w2)]
            .into_iter()
            .filter(|(t, _)| readable(t))
            .map(|(t, w)| (t.as_ref().expect("readable"), w))
            .collect();
        let boxes: Vec<Vec<Rect>> = read.iter().map(|(t, _)| ax_boxes(t).0).collect();
        let refs: Vec<&[Rect]> = boxes.iter().map(|b| b.as_slice()).collect();
        let mut masks = union(&refs);
        let mut notes = vec![if read.is_empty() {
            window_unread()
        } else {
            window_half_read()
        }];
        if !read.iter().all(|(t, _)| ax_boxes(t).1) {
            notes.push(field_unplaced());
        }
        for (t, w) in &read {
            if let Some(state) = w.as_ref().and_then(|w| w.state.as_ref()) {
                let (rects, ok) = placed(&look_items(state, None), state, t);
                masks.extend(rects);
                if !ok {
                    notes.push(page_unplaced());
                }
            }
        }
        if let Some((first, _)) = read.first() {
            for (unread, w) in [(t1, w1), (t2, w2)] {
                if !readable(unread) {
                    if let Some(state) = w.as_ref().and_then(|w| w.state.as_ref()) {
                        let (rects, ok) = placed(&look_items(state, None), state, first);
                        masks.extend(rects);
                        if !ok {
                            notes.push(page_unplaced());
                        }
                    }
                }
            }
        }
        return (masks, dedup_notes(notes));
    }
    let (t1, t2) = (t1.as_ref().expect("read"), t2.as_ref().expect("read"));
    let ((ax1, whole1), (ax2, whole2)) = (ax_boxes(t1), ax_boxes(t2));
    let mut notes: Vec<String> = if !(whole1 && whole2) {
        vec![field_unplaced()]
    } else if sorted(&ax1) != sorted(&ax2) {
        vec![field_moved()]
    } else {
        vec![]
    };
    let mut masks = union(&[&ax1, &ax2]);
    let p1 = w1.as_ref().and_then(|w| w.state.as_ref());
    let p2 = w2.as_ref().and_then(|w| w.state.as_ref());
    if let (Some(p1), Some(p2)) = (p1, p2) {
        let (page, note) = page_on_screen_looks(p1, p2, t1, t2);
        masks.extend(page);
        if !note.is_empty() {
            notes.push(note);
        }
        return (masks, notes.join("; "));
    }
    let mut was_placed = false;
    for (p, t) in [(p1, t1), (p2, t2)] {
        if let Some(p) = p {
            let (rects, ok) = placed(&look_items(p, None), p, t);
            masks.extend(rects);
            was_placed = ok;
        }
    }
    if browser {
        let why2 = w2.as_ref().map(|w| w.why.clone()).unwrap_or_default();
        let why1 = w1.as_ref().map(|w| w.why.clone()).unwrap_or_default();
        let why = if !why2.is_empty() {
            why2
        } else if !why1.is_empty() {
            why1
        } else {
            EXTENSION_UNREAD.to_string()
        };
        let ax_why = [t1, t2].iter().find_map(|t| {
            let outer = outermost(&t.web_areas);
            if !t.has_web || outer.iter().any(|a| t.empty_web_areas.contains(a)) {
                Some(if t.has_web { AX_EMPTY } else { AX_NO_AREA })
            } else {
                None
            }
        });
        notes.push(if was_placed {
            one_look(&why)
        } else if let Some(ax_why) = ax_why {
            page_unread(&format!("{why}; {ax_why}"))
        } else {
            ax_only(&why)
        });
    }
    (masks, notes.join("; "))
}

/// driver.py:219: the captured browser window's active tab as the extension named it, latest look first, only when
/// its title is one the window server gave the captured window.
pub fn page_tab(titles: &[String], looks: &[&Look]) -> Option<(String, String)> {
    let known: Vec<&String> = titles.iter().filter(|t| !t.is_empty()).collect();
    for (_, web) in looks.iter().rev() {
        if let Some(tab) = web.as_ref().and_then(|w| w.tab.as_ref()) {
            if known.iter().any(|t| same_title(&tab.1, t)) {
                return Some(tab.clone());
            }
        }
    }
    None
}
