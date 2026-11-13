//! The browser snapshot's projection: a port of `ferb/act/browser/snapshot.py`. It must not discard
//! source labels, values or frame text (browser.read pages the complete saved snapshot). Secret
//! values show only filled / empty. Checked against `rust/fixtures/browser/snapshot.json`.
//!
//! Not reproduced: Python keeps an element's `value`, `input_type`, `href`, `form` and `offscreen`
//! as whatever JSON the extension sent; ferb-core's `PageElement` holds text, so a non-text value
//! becomes its Python `str()` (the extension sends text for all of them).

use ferb_core::types::{Dict, FormSubmit, PageElement, PageSnapshot};
use serde_json::{json, Map, Value};

use crate::error::{Error, Result};

pub const FIELD_KINDS: [&str; 6] = [
    "password",
    "new_password",
    "one_time_code",
    "card_number",
    "card_csc",
    "card_expiry",
];

/// Initial tool preview only; browser.read retrieves the complete saved snapshot.
pub const MAX_TEXT: usize = 8000;

/// HAND-208: the frames bot-check widgets draw in (host, path prefix, name).
pub const CHALLENGE_FRAMES: [(&str, &str, &str); 9] = [
    ("challenges.cloudflare.com", "", "Cloudflare Turnstile"),
    ("hcaptcha.com", "", "hCaptcha"),
    ("google.com", "/recaptcha/", "reCAPTCHA"),
    ("recaptcha.net", "/recaptcha/", "reCAPTCHA"),
    ("arkoselabs.com", "", "Arkose"),
    ("funcaptcha.com", "", "Arkose"),
    ("captcha-delivery.com", "", "DataDome"),
    ("awswaf.com", "", "AWS WAF"),
    ("geetest.com", "", "GeeTest"),
];
pub const BOT_CHECK_ABILITY: &str = "browser_verification";
/// HAND-300.
pub const BOT_CHECK_FACT: &str = "A bot-check widget's frame is on this page. It may be only a passive badge that \
                                  blocks nothing; the page's own words say whether a check is asked.";
/// HAND-513.
pub const PDF_FACT: &str =
    "This tab shows a PDF in the browser's own PDF viewer: the snapshot, find, read and html \
                            see none of its text, and nothing in it can be acted on by id.";
pub const PDF_NEXT: &str = "Look at it with browser_screenshot (the page in view). For its text, take the file itself: \
                            when its address needs no sign-in, fetch it with the shell (curl -L -o name.pdf 'address') \
                            into your workspace and read it there; otherwise say what the screenshot shows.";

const FIELD_ROLES: [&str; 9] = [
    "textbox",
    "searchbox",
    "combobox",
    "checkbox",
    "radio",
    "spinbutton",
    "slider",
    "switch",
    "listbox",
];
/// The omitted buttons' and links' labels listed in a preview's index, at most.
pub const OMITTED_LABELS_CHARS: usize = 3000;

fn prefix(s: &str, n: usize) -> String {
    ferb_py::value::prefix(s, n).to_string()
}

/// The path of a URL as `urlsplit` gives it (after the netloc, before `?` / `#`).
fn url_path(url: &str) -> String {
    let rest = match url.find(':') {
        Some(i) if i > 0 && url.as_bytes()[0].is_ascii_alphabetic() => &url[i + 1..],
        _ => url,
    };
    let rest = match rest.strip_prefix("//") {
        Some(after) => &after[after.find(['/', '?', '#']).unwrap_or(after.len())..],
        None => rest,
    };
    rest[..rest.find(['?', '#']).unwrap_or(rest.len())].to_string()
}

/// `bot_check`: the bot-check widgets a page's frames show, with the ability that covers them.
pub fn bot_check(frame_urls: &[String]) -> Option<Value> {
    let mut found: Vec<(String, &str)> = vec![];
    for url in frame_urls {
        let Ok(host) = crate::url::hostname(url) else {
            continue;
        };
        let host = host.unwrap_or_default().to_lowercase();
        let path = url_path(url);
        for (domain, start, name) in CHALLENGE_FRAMES {
            if (host == domain || host.ends_with(&format!(".{domain}")))
                && path.starts_with(start)
                && !found.iter().any(|(h, _)| *h == host)
            {
                found.push((host.clone(), name));
            }
        }
    }
    if found.is_empty() {
        return None;
    }
    let mut widgets: Vec<&str> = found.iter().map(|(_, n)| *n).collect();
    widgets.sort_unstable();
    widgets.dedup();
    Some(json!({"fact": BOT_CHECK_FACT, "widgets": widgets,
                "frames": found.iter().map(|(h, _)| h.clone()).collect::<Vec<_>>(),
                "ability": BOT_CHECK_ABILITY, "read": format!("abilities.read name={BOT_CHECK_ABILITY}")}))
}

/// `_submit`: the extension's `submit` fact (protocol 18), kept only in its exact shape.
fn submit(raw: Option<&Value>) -> Option<FormSubmit> {
    let raw = raw?.as_object()?;
    let click = raw.get("click") == Some(&Value::Bool(true));
    let enter = raw.get("enter") == Some(&Value::Bool(true));
    if !(click || enter) {
        return None;
    }
    let text = |k: &str| raw.get(k).and_then(Value::as_str).filter(|s| !s.is_empty());
    let kinds = match raw.get("kinds") {
        Some(Value::Array(k)) => k
            .iter()
            .filter_map(Value::as_str)
            .filter(|k| FIELD_KINDS.contains(k))
            .map(String::from)
            .collect(),
        _ => vec![],
    };
    Some(FormSubmit {
        on_click: click,
        on_enter: enter,
        action_host: text("action_host").map(|h| prefix(h, 253)),
        method: text("method").map(|m| prefix(&m.to_uppercase(), 10)),
        field_kinds: kinds,
        label: raw
            .get("label")
            .and_then(Value::as_str)
            .map(|l| prefix(l, 200))
            .unwrap_or_default(),
    })
}

/// Python's `float(v)` for a rect value.
fn py_float(v: &Value) -> Result<f64> {
    let bad = || Error::driver("a snapshot element's rect wasn't numbers");
    match v {
        Value::Number(n) => n.as_f64().ok_or_else(bad),
        Value::Bool(b) => Ok(if *b { 1.0 } else { 0.0 }),
        Value::String(s) => {
            let t = ferb_py::value::strip(s);
            let lower = t.to_ascii_lowercase();
            let body = lower.trim_start_matches(['+', '-']);
            if ["inf", "infinity", "nan"].contains(&body) {
                return lower.parse::<f64>().map_err(|_| bad());
            }
            let ok = !t.is_empty()
                && !t.starts_with('_')
                && !t.ends_with('_')
                && !t.contains("__")
                && t.chars()
                    .all(|c| c.is_ascii_digit() || "+-.eE_".contains(c));
            if !ok {
                return Err(bad());
            }
            t.replace('_', "").parse::<f64>().map_err(|_| bad())
        }
        _ => Err(bad()),
    }
}

fn opt_text(v: Option<&Value>) -> Option<String> {
    match v {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s.clone()),
        Some(other) => Some(ferb_py::value::py_str(other)),
    }
}

fn truthy(v: Option<&Value>) -> bool {
    v.is_some_and(ferb_py::value::truthy)
}

/// `element(eid, frame_id, raw)`: one element of a snapshot frame. `Err` where Python raises (a
/// rect that isn't four numbers, a level that isn't finite).
pub fn element(eid: &str, frame_id: &str, raw: &Map<String, Value>) -> Result<PageElement> {
    let rect = match raw.get("rect") {
        None => vec![0.0; 4],
        Some(Value::Array(v)) if v.len() == 4 => {
            v.iter().map(py_float).collect::<Result<Vec<f64>>>()?
        }
        Some(_) => {
            return Err(Error::driver(
                "a snapshot element's rect wasn't four numbers",
            ))
        }
    };
    let kind = raw
        .get("field_kind")
        .and_then(Value::as_str)
        .filter(|k| FIELD_KINDS.contains(k))
        .map(String::from);
    let secret = truthy(raw.get("secret")) || kind.is_some();
    let mut value = opt_text(raw.get("value"));
    if secret {
        value = Some(if truthy(raw.get("value")) {
            "filled".into()
        } else {
            String::new()
        });
    }
    let mut state = vec![];
    if truthy(raw.get("disabled")) {
        state.push("disabled".to_string());
    }
    match raw.get("expanded") {
        Some(Value::Bool(true)) => state.push("expanded".into()),
        Some(Value::Bool(false)) => state.push("collapsed".into()),
        _ => {}
    }
    if raw.get("selected") == Some(&Value::Bool(true)) {
        state.push("selected".into());
    }
    let in_view = raw.get("in_view") != Some(&Value::Bool(false));
    let level = match raw.get("level") {
        Some(Value::Bool(b)) => Some(i64::from(*b)),
        Some(Value::Number(n)) => match n.as_i64() {
            Some(i) => Some(i),
            None => {
                let f = n.as_f64().unwrap_or(f64::NAN);
                if !f.is_finite() {
                    return Err(Error::driver(
                        "a snapshot element's level wasn't a finite number",
                    ));
                }
                Some(f.trunc() as i64)
            }
        },
        _ => None,
    };
    let text_or = |k: &str, default: &str| {
        let v = raw.get(k);
        if truthy(v) {
            ferb_py::value::py_str(v.expect("truthy"))
        } else {
            default.to_string()
        }
    };
    Ok(PageElement {
        id: eid.into(),
        frame_id: frame_id.into(),
        role: text_or("role", "generic"),
        label: text_or("label", ""),
        value,
        rect: [rect[0], rect[1], rect[2], rect[3]],
        visible: true,
        input_type: opt_text(raw.get("input_type")),
        href: opt_text(raw.get("href")),
        form: opt_text(raw.get("form")),
        secret,
        code_fill: secret && raw.get("code_fill") == Some(&Value::Bool(true)),
        in_view,
        offscreen: if in_view {
            None
        } else {
            opt_text(raw.get("offscreen"))
        },
        occluded: truthy(raw.get("occluded")),
        container: if truthy(raw.get("container")) {
            opt_text(raw.get("container"))
        } else {
            None
        },
        state,
        level,
        terminal: raw.get("terminal") == Some(&Value::Bool(true)),
        field_kind: kind,
        submit: submit(raw.get("submit")),
        path: raw
            .get("path")
            .and_then(Value::as_str)
            .map(|p| prefix(p, 2000))
            .unwrap_or_default(),
        backend_node: raw
            .get("backend_node")
            .and_then(Value::as_i64)
            .filter(|_| !raw["backend_node"].is_boolean()),
        in_dialog: raw.get("dialog") == Some(&Value::Bool(true)),
    })
}

/// `_form_of`: the form an element is in, by its DOM path up to the nearest `<form>`.
fn form_of(path: &str) -> Option<String> {
    if path.is_empty() {
        return None;
    }
    let parts: Vec<&str> = path.split('>').collect();
    (0..parts.len())
        .rev()
        .find(|i| parts[*i].starts_with("form:"))
        .map(|i| parts[..=i].join(">"))
}

/// `_ranks` (HAND-483): 0 inside an open dialog or the submit button of a form with a field on
/// screen, 1 on screen, 2 the rest.
fn ranks(elements: &[PageElement]) -> Vec<u8> {
    let shown = |e: &PageElement| {
        e.in_view || (e.occluded && e.offscreen.as_deref().is_none_or(str::is_empty))
    };
    let forms: Vec<String> = elements
        .iter()
        .filter(|e| shown(e) && FIELD_ROLES.contains(&e.role.as_str()))
        .filter_map(|e| form_of(&e.path))
        .collect();
    elements
        .iter()
        .map(|e| {
            let submits = e.submit.as_ref().is_some_and(|s| s.on_click)
                && form_of(&e.path).is_some_and(|f| forms.contains(&f));
            if e.in_dialog || submits {
                0
            } else if shown(e) {
                1
            } else {
                2
            }
        })
        .collect()
}

/// `_omitted_index`: how many of each role the preview left out, and its buttons' and links' labels.
fn omitted_index(elements: &[&PageElement]) -> Map<String, Value> {
    let mut by_role: Vec<(String, i64)> = vec![];
    let mut labels: Vec<String> = vec![];
    let (mut used, mut more) = (0usize, 0i64);
    for e in elements {
        match by_role.iter_mut().find(|(r, _)| *r == e.role) {
            Some(entry) => entry.1 += 1,
            None => by_role.push((e.role.clone(), 1)),
        }
        let label = prefix(&e.label, 80);
        if (e.role == "button" || e.role == "link")
            && !e.label.is_empty()
            && !labels.contains(&label)
        {
            let n = label.chars().count();
            if used + n <= OMITTED_LABELS_CHARS {
                labels.push(label);
                used += n;
            } else {
                more += 1;
            }
        }
    }
    by_role.sort_by_key(|(_, n)| -n); // stable: ties keep their first-seen order, as Python's sorted
    let mut out = Map::new();
    out.insert(
        "by_role".into(),
        Value::Object(
            by_role
                .into_iter()
                .map(|(r, n)| (r, Value::from(n)))
                .collect(),
        ),
    );
    if !labels.is_empty() {
        out.insert("buttons_and_links".into(), json!(labels));
    }
    if more > 0 {
        out.insert("more_labels".into(), more.into());
    }
    out
}

fn element_json(e: &PageElement) -> Map<String, Value> {
    let mut d = Map::new();
    d.insert("id".into(), e.id.clone().into());
    d.insert("role".into(), e.role.clone().into());
    d.insert("label".into(), e.label.clone().into());
    if e.secret {
        d.insert("sensitive".into(), true.into());
        d.insert(
            "filled".into(),
            e.value.as_deref().is_some_and(|v| !v.is_empty()).into(),
        );
        if let Some(k) = &e.field_kind {
            d.insert("field".into(), k.clone().into());
        }
    } else if let Some(v) = e.value.as_ref().filter(|v| !v.is_empty()) {
        d.insert("value".into(), v.clone().into());
    }
    if let Some(t) = e
        .input_type
        .as_ref()
        .filter(|t| !t.is_empty() && !["text", "submit", "button"].contains(&t.as_str()))
    {
        d.insert("type".into(), t.clone().into());
    }
    if let Some(h) = e.href.as_ref().filter(|h| !h.is_empty()) {
        d.insert("href".into(), h.clone().into());
    }
    if let Some(f) = e.form.as_ref().filter(|f| !f.is_empty()) {
        d.insert("form".into(), f.clone().into());
    }
    if let Some(l) = e.level {
        d.insert("level".into(), l.into());
    }
    if !e.in_view {
        d.insert("in_view".into(), false.into());
        if let Some(o) = e.offscreen.as_ref().filter(|o| !o.is_empty()) {
            d.insert("offscreen".into(), o.clone().into());
        }
        if e.occluded {
            d.insert("occluded".into(), true.into());
        }
    }
    if let Some(c) = e.container.as_ref().filter(|c| !c.is_empty()) {
        d.insert("container".into(), c.clone().into());
    }
    if !e.state.is_empty() {
        d.insert("state".into(), json!(e.state));
    }
    if e.in_dialog {
        d.insert("dialog".into(), true.into());
    }
    d
}

/// `for_model`: what a worker sees of a snapshot (the gateway redacts it again). `full`: every
/// element and the whole text (browser.read's saved form); `probe`: a loop probe's read, with the
/// media keyed by id.
pub fn for_model(snap: &PageSnapshot, full: bool, probe: bool) -> Map<String, Value> {
    let mut els: Vec<Map<String, Value>> = snap.elements.iter().map(element_json).collect();
    let (mut omitted, mut omitted_in_view) = (0i64, 0i64);
    let mut index = Map::new();
    if !full {
        let ranks = ranks(&snap.elements);
        let mut budget: i64 = 24000;
        let mut keep = vec![false; els.len()];
        let mut left = vec![];
        for rank in 0..3u8 {
            for (i, item) in els.iter().enumerate() {
                if ranks[i] != rank {
                    continue;
                }
                let size = ferb_py::json::dumps_utf8(item).chars().count() as i64;
                if size <= budget {
                    keep[i] = true;
                    budget -= size;
                } else {
                    omitted += 1;
                    omitted_in_view += i64::from(rank < 2);
                    left.push(i);
                }
            }
        }
        els = els
            .into_iter()
            .enumerate()
            .filter(|(i, _)| keep[*i])
            .map(|(_, e)| e)
            .collect();
        if !left.is_empty() {
            left.sort_unstable();
            index = omitted_index(&left.iter().map(|i| &snap.elements[*i]).collect::<Vec<_>>());
        }
    }
    let total = snap.text.chars().count();
    let mut text = if full {
        snap.text.clone()
    } else {
        prefix(&snap.text, MAX_TEXT)
    };
    let shortened = text.chars().count() < total;
    let mut out = Map::new();
    out.insert("tab".into(), snap.tab.tab_id.clone().into());
    out.insert("url".into(), snap.url.clone().into());
    out.insert("title".into(), snap.title.clone().into());
    if snap.loading {
        out.insert(
            "loading".into(),
            "the page was still loading when it was read; parts of it may be missing".into(),
        );
    }
    if snap.content_type.as_deref() == Some("application/pdf") {
        out.insert("pdf".into(), json!({"fact": PDF_FACT, "next": PDF_NEXT}));
    }
    if let Some(check) = bot_check(&snap.frame_urls) {
        out.insert("bot_check".into(), check);
    }
    if !snap.media.is_empty() {
        out.insert("media".into(), json!(snap.media));
        if probe {
            let by_id: Map<String, Value> = snap
                .media
                .iter()
                .filter_map(|m| {
                    m.get("id")
                        .map(|id| (ferb_py::value::py_str(id), Value::Object(m.clone())))
                })
                .collect();
            if !by_id.is_empty() {
                out.insert("media_by_id".into(), Value::Object(by_id));
            }
        }
    }
    if shortened && !full {
        text.push_str("\n[Text preview ends; fetch the full snapshot with browser.read]");
    }
    out.insert(
        "elements".into(),
        Value::Array(els.into_iter().map(Value::Object).collect()),
    );
    out.insert("text".into(), text.into());
    if let Some(scroll) = snap.scroll.as_ref().filter(|s| !s.is_empty()) {
        out.insert("scroll".into(), Value::Object(scroll.clone()));
    }
    if !snap.scrollers.is_empty() {
        out.insert("scrollers".into(), json!(snap.scrollers));
    }
    if snap.truncated || omitted > 0 || shortened {
        out.insert("truncated".into(), true.into());
        out.insert("omitted_elements".into(), omitted.into());
        if !index.is_empty() {
            out.insert("omitted_index".into(), Value::Object(index));
        }
        if omitted_in_view > 0 {
            out.insert("omitted_in_view_elements".into(), omitted_in_view.into());
            out.insert(
                "omitted_note".into(),
                format!(
                    "{omitted_in_view} of the left-out elements are on screen; browser.find with their words returns \
                     their ids, and browser.read pages the full snapshot"
                )
                .into(),
            );
        }
        out.insert("total_text_chars".into(), (total as i64).into());
        out.insert(
            "next".into(),
            "browser.read / ferb_browser.read with this tab and offset=0 retrieves the full saved snapshot as paged JSON".into(),
        );
    }
    if !snap.dialogs.is_empty() {
        out.insert("dialogs".into(), json!(snap.dialogs));
    }
    out
}

/// A snapshot frame's media entry as the extension sends it, kept as a dict (`Dict`).
pub type Media = Dict;
