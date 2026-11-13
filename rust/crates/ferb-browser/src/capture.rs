//! Pictures of a tab: a port of `_TabDriver.screenshot` (decision: L13: every secret field the
//! extension reports painted over before anything else sees it), `_unread` and `_native_boxes` (B6:
//! a page the extension couldn't read, masked where it can be), `_native_window` (I1: a tab the
//! debugger can't attach to, its whole window captured natively), `visual_snapshot` and `_carry`
//! (HAND-488: earlier element ids that keep working), `point` (a screenshot point to an element),
//! `owner_view` (HAND-497: the owner's payment card only), `pdf` (HAND-427, HAND-428) and
//! `recording_source` (a recording's frames).

use std::sync::Arc;

use ferb_core::types::{PageElement, TabRef};
use serde_json::{json, Map, Value};

use crate::driver::{motion, Driver, Tab, Visual};
use crate::error::{Error, Result};
use crate::mask::{self, ImageBox, Images, PageState};
use crate::ops::{hidden, py_int, version_at_least, window_bounds, IN_VIEW_LIST_CHARS};
use crate::protocol::PDF_EXTENSION_VERSION;

/// HAND-497: an unmasked picture of a page for the owner's eyes only (the payment card). It never
/// enters a tool result, a model's context, a trace or a store: no `Serialize`, no `Clone`, and its
/// `Debug` shows only its size.
pub struct OwnerOnlyImage {
    jpeg: Vec<u8>,
}

impl OwnerOnlyImage {
    pub fn jpeg(&self) -> &[u8] {
        &self.jpeg
    }
}

impl std::fmt::Debug for OwnerOnlyImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "<owner-only page picture, {} bytes>", self.jpeg.len())
    }
}

/// No image library connected: nothing can be painted.
struct NoImages;

impl Images for NoImages {
    fn size(&self, _: &[u8]) -> std::result::Result<(i64, i64), String> {
        Err("no image library".into())
    }
    fn mask(&self, _: &[u8], _: &[ImageBox]) -> std::result::Result<Vec<u8>, String> {
        Err("no image library".into())
    }
}

fn b64(data: &str) -> Result<Vec<u8>> {
    ferb_identity::py::b64decode(data)
        .ok_or_else(|| Error::driver("the browser's picture wasn't in FERB's format"))
}

fn text_of(v: &Value, key: &str) -> String {
    crate::rules::text_or_empty(v.get(key))
}

impl Driver {
    fn images(&self) -> Arc<dyn Images> {
        self.seams()
            .images
            .unwrap_or_else(|| Arc::new(NoImages) as Arc<dyn Images>)
    }

    /// The parts of a capture too big for one message, joined.
    async fn parts(
        &self,
        t: &Tab,
        op: &str,
        n: i64,
        extra: Map<String, Value>,
        timeout: f64,
    ) -> Result<String> {
        let mut out = String::new();
        for i in 0..n.max(0) {
            let mut args = json!({"tab_id": t.chrome_id, "part": i});
            for (k, v) in &extra {
                args[k] = v.clone();
            }
            let part = t.bridge.call(op, args, timeout, None).await?;
            let data = part
                .get("data")
                .ok_or_else(|| Error::driver("the browser's picture part had no data"))?;
            out.push_str(&ferb_py::value::py_str(data));
        }
        Ok(out)
    }

    /// `screenshot`: a JPEG of the tab with every secret field painted over (L13); `looks: false`
    /// after a read that timed out (HAND-435).
    pub async fn screenshot(&self, tab: &TabRef, looks: bool) -> Result<Vec<u8>> {
        let t = self.live_tab(tab)?;
        self.update(&tab.tab_id, |x| {
            x.masking.clear();
            x.native_shot = false;
            x.shot_timing = None;
        });
        let mut args = json!({"tab_id": t.chrome_id, "snap": t.snap_gen});
        if !looks {
            args["looks"] = false.into();
        }
        let v = t
            .bridge
            .call("screenshot", args, crate::bridge::CMD_TIMEOUT_S, None)
            .await?;
        let shot_timing = crate::rules::steps(v.get("steps"));
        let note = ferb_py::value::prefix(&text_of(&v, "input_note"), 300).to_string();
        self.update(&tab.tab_id, |x| {
            x.shot_timing = shot_timing;
            x.input_note = note;
        });
        if v.get("undebuggable").is_some_and(ferb_py::value::truthy) {
            let (jpeg, masking) = self.native_window(&t, &v).await?;
            self.update(&tab.tab_id, |x| {
                x.masking = masking;
                x.native_shot = true;
            });
            return Ok(jpeg);
        }
        if v.get("withheld").is_some_and(ferb_py::value::truthy) {
            return mask::painted(self.images().as_ref(), vec![], &v); // raises: nothing was captured
        }
        let jpeg = match v.get("data") {
            Some(d) => b64(&ferb_py::value::py_str(d))?,
            None => {
                let n = py_int(Some(v.get("parts").unwrap_or(&json!(0)))).unwrap_or(0);
                let joined = self
                    .parts(
                        &t,
                        "screenshot_part",
                        n,
                        Map::new(),
                        crate::bridge::CMD_TIMEOUT_S,
                    )
                    .await?;
                b64(&joined)?
            }
        };
        if v.get("unread").is_some_and(ferb_py::value::truthy) {
            let t = self.live_tab(tab)?;
            let (jpeg, masking) = self.unread(&t, jpeg, &v).await?;
            self.update(&tab.tab_id, |x| x.masking = masking);
            return Ok(jpeg);
        }
        mask::painted(self.images().as_ref(), jpeg, &v)
    }

    /// `_unread` (B6): a capture of a page the extension couldn't read, painted where it can be.
    async fn unread(&self, t: &Tab, jpeg: Vec<u8>, reply: &Value) -> Result<(Vec<u8>, String)> {
        let images = self.images();
        let why = ferb_py::value::prefix(
            &ferb_py::value::py_str(reply.get("unread").unwrap_or(&Value::Null)),
            200,
        )
        .to_string();
        let unpaintable =
            || Error::withheld("pixels withheld: the capture couldn't be painted over");
        let size = images.size(&jpeg).map_err(|_| unpaintable())?;
        let mut before_boxes: Vec<ImageBox> = vec![];
        if let Some(m) = reply.get("mask").filter(|m| !m.is_null()) {
            let before = m
                .as_object()
                .and_then(|m| PageState::parse(m.get("before")));
            let Some(before) = before else {
                return Err(Error::withheld(
                    "pixels withheld: the extension didn't say where the page's secret fields are",
                ));
            };
            let rects: Vec<mask::Rect> = before
                .items
                .iter()
                .filter(|i| i.paint)
                .map(|i| i.rect)
                .collect();
            before_boxes = mask::to_image_boxes(&rects, before.viewport, size);
        }
        let (ax_boxes, read, whole, ax_why) = self.native_boxes(t, reply.get("window"), size).await;
        let mut boxes = before_boxes.clone();
        boxes.extend(ax_boxes);
        let jpeg = if boxes.is_empty() {
            jpeg
        } else {
            images.mask(&jpeg, &boxes).map_err(|_| unpaintable())?
        };
        let mut done = vec![];
        if read {
            done.push(format!(
                "masked from Accessibility (the secret fields it finds in the tab's window, read just after the \
                 capture{})",
                if whole { "" } else { "; one it found has no box to paint" }
            ));
        }
        if !before_boxes.is_empty() {
            done.push("the fields the look before the capture found are painted".into());
        }
        let ext = format!("the browser extension couldn't read the page ({why})");
        let ax_why = ax_why.unwrap_or_else(|| "None".into());
        if !done.is_empty() {
            return Ok((
                jpeg,
                format!(
                    "{ext}: {}{}; anything else is left to text recognition (a secret neither can find could show)",
                    done.join("; "),
                    if read { String::new() } else { format!(" ({ax_why})") }
                ),
            ));
        }
        Ok((
            jpeg,
            format!(
                "the page's secret fields couldn't be located ({ext}; {ax_why}); masked by text recognition only (a \
                 secret it can't read as text could show)"
            ),
        ))
    }

    /// `_native_boxes`: Accessibility's secret fields for an unread tab, as image boxes; whether it
    /// read the page, whether every field had a box, and why it couldn't read it.
    async fn native_boxes(
        &self,
        t: &Tab,
        window: Option<&Value>,
        size: (i64, i64),
    ) -> (Vec<ImageBox>, bool, bool, Option<String>) {
        let hid = hidden(window);
        let Some(w) = window.and_then(Value::as_object).filter(|_| hid.is_empty()) else {
            return (
                vec![],
                false,
                false,
                Some(format!("{hid}, so Accessibility can't see it")),
            );
        };
        if w.get("emulated").is_some_and(ferb_py::value::truthy) {
            return (
                vec![],
                false,
                false,
                Some(
                    "the tab is laid out at FERB's own size, so Accessibility's places in its window don't match the \
                     picture"
                        .into(),
                ),
            );
        }
        let (native, pid) = (self.seams().native, t.bridge.browser_pid());
        let (Some(native), Some(pid)) = (native, pid) else {
            return (
                vec![],
                false,
                false,
                Some("Accessibility can't be asked about this browser".into()),
            );
        };
        let Some(bounds) = window_bounds(w) else {
            return (
                vec![],
                false,
                false,
                Some("the extension didn't say where the tab's window is".into()),
            );
        };
        let title = text_of(&Value::Object(w.clone()), "title");
        match native.secrets(pid, bounds, &title).await {
            None => (
                vec![],
                false,
                false,
                Some("Accessibility couldn't read the page in its window".into()),
            ),
            Some((rects, area, whole)) => {
                (mask::to_image_boxes(&rects, area, size), true, whole, None)
            }
        }
    }

    /// `_native_window` (I1): a tab the debugger can't attach to, its whole window captured natively.
    async fn native_window(&self, t: &Tab, reply: &Value) -> Result<(Vec<u8>, String)> {
        let reason = ferb_py::value::prefix(
            &reply
                .get("withheld")
                .filter(|w| ferb_py::value::truthy(w))
                .map_or_else(
                    || "the debugger can't attach to this page".to_string(),
                    ferb_py::value::py_str,
                ),
            300,
        )
        .to_string();
        let window = reply.get("window");
        let hid = hidden(window);
        let Some(w) = window.and_then(Value::as_object).filter(|_| hid.is_empty()) else {
            return Err(Error::withheld(format!(
                "pixels withheld: {reason}, and {hid}, so its window can't be captured in its place"
            )));
        };
        let pid = t.bridge.browser_pid();
        let bounds = window_bounds(w);
        let native = self.seams().native;
        let missing = if pid.is_none() {
            "the browser's process isn't known"
        } else if bounds.is_none() {
            "the extension didn't say where the tab's window is"
        } else if native.is_none() {
            "no native window capture is connected"
        } else {
            ""
        };
        let (Some(pid), Some(bounds), Some(native), true) =
            (pid, bounds, native, missing.is_empty())
        else {
            return Err(Error::withheld(format!(
                "pixels withheld: {reason}, and its window can't be captured in its place: {missing}"
            )));
        };
        let title = text_of(&Value::Object(w.clone()), "title");
        let (jpeg, note) = native.capture(pid, bounds, &title).await;
        let Some(jpeg) = jpeg else {
            return Err(Error::withheld(format!(
                "pixels withheld: {reason}, and its window couldn't be captured in its place: {note}"
            )));
        };
        let note = if note.is_empty() {
            "the secret fields Accessibility and the browser extension found are painted"
                .to_string()
        } else {
            note
        };
        Ok((
            jpeg,
            format!(
                "{reason}: this is a picture of the whole browser window showing it, masked as a mac.screenshot \
                 ({note}) and by text recognition"
            ),
        ))
    }

    /// `owner_view_supported` (GATE-579).
    pub fn owner_view_supported(&self, tab: &TabRef) -> Result<bool> {
        Ok(version_at_least(
            &self.live_tab(tab)?.bridge.version(),
            crate::protocol::OWNER_VIEW_EXTENSION_VERSION,
        ))
    }

    /// `owner_view` (HAND-497): one unmasked picture for the owner's payment card and nothing else.
    pub async fn owner_view(&self, tab: &TabRef) -> Result<OwnerOnlyImage> {
        let t = self.live_tab(tab)?;
        let v = match t
            .bridge
            .call("owner_view", json!({"tab_id": t.chrome_id}), 15.0, None)
            .await
        {
            Ok(v) => v,
            Err(e) if e.is_driver_error() => {
                return Err(Error::capture_unavailable(format!(
                    "couldn't capture the page ({})",
                    ferb_py::value::prefix(&e.message(), 200)
                )))
            }
            Err(e) => return Err(e),
        };
        match v
            .get("data")
            .and_then(Value::as_str)
            .filter(|d| !d.is_empty())
        {
            Some(d) => Ok(OwnerOnlyImage { jpeg: b64(d)? }),
            None => {
                let why = ferb_py::value::prefix(&text_of(&v, "unavailable"), 200).to_string();
                Err(Error::capture_unavailable(if why.is_empty() {
                    "couldn't capture the page".to_string()
                } else {
                    format!("couldn't capture the page ({why})")
                }))
            }
        }
    }

    /// `pdf` (HAND-427, HAND-428): the tab as Chrome prints it; a page with a secret field is refused.
    pub async fn pdf(&self, tab: &TabRef, landscape: bool) -> Result<Vec<u8>> {
        let t = self.live_tab(tab)?;
        let running = t.bridge.version();
        if !version_at_least(&running, PDF_EXTENSION_VERSION) {
            return Err(Error::refused(format!(
                "the FERB extension in this profile is {}; making a PDF needs {PDF_EXTENSION_VERSION} or later. Don't \
                 start a browser or app from the shell instead (it can't run there and crashes on the owner's screen). \
                 Tell the owner: their browser updates the extension by itself, or they can update it now on its \
                 extensions page (Developer mode, Update); then ask again",
                if running.is_empty() { "an older version" } else { &running }
            )));
        }
        let v = t
            .bridge
            .call(
                "pdf",
                json!({"tab_id": t.chrome_id, "landscape": landscape}),
                crate::bridge::CMD_TIMEOUT_S,
                None,
            )
            .await?;
        if let Some(w) = v.get("withheld").filter(|w| ferb_py::value::truthy(w)) {
            return Err(Error::refused(format!(
                "no PDF of this page: {}",
                ferb_py::value::prefix(&ferb_py::value::py_str(w), 300)
            )));
        }
        let data = match v.get("data") {
            Some(d) => ferb_py::value::py_str(d),
            None => {
                let n = py_int(Some(v.get("parts").unwrap_or(&json!(0)))).unwrap_or(0);
                self.parts(&t, "pdf_part", n, Map::new(), crate::bridge::CMD_TIMEOUT_S)
                    .await?
            }
        };
        let pdf = b64(&data)?;
        if !pdf.starts_with(b"%PDF-") {
            return Err(Error::refused(
                "the browser didn't return a PDF for this page",
            ));
        }
        Ok(pdf)
    }

    /// `visual_snapshot`: the snapshot the shot's points resolve against, then the picture; (shot
    /// id, JPEG, width, height).
    pub async fn visual_snapshot(&self, tab: &TabRef) -> Result<(String, Vec<u8>, i64, i64)> {
        let earlier = self
            .tab(&tab.tab_id)
            .map(|b| (b.snap_gen, b.snap_nav, b.elements.clone()));
        let failed = match self.snapshot(tab).await {
            Ok(_) => {
                if let Some((gen, nav, old)) = earlier {
                    self.live_tab(tab)?;
                    self.carry(&tab.tab_id, gen, nav, &old);
                }
                None
            }
            Err(e) => Some(e),
        };
        let stuck = failed
            .as_ref()
            .is_some_and(|e| e.reason() == Some("page_not_responding"));
        let jpeg = self.screenshot(tab, !stuck).await?;
        let t = self.live_tab(tab)?;
        if let Some(e) = failed.as_ref().filter(|_| t.masking.is_empty()) {
            return Err(e.clone());
        }
        let (width, height) = self
            .images()
            .size(&jpeg)
            .map_err(|_| Error::driver("the browser's picture couldn't be read"))?;
        let shot = format!("bs-{}", crate::pairing::token_hex(self.random(), 8));
        let visual = (failed.is_none() && !t.native_shot).then(|| Visual {
            shot: shot.clone(),
            snap_gen: t.snap_gen,
            snap_nav: t.snap_nav,
            at: tokio::time::Instant::now(),
            width,
            height,
        });
        let unread = visual.is_none().then(|| shot.clone());
        self.update(&tab.tab_id, |x| {
            x.visual = visual;
            x.unread_shot = unread;
        });
        Ok((shot, jpeg, width, height))
    }

    /// `_carry` (HAND-488): each earlier id whose node the new snapshot found again keeps working.
    pub(crate) fn carry(
        &self,
        alias: &str,
        gen: i64,
        nav: i64,
        old: &indexmap::IndexMap<String, (i64, i64, i64, PageElement)>,
    ) {
        self.update(alias, |t| carry(t, gen, nav, old));
    }

    /// `point`: a point in a recent screenshot to an element of the latest snapshot.
    pub async fn point(&self, tab: &TabRef, shot: &str, x: f64, y: f64) -> Result<PageElement> {
        let t = self.live_tab(tab)?;
        if t.unread_shot.as_deref() == Some(shot) {
            return Err(Error::refused(
                "a point on that screenshot can't be resolved to an element: it shows a page the browser extension \
                 can't read, or the whole browser window around it",
            ));
        }
        let v = t.visual.clone().filter(|v| {
            v.shot == shot
                && v.snap_gen == t.snap_gen
                && v.snap_nav == t.bridge.navs(t.chrome_id)
                && v.at.elapsed() <= std::time::Duration::from_secs(30)
        });
        let Some(v) = v else {
            return Err(Error::stale(
                "the screenshot is stale; take another browser screenshot",
            ));
        };
        if !((0.0..(v.width as f64)).contains(&x) && (0.0..(v.height as f64)).contains(&y)) {
            return Err(Error::refused("point outside the screenshot"));
        }
        let raw = t
            .bridge
            .call(
                "point",
                json!({"tab_id": t.chrome_id, "snap": t.snap_gen, "x": x, "y": y,
                       "image_width": v.width, "image_height": v.height}),
                crate::bridge::CMD_TIMEOUT_S,
                None,
            )
            .await?;
        if let Some(e) = raw.get("error").filter(|e| ferb_py::value::truthy(e)) {
            return Err(Error::stale(ferb_py::value::py_str(e)));
        }
        let malformed = || Error::driver("the browser's point wasn't in FERB's format");
        let element = raw
            .get("element")
            .and_then(Value::as_object)
            .ok_or_else(malformed)?;
        let local = py_int(element.get("id")).ok_or_else(malformed)?;
        let frame = py_int(raw.get("frame_id")).ok_or_else(malformed)?;
        let pointer = ferb_py::value::py_str(raw.get("pointer").ok_or_else(malformed)?);
        let t = self.live_tab(tab)?;
        let eid = format!("s{}e{}", t.snap_gen, t.elements.len() + 1);
        let info = crate::snapshot::element(&eid, &frame.to_string(), element)?;
        let m = motion(element);
        let (gen, kept) = (t.snap_gen, info.clone());
        self.update(&tab.tab_id, |x| {
            x.elements.insert(eid.clone(), (frame, gen, local, kept));
            if m.0.is_some() || m.1 {
                x.motion.insert(eid.clone(), m);
            }
            x.points.insert(eid, pointer);
        });
        Ok(info)
    }

    /// `recording_source`: what a recording of this tab captures (its target) and its frame capture.
    pub fn recording_source(
        &self,
        tab: &TabRef,
        owner: &str,
    ) -> Result<(Map<String, Value>, Recording)> {
        let t = self.live_tab(tab)?;
        let mut target = Map::new();
        target.insert("kind".into(), "tab".into());
        target.insert("browser".into(), tab.browser.clone().into());
        target.insert("profile".into(), tab.profile.clone().into());
        target.insert("tab_id".into(), t.chrome_id.into());
        target.insert("tab".into(), tab.tab_id.clone().into());
        let driver = self
            .me()
            .ok_or_else(|| Error::refused("recording tab identity changed"))?;
        Ok((
            target,
            Recording {
                driver,
                tab: tab.clone(),
                owner: owner.to_string(),
                chrome_id: t.chrome_id,
                bridge: Arc::clone(&t.bridge),
            },
        ))
    }
}

/// A recording's frame capture of one tab (`recording_source`'s `capture`).
pub struct Recording {
    driver: Arc<Driver>,
    tab: TabRef,
    owner: String,
    chrome_id: i64,
    bridge: Arc<crate::bridge::Bridge>,
}

impl Recording {
    /// One frame, with a note when the page couldn't be read (B6). `left`: the time a live preview has
    /// left (DMN-406), else the command's own limit.
    pub async fn capture(&self, left: Option<f64>) -> Result<(Vec<u8>, Option<String>)> {
        let d = &self.driver;
        d.owned_by(&self.tab.tab_id, &self.owner, true)?;
        let t = d.live_tab(&self.tab)?;
        if t.chrome_id != self.chrome_id || !Arc::ptr_eq(&t.bridge, &self.bridge) {
            return Err(Error::refused("recording tab identity changed"));
        }
        let timeout = left.map_or(crate::bridge::CMD_TIMEOUT_S, |l| l.max(0.5));
        let mut args = json!({"tab_id": t.chrome_id});
        if t.stuck {
            args["looks"] = false.into();
        }
        let result = t.bridge.call("record_frame", args, timeout, None).await?;
        if result
            .get("undebuggable")
            .is_some_and(ferb_py::value::truthy)
        {
            let (jpeg, note) = d.native_window(&t, &result).await?;
            return Ok((jpeg, Some(note)));
        }
        if result.get("withheld").is_some_and(ferb_py::value::truthy) {
            return mask::painted(d.images().as_ref(), vec![], &result).map(|j| (j, None));
        }
        let frame = match result.get("data") {
            Some(data) => b64(&ferb_py::value::py_str(data))?,
            None => {
                let malformed = || Error::driver("the browser's frame wasn't in FERB's format");
                let n = py_int(result.get("parts")).ok_or_else(malformed)?;
                let capture_id = result.get("capture_id").cloned().ok_or_else(malformed)?;
                let mut extra = Map::new();
                extra.insert("capture_id".into(), capture_id);
                let mut joined = String::new();
                for i in 0..n.max(0) {
                    let mut a = json!({"tab_id": t.chrome_id, "part": i});
                    a["capture_id"] = extra["capture_id"].clone();
                    let part = t.bridge.call("record_frame", a, timeout, None).await?;
                    joined.push_str(&ferb_py::value::py_str(
                        part.get("data").ok_or_else(malformed)?,
                    ));
                }
                b64(&joined)?
            }
        };
        if result.get("unread").is_some_and(ferb_py::value::truthy) {
            let (jpeg, note) = d.unread(&t, frame, &result).await?;
            return Ok((jpeg, Some(note)));
        }
        mask::painted(d.images().as_ref(), frame, &result).map(|j| (j, None))
    }
}

type Key = (i64, &'static str, Option<i64>, String, String, String);

fn key(frame: i64, e: &PageElement) -> Key {
    match e.backend_node {
        Some(n) => (
            frame,
            "b",
            Some(n),
            String::new(),
            String::new(),
            String::new(),
        ),
        None => (
            frame,
            "p",
            None,
            e.path.clone(),
            e.role.clone(),
            e.label.clone(),
        ),
    }
}

/// `_carry` on the tab itself.
fn carry(
    t: &mut Tab,
    gen: i64,
    nav: i64,
    old: &indexmap::IndexMap<String, (i64, i64, i64, PageElement)>,
) {
    if gen == t.snap_gen || old.is_empty() {
        return;
    }
    let mut new: std::collections::HashMap<Key, String> = std::collections::HashMap::new();
    for (eid, h) in &t.elements {
        new.insert(key(h.0, &h.3), eid.clone());
    }
    let (mut gone, mut found) = (0usize, std::collections::HashSet::new());
    if nav == t.snap_nav {
        for (eid, h) in old {
            let e = &h.3;
            let now = (e.backend_node.is_some() || !e.path.is_empty())
                .then(|| new.get(&key(h.0, e)).cloned())
                .flatten();
            match now {
                Some(now) if !t.elements.contains_key(eid) => {
                    let hit = t.elements[&now].clone();
                    t.elements.insert(eid.clone(), hit);
                    t.carried.insert(eid.clone());
                    found.insert(now);
                }
                _ => gone += usize::from(!t.elements.contains_key(eid)),
            }
        }
    } else {
        gone = old.len();
    }
    if gone == 0 {
        t.ids_note = [(
            "ids".to_string(),
            Value::from(format!(
                "the last snapshot's element ids still work (snapshot s{} found the same elements)",
                t.snap_gen
            )),
        )]
        .into_iter()
        .collect();
        return;
    }
    let (mut shown, mut used, mut more) = (vec![], 0usize, 0usize);
    for (eid, h) in &t.elements {
        let e = &h.3;
        if t.carried.contains(eid) || found.contains(eid) || !e.in_view {
            continue;
        }
        let item =
            json!({"id": eid, "role": e.role, "label": ferb_py::value::prefix(&e.label, 80)});
        let size = ferb_py::value::char_len(&ferb_py::json::dumps_utf8(&item));
        if used + size > IN_VIEW_LIST_CHARS {
            more += 1;
            continue;
        }
        shown.push(item);
        used += size;
    }
    let mut note = Map::new();
    note.insert(
        "ids".into(),
        format!(
            "element ids are now from snapshot s{}: {} earlier ids still work for the same elements, {gone} went (the \
             page changed)",
            t.snap_gen,
            t.carried.len()
        )
        .into(),
    );
    note.insert("in_view_elements".into(), Value::Array(shown));
    if more > 0 {
        note.insert("more_in_view".into(), more.into());
    }
    t.ids_note = note;
}
