//! The small rules and words of `ext_bridge.py` that don't need a connection: which key press
//! enters text, a task's tab-group label, why a connection or hello is refused (decision:
//! HAND-406), and the "page isn't responding" error (decisions: HAND-435, HAND-441).

use ferb_core::types::Dict;
use serde_json::{json, Map, Value};

use crate::error::Error;
use crate::generated::ASCII_FOLDS;
use crate::protocol::{MIN_EXTENSION_VERSION, PROTOCOL_VERSION};

/// `str.casefold()` exactly where the result can equal ASCII text: ASCII lowercased, the
/// characters whose fold is ASCII (`ſ` → `s`, `K` → `k`, `ﬀ` → `ff`...) folded, every other
/// character kept as it is (its own fold is not ASCII, so it can never match an ASCII word, nor
/// make a prefix match where Python's wouldn't).
pub fn fold(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c.is_ascii() {
            out.push(c.to_ascii_lowercase());
        } else if let Ok(at) = ASCII_FOLDS.binary_search_by_key(&u32::from(c), |(cp, _)| *cp) {
            out.push_str(ASCII_FOLDS[at].1);
        } else {
            out.push(c);
        }
    }
    out
}

/// `enters_text`: whether a key press puts text into the focused field (decision: HAND-218), as the
/// extension decides it (`protocol.js` `entersText`, the other end): see [`crate::keys`].
pub fn enters_text(spec: Option<&str>) -> bool {
    crate::keys::enters_text(spec)
}

/// `group_title`: a task's tab-group label (HAND-201), "FERB · <the task's first words>", at most
/// about 30 characters; "FERB" when it has none.
pub fn group_title(text: &str) -> String {
    let mut words = ferb_py::value::squeeze(text);
    if words.chars().count() > 30 {
        let head = ferb_py::value::prefix(&words, 30);
        let cut = match head.rfind(' ') {
            Some(at) => &head[..at],
            None => head,
        };
        words = format!("{}…", cut.trim_end_matches([',', '.', ';', ':']));
    }
    if words.is_empty() {
        "FERB".into()
    } else {
        format!("FERB · {words}")
    }
}

/// `too_old`: the plain reason a profile's FERB extension is below the minimum (HAND-406).
pub fn too_old(version: &str) -> String {
    format!(
        "the FERB extension in this profile is too old (version {version}; FERB needs {MIN_EXTENSION_VERSION} or later): update it"
    )
}

/// `version_refusal`: (reason code, plain why) for a hello whose version or protocol FERB can't use.
pub fn version_refusal(version: &str, protocol: i64) -> (&'static str, String) {
    if protocol > PROTOCOL_VERSION {
        return (
            "extension_newer",
            format!(
                "the FERB extension in this profile (version {version}, protocol {protocol}) is newer than this FERB \
                 (protocol {PROTOCOL_VERSION}): update FERB"
            ),
        );
    }
    (
        "extension_too_old",
        format!(
            "{} (its protocol {protocol}, FERB's {PROTOCOL_VERSION})",
            too_old(version)
        ),
    )
}

/// A browser FERB pairs with (`profiles.BROWSERS`): its app's bundle id and the name the owner knows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrowserInfo {
    pub bundle_id: String,
    pub label: String,
}

/// Which apps are web browsers (`ferb/act/mac/browsers.py` `known`, no I/O) and the browsers FERB
/// pairs with (`profiles.BROWSERS`): the Mac-hands part's tables, behind a seam.
pub trait BrowserApps: Send + Sync {
    fn known(&self, bundle_id: &str) -> bool;
    fn browser(&self, key: &str) -> Option<BrowserInfo>;
}

/// `peer_refusal`: why a native-host connection isn't taken, `None` when it is (protocol 18; HAND-223,
/// HAND-438). `app` is the bundle id of the app whose process started the native host;
/// `browser` the paired profile's browser key, for a hello.
pub fn peer_refusal(
    apps: &dyn BrowserApps,
    app: Option<&str>,
    browser: Option<&str>,
) -> Option<String> {
    let Some(app) = app else {
        return Some(
            "FERB couldn't tell which app started this browser connection, so it didn't take it (the FERB extension \
             reconnects by itself)"
                .into(),
        );
    };
    if !apps.known(app) {
        return Some(format!(
            "this browser connection was started by {app}, not a web browser FERB knows, so FERB didn't take it"
        ));
    }
    if let Some(info) = browser.and_then(|b| apps.browser(b)) {
        let (want, got) = (fold(&info.bundle_id), fold(app));
        if got != want && !got.starts_with(&format!("{want}.helper")) {
            return Some(format!(
                "this profile was paired in {}, but the connection comes from {app}, so FERB didn't take it",
                info.label
            ));
        }
    }
    None
}

/// What each step of a read is, for "the page isn't responding" (extension 0.19.6 names the step).
pub const READ_STEPS: [(&str, &str); 10] = [
    ("wake", "keeping the tab awake"),
    ("attach", "preparing the tab"),
    ("inject", "loading FERB's reader into the page"),
    ("script", "reading the page"),
    ("ax", "reading the page's accessibility tree"),
    ("unbind", "tidying up after the read"),
    ("arm", "preparing the tab"),
    ("ready", "preparing the tab"),
    ("look", "finding the page's secret fields"),
    ("capture", "taking the picture"),
];

/// `_steps`: the extension's step times as numbers only (at most 30, keys cut to 40 characters);
/// `None` from an older extension.
pub fn steps(raw: Option<&Value>) -> Option<Map<String, Value>> {
    let Some(Value::Object(m)) = raw else {
        return None;
    };
    Some(
        m.iter()
            .take(30)
            .filter(|(_, v)| v.is_number())
            .map(|(k, v)| (ferb_py::value::prefix(k, 40).to_string(), v.clone()))
            .collect(),
    )
}

/// Python's `f"{x:g}"`.
pub fn fmt_g(x: f64) -> String {
    if x == 0.0 {
        return if x.is_sign_negative() {
            "-0".into()
        } else {
            "0".into()
        };
    }
    if !x.is_finite() {
        return if x.is_nan() {
            "nan".into()
        } else if x > 0.0 {
            "inf".into()
        } else {
            "-inf".into()
        };
    }
    let sci = format!("{x:.5e}");
    let (mantissa, exp) = sci.split_once('e').expect("{:e} writes an exponent");
    let exp: i32 = exp.parse().expect("an integer exponent");
    if !(-4..6).contains(&exp) {
        let m = mantissa.trim_end_matches('0').trim_end_matches('.');
        let sign = if exp < 0 { '-' } else { '+' };
        return format!("{m}e{sign}{:02}", exp.abs());
    }
    let decimals = usize::try_from(5 - exp).unwrap_or(0);
    let fixed = format!("{x:.decimals$}");
    if fixed.contains('.') {
        fixed
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_string()
    } else {
        fixed
    }
}

/// `not_responding`: a read of a tab that timed out (HAND-435, HAND-441).
pub fn not_responding(
    op: &str,
    timeout: f64,
    step: Option<&str>,
    raw_steps: Option<&Value>,
) -> Error {
    let step = step.filter(|s| !s.is_empty());
    let place = step
        .map(|s| {
            let words = READ_STEPS
                .iter()
                .find(|(k, _)| *k == s)
                .map_or(s, |(_, w)| *w);
            format!(" (it stopped at: {words})")
        })
        .unwrap_or_default();
    let mut facts = Dict::new();
    facts.insert("reason".into(), "page_not_responding".into());
    if let Some(s) = step {
        facts.insert("step".into(), s.into());
    }
    let mut e = Error::driver_with(
        format!(
            "the page isn't responding: FERB's {op} got no answer from it within {} s{place}. Reading this tab again \
             usually hangs the same way: refresh it once (navigate it to its address again); if it still doesn't \
             answer, open the same address in a new tab and read that; if that fails too, report it",
            fmt_g(timeout)
        ),
        facts,
    );
    e.timing = steps(raw_steps)
        .filter(|s| !s.is_empty())
        .map(|s| json!({"read": s}));
    e
}

/// `frame_urls`: the addresses of a snapshot's embedded frames, in order, once each (HAND-208).
pub fn frame_urls(frames: &[Value]) -> Vec<String> {
    let mut found: Vec<String> = vec![];
    for f in frames {
        let mut urls: Vec<String> = f
            .get("iframes")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter(|i| i.is_object())
                    .map(|i| text_or_empty(i.get("src")))
                    .collect()
            })
            .unwrap_or_default();
        if frame_id(f) != 0 {
            urls.push(text_or_empty(f.get("url")));
        }
        for u in urls {
            if (u.starts_with("http://") || u.starts_with("https://")) && !found.contains(&u) {
                found.push(u);
            }
        }
    }
    found
}

/// `str(v or "")` for the text fields the extension sends.
pub fn text_or_empty(v: Option<&Value>) -> String {
    match v {
        None => String::new(),
        Some(v) if !ferb_py::value::truthy(v) => String::new(),
        Some(v) => ferb_py::value::py_str(v),
    }
}

/// `int(f.get("frame_id") or 0)` for the integer frame ids the extension sends.
pub fn frame_id(f: &Value) -> i64 {
    match f.get("frame_id") {
        Some(v) if ferb_py::value::truthy(v) => crate::lax::int(v).unwrap_or(0),
        _ => 0,
    }
}
