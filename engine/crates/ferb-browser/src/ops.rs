//! The tab driver's page operations: a port of the rest of `ext_bridge.py`'s `_TabDriver`: `act`
//! with its structural facts (decision: GATE-202: whether it submits a form, the fields' kinds, the
//! page's amounts; facts, never intent), screenshot points and gestures, `html` (HAND-496), `find`
//! (HAND-484), `scroll` (HAND-510), `media` and the tab's sound (L26, HAND-207), `wait`, a human
//! check's facts, web-terminal input (D31), the element identity a ledger keys an act by (P19),
//! where FERB's cursor points (L33), and a loop's rest between rounds (L31).
//!
//! Seams: [`Seams`] (the daemon's downloads and uploads, the Mac clipboard, the image library and
//! the native captures), set on the driver once.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use ferb_core::interfaces::BoxFuture;
use ferb_core::types::{
    ActionFacts, ActionResult, BrowserAction, BrowserActionKind, Dict, ElementRef, FieldKind,
    PageElement, TabRef, TerminalInput, WaitCondition,
};
use serde_json::{json, Map, Value};

use crate::driver::{Driver, Tab};
use crate::error::{Error, Result};
use crate::mask::{Images, Rect};
use crate::protocol::{self, INPUT_EXTENSION_VERSION, MIN_EXTENSION_VERSION};
use crate::rules;

/// `TRUSTED_KINDS`: real input by default.
pub const TRUSTED_KINDS: [&str; 6] = ["click", "dblclick", "rightclick", "hover", "type", "press"];
/// `CENTRING`: acts that scroll their element to the viewport's centre first.
pub const CENTRING: [&str; 4] = ["click", "dblclick", "rightclick", "hover"];
pub const CENTRING_REAL: [&str; 2] = ["type", "press"];
/// `POINTER_KINDS`: what a screenshot point takes.
pub const POINTER_KINDS: [&str; 5] = ["click", "dblclick", "rightclick", "hover", "drag"];
/// `STILL`: acts that never scroll the page.
pub const STILL: [&str; 2] = ["select", "upload"];
/// HAND-447: acts that put input into the page.
pub const WRITES: [&str; 5] = ["type", "paste", "select", "press", "upload"];
/// `IN_VIEW_LIST_CHARS` (HAND-488).
pub const IN_VIEW_LIST_CHARS: usize = 8000;
/// D31 (`ferb/policy/terminal.py`): a key that runs a terminal's command line.
pub const RETURN_KEYS: [&str; 2] = ["return", "enter"];
/// D31 (`terminal.py` `UNASKED`): the refusal when a call carries no Yes for that terminal input.
pub const UNASKED: &str = "FERB types into a terminal only after the owner's Yes to that exact input (D31): this input \
would reach {why} in {app}, and the call carries no Yes for it, so nothing was typed. Run commands with FERB's shell \
tool; if FERB's sandbox blocks a command you need, request the escape hatch (shell.rules lists what it lifts), which \
asks the owner once";

/// The daemon's download records (`downloads.Downloads`, D35), as the drivers use them.
pub trait DownloadsView: Send + Sync {
    /// A transfer in this tab is under way.
    fn busy(&self, br: &Arc<crate::bridge::Bridge>, chrome_id: i64) -> bool;
    /// Waits (at most `timeout` s) for this tab's transfers to end.
    fn settle(
        &self,
        br: &Arc<crate::bridge::Bridge>,
        chrome_id: i64,
        timeout: f64,
    ) -> BoxFuture<'_, ()>;
    /// This tab's downloads the worker hasn't seen yet (marked seen).
    fn unreported(
        &self,
        br: &Arc<crate::bridge::Bridge>,
        chrome_id: i64,
    ) -> Vec<Map<String, Value>>;
    /// Every download of these tasks in this identity's profiles.
    fn facts_for(&self, tasks: &[String], identity: &str) -> Vec<Map<String, Value>>;
    /// browser.download_task_max_gb in bytes.
    fn task_bytes(&self) -> i64;
    /// browser.download_total_gb in bytes.
    fn total_bytes(&self) -> i64;
}

/// FERB's own copies of files the browser uploads (`uploads.Uploads.stage_all`, rule 17).
pub trait Staging: Send + Sync {
    /// Copies each open file (its handle, the path given, its sha256) into FERB's uploads folder for
    /// this task tree and tab; the copies' paths.
    fn stage_all(
        &self,
        tree: &str,
        sources: &[(i32, String, String)],
        tab: &str,
    ) -> std::result::Result<Vec<String>, String>;
    /// That tab is gone: the copies it showed need not stay. `true` when it showed a copy of a tree
    /// that already ended (the daemon then collects).
    fn closed(&self, tab: &str) -> bool;
}

/// What the platform reads natively (B6, I1, L33): Accessibility's secret fields in a browser
/// window, a capture of that window, and where a page rect is on screen.
pub trait Native: Send + Sync {
    /// (secret fields in screen points, the page area, whether every field has a box); `None`: it
    /// can't tell.
    fn secrets(
        &self,
        pid: i32,
        bounds: Rect,
        title: &str,
    ) -> BoxFuture<'_, Option<(Vec<Rect>, Rect, bool)>>;
    /// The window captured (JPEG) and its masking note, or `None` and why.
    fn capture(
        &self,
        pid: i32,
        bounds: Rect,
        title: &str,
    ) -> BoxFuture<'_, (Option<Vec<u8>>, String)>;
    /// The rect in screen points on the page area of the window showing the tab.
    fn place(
        &self,
        rect: Rect,
        viewport: Rect,
        pid: i32,
        bounds: Rect,
        title: &str,
    ) -> BoxFuture<'_, Option<Rect>>;
}

/// The Mac clipboard: (change count, text, concealed) (D31 round 2, O2).
pub type Clipboard = Arc<dyn Fn() -> (i64, Option<String>, bool) + Send + Sync>;

/// What the daemon supplies to the drivers besides the tasks.
#[derive(Clone, Default)]
pub struct Seams {
    pub downloads: Option<Arc<dyn DownloadsView>>,
    pub uploads: Option<Arc<dyn Staging>>,
    pub clipboard: Option<Clipboard>,
    pub images: Option<Arc<dyn Images>>,
    pub native: Option<Arc<dyn Native>>,
    /// The upload files' reading (the platform's file handles; [`crate::upload::Files`]).
    pub files: Option<Arc<dyn crate::upload::Files>>,
    /// The profiles the daemon knows (`known`, discovery, `unseen`).
    pub profiles: Option<Arc<dyn crate::owner::ProfilesView>>,
    /// FERB's inbox (its address, the feed), for FERB's own profile.
    pub inbox: Option<crate::owner::Inbox>,
}

fn text_of(v: &Value, key: &str) -> String {
    rules::text_or_empty(v.get(key))
}

/// `str(v.get(key, ""))`: Python's text of the value, "" when absent (`None` shows as "None").
pub(crate) fn str_or_empty(v: &Value, key: &str) -> String {
    v.get(key).map(ferb_py::value::py_str).unwrap_or_default()
}

fn without(v: &Value, keys: &[&str]) -> Dict {
    v.as_object()
        .map(|m| {
            m.iter()
                .filter(|(k, _)| !keys.contains(&k.as_str()))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect()
        })
        .unwrap_or_default()
}

/// `version_ok(running, minimum)`; a version Python's `int()` refuses counts as too old (Python
/// raises there: fixed against Python).
pub fn version_at_least(running: &str, minimum: &str) -> bool {
    protocol::version_ok(running, minimum).unwrap_or(false)
}

/// Python's `int(v)` for the numbers the extension sends.
pub(crate) fn py_int(v: Option<&Value>) -> Option<i64> {
    match v? {
        Value::Bool(b) => Some(i64::from(*b)),
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f.trunc() as i64)),
        Value::String(s) => ferb_py::value::strip(s).replace('_', "").parse().ok(),
        _ => None,
    }
}

/// Code points `[offset, offset + max)` of `text`, and where the next page starts.
pub(crate) fn page_of(
    text: &str,
    offset: usize,
    max_chars: usize,
) -> (String, Option<usize>, usize) {
    let total = ferb_py::value::char_len(text);
    let end = (offset.saturating_add(max_chars)).min(total);
    let piece: String = if offset < end {
        text.chars().skip(offset).take(end - offset).collect()
    } else {
        String::new()
    };
    (piece, (end < total).then_some(end), total)
}

fn kind_str(k: BrowserActionKind) -> String {
    serde_json::to_value(k)
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_default()
}

fn field_kind(k: &str) -> Option<FieldKind> {
    serde_json::from_value(Value::String(k.to_string())).ok()
}

fn push_unique<T: PartialEq>(v: &mut Vec<T>, x: T) {
    if !v.contains(&x) {
        v.push(x);
    }
}

/// `_detached_msg`.
pub fn detached_msg(reason: &str) -> String {
    if reason == "canceled_by_user" {
        return "the owner cancelled FERB's debugger on that tab mid-action; outcome unknown"
            .into();
    }
    format!("the debugger left the tab mid-action ({reason}); outcome unknown")
}

/// A failed act whose text says the debugger left: outcome unknown (`act`'s `except DriverError`).
pub(crate) fn detached(e: Error) -> Error {
    match e.message().strip_prefix("detached:") {
        Some(reason) if e.is_driver_error() => Error::unknown(detached_msg(reason)),
        _ => e,
    }
}

/// `_pastes`: a paste key the extension turns into the page's paste command (⌘V).
pub fn pastes(text: Option<&str>) -> bool {
    let parts: Vec<String> = text
        .unwrap_or("")
        .replace('-', "+")
        .split('+')
        .map(|p| ferb_sanitize::parity::casefold(ferb_py::value::strip(p)))
        .collect();
    parts.len() > 1
        && parts[parts.len() - 1] == "v"
        && parts[..parts.len() - 1]
            .iter()
            .any(|p| ["cmd", "meta", "command"].contains(&p.as_str()))
}

/// `terminals.newline`: typed text that holds its own Return.
pub fn newline(text: &str) -> bool {
    text.contains('\n') || text.contains('\r')
}

fn is_return(text: &str) -> bool {
    RETURN_KEYS.contains(&ferb_sanitize::parity::casefold(ferb_py::value::strip(text)).as_str())
}

/// `_hidden`: why a tab's window doesn't show it on screen ("" when it does).
pub fn hidden(window: Option<&Value>) -> String {
    let Some(w) = window.and_then(Value::as_object) else {
        return "the extension didn't say where the tab's window is".into();
    };
    if w.get("minimized").is_some_and(ferb_py::value::truthy) {
        return "its window is minimized".into();
    }
    if w.get("active").is_some_and(ferb_py::value::truthy) {
        String::new()
    } else {
        "the tab isn't the one its window shows".into()
    }
}

/// `_window_bounds`: the window's screen bounds as the extension reported them (JSON carries no
/// infinite number, so Python's missing finiteness check changes nothing).
pub fn window_bounds(window: &Map<String, Value>) -> Option<Rect> {
    crate::mask::rect(window.get("bounds"))
}

impl Driver {
    pub(crate) fn seams(&self) -> Seams {
        self.seams_lock().clone()
    }

    /// The browser process behind this tab's connection.
    pub fn browser_pid(&self, tab: &TabRef) -> Result<Option<i32>> {
        Ok(self.live_tab(tab)?.bridge.browser_pid())
    }

    /// `html` (HAND-496): the page's real markup, read only, paged like `read`; offset 0 reads afresh.
    #[allow(clippy::too_many_arguments)] // the tool's arguments
    pub async fn html(
        &self,
        tab: &TabRef,
        element: Option<&str>,
        selector: Option<&str>,
        frame: Option<i64>,
        scripts: bool,
        offset: usize,
        max_chars: usize,
    ) -> Result<Map<String, Value>> {
        let t = self.live_tab(tab)?;
        let key = ferb_py::json::dumps(&json!([element, selector, frame, scripts]));
        if offset == 0 || t.markup.0 != key {
            let running = t.bridge.version();
            if !version_at_least(&running, protocol::HTML_EXTENSION_VERSION) {
                return Err(Error::refused(format!(
                    "the FERB extension in this profile is {}; reading a page's markup needs {} or later. Use the \
                     snapshot, find and read meanwhile. Tell the owner: their browser updates the extension by itself, \
                     or they can update it on its extensions page",
                    if running.is_empty() { "an older version" } else { &running },
                    protocol::HTML_EXTENSION_VERSION
                )));
            }
            let mut args = Map::new();
            args.insert("tab_id".into(), t.chrome_id.into());
            args.insert("snap".into(), t.snap_gen.into());
            args.insert("scripts".into(), scripts.into());
            if let Some(e) = element.filter(|e| !e.is_empty()) {
                let (_, fid, snap, local, _) = self.resolve(&self.element_ref(tab, e))?;
                args.insert("frame_id".into(), fid.into());
                args.insert("snap".into(), snap.into());
                args.insert("el".into(), local.into());
            } else if let Some(f) = frame {
                args.insert("frame_id".into(), f.into());
            }
            if let Some(s) = selector.filter(|s| !s.is_empty()) {
                args.insert("selector".into(), s.into());
            }
            let v = t
                .bridge
                .call("html", Value::Object(args), 30.0, None)
                .await?;
            let t = self.live_tab(tab).unwrap_or(t);
            let nonce = text_of(&v, "nonce");
            let ids: std::collections::HashMap<(i64, i64), String> = t
                .elements
                .iter()
                .filter(|(_, h)| h.1 == t.snap_gen)
                .map(|(eid, h)| ((h.0, h.2), eid.clone()))
                .collect();
            let (mut parts, mut skipped, mut errors) = (vec![], vec![], vec![]);
            for fr in v
                .get("frames")
                .filter(|f| ferb_py::value::truthy(f))
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let fid = py_int(fr.get("frame_id")).unwrap_or(0);
                if fr.get("undrawn").is_some_and(ferb_py::value::truthy) {
                    skipped.push(fid);
                    continue;
                }
                if let Some(err) = fr.get("error").filter(|e| ferb_py::value::truthy(e)) {
                    errors.push(format!("frame {fid}: {}", ferb_py::value::py_str(err)));
                    continue;
                }
                let mut text = text_of(fr, "html");
                if !nonce.is_empty() {
                    text = mark_elements(&text, &nonce, fid, &ids);
                }
                let cut = if fr.get("truncated").is_some_and(ferb_py::value::truthy) {
                    " (cut at its size limit)"
                } else {
                    ""
                };
                parts.push(format!(
                    "<!-- frame {fid}: {}{cut} -->\n{text}",
                    text_of(fr, "url")
                ));
            }
            if !errors.is_empty() && parts.is_empty() {
                return Err(Error::refused(
                    ferb_py::value::prefix(&errors.join("; "), 500).to_string(),
                ));
            }
            let markup = (key, parts.join("\n"), skipped);
            self.update(&tab.tab_id, |x| x.markup = markup);
        }
        let t = self.live_tab(tab)?;
        let (_, text, skipped) = &t.markup;
        let (piece, next, total) = page_of(text, offset, max_chars);
        let mut out = Map::new();
        out.insert("tab".into(), tab.tab_id.clone().into());
        out.insert("snapshot".into(), t.snap_gen.into());
        out.insert("text".into(), piece.into());
        out.insert("offset".into(), offset.into());
        out.insert("next_offset".into(), json!(next));
        out.insert("total_chars".into(), total.into());
        if !skipped.is_empty() {
            out.insert("undrawn_frames_skipped".into(), json!(skipped));
        }
        out.insert(
            "note".into(),
            "the page's markup, read only. ferb-element=\"sNeM\" is that element's id in the latest snapshot (act on \
             it with browser.act); ferb-frame is a frame's id. Values of password, code and card fields are never \
             shown; scripts' and styles' contents are left out."
                .into(),
        );
        Ok(out)
    }

    /// `act`: one act on an element of the latest snapshot.
    pub async fn act(
        &self,
        el: &ElementRef,
        action: &BrowserAction,
        intent_id: &str,
    ) -> Result<ActionResult> {
        let (t, frame, snap, local, info) = self.resolve(el)?;
        let kind = kind_str(action.kind);
        if kind == "drag" {
            // a drop onto a web terminal (D31 round 2, finding 4)
            let end = t
                .elements
                .get(action.end_element.as_deref().unwrap_or(""))
                .cloned();
            if end.is_some_and(|e| e.3.terminal) {
                if let Some(refusal) = self.terminal_refusal(&t, &el.element_id, action) {
                    return Err(Error::refused(refusal));
                }
            }
        }
        if t.points.contains_key(&el.element_id) || kind == "drag" {
            return self.pointer(&t, el, action, intent_id).await;
        }
        self.update(&t.tab.tab_id, |x| x.visual = None);
        if info.secret
            && (kind == "type"
                || kind == "paste"
                || (kind == "press" && rules::enters_text(action.text.as_deref())))
        {
            return Err(Error::refused(format!(
                "that's a secret field ({}); FERB never types into it or presses a key that puts text there: fill it \
                 through browser.secret or a saved card",
                info.field_kind
                    .as_deref()
                    .filter(|k| !k.is_empty())
                    .unwrap_or("password or code")
            )));
        }
        let into = ["type", "press", "paste"].contains(&kind.as_str()) && info.terminal;
        let version = t.bridge.version();
        let shown = if version.is_empty() {
            "an older version".to_string()
        } else {
            version.clone()
        };
        if kind == "paste" && !version_at_least(&version, INPUT_EXTENSION_VERSION) {
            return Err(Error::refused(format!(
                "the FERB extension in this profile is {shown}; paste needs {INPUT_EXTENSION_VERSION} or later: type the \
                 text instead. Their browser updates the extension by itself from the Chrome Web Store"
            )));
        }
        if into {
            if let Some(refusal) = self.terminal_refusal(&t, &el.element_id, action) {
                return Err(Error::refused(refusal));
            }
        }
        if WRITES.contains(&kind.as_str()) {
            self.note_input(&t); // HAND-447: the page may now hold unsaved input
        }
        if kind == "upload" {
            return self
                .upload(
                    &t,
                    frame,
                    snap,
                    local,
                    &action.files,
                    intent_id,
                    action.files_closed.clone(),
                    info.input_type.as_deref() == Some("file"),
                )
                .await;
        }
        let mut args = Map::new();
        args.insert("tab_id".into(), t.chrome_id.into());
        args.insert("frame_id".into(), frame.into());
        args.insert("snap".into(), snap.into());
        args.insert("el".into(), local.into());
        args.insert("kind".into(), kind.clone().into());
        args.insert("text".into(), json!(action.text));
        args.insert(
            "page_mode".into(),
            (action.page_mode && !action.trusted && TRUSTED_KINDS.contains(&kind.as_str())).into(),
        );
        if kind == "type" {
            if let Some(mode) = action.mode {
                let mode = serde_json::to_value(mode).unwrap_or(Value::Null);
                let append = mode == "insert";
                args.insert("mode".into(), mode);
                args.insert("append".into(), append.into());
            }
        }
        if kind == "paste" {
            if let Some(h) = action.html.as_deref().filter(|h| !h.is_empty()) {
                args.insert("html".into(), h.into());
            }
        }
        if !action.modifiers.is_empty() {
            if !version_at_least(&version, INPUT_EXTENSION_VERSION) {
                return Err(Error::refused(format!(
                    "the FERB extension in this profile is {shown}; a click with held keys needs \
                     {INPUT_EXTENSION_VERSION} or later: click the items one by one"
                )));
            }
            let mut bits = 0;
            let mut seen = HashSet::new();
            for m in &action.modifiers {
                if seen.insert(*m) {
                    bits += match serde_json::to_value(m)
                        .ok()
                        .as_ref()
                        .and_then(Value::as_str)
                    {
                        Some("alt") => 1,
                        Some("ctrl") => 2,
                        Some("cmd") => 4,
                        _ => 8,
                    };
                }
            }
            args.insert("modifiers".into(), bits.into());
        }
        if let Some(d) = action.expect_dialog {
            args.insert(
                "expect_dialog".into(),
                serde_json::to_value(d).unwrap_or(Value::Null),
            );
            if let Some(text) = &action.dialog_text {
                args.insert("dialog_text".into(), text.clone().into());
            }
        }
        if !STILL.contains(&kind.as_str()) {
            self.update(&t.tab.tab_id, |x| x.moved = true); // it may scroll the page (L33)
        }
        let cid = (!intent_id.is_empty()).then(|| intent_id.to_string());
        let mut v = t
            .bridge
            .call("act", Value::Object(args), 30.0, cid)
            .await
            .map_err(detached)?;
        if v.get("detail").and_then(Value::as_str) == Some("stale") {
            return Err(Error::stale(format!(
                "element {} is gone; take a new snapshot",
                el.element_id
            )));
        }
        let detail = text_of(&v, "detail");
        let outside = detail.starts_with("outside_page:");
        if outside || v.get("unpointed").and_then(Value::as_str) == Some("outside") {
            // HAND-482: a copy parked off the page; HAND-504: keys went to the copy outside the page
            let twin = t
                .elements
                .iter()
                .find(|(eid, h)| {
                    **eid != el.element_id
                        && h.3.in_view
                        && h.3.role == info.role
                        && h.3.label == info.label
                })
                .map(|(eid, _)| eid.clone());
            if let Some(twin) = twin {
                if outside {
                    v["detail"] = format!(
                        "{}. The same {} \"{}\" is on screen as {twin}",
                        ferb_py::value::py_str(&v["detail"]),
                        info.role,
                        info.label
                    )
                    .into();
                    v["on_screen"] = twin.into();
                } else {
                    v["on_screen"] = twin.clone().into();
                    v["note"] = format!(
                        "the same {} \"{}\" is on screen as {twin}; if nothing changed, act on that one",
                        info.role, info.label
                    )
                    .into();
                }
            }
        }
        if into {
            // one Return stays covered after approved text, nothing else
            let armed = action.terminal.as_ref().and_then(|g| {
                (kind == "type" && g.input.enter && v.get("ok").is_some_and(ferb_py::value::truthy))
                    .then(|| (el.element_id.clone(), g.confirmation_id.clone()))
            });
            self.update(&t.tab.tab_id, |x| x.armed = armed);
        }
        let mut facts =
            self.act_facts(&t.tab.tab_id, &el.element_id, &kind, action.text.as_deref());
        let submitted: Vec<Map<String, Value>> = v
            .get("submitted")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|s| s.as_object().cloned()).collect())
            .unwrap_or_default();
        if let Some(s) = submitted.first() {
            // the page's own submit event, seen during this act
            let mut kinds: Vec<FieldKind> = facts
                .as_ref()
                .map(|f| f.field_kinds.clone())
                .unwrap_or_default();
            for x in &submitted {
                for k in x
                    .get("kinds")
                    .filter(|k| ferb_py::value::truthy(k))
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                {
                    if crate::snapshot::FIELD_KINDS.contains(&k) {
                        if let Some(k) = field_kind(k) {
                            push_unique(&mut kinds, k);
                        }
                    }
                }
            }
            let mut f = facts.unwrap_or_default();
            f.submits_form = Some(true);
            f.form_action_host = s
                .get("action_host")
                .and_then(Value::as_str)
                .map(String::from);
            f.form_method = s.get("method").and_then(Value::as_str).map(String::from);
            f.field_kinds = kinds;
            facts = Some(f);
        }
        Ok(ActionResult {
            ok: v.get("ok").is_some_and(ferb_py::value::truthy),
            detail: str_or_empty(&v, "detail"),
            facts: without(&v, &["ok", "detail"]),
            observed: facts,
        })
    }

    /// `act_facts` (GATE-202): what code reads, from the latest snapshot, about an act on this
    /// element. `None`: no such element.
    pub fn act_facts(
        &self,
        tab_id: &str,
        element_id: &str,
        kind: &str,
        text: Option<&str>,
    ) -> Option<ActionFacts> {
        let t = self.tab(tab_id)?;
        let (frame, _, _, info) = t.elements.get(element_id)?.clone();
        let sub = info.submit.as_ref();
        let enter = kind == "press"
            && ["enter", "return"].contains(
                &ferb_sanitize::parity::casefold(ferb_py::value::strip(text.unwrap_or("")))
                    .as_str(),
            );
        let submits = sub.is_some_and(|s| {
            (["click", "dblclick"].contains(&kind) && s.on_click) || (enter && s.on_enter)
        });
        let mut kinds: Vec<FieldKind> = vec![];
        if let Some(k) = info
            .field_kind
            .as_deref()
            .filter(|k| !k.is_empty() && ["type", "select"].contains(&kind))
        {
            if let Some(k) = field_kind(k) {
                push_unique(&mut kinds, k);
            }
        }
        if submits {
            for k in &sub.expect("submits").field_kinds {
                if let Some(k) = field_kind(k) {
                    push_unique(&mut kinds, k);
                }
            }
        }
        let clicked = ["click", "dblclick"].contains(&kind);
        let amount =
            crate::money::label_amount(clicked.then_some(info.label.as_str())).or_else(|| {
                submits
                    .then(|| crate::money::label_amount(sub.map(|s| s.label.as_str())))
                    .flatten()
            });
        let page = crate::url::host(&t.url);
        let here = if frame != 0 {
            crate::url::host(t.frame_urls.get(&frame).map_or("", String::as_str))
        } else {
            None
        };
        let commerce = |k: &str| t.commerce.get(k).and_then(Value::as_str).map(String::from);
        Some(ActionFacts {
            submits_form: submits.then_some(true),
            form_action_host: sub.filter(|_| submits).and_then(|s| s.action_host.clone()),
            form_method: sub.filter(|_| submits).and_then(|s| s.method.clone()),
            field_kinds: kinds,
            page_amount: amount.or_else(|| commerce("amount")),
            page_merchant: commerce("merchant"),
            page_amounts: t.amounts.clone(),
            secure_field: info.secret.then_some(true),
            terminal_target: info.terminal.then_some(true),
            target_role: Some(info.role.clone()).filter(|r| !r.is_empty()),
            page_url: Some(t.url.clone()).filter(|u| !u.is_empty()),
            frame_host: here.filter(|h| Some(h) != page.as_ref()),
            tab_chain: Some(self.tab_chain(tab_id)),
            ..ActionFacts::default()
        })
    }

    /// `element_identity` (P19): the page (origin and path), the frame's for a child frame, the role
    /// and the node (the debugger's node id, else its DOM path). `None`: not of the latest snapshot.
    pub fn element_identity(&self, tab_id: &str, element_id: &str) -> Option<Map<String, Value>> {
        let t = self.tab(tab_id)?;
        let (frame, _, _, info) = t.elements.get(element_id)?.clone();
        let place = |url: &str| -> String {
            match crate::url::parts(url) {
                Ok((scheme, netloc, path)) if !scheme.is_empty() => {
                    format!("{scheme}://{netloc}{path}")
                }
                _ => String::new(),
            }
        };
        let node = match (info.backend_node, info.path.as_str()) {
            (Some(n), _) => format!("b:{n}"),
            (None, p) if !p.is_empty() => format!("p:{p}"),
            _ => return None,
        };
        let mut out = Map::new();
        out.insert("page".into(), place(&t.url).into());
        if frame != 0 {
            out.insert(
                "frame".into(),
                place(t.frame_urls.get(&frame).map_or("", String::as_str)).into(),
            );
        }
        out.insert("role".into(), info.role.into());
        out.insert("node".into(), node.into());
        Some(out)
    }

    /// `_visual_fresh`: the tab's latest screenshot still resolves points.
    pub(crate) fn visual_fresh(t: &Tab) -> bool {
        t.visual.as_ref().is_some_and(|v| {
            v.snap_gen == t.snap_gen
                && v.snap_nav == t.bridge.navs(t.chrome_id)
                && v.at.elapsed() <= Duration::from_secs(30)
        })
    }

    /// `_pointer`: a gesture at screenshot points (HAND-493).
    async fn pointer(
        &self,
        t: &Tab,
        el: &ElementRef,
        action: &BrowserAction,
        intent_id: &str,
    ) -> Result<ActionResult> {
        let kind = kind_str(action.kind);
        if action.page_mode || !POINTER_KINDS.contains(&kind.as_str()) {
            return Err(Error::refused(
                "a screenshot point takes click, dblclick, rightclick, hover or drag, as real input: drop page_mode; to \
                 type, select or press a key, act on an element id from browser.snapshot",
            ));
        }
        if !Self::visual_fresh(t) {
            return Err(Error::stale(
                "the visual target is stale; take a fresh screenshot and resolve its points",
            ));
        }
        let start = t.points.get(&el.element_id).cloned();
        let end = t
            .points
            .get(action.end_element.as_deref().unwrap_or(""))
            .cloned();
        let Some(start) = start.filter(|s| !s.is_empty()) else {
            return Err(Error::refused(
                "drag needs two points resolved from the same screenshot",
            ));
        };
        if kind == "drag" && end.as_deref().is_none_or(str::is_empty) {
            return Err(Error::refused(
                "drag needs two points resolved from the same screenshot",
            ));
        }
        self.update(&t.tab.tab_id, |x| {
            x.visual = None;
            x.moved = true; // a gesture may scroll the page (L33)
        });
        let cid = (!intent_id.is_empty()).then(|| intent_id.to_string());
        let r = t
            .bridge
            .call(
                "pointer",
                json!({"tab_id": t.chrome_id, "point": start, "end": end, "kind": kind}),
                10.0,
                cid,
            )
            .await?;
        Ok(ActionResult {
            ok: r.get("ok").is_some_and(ferb_py::value::truthy),
            detail: str_or_empty(&r, "detail"),
            facts: without(&r, &["ok", "detail"]),
            observed: None,
        })
    }

    /// `find` (HAND-484): search every frame; the latest snapshot's matching elements first, then the
    /// page's text lines; saved and paged.
    pub async fn find(
        &self,
        tab: &TabRef,
        text: &str,
        offset: usize,
        max_chars: usize,
    ) -> Result<Map<String, Value>> {
        let t = self.live_tab(tab)?;
        if offset == 0 {
            let value = t
                .bridge
                .call(
                    "find",
                    json!({"tab_id": t.chrome_id, "text": text}),
                    30.0,
                    None,
                )
                .await?;
            let t = self.live_tab(tab).unwrap_or(t);
            let mut hits: Vec<Value> = match value.get("hits") {
                Some(h) => h.as_array().cloned().unwrap_or_default(),
                None => value
                    .get("examples")
                    .and_then(Value::as_array)
                    .map(|x| {
                        x.iter()
                            .map(|e| json!({"frame_id": 0, "text": e}))
                            .collect()
                    })
                    .unwrap_or_default(),
            };
            let current = t.snap_nav == t.bridge.navs(t.chrome_id);
            let found = if current {
                element_hits(&t, text)
            } else {
                vec![]
            };
            if current {
                // a text line that is exactly a control's label: that control
                let mut labels: Vec<(String, String)> = vec![];
                for (eid, h) in &t.elements {
                    let k = ferb_sanitize::parity::casefold(&ferb_py::value::squeeze(&h.3.label));
                    if !labels.iter().any(|(x, _)| *x == k) {
                        labels.push((k, eid.clone()));
                    }
                }
                for h in &mut hits {
                    let line = h
                        .get("text")
                        .map(ferb_py::value::py_str)
                        .unwrap_or_default();
                    let k = ferb_sanitize::parity::casefold(&ferb_py::value::squeeze(&line));
                    if let Some((_, eid)) = labels.iter().find(|(x, _)| *x == k) {
                        if let Some(m) = h.as_object_mut() {
                            m.insert("element".into(), eid.clone().into());
                        }
                    }
                }
            }
            let mut all: Vec<Value> = found.iter().cloned().map(Value::Object).collect();
            all.extend(hits);
            let payload = ferb_py::json::dumps_utf8(&Value::Array(all));
            let saved = (
                payload,
                py_int(value.get("count")).unwrap_or(0),
                py_int(value.get("lines")).unwrap_or(0),
                found.len(),
                current,
            );
            self.update(&tab.tab_id, |x| {
                x.searches.insert(text.to_string(), saved);
            });
        }
        let t = self.live_tab(tab)?;
        let Some((payload, count, lines, elements, current)) = t.searches.get(text).cloned() else {
            return Err(Error::refused(
                "start this search at offset=0 before fetching its saved result pages",
            ));
        };
        let (piece, next, total) = page_of(&payload, offset, max_chars);
        let examples: Vec<Value> = if total <= max_chars {
            serde_json::from_str::<Vec<Value>>(&payload)
                .unwrap_or_default()
                .into_iter()
                .filter_map(|h| h.get("text").cloned())
                .collect()
        } else {
            vec![]
        };
        let mut note = "Saved all-frame search inventory: the latest snapshot's matching elements first (act on them by \
                        `element`), then the page's text lines (with `element` when a line is a control's whole label). \
                        A line with no element id isn't a control of the latest snapshot: if it is one, it appeared \
                        after that snapshot, so take a new one. Frames the page doesn't draw are skipped. offset=0 \
                        refreshes it; later pages keep the same evidence."
            .to_string();
        if !current && !t.elements.is_empty() {
            note.push_str(" The page navigated since the last snapshot: take one for element ids.");
        }
        let mut out = Map::new();
        out.insert("matches".into(), count.into());
        out.insert("element_matches".into(), elements.into());
        out.insert("lines_on_page".into(), lines.into());
        out.insert("text".into(), piece.into());
        out.insert("offset".into(), offset.into());
        out.insert("next_offset".into(), json!(next));
        out.insert("total_chars".into(), total.into());
        out.insert("examples".into(), Value::Array(examples));
        out.insert("note".into(), note.into());
        Ok(out)
    }

    /// `scroll` (HAND-510): the page, or a scroll container of the latest snapshot, by about a screen,
    /// `screens` screens, or to its start or end.
    pub async fn scroll(
        &self,
        tab: &TabRef,
        direction: &str,
        container: Option<&str>,
        screens: Option<f64>,
        to: Option<&str>,
    ) -> Result<Map<String, Value>> {
        let t = self.live_tab(tab)?;
        let container = container.filter(|c| !c.is_empty());
        if let Some(c) = container {
            if !t.containers.contains_key(c) {
                return Err(Error::stale(format!(
                    "no container {c} in the latest snapshot"
                )));
            }
        }
        let version = t.bridge.version();
        let to = to.filter(|x| !x.is_empty());
        if (direction == "left" || direction == "right" || screens.is_some() || to.is_some())
            && !version_at_least(&version, INPUT_EXTENSION_VERSION)
        {
            return Err(Error::refused(format!(
                "the FERB extension in this profile is {}; scrolling sideways, by several screens or to the start or \
                 end needs {INPUT_EXTENSION_VERSION} or later: scroll up or down one screen at a time meanwhile. Their \
                 browser updates the extension by itself",
                if version.is_empty() { "an older version" } else { &version }
            )));
        }
        let (frame, local) = match container {
            Some(c) => {
                let (f, l) = t.containers[c].clone();
                (f, Some(l))
            }
            None => (0, None),
        };
        let mut args = Map::new();
        args.insert("tab_id".into(), t.chrome_id.into());
        args.insert("direction".into(), direction.into());
        args.insert("frame_id".into(), frame.into());
        if let Some(n) = screens {
            args.insert("screens".into(), json!(n));
        }
        if let Some(x) = to {
            args.insert("to".into(), x.into());
        }
        if let Some(l) = &local {
            args.insert("container".into(), l.clone().into());
        }
        self.update(&tab.tab_id, |x| x.moved = true); // L33: the snapshot's rects go stale
        let v = match t
            .bridge
            .call("scroll", Value::Object(args), 15.0, None)
            .await
        {
            Ok(v) => v,
            Err(e) => {
                let e = detached(e);
                if e.is_driver_error() && e.message().contains("unknown op") {
                    return Err(Error::refused(format!(
                        "the FERB extension in that browser is older than FERB: reload it ({MIN_EXTENSION_VERSION} or later)"
                    )));
                }
                return Err(e);
            }
        };
        let got_container = v.get("container").cloned().unwrap_or(Value::Null);
        let want = local.clone().map_or(Value::Null, Value::from);
        if !ferb_py::value::py_eq(&got_container, &want)
            || py_int(Some(v.get("frame_id").unwrap_or(&json!(0)))) != Some(frame)
        {
            return Err(Error::driver(
                "browser scrolled a different surface; inspect it before continuing",
            ));
        }
        let int = |k: &str| py_int(Some(v.get(k).unwrap_or(&json!(0)))).unwrap_or(0);
        let mut out = Map::new();
        out.insert("y".into(), int("y").into());
        out.insert("page_height".into(), int("height").into());
        out.insert(
            "at_end".into(),
            v.get("at_end").is_some_and(ferb_py::value::truthy).into(),
        );
        if v.get("viewport_h").is_some_and(|x| !x.is_null()) {
            out.insert("viewport_h".into(), int("viewport_h").into());
        }
        if (direction == "left" || direction == "right") && v.get("x").is_some_and(|x| !x.is_null())
        {
            let or0 = |k: &str| {
                v.get(k)
                    .filter(|x| ferb_py::value::truthy(x))
                    .and_then(|x| py_int(Some(x)))
                    .unwrap_or(0)
            };
            out.insert("x".into(), int("x").into());
            out.insert("page_width".into(), or0("width").into());
            out.insert("viewport_w".into(), or0("viewport_w").into());
        }
        if let Some(c) = container {
            out.insert("container".into(), c.into());
            out.insert("frame".into(), frame.into());
        }
        Ok(out)
    }

    /// `media` (L26, L31): play, pause, mute, unmute or seek one media element of the latest
    /// snapshot, by index or own id; its state afterwards.
    pub async fn media(
        &self,
        tab: &TabRef,
        action: &str,
        index: Option<i64>,
        to: Option<f64>,
        media_id: Option<&str>,
    ) -> Result<Map<String, Value>> {
        let t = self.live_tab(tab)?;
        if t.snapshot_json.is_empty() {
            return Err(Error::refused(
                "take a snapshot first: browser.media uses an index from its `media` list",
            ));
        }
        let mut index = index;
        if let Some(id) = media_id {
            let Some(i) = t.media_ids.get(id) else {
                return Err(Error::refused(format!(
                    "the latest snapshot has no media element {id}: take a new snapshot"
                )));
            };
            index = Some(*i);
        }
        let index = match index {
            Some(i) => i,
            None => {
                if t.media.len() != 1 {
                    return Err(Error::refused(if t.media.is_empty() {
                        "the latest snapshot lists no video or audio on this page".to_string()
                    } else {
                        format!(
                            "this page has {} media elements in the latest snapshot; choose one by its index",
                            t.media.len()
                        )
                    }));
                }
                *t.media.keys().next().expect("one")
            }
        };
        let Some((frame, local)) = t.media.get(&index).copied() else {
            return Err(Error::refused(format!(
                "the latest snapshot has no media element {index}"
            )));
        };
        let v = match t
            .bridge
            .call(
                "media",
                json!({"tab_id": t.chrome_id, "frame_id": frame, "snap": t.snap_gen, "index": local,
                       "action": action, "to": to}),
                crate::bridge::CMD_TIMEOUT_S,
                None,
            )
            .await
        {
            Ok(v) => v,
            Err(e) if e.is_driver_error() && e.message().contains("unknown op") => {
                return Err(Error::refused(format!(
                    "the FERB extension in that browser is older than FERB: reload it ({MIN_EXTENSION_VERSION} or later)"
                )))
            }
            Err(e) => return Err(e),
        };
        let own = t
            .media_ids
            .iter()
            .find(|(_, i)| **i == index)
            .map(|(q, _)| q.clone());
        let mut sound = Map::new();
        if (action == "mute" || action == "unmute")
            && !t.attached
            && !v.get("error").is_some_and(ferb_py::value::truthy)
        {
            // HAND-207: in a tab FERB opened, the task's own unmute lets the tab be heard
            sound = self.tab_sound(tab, action == "unmute").await?;
        }
        let mut out = Map::new();
        for k in crate::driver::MEDIA_FIELDS.iter().chain(&["error"]) {
            if let Some(x) = v.get(*k) {
                out.insert((*k).into(), x.clone());
            }
        }
        out.insert("index".into(), index.into());
        if let Some(own) = own {
            out.insert("id".into(), own.into());
        }
        if frame != 0 {
            out.insert("frame".into(), frame.into());
        }
        out.extend(sound);
        Ok(out)
    }

    /// `tab_sound` (HAND-207): let a tab FERB opened be heard, or mute it again; the owner's tabs are
    /// never touched. A failure is a fact, never the caller's failure.
    pub async fn tab_sound(&self, tab: &TabRef, on: bool) -> Result<Map<String, Value>> {
        let t = self.live_tab(tab)?;
        let mut out = Map::new();
        if t.attached {
            return Ok(out);
        }
        match t
            .bridge
            .call(
                "mute_tab",
                json!({"tab_id": t.chrome_id, "muted": !on}),
                10.0,
                None,
            )
            .await
        {
            Err(e) if e.is_driver_error() => {
                out.insert("tab_muted".into(), t.muted.into());
                out.insert("tab_sound_error".into(), e.message().into());
                Ok(out)
            }
            Err(e) => Err(e),
            Ok(v) => {
                let mut muted = t.muted;
                if v.get("changed") == Some(&Value::Bool(true)) {
                    muted = v.get("muted") == Some(&Value::Bool(true));
                    self.update(&tab.tab_id, |x| x.muted = muted);
                }
                out.insert("tab_muted".into(), muted.into());
                if v.get("changed") == Some(&Value::Bool(false)) {
                    out.insert(
                        "tab_sound_error".into(),
                        ferb_py::value::py_str(v.get("reason").unwrap_or(&Value::Null)).into(),
                    );
                }
                Ok(out)
            }
        }
    }

    /// `show_tab`: the owner clicked FERB's human-check notification; `false` when the tab isn't
    /// FERB's to show any more.
    pub async fn show_tab(&self, tab: &TabRef) -> Result<bool> {
        let Some(t) = self
            .tab(&tab.tab_id)
            .filter(|t| !t.closed && t.bridge.is_connected())
        else {
            return Ok(false);
        };
        t.bridge
            .call("show_tab", json!({"tab_id": t.chrome_id}), 10.0, None)
            .await?;
        Ok(true)
    }

    /// `tab_profile`: (browser, folder) of the profile this tab is in.
    pub fn tab_profile(&self, tab: &TabRef) -> Result<(String, String)> {
        let b = self.live_tab(tab)?.bridge.binding.clone();
        Ok((b.browser, b.folder))
    }

    /// `shows`: facts for a human check's wait.
    pub async fn shows(&self, tab: &TabRef, text: &str) -> Result<Map<String, Value>> {
        let t = self.live_tab(tab)?;
        let listed = t.bridge.call("list_tabs", json!({}), 10.0, None).await?;
        let row = listed
            .get("tabs")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .find(|r| py_int(Some(r.get("tab_id").unwrap_or(&json!(-1)))) == Some(t.chrome_id))
            .cloned();
        let mut out = Map::new();
        let Some(row) = row else {
            out.insert("open".into(), false.into());
            return Ok(out);
        };
        let shows = match t
            .bridge
            .call(
                "find",
                json!({"tab_id": t.chrome_id, "text": text}),
                15.0,
                None,
            )
            .await
        {
            Ok(found) => {
                json!(py_int(Some(found.get("count").unwrap_or(&json!(0)))).unwrap_or(0) > 0)
            }
            Err(e) if e.is_driver_error() => Value::Null,
            Err(e) => return Err(e),
        };
        out.insert("open".into(), true.into());
        out.insert("shows".into(), shows);
        out.insert("url".into(), text_of(&row, "url").into());
        out.insert("title".into(), text_of(&row, "title").into());
        Ok(out)
    }

    /// `wait`: until the tab loaded, shows a text or its address holds a value; `false` when the
    /// outcome is unknown.
    pub async fn wait(&self, tab: &TabRef, until: &WaitCondition, timeout: f64) -> Result<bool> {
        let t = self.live_tab(tab)?;
        let kind = serde_json::to_value(until.kind).unwrap_or(Value::Null);
        match t
            .bridge
            .call(
                "wait",
                json!({"tab_id": t.chrome_id, "until": kind, "value": until.value}),
                timeout,
                None,
            )
            .await
        {
            Err(e) if e.unknown => Ok(false),
            Err(e) => Err(e),
            Ok(v) => Ok(v.get("met").is_some_and(ferb_py::value::truthy)),
        }
    }

    /// `claim`: the tab becomes this task's.
    pub fn claim(&self, tab: &TabRef, owner: &str) -> Result<()> {
        self.live_tab(tab)?;
        self.update(&tab.tab_id, |x| x.owner = Some(owner.to_string()));
        Ok(())
    }

    /// `tab_url`: the tab's last known address.
    pub fn tab_url(&self, tab_id: &str) -> Option<String> {
        self.tab(tab_id).map(|t| t.url)
    }

    /// `element_info`: an element of the latest snapshot.
    pub fn element_info(&self, tab_id: &str, element_id: &str) -> Option<PageElement> {
        self.tab(tab_id)?
            .elements
            .get(element_id)
            .map(|h| h.3.clone())
    }

    /// `owner_of`: the task that opened this tab, if FERB did.
    pub fn owner_of(
        &self,
        br: &Arc<crate::bridge::Bridge>,
        chrome_id: i64,
    ) -> Option<Option<String>> {
        self.st()
            .tabs
            .values()
            .find(|t| Arc::ptr_eq(&t.bridge, br) && t.chrome_id == chrome_id)
            .map(|t| t.owner.clone())
    }

    /// `terminal_input` (D31): what this act would put into a web terminal, for the gateway's
    /// question; `None` when it puts nothing there.
    pub fn terminal_input(
        &self,
        tab_id: &str,
        element_id: &str,
        kind: &str,
        text: Option<&str>,
        end_element: Option<&str>,
    ) -> Option<TerminalInput> {
        let t = self.tab(tab_id)?;
        let host = crate::url::host(&t.url).unwrap_or_default();
        let base = TerminalInput {
            app: if host.is_empty() {
                "a web page".into()
            } else {
                format!("the page on {host}")
            },
            bundle_id: format!("web:{host}"),
            surface: "a terminal in the page (xterm.js)".into(),
            ..TerminalInput::default()
        };
        if kind == "drag" {
            let end = self.element_info(tab_id, end_element.unwrap_or(""))?;
            return end.terminal.then_some(TerminalInput { drag: true, ..base });
        }
        let info = self.element_info(tab_id, element_id)?;
        let text = text?;
        if !info.terminal || !["type", "press", "paste"].contains(&kind) {
            return None;
        }
        if kind == "type" || kind == "paste" {
            return Some(TerminalInput {
                text: Some(text.into()),
                enter: !newline(text),
                ..base
            });
        }
        if pastes(Some(text)) {
            let (count, pasted, concealed) =
                self.seams().clipboard.map_or((0, None, false), |c| c());
            return Some(TerminalInput {
                enter: pasted.as_deref().is_some_and(|p| !newline(p)),
                text: pasted,
                paste: true,
                concealed,
                clipboard: Some(count),
                ..base
            });
        }
        if is_return(text) && t.armed.as_ref().is_some_and(|a| a.0 == element_id) {
            return Some(TerminalInput {
                keys: Some("return".into()),
                covered_by: t.armed.map(|a| a.1),
                ..base
            });
        }
        Some(TerminalInput {
            keys: Some(text.into()),
            ..base
        })
    }

    /// `_terminal_refusal`: `None` when the owner's Yes names exactly this input, else the refusal.
    pub(crate) fn terminal_refusal(
        &self,
        t: &Tab,
        element_id: &str,
        action: &BrowserAction,
    ) -> Option<String> {
        let kind = kind_str(action.kind);
        let g = action.terminal.as_ref();
        if let Some(g) = g {
            if kind == "drag" && g.input.drag {
                return None;
            }
            if (kind == "type" || kind == "paste")
                && g.input.text == action.text
                && g.input.keys.as_deref().is_none_or(str::is_empty)
            {
                return None;
            }
        }
        if kind == "press" && pastes(action.text.as_deref()) {
            if let Some(g) = g.filter(|g| g.input.paste) {
                let count = self.seams().clipboard.map(|c| c().0);
                if count.is_some() && count == g.input.clipboard {
                    return None;
                }
            }
        } else if let (Some(g), "press", Some(text)) = (g, kind.as_str(), action.text.as_deref()) {
            if let Some(covered) = &g.input.covered_by {
                if is_return(text)
                    && *covered == g.confirmation_id
                    && t.armed.as_ref()
                        == Some(&(element_id.to_string(), g.confirmation_id.clone()))
                {
                    return None;
                }
            } else if g.input.keys.as_deref() == Some(text) && g.input.text.is_none() {
                return None;
            }
        }
        let host = crate::url::host(&t.url).unwrap_or_default();
        Some(
            UNASKED
                .replace("{why}", "a terminal in the page (xterm.js)")
                .replace("{app}", if host.is_empty() { "a web page" } else { &host }),
        )
    }

    /// `_refused`: whether `act` would refuse this act before doing anything.
    fn act_refused(
        &self,
        t: &Tab,
        el: &ElementRef,
        info: &PageElement,
        action: Option<&BrowserAction>,
    ) -> bool {
        let Some(action) = action else { return false };
        let kind = kind_str(action.kind);
        if t.points.contains_key(&el.element_id) || kind == "drag" {
            return action.page_mode
                || !POINTER_KINDS.contains(&kind.as_str())
                || !Self::visual_fresh(t)
                || !t.points.contains_key(&el.element_id)
                || (kind == "drag"
                    && !t
                        .points
                        .contains_key(action.end_element.as_deref().unwrap_or("")));
        }
        info.secret
            && (kind == "type" || (kind == "press" && rules::enters_text(action.text.as_deref())))
    }

    /// `page_rect` (L33, FERB's cursor): where the element the act touches will be, in the top
    /// frame's layout viewport (CSS px); `None` whenever unsure.
    pub fn page_rect(
        &self,
        el: &ElementRef,
        action: Option<&BrowserAction>,
        visible: bool,
    ) -> Option<Rect> {
        let (t, frame, _, _, info) = self.resolve(el).ok()?;
        let page = t.page.clone()?;
        let visual = page.visual?;
        if frame != 0 || self.act_refused(&t, el, &info, action) {
            return None;
        }
        let (pinned, boxed) = t
            .motion
            .get(&el.element_id)
            .cloned()
            .unwrap_or((None, false));
        let r = (info.rect[0], info.rect[1], info.rect[2], info.rect[3]);
        let centres = action.is_some_and(|a| {
            let kind = kind_str(a.kind);
            !t.points.contains_key(&el.element_id)
                && (CENTRING.contains(&kind.as_str())
                    || (CENTRING_REAL.contains(&kind.as_str()) && !(a.page_mode && !a.trusted)))
        });
        let rect = if centres {
            if pinned.is_some() || boxed {
                None
            } else {
                page.centred(r)
            }
        } else if matches!(pinned.as_deref(), Some("sticky" | "unknown"))
            || (t.moved && (pinned.as_deref() != Some("fixed") || boxed))
        {
            None
        } else {
            Some(r)
        };
        let [vx, vy, vw, vh, _] = visual;
        let rect = rect?;
        let (cx, cy) = (rect.0 + rect.2 / 2.0, rect.1 + rect.3 / 2.0);
        if rect.2 <= 0.0
            || rect.3 <= 0.0
            || !(vx <= cx && cx <= vx + vw && vy <= cy && cy <= vy + vh)
        {
            return None;
        }
        if visible {
            let (left, top) = (rect.0.max(vx), rect.1.max(vy));
            return Some((
                left,
                top,
                (rect.0 + rect.2).min(vx + vw) - left,
                (rect.1 + rect.3).min(vy + vh) - top,
            ));
        }
        Some(rect)
    }

    /// `screen_rect` (L33): `page_rect` in global screen points on the tab's window.
    pub async fn screen_rect(
        &self,
        el: &ElementRef,
        action: Option<&BrowserAction>,
        visible: bool,
    ) -> Option<Rect> {
        let rect = self.page_rect(el, action, visible);
        let t = self.tab(&el.tab.tab_id)?;
        let page = t.page.clone()?;
        let window = page.window.clone()?;
        let pid = t.bridge.browser_pid()?;
        let native = self.seams().native?;
        let rect = rect?;
        let w = Value::Object(window.clone());
        if !hidden(Some(&w)).is_empty()
            || window.get("emulated").is_some_and(ferb_py::value::truthy)
        {
            return None;
        }
        let (bounds, viewport) = (window_bounds(&window)?, page.mapping()?);
        let title = text_of(&w, "title");
        native.place(rect, viewport, pid, bounds, &title).await
    }

    /// `rest` (L31): a loop's task ends a round (`resting`) or starts one: the owner's tabs it holds
    /// give their file picker back between rounds; taking it back is confirmed, retried once.
    pub async fn rest(&self, owner: &str, resting: bool) -> Result<()> {
        let tabs: Vec<Tab> = self
            .st()
            .tabs
            .values()
            .filter(|t| t.owner.as_deref() == Some(owner) && t.attached && !t.closed)
            .cloned()
            .collect();
        let mut unconfirmed = vec![];
        for t in tabs {
            if !t.bridge.is_connected() {
                continue;
            }
            let attempts = if resting { 1 } else { 2 };
            let mut confirmed = false;
            for _ in 0..attempts {
                let reply = t
                    .bridge
                    .call(
                        "rest",
                        json!({"tab_id": t.chrome_id, "resting": resting}),
                        10.0,
                        None,
                    )
                    .await;
                let reply = match reply {
                    Ok(v) => Some(v),
                    Err(e) if e.is_driver_error() => None,
                    Err(e) => return Err(e),
                };
                if resting
                    || reply.is_some_and(|r| {
                        r.get("file_chooser_intercepted") == Some(&Value::Bool(true))
                            || r.get("held") == Some(&Value::Bool(false))
                    })
                {
                    confirmed = true;
                    break;
                }
            }
            if !confirmed {
                unconfirmed.push(t.tab.tab_id.clone());
            }
        }
        if !unconfirmed.is_empty() {
            return Err(Error::driver(format!(
                "the browser didn't confirm taking the file picker back for {}",
                unconfirmed.join(", ")
            )));
        }
        Ok(())
    }

    /// `downloads_for`: downloads this tab started that the worker hasn't seen yet; `wait` bounds
    /// how long to let ones in progress finish.
    pub async fn downloads_for(&self, tab: &TabRef, wait: f64) -> Result<Vec<Map<String, Value>>> {
        let t = self.live_tab(tab)?;
        let Some(d) = self.seams().downloads else {
            return Ok(vec![]);
        };
        if wait != 0.0 {
            d.settle(&t.bridge, t.chrome_id, wait).await;
        }
        Ok(d.unreported(&t.bridge, t.chrome_id))
    }

    /// `downloads_of`: every download this task's tabs (a registered checker: its worker's) started.
    pub fn downloads_of(&self, task: &str) -> Vec<Map<String, Value>> {
        let Some(d) = self.seams().downloads else {
            return vec![];
        };
        let mut tasks = vec![task.to_string()];
        if let Some(p) = (self.tasks.inspection_parent)(task).filter(|p| !p.is_empty()) {
            if p != task {
                tasks.push(p);
            }
        }
        d.facts_for(&tasks, self.identity.as_str())
    }

    /// `download_only` (HAND-511): this tab's only content became a download FERB took.
    pub fn download_only(&self, tab: &TabRef) -> bool {
        self.tab(&tab.tab_id).is_some_and(|t| t.download_only)
    }

    /// `ids_note` (HAND-488).
    pub fn ids_note(&self, tab: &TabRef) -> Map<String, Value> {
        self.tab(&tab.tab_id)
            .map(|t| t.ids_note)
            .unwrap_or_default()
    }

    /// `masking`: how the latest screenshot's secret fields were found ("" otherwise).
    pub fn masking(&self, tab: &TabRef) -> Result<String> {
        Ok(self.live_tab(tab)?.masking)
    }

    /// `timing`: the extension's step times for the tab's latest read and screenshot.
    pub fn timing(&self, tab: &TabRef) -> Result<Option<Map<String, Value>>> {
        let t = self.live_tab(tab)?;
        let mut out = Map::new();
        if let Some(r) = t.timing.filter(|r| !r.is_empty()) {
            out.insert("read".into(), Value::Object(r));
        }
        if let Some(s) = t.shot_timing.filter(|s| !s.is_empty()) {
            out.insert("shot".into(), Value::Object(s));
        }
        Ok((!out.is_empty()).then_some(out))
    }

    /// `input_note`.
    pub fn input_note(&self, tab: &TabRef) -> Result<String> {
        Ok(self.live_tab(tab)?.input_note)
    }

    /// `pointable`: whether browser_point can resolve elements on this screenshot.
    pub fn pointable(&self, tab: &TabRef, shot: &str) -> Result<bool> {
        Ok(self.live_tab(tab)?.unread_shot.as_deref() != Some(shot))
    }

    /// `shutdown`: every tab closed or let go of; a failure is published.
    pub async fn shutdown(&self) {
        let tabs: Vec<TabRef> = self.st().tabs.values().map(|t| t.tab.clone()).collect();
        for tab in tabs {
            if let Err(e) = self.close(&tab, None).await {
                if e.is_driver_error() {
                    let mut data = Map::new();
                    data.insert("tab".into(), tab.tab_id.clone().into());
                    data.insert("error".into(), e.message().into());
                    self.hub.publish("browser.cleanup_failed", data);
                }
            }
        }
    }
}

/// `_element_hits`: the latest snapshot's elements whose label, value, link or role hold `text`.
fn element_hits(t: &Tab, text: &str) -> Vec<Map<String, Value>> {
    let needle = ferb_sanitize::parity::casefold(text);
    let mut out = vec![];
    for (eid, h) in &t.elements {
        let e = &h.3;
        let value = if e.secret { None } else { e.value.clone() };
        let fields = [
            e.label.as_str(),
            value.as_deref().unwrap_or(""),
            e.href.as_deref().unwrap_or(""),
            e.role.as_str(),
        ];
        if !fields
            .iter()
            .any(|s| ferb_sanitize::parity::casefold(s).contains(&needle))
        {
            continue;
        }
        let mut hit = Map::new();
        hit.insert("element".into(), eid.clone().into());
        hit.insert("role".into(), e.role.clone().into());
        hit.insert("label".into(), e.label.clone().into());
        if let Some(v) = value.filter(|v| !v.is_empty()) {
            hit.insert("value".into(), v.into());
        }
        if let Some(href) = e
            .href
            .as_ref()
            .filter(|h| !h.is_empty() && ferb_sanitize::parity::casefold(h).contains(&needle))
        {
            hit.insert("href".into(), href.clone().into());
        }
        if !e.in_view {
            hit.insert("in_view".into(), false.into());
            if let Some(o) = e.offscreen.as_ref().filter(|o| !o.is_empty()) {
                hit.insert("offscreen".into(), o.clone().into());
            }
        }
        if !e.state.is_empty() {
            hit.insert("state".into(), json!(e.state));
        }
        out.push(hit);
    }
    out
}

/// The markup's element markers (`ferb-el-<nonce>="<n>"`, `\d+` as Python's `re` reads it) as
/// `ferb-element="<id of the latest snapshot>"`, or nothing for an element it doesn't have.
fn mark_elements(
    text: &str,
    nonce: &str,
    fid: i64,
    ids: &std::collections::HashMap<(i64, i64), String>,
) -> String {
    let head = format!("ferb-el-{nonce}=\"");
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(&head) {
        let after = &rest[at + head.len()..];
        let digits: String = after
            .chars()
            .take_while(|c| ferb_sanitize::parity::is_decimal(*c))
            .collect();
        let tail = &after[digits.len()..];
        if digits.is_empty() || !tail.starts_with('"') {
            out.push_str(&rest[..at + 1]);
            rest = &rest[at + 1..];
            continue;
        }
        out.push_str(&rest[..at]);
        let n: Option<i64> = digits
            .chars()
            .map(|c| crate::lax::decimal_value(c).map(i64::from))
            .try_fold(0i64, |acc, d| acc.checked_mul(10)?.checked_add(d?));
        if let Some(eid) = n.and_then(|n| ids.get(&(fid, n))) {
            out.push_str(&format!("ferb-element=\"{eid}\""));
        }
        rest = &tail[1..];
    }
    out.push_str(rest);
    out
}
