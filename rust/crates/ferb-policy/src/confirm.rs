//! policy/confirm.py: the code floor (D23) and the sink rule (S4): when a consequential call needs the owner, decided
//! from facts (the kind the capability derives, its targets' provenance, what the task tree has read, the known
//! destinations), and FERB's question in the owner's terms. Never judges intent or content: a declared purpose may
//! add a need, never remove one (W2).
//!
//! What another part of FERB answers comes in through seams: the running and installed apps (`Apps`), whether a
//! destination is known or approved (`known`/`approved` closures over the store), the owner's own addresses (`own`).

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::OnceLock;

use ferb_core::types::Target;
use ferb_sanitize::origin;
use serde_json::{json, Value};

use crate::destinations::destination_of;
use crate::provenance::{self, ArgSources, Text, MIN_LEN};
use crate::rules::Kind;
use crate::rx::once;
use crate::{idna, norm, py, pyurl, tls, urls};

pub const UNREAD_AMOUNT: &str = "an amount FERB couldn't read";
pub const ACCOUNT_SECURITY: &str =
    "a password, two-factor or recovery setting, or deleting the account";
pub const SEND_CLASS: &[&str] = &[
    Kind::SEND,
    Kind::REPLY,
    Kind::POST,
    Kind::MAILBOX_SEND,
    Kind::SUBMIT,
    Kind::CALL,
];
pub const OWNER_SOURCES: &[&str] = &["owner_words", "owner_answer"];
pub const SINK_DESTINATION: &str = "sink:destination";
pub const BULK: &str = "sink:irreversible_bulk";
pub const TLS_OFF: &str = "floor:tls_off";
pub const TLS_QUESTION: &str =
    "Run this with certificate checks turned off? `{command}` — this skips the check that {site}";
pub const TERMINAL_INPUT: &str = "floor:terminal_input";
pub const TERMINAL_APP: &str = "floor:terminal_app";
pub const FOREGROUND: &str = "floor:foreground";
pub const FOREGROUND_TOOLS: &[&str] = &["mac.act", "mac.click", "mac.pointer"];
pub const BROWSER_PAGE: &str = "floor:browser_page";
pub const BROWSER_SCHEMES: &[&str] = &[
    "chrome://",
    "brave://",
    "edge://",
    "arc://",
    "vivaldi://",
    "opera://",
    "chrome-extension://",
    "extension://",
    "moz-extension://",
    "safari-web-extension://",
    "about:",
];
pub const NOT_BROWSER_PAGES: &[&str] = &["about:blank", "about:srcdoc"];
pub const PAIR: &str = "floor:pair_profile";
/// GATE-207: the routine questions a "don't ask for 30 minutes" window may answer Yes to.
pub const ROUTINE: &[&str] = &[FOREGROUND, BROWSER_PAGE, "floor:new_recipient", PAIR];
pub const ALLOW_WINDOW_S: f64 = 1800.0;
pub const TYPING_ACTIONS: &[&str] = &["type", "set_value", "edit", "key", "select"];
pub const SHOWN: usize = 3;
pub const QUOTED_CHARS: usize = 200;
pub const FROM_TASK: &str = "(from the task) ";
pub const CHECK_PREFIX: &str = "FERB:";
pub const FROM_AI: &str = "(written by the AI) ";
pub const ALWAYS_PAY: &str = "floor:pay";
pub const ALWAYS_ACCOUNT_SECURITY: &str = "floor:account_security";
const TRAIL: &str = ".,;:!?)]}>\"'";
const TWO_LEVEL: &[&str] = &[
    "co.uk", "org.uk", "gov.uk", "ac.uk", "me.uk", "com.au", "net.au", "org.au", "edu.au",
    "gov.au", "co.nz", "org.nz", "govt.nz", "co.za", "org.za", "co.in", "net.in", "org.in",
    "co.jp", "or.jp", "ne.jp", "com.br", "com.cn", "net.cn", "org.cn", "gov.cn", "co.kr", "com.mx",
    "com.sg", "com.hk", "com.tw", "co.il", "com.tr", "co.id", "com.ua",
];
const WORD_JOINERS: &str = ".-_'\u{2019}";

/// `ALWAYS[kind]`.
pub fn always_rule(kind: &str) -> Option<&'static str> {
    match kind {
        Kind::PAY => Some(ALWAYS_PAY),
        Kind::ACCOUNT_SECURITY => Some(ALWAYS_ACCOUNT_SECURITY),
        _ => None,
    }
}

/// A fact a sentence needs is missing (Python's KeyError).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Missing(pub String);

/// A need's facts, in the order they were written (Python's dict).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Values(pub Vec<(String, String)>);

impl Values {
    pub fn new() -> Values {
        Values(Vec::new())
    }

    pub fn of(pairs: &[(&str, &str)]) -> Values {
        let mut v = Values::new();
        for (k, x) in pairs {
            v.set(k, x);
        }
        v
    }

    /// `v.get(k)`.
    pub fn get(&self, k: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(key, _)| key == k)
            .map(|(_, v)| v.as_str())
    }

    /// `v.get(k, "")`.
    pub fn opt(&self, k: &str) -> &str {
        self.get(k).unwrap_or("")
    }

    /// `bool(v.get(k))`.
    pub fn has(&self, k: &str) -> bool {
        !self.opt(k).is_empty()
    }

    /// `v[k]`.
    pub fn req(&self, k: &str) -> Result<&str, Missing> {
        self.get(k).ok_or_else(|| Missing(k.to_string()))
    }

    /// `v[k] = x` (an existing key keeps its place).
    pub fn set(&mut self, k: &str, x: &str) {
        match self.0.iter_mut().find(|(key, _)| key == k) {
            Some(slot) => slot.1 = x.to_string(),
            None => self.0.push((k.to_string(), x.to_string())),
        }
    }

    /// As a JSON object (key order kept).
    pub fn to_json(&self) -> Value {
        Value::Object(self.0.iter().map(|(k, v)| (k.clone(), json!(v))).collect())
    }
}

/// confirm.py:139 `Need`.
#[derive(Debug, Clone, PartialEq)]
pub struct Need {
    pub rule: String,
    /// What one answer covers in this task tree.
    pub key: String,
    pub values: Values,
    /// SQ1: the Yes covers one run of the call, never a later one.
    pub always: bool,
    /// (kind, value) a Yes makes known (SQ6).
    pub destinations: Vec<(String, String)>,
    /// Asked every time: an earlier Yes never covers it (a No still refuses).
    pub fresh: bool,
    /// An always-ask Yes spent when given, never released.
    pub spent_on_ask: bool,
    /// The target this need was decided for: what a standing scope must cover (D30).
    pub reaches: Option<Target>,
    /// GATE-586: a wider key a Yes to this need also answers Yes for the rest of the task tree.
    pub grants: String,
}

impl Need {
    pub fn new(rule: &str, key: String, values: Values) -> Need {
        Need {
            rule: rule.to_string(),
            key,
            values,
            always: false,
            destinations: Vec::new(),
            fresh: false,
            spent_on_ask: false,
            reaches: None,
            grants: String::new(),
        }
    }
}

/// A running app (`apps.RunningApp`): its bundle id and name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunningApp {
    pub bundle_id: String,
    pub name: String,
}

/// The running-app list or Launch Services couldn't be read (Python's `except Exception` around the lookup).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppsUnread;

/// The Mac's apps, as ferb/act/mac/apps.py and browsers.py read them (a seam for the platform crate).
pub trait Apps {
    /// `apps.candidates(name)`: the running apps a name, `pid:N` or bundle id names; Err when the list can't be read.
    fn candidates(&self, name: &str) -> Result<Vec<RunningApp>, AppsUnread>;
    /// `apps.installed_bundle(name)`: the installed app's bundle id macOS finds by this name; Err when it can't ask.
    fn installed_bundle(&self, name: &str) -> Result<Option<String>, AppsUnread>;
    /// `browsers.is_browser_app(app)`.
    fn is_browser_app(&self, app: &str) -> bool;
}

/// `key(*parts)`: the rule and the exact values one answer covers (JSON of the tuple, ensure_ascii=False).
pub fn key(parts: &[Value]) -> String {
    ferb_py::json::dumps_utf8(&Value::Array(parts.to_vec()))
}

fn s(x: &str) -> Value {
    json!(x)
}

fn opt(x: &Option<String>) -> Value {
    x.as_ref().map_or(Value::Null, |v| json!(v))
}

/// `browsers.clean`: an app's name or id as compared: invisible marks removed, outer spaces trimmed, casefolded.
pub fn clean(name: &str) -> String {
    let kept: String = name
        .chars()
        .filter(|&c| {
            !matches!(c as u32, 0x200b..=0x200f | 0x202a..=0x202e | 0x2060..=0x2064 | 0x2066..=0x2069 | 0xfeff)
        })
        .collect();
    py::casefold(py::strip(&kept))
}

/// `pair_need`: one Yes per profile in the task.
pub fn pair_need(name: &str, browser: &str, folder: &str, email: &str) -> Need {
    Need::new(
        PAIR,
        key(&[s(PAIR), s(browser), s(folder)]),
        Values::of(&[
            ("name", if name.is_empty() { folder } else { name }),
            ("browser", &py::capitalize(browser)),
            ("email", email),
        ]),
    )
}

/// `read_lift_key`.
pub fn read_lift_key(rule: &str) -> String {
    format!("lift:read:{rule}")
}

/// `routine`: whether a window of "don't ask" may answer this need (GATE-207).
pub fn routine(n: &Need) -> bool {
    ROUTINE.contains(&n.rule.as_str())
        || (n.rule.starts_with("escape:") && n.values.get("lift") == Some("read"))
}

/// `browser_page`: whether an address is one of a browser's own pages (scheme only).
pub fn browser_page(address: Option<&str>) -> bool {
    let text = py::casefold(py::strip(address.unwrap_or("")));
    BROWSER_SCHEMES.iter().any(|p| text.starts_with(p))
        && !NOT_BROWSER_PAGES.iter().any(|p| text.starts_with(p))
}

/// `browser_page_need`: every input on that browser page asks (always).
pub fn browser_page_need(app: &str, page: &str, verb: &str) -> Need {
    let page = py::strip(page);
    let mut values = Values::of(&[("app", app), ("page", page)]);
    if !verb.is_empty() {
        values.set("verb", verb);
    }
    let mut n = Need::new(
        BROWSER_PAGE,
        key(&[s(BROWSER_PAGE), s(&py::casefold(app)), s(page)]),
        values,
    );
    n.always = true;
    n
}

fn arg_str(args: &serde_json::Map<String, Value>, k: &str) -> String {
    match args.get(k) {
        None | Some(Value::Null) => String::new(),
        Some(v) if !ferb_py::value::truthy(v) => String::new(),
        Some(v) => ferb_py::value::py_str(v),
    }
}

/// `input_verb`: what a Mac input does, as a card says it (GATE-566). Display only.
pub fn input_verb(tool: &str, args: &serde_json::Map<String, Value>) -> &'static str {
    if tool == "mac.applescript" {
        return "Run a script";
    }
    if tool == "mac.act" && TYPING_ACTIONS.contains(&arg_str(args, "action").as_str()) {
        "Type"
    } else {
        "Click"
    }
}

/// `foreground_need` (rule 17).
pub fn foreground_need(
    tool: &str,
    args: &serde_json::Map<String, Value>,
    apps: &dyn Apps,
) -> Option<Need> {
    let acts = FOREGROUND_TOOLS.contains(&tool)
        || tool == "mac.applescript"
        || (tool == "mac.menu" && !py::strip(&arg_str(args, "path")).is_empty());
    let in_browser = acts && apps.is_browser_app(&arg_str(args, "app"));
    let flagged = args
        .get("allow_foreground")
        .is_some_and(ferb_py::value::truthy)
        || args.get("mechanism") == Some(&json!("foreground"));
    if !in_browser && (!FOREGROUND_TOOLS.contains(&tool) || !flagged) {
        return None;
    }
    let app = {
        let a = arg_str(args, "app");
        if a.is_empty() {
            "an app".to_string()
        } else {
            a
        }
    };
    let mut values = Values::of(&[("app", &app)]);
    if tool == "mac.applescript" {
        values.set("script", "yes");
        values.set("script_text", &arg_str(args, "script"));
    }
    Some(Need::new(
        FOREGROUND,
        key(&[s(FOREGROUND), s(&app_key(&app, apps))]),
        values,
    ))
}

/// `app_key`: one app however a Mac tool's `app` argument spells it (GATE-201, GATE-560).
pub fn app_key(app: &str, apps: &dyn Apps) -> String {
    let Ok(found) = apps.candidates(py::strip(app)) else {
        return clean(app);
    };
    if found.len() == 1 {
        return py::casefold(&found[0].bundle_id);
    }
    if !found.is_empty() {
        return clean(app);
    }
    match apps.installed_bundle(py::strip(app)) {
        Ok(Some(b)) if !b.is_empty() => py::casefold(&b),
        _ => clean(app),
    }
}

fn bundle_id_shape(text: &str) -> bool {
    // (?:[A-Za-z0-9-]+\.){2,}[A-Za-z0-9-]+|pid:\d+
    if let Some(rest) = text.strip_prefix("pid:") {
        if !rest.is_empty() && rest.chars().all(py::is_decimal) {
            return true;
        }
    }
    let parts: Vec<&str> = text.split('.').collect();
    parts.len() >= 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
}

/// `app_label`: an app as the owner knows it (a bundle id or `pid:N` naming exactly one running app becomes its
/// name). Display only.
pub fn app_label(app: &str, apps: &dyn Apps) -> String {
    let text = py::strip(app);
    if !bundle_id_shape(text) {
        return app.to_string();
    }
    match apps.candidates(text) {
        Ok(found) if found.len() == 1 && !found[0].name.is_empty() => found[0].name.clone(),
        _ => app.to_string(),
    }
}

pub(crate) fn who(target: &Target) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = target
        .recipients
        .iter()
        .enumerate()
        .map(|(i, r)| (format!("recipient:{i}"), r.clone()))
        .collect();
    if let Some(c) = &target.contact {
        if !c.is_empty() {
            out.push(("contact".to_string(), c.clone()));
        }
    }
    out
}

/// `site_is_destination`: a send-class call whose target has no recipient or contact.
pub fn site_is_destination<'a>(kinds: impl IntoIterator<Item = &'a str>, keys: &[String]) -> bool {
    kinds.into_iter().any(|k| SEND_CLASS.contains(&k))
        && !keys
            .iter()
            .any(|k| k == "contact" || k.starts_with("recipient:"))
}

fn owner(sources: &[String]) -> bool {
    sources.iter().any(|s| OWNER_SOURCES.contains(&s.as_str()))
}

/// `owner_named`.
pub fn owner_named(sources: &[String]) -> bool {
    owner(sources)
}

/// `shown_quoted`: a page or item inside FERB's sentence: one line, clipped, quoted as data.
pub fn shown_quoted(text: &str) -> String {
    let one = py::squeeze(text);
    let clipped = if py::char_len(&one) <= QUOTED_CHARS {
        one
    } else {
        format!("{}…", py::prefix(&one, QUOTED_CHARS - 1))
    };
    ferb_py::json::dumps_utf8(&clipped)
}

/// `sample`: up to SHOWN items, each quoted, the rest counted. Display only.
pub fn sample<I: IntoIterator<Item = S>, S: AsRef<str>>(items: I) -> String {
    let listed: Vec<String> = items
        .into_iter()
        .map(|i| ferb_py::json::dumps_utf8(i.as_ref()))
        .collect();
    if listed.len() <= SHOWN {
        return listed.join(", ");
    }
    format!(
        "{} and {} more",
        listed[..SHOWN].join(", "),
        listed.len() - SHOWN
    )
}

fn counted(n: i128, thing: &str) -> String {
    format!("{n} {thing}{}", if n == 1 { "" } else { "s" })
}

fn host_shown(where_: &str) -> String {
    let w = py::strip(where_);
    if py::casefold(w).starts_with("www.") {
        py::after(w, 4).to_string()
    } else {
        w.to_string()
    }
}

/// `f"{x:.2f}"` (round half to even on the exact binary value, as Python does).
pub(crate) fn fixed2(x: f64) -> String {
    format!("{x:.2}")
}

/// `shown_amount`: an amount as the owner reads it, with the page's currency.
pub fn shown_amount(amount: Option<f64>, currency: Option<&str>) -> String {
    let Some(amount) = amount else {
        return String::new();
    };
    let number = if amount == 0.0 {
        "0".to_string()
    } else if amount.is_finite() && amount == amount.trunc() {
        format!("{amount:.0}")
    } else {
        fixed2(amount)
    };
    let unit = py::prefix(&py::squeeze(currency.unwrap_or("")), 8).to_string();
    if unit.is_empty() {
        number
    } else if py::char_len(&unit) == 1 {
        format!("{unit}{number}")
    } else {
        format!("{unit} {number}")
    }
}

fn bundle_like(detail: &str) -> bool {
    // [a-z0-9-]+(?:\.[a-z0-9-]+){2,} under re.I
    let parts: Vec<&str> = detail.split('.').collect();
    parts.len() >= 3
        && parts.iter().all(|p| {
            !p.is_empty()
                && p.chars()
                    .all(|c| py::ci_ascii_letter(c) || c.is_ascii_digit() || c == '-')
        })
}

/// `shown_origin`: one origin label in the owner's terms.
pub fn shown_origin(label: &str) -> String {
    let (head, _, rest) = py::partition(label, ":");
    if head != "external" {
        return label.to_string();
    }
    let (kind, _, detail) = py::partition(rest, ":");
    match kind {
        "web" => match detail {
            "excluded" => "a site on your exclusion list".into(),
            "trail" => "pages you visited".into(),
            "" => "a web page".into(),
            d => format!("the web page on {d}"),
        },
        "paste" => "text you pasted".into(),
        "" | "unlabeled" => "content FERB couldn't label".into(),
        "download" => "a downloaded file".into(),
        "memory" => "something FERB remembered".into(),
        "email" => "an email".into(),
        "message" => {
            if !detail.is_empty() && !bundle_like(detail) {
                format!("a message, {detail}")
            } else {
                "a message".into()
            }
        }
        k => format!("content from {k}"),
    }
}

/// `reads_as_ferbs`: whether a text starts with "FERB", ignoring case, spaces, punctuation and compatibility forms.
pub fn reads_as_ferbs(text: &str) -> bool {
    let kept: String = norm::nfkc(text)
        .chars()
        .filter(|&c| py::is_alnum(c))
        .collect();
    py::casefold(&kept).starts_with("ferb")
}

/// `from_task`.
pub fn from_task(text: &str) -> String {
    if reads_as_ferbs(text) {
        format!("{FROM_TASK}{text}")
    } else {
        text.to_string()
    }
}

/// `task_question`.
pub fn task_question(text: &str) -> String {
    from_task(&py::squeeze(text))
}

/// `from_task_lines`.
pub fn from_task_lines(text: &str) -> String {
    text.split('\n')
        .map(from_task)
        .collect::<Vec<_>>()
        .join("\n")
}

/// `reads_as_ferbs_check` (GATE-581).
pub fn reads_as_ferbs_check(text: &str) -> bool {
    let kept: String = norm::nfkc(text)
        .chars()
        .filter(|&c| py::is_alnum(c) || c == ':')
        .collect();
    py::casefold(kept.trim_start_matches(':')).starts_with(&py::casefold(CHECK_PREFIX))
}

/// `reply_lines` (GATE-581).
pub fn reply_lines(text: &str) -> String {
    text.split('\n')
        .map(|line| {
            if reads_as_ferbs_check(line) {
                format!("{FROM_AI}{line}")
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn ordered_labels<'a>(labels: impl IntoIterator<Item = &'a str>) -> Vec<&'a str> {
    let set: BTreeSet<&str> = labels.into_iter().collect();
    let mut v: Vec<&str> = set.into_iter().collect();
    v.sort_by_key(|o| (o.matches(':').count() < 2, *o));
    v
}

fn dedup(items: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    items
        .into_iter()
        .filter(|i| seen.insert(i.clone()))
        .collect()
}

/// `shown_origins`: the outside sources in the owner's terms, the first SHOWN named and the rest counted.
pub fn shown_origins<'a>(labels: impl IntoIterator<Item = &'a str>) -> String {
    let items = dedup(
        ordered_labels(labels)
            .into_iter()
            .map(shown_origin)
            .collect(),
    );
    if items.len() <= SHOWN {
        return items.join("; ");
    }
    format!(
        "{}; and {} more",
        items[..SHOWN].join("; "),
        items.len() - SHOWN
    )
}

/// `shown_place`.
pub fn shown_place(label: &str) -> String {
    let (head, _, rest) = py::partition(label, ":");
    let (kind, _, detail) = py::partition(rest, ":");
    if head == "external" && kind == "web" && detail != "excluded" && detail != "trail" {
        return if detail.is_empty() {
            "a web page".into()
        } else {
            format!("a {detail} page")
        };
    }
    if head == "external" && kind == "message" {
        return "a message".into();
    }
    shown_origin(label)
}

/// `shown_places`.
pub fn shown_places<'a>(labels: impl IntoIterator<Item = &'a str>) -> String {
    let mut items = dedup(
        ordered_labels(labels)
            .into_iter()
            .map(shown_place)
            .collect(),
    );
    if items.len() > SHOWN {
        let more = items.len() - SHOWN;
        items.truncate(SHOWN);
        items.push(format!("{more} more places"));
    }
    match items.len() {
        0 => String::new(),
        1 => items[0].clone(),
        n => format!("{} and {}", items[..n - 1].join(", "), items[n - 1]),
    }
}

fn email_re() -> &'static crate::rx::Compiled {
    once!(plain, r"[{W}.+\-]+@[{W}\-]+(?:\.[{W}\-]+)+")
}

fn phone_re() -> &'static crate::rx::Compiled {
    once!(plain, r"\+?[{D}][{D} ().\-]{5,}[{D}]")
}

/// `value_kind` (GATE-509): "email address", "phone number", else "text".
pub fn value_kind(value: &str) -> &'static str {
    let text = py::strip_chars(py::strip(value), "\"");
    if once!(plain, r"\A(?:[{W}.+\-]+@[{W}\-]+(?:\.[{W}\-]+)+)\z").full(text) {
        return "email address";
    }
    if once!(plain, r"\A(?:\+?[{D}][{D} ().\-]{5,}[{D}])\z").full(text)
        && text.chars().filter(|&c| py::is_digit(c)).count() >= 7
    {
        return "phone number";
    }
    "text"
}

/// `destination_key` (SQ6).
pub fn destination_key(host: &str) -> String {
    key(&[s(SINK_DESTINATION), s(host)])
}

/// `_FINE.split(text)`: pieces between runs of whitespace and the punctuation framing a value (empty ones kept).
pub fn fine_split(text: &str) -> Vec<&str> {
    let sep = |c: char| py::is_space(c) || "=:\"',;/&?#@<>()[]{}|~`!*+".contains(c);
    let mut out = Vec::new();
    let mut start = 0;
    let mut in_sep = false;
    for (i, c) in text.char_indices() {
        if sep(c) {
            if !in_sep {
                out.push(&text[start..i]);
                in_sep = true;
            }
        } else if in_sep {
            start = i;
            in_sep = false;
        }
    }
    out.push(if in_sep { "" } else { &text[start..] });
    out
}

/// `registrable`: a host's site (its registrable domain); an IP address is its own site.
pub fn registrable(host: &str) -> String {
    let host = py::lower(host);
    let host = host.trim_end_matches('.');
    if pyurl::ip_address(py::strip_chars(host, "[]")).is_some() {
        return host.to_string();
    }
    let labels: Vec<&str> = host.split('.').collect();
    let reg = if labels.len() >= 3
        && TWO_LEVEL.contains(&labels[labels.len() - 2..].join(".").as_str())
    {
        3
    } else {
        2
    };
    labels[labels.len().saturating_sub(reg)..].join(".")
}

fn fine_tokens(text: &str) -> HashSet<String> {
    fine_split(&py::lower(text))
        .into_iter()
        .filter(|t| py::char_len(t) >= MIN_LEN)
        .map(str::to_string)
        .collect()
}

/// `_GROUPED_DIGITS.finditer`: digit groups split only by grouping separators (`\d+(?:[\s.,\-]+\d+)+`).
#[doc(hidden)]
pub fn grouped_digits(value: &str) -> Vec<&str> {
    let chars: Vec<(usize, char)> = value.char_indices().collect();
    let sep = |c: char| py::is_space(c) || ".,-".contains(c);
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if !py::is_decimal(chars[i].1) {
            i += 1;
            continue;
        }
        let mut j = i;
        while j < chars.len() && py::is_decimal(chars[j].1) {
            j += 1;
        }
        let mut end = j;
        let mut more = 0;
        loop {
            let mut k = end;
            while k < chars.len() && sep(chars[k].1) {
                k += 1;
            }
            if k == end || k >= chars.len() || !py::is_decimal(chars[k].1) {
                break;
            }
            while k < chars.len() && py::is_decimal(chars[k].1) {
                k += 1;
            }
            end = k;
            more += 1;
        }
        if more > 0 {
            let a = chars[i].0;
            let b = chars.get(end).map_or(value.len(), |x| x.0);
            out.push(&value[a..b]);
            i = end;
        } else {
            i = j;
        }
    }
    out
}

#[doc(hidden)]
pub fn spellings_of(value: &str, digits: bool) -> HashSet<String> {
    let mut out = HashSet::new();
    let skeleton: String = value.chars().filter(|&c| py::is_alnum(c)).collect();
    let n = py::char_len(&skeleton);
    if n >= MIN_LEN && skeleton.chars().any(py::is_digit) {
        out.insert(skeleton.clone());
    }
    if digits {
        for m in grouped_digits(value) {
            let j: String = m.chars().filter(|&c| py::is_digit(c)).collect();
            if py::char_len(&j) >= MIN_LEN {
                out.insert(j);
            }
        }
    }
    if py::lstrip(value).starts_with('+')
        && py::all_digits(&skeleton)
        && n >= provenance::DESTINATION_PHONE_DIGITS
    {
        for k in 1..=3 {
            let rest = py::after(&skeleton, k);
            out.insert(format!("0{rest}"));
            out.insert(rest.to_string());
        }
    }
    out
}

/// `is_year`: a plain year, 1900-2099, alone (GATE-535).
pub fn is_year(value: &str) -> bool {
    let c: Vec<char> = value.chars().collect();
    c.len() == 4
        && ((c[0] == '1' && c[1] == '9') || (c[0] == '2' && c[1] == '0'))
        && py::is_decimal(c[2])
        && py::is_decimal(c[3])
}

/// `is_date`: a full calendar date as numbers alone (GATE-535).
pub fn is_date(value: &str) -> bool {
    let c: Vec<char> = value.chars().collect();
    let sep = |x: char| "-/.".contains(x);
    let dec = |x: char| py::is_decimal(x);
    let year_at = |i: usize| -> bool {
        c.len() >= i + 4
            && ((c[i] == '1' && c[i + 1] == '9') || (c[i] == '2' && c[i + 1] == '0'))
            && dec(c[i + 2])
            && dec(c[i + 3])
    };
    let num = |from: usize, to: usize| -> u32 {
        c[from..to]
            .iter()
            .fold(0, |n, &x| n * 10 + py::decimal_value(x).unwrap_or(0))
    };
    // year first: (19|20)\d\d[-/.](\d{1,2})[-/.](\d{1,2})
    if year_at(0) && c.len() > 4 && sep(c[4]) {
        for m_len in [1usize, 2] {
            let m_end = 5 + m_len;
            if c.len() > m_end && c[5..m_end].iter().all(|&x| dec(x)) && sep(c[m_end]) {
                let d_len = c.len() - m_end - 1;
                if (1..=2).contains(&d_len) && c[m_end + 1..].iter().all(|&x| dec(x)) {
                    let (a, b) = (num(5, m_end), num(m_end + 1, c.len()));
                    return (1..=12).contains(&a) && (1..=31).contains(&b);
                }
            }
        }
    }
    // (\d{1,2})[-/.](\d{1,2})[-/.](19|20)\d\d
    for a_len in [1usize, 2] {
        if c.len() <= a_len || !c[..a_len].iter().all(|&x| dec(x)) || !sep(c[a_len]) {
            continue;
        }
        for b_len in [1usize, 2] {
            let b_end = a_len + 1 + b_len;
            if c.len() == b_end + 5
                && c[a_len + 1..b_end].iter().all(|&x| dec(x))
                && sep(c[b_end])
                && year_at(b_end + 1)
            {
                let (a, b) = (num(0, a_len), num(a_len + 1, b_end));
                return ((1..=31).contains(&a) && (1..=12).contains(&b))
                    || ((1..=12).contains(&a) && (1..=31).contains(&b));
            }
        }
    }
    false
}

// ---------- grouped values (GATE-516..518, GATE-527) ----------

/// `_RUN.finditer(text)`: each run of digit groups joined by single spaces, as byte spans
/// (`(?<![\w+\-./@%:,])(G(?: G)+)(?![\w\-/@%]|[.:,]\d)`, G = `\+?[^\W_]*\d[^\W_]*`).
#[doc(hidden)]
pub fn run_spans(text: &str) -> Vec<(usize, usize)> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let at = |i: usize| chars.get(i).map(|x| x.1);
    let pos = |i: usize| chars.get(i).map_or(text.len(), |x| x.0);
    let alnum = crate::rx::alnum;
    // one G from i: Some(end) when '+'? then an alnum run holding a decimal digit
    let group = |i: usize| -> Option<usize> {
        let mut j = i;
        if at(j) == Some('+') {
            j += 1;
        }
        let start = j;
        let mut digit = false;
        while let Some(c) = at(j) {
            if !alnum(c) {
                break;
            }
            digit |= py::is_decimal(c);
            j += 1;
        }
        (j > start && digit).then_some(j)
    };
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let before_ok =
            i == 0 || !matches!(at(i - 1), Some(c) if crate::rx::word(c) || "+-./@%:,".contains(c));
        let first = if before_ok { group(i) } else { None };
        let Some(mut end) = first else {
            i += 1;
            continue;
        };
        let mut ends = vec![end];
        while at(end) == Some(' ') {
            match group(end + 1) {
                Some(e) => {
                    end = e;
                    ends.push(e);
                }
                None => break,
            }
        }
        let ahead_ok = |e: usize| -> bool {
            match at(e) {
                None => true,
                Some(c) if crate::rx::word(c) || "-/@%".contains(c) => false,
                Some(c) if ".:,".contains(c) => !at(e + 1).is_some_and(py::is_decimal),
                Some(_) => true,
            }
        };
        let found = (1..ends.len()).rev().find(|&k| ahead_ok(ends[k]));
        match found {
            Some(k) => {
                out.push((pos(i), pos(ends[k])));
                i = ends[k];
            }
            None => i += 1,
        }
    }
    out
}

/// A grouped value: its groups' letters and digits, whether it starts with "+", the groups joined, and where each
/// group starts in the joined form (byte offset → group index).
#[derive(Debug, Clone)]
pub(crate) struct Run {
    groups: Vec<String>,
    plus: bool,
    joined: String,
    starts: HashMap<usize, usize>,
}

fn alnum_only(g: &str) -> String {
    g.chars().filter(|&c| py::is_alnum(c)).collect()
}

/// `_one_value` (GATE-527).
fn one_value(run: &str) -> bool {
    let mut groups: Vec<String> = run.split(' ').map(alnum_only).collect();
    if run.starts_with('+') {
        groups.remove(0);
    }
    let sizes: Vec<usize> = groups.iter().map(|g| py::char_len(g)).collect();
    let (Some(&min), Some(&max)) = (sizes.iter().min(), sizes.iter().max()) else {
        return false;
    };
    sizes.len() >= 2 && min >= 2 && max <= 6 && max <= 2 * min
}

fn grouped(text: &str) -> Vec<Run> {
    let mut found = Vec::new();
    for (a, b) in run_spans(text) {
        let m = &text[a..b];
        if !one_value(m) {
            continue;
        }
        let groups: Vec<String> = m.split(' ').map(alnum_only).collect();
        let mut starts = HashMap::new();
        let mut at = 0;
        for (i, g) in groups.iter().enumerate() {
            starts.insert(at, i);
            at += g.len();
        }
        starts.insert(at, groups.len());
        found.push(Run {
            joined: groups.concat(),
            groups,
            plus: m.starts_with('+'),
            starts,
        });
    }
    found
}

fn spans_form(run: &Run, form: &str, first: usize, exact: bool) -> bool {
    let mut from = 0;
    while let Some(i) = run.joined[from..].find(form) {
        let at = from + i;
        if let (Some(&start), Some(&end)) =
            (run.starts.get(&at), run.starts.get(&(at + form.len())))
        {
            if end >= start + 2
                && (if exact {
                    start == first
                } else {
                    start >= first
                })
            {
                return true;
            }
        }
        from = at + run.joined[at..].chars().next().map_or(1, char::len_utf8);
    }
    false
}

fn in_runs(runs: &[Run], groups: &[Vec<String>], forms: &[String]) -> bool {
    let groups: Vec<&Vec<String>> = groups
        .iter()
        .filter(|g| g.len() >= 2 && py::char_len(&g.concat()) >= MIN_LEN)
        .collect();
    let forms: Vec<&String> = forms
        .iter()
        .filter(|f| py::char_len(f) >= MIN_LEN)
        .collect();
    for run in runs {
        for want in &groups {
            let n = want.len();
            if run.groups.len() >= n
                && (0..=run.groups.len() - n).any(|i| run.groups[i..i + n] == want[..])
            {
                return true;
            }
        }
        for form in &forms {
            if spans_form(run, form, 0, false)
                || (run.plus && form.starts_with('0') && spans_form(run, &form[1..], 1, true))
            {
                return true;
            }
        }
    }
    false
}

fn candidate(piece: &str) -> (Vec<Vec<String>>, Vec<String>) {
    let low = py::lower(piece);
    let runs = grouped(&low);
    let mut forms: Vec<String> = vec![alnum_only(&low)];
    forms.extend(runs.iter().map(|r| r.joined.clone()));
    let mut seen = HashSet::new();
    let forms = forms
        .into_iter()
        .filter(|f| f.chars().any(py::is_digit) && seen.insert(f.clone()))
        .collect();
    (runs.into_iter().map(|r| r.groups).collect(), forms)
}

// ---------- value forms ----------

#[doc(hidden)]
pub fn value_forms(text: &str, coarse: Option<&HashSet<String>>) -> HashSet<String> {
    let mut toks: HashSet<String> = match coarse {
        Some(c) => c.clone(),
        None => provenance::tokens(text).into_iter().collect(),
    };
    toks.extend(fine_tokens(text));
    toks.retain(|t| !is_date(t));
    let spans: Vec<(usize, usize)> = run_spans(text)
        .into_iter()
        .filter(|&(a, b)| one_value(&text[a..b]))
        .collect();
    if !spans.is_empty() {
        let mut rest = String::new();
        let mut cut = 0;
        for &(a, b) in &spans {
            rest.push_str(&text[cut..a]);
            cut = b;
        }
        rest.push_str(&text[cut..]);
        let mut members: HashSet<String> = HashSet::new();
        for &(a, b) in &spans {
            members.extend(provenance::tokens(&text[a..b]));
            members.extend(fine_tokens(&text[a..b]));
        }
        let mut kept: HashSet<String> = provenance::tokens(&rest).into_iter().collect();
        kept.extend(fine_tokens(&rest));
        for m in members {
            if !kept.contains(&m) {
                toks.remove(&m);
            }
        }
    }
    let mut forms = toks.clone();
    for value in &toks {
        if py::char_len(value) <= 64 && !value.contains('/') {
            forms.extend(spellings_of(value, true));
        }
    }
    forms
}

/// A read text's value forms, sequence, grouped values and URL parts, each made once (Python's weak caches).
#[derive(Debug, Clone, Default)]
pub struct TextForms {
    toks: OnceLock<HashSet<String>>,
    seq: OnceLock<Vec<String>>,
    runs: OnceLock<Vec<Run>>,
    url_parts: OnceLock<UrlParts>,
}

type UrlParts = (Vec<(String, HashSet<String>)>, HashSet<String>);

fn cache(t: &Text) -> &TextForms {
    t.forms.get_or_init(TextForms::default)
}

fn toks(t: &Text) -> &HashSet<String> {
    cache(t)
        .toks
        .get_or_init(|| value_forms(&t.folded, Some(&t.tokens)))
}

fn sequence(t: &Text) -> &Vec<String> {
    cache(t).seq.get_or_init(|| {
        fine_split(&t.folded)
            .into_iter()
            .filter(|x| !x.is_empty())
            .map(str::to_string)
            .collect()
    })
}

fn runs(t: &Text) -> &Vec<Run> {
    cache(t).runs.get_or_init(|| grouped(&t.folded))
}

/// `with_forms`: `t` with its value forms and URL parts made now.
pub fn with_forms(t: Text) -> Text {
    toks(&t);
    url_parts(&t);
    runs(&t);
    t
}

/// `_is_value`: a form holding a digit or a character other than a letter, a mark and the joiners inside words.
pub fn is_value(form: &str) -> bool {
    form.chars()
        .any(|c| !(py::is_alpha(c) || WORD_JOINERS.contains(c) || py::is_mark(c)))
}

fn phrase(piece: &str, t: &Text, known_to_site: &HashSet<String>) -> bool {
    let low = py::lower(piece);
    let words: Vec<&str> = fine_split(&low)
        .into_iter()
        .filter(|x| !x.is_empty())
        .collect();
    let forms = toks(t);
    if words.len() < 2
        || !words.iter().any(|w| {
            py::char_len(w) >= MIN_LEN
                && is_value(w)
                && forms.contains(*w)
                && !known_to_site.contains(*w)
                && !is_year(w)
        })
    {
        return false;
    }
    let seq = sequence(t);
    let n = words.len();
    (0..seq.len()).any(|i| {
        seq[i] == words[0]
            && seq.len() >= i + n
            && seq[i..i + n].iter().zip(&words).all(|(a, b)| a == b)
    })
}

// ---------- shape-only decodings (N2) ----------

fn b64_shape(piece: &str) -> bool {
    // [A-Za-z0-9+/_-]{8,}={0,2}
    let body = piece.trim_end_matches('=');
    let pads = piece.len() - body.len();
    pads <= 2
        && body.len() >= 8
        && body
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"+/_-".contains(&b))
}

fn hex_shape(piece: &str) -> bool {
    piece.len() >= 8
        && piece.len().is_multiple_of(2)
        && piece.bytes().all(|b| b.is_ascii_hexdigit())
}

/// `base64.b64decode(s, altchars=b"-_", validate=False)` (binascii's non-strict decoder); None where it raises.
#[doc(hidden)]
pub fn b64decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let (mut quad, mut left, mut pads) = (0u8, 0u32, 0u8);
    for b in s.bytes() {
        let b = match b {
            b'-' => b'+',
            b'_' => b'/',
            x => x,
        };
        if b == b'=' {
            if quad >= 2 {
                pads += 1;
                if quad + pads >= 4 {
                    return Some(out);
                }
            }
            continue;
        }
        let v = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => continue,
        } as u32;
        pads = 0;
        match quad {
            0 => {
                quad = 1;
                left = v;
            }
            1 => {
                quad = 2;
                out.push(((left << 2) | (v >> 4)) as u8);
                left = v & 0x0f;
            }
            2 => {
                quad = 3;
                out.push(((left << 4) | (v >> 2)) as u8);
                left = v & 0x03;
            }
            _ => {
                quad = 0;
                out.push(((left << 6) | v) as u8);
                left = 0;
            }
        }
    }
    (quad == 0).then_some(out)
}

#[doc(hidden)]
pub fn decodings(piece: &str) -> HashSet<String> {
    let mut out = HashSet::new();
    let mut add = |bytes: Option<Vec<u8>>| {
        let Some(bytes) = bytes else {
            return;
        };
        if !bytes.is_ascii() {
            return;
        }
        let dec = String::from_utf8(bytes).unwrap_or_default();
        if py::all_printable(&dec) {
            out.insert(py::lower(&dec));
            out.extend(fine_tokens(&dec));
        }
    };
    if b64_shape(piece) {
        let padded = format!("{piece}{}", "=".repeat((4 - piece.len() % 4) % 4));
        add(b64decode(&padded));
    }
    if hex_shape(piece) {
        add(hex::decode(piece).ok());
    }
    out
}

// ---------- links in read text ----------

/// `_URL_IN_TEXT.finditer`: `[a-z][a-z0-9+.-]*://[^\s"'<>\[\]{}|\\^`]+`, as byte spans.
#[doc(hidden)]
pub fn url_spans(text: &str) -> Vec<(usize, usize)> {
    let b = text.as_bytes();
    let body = |c: char| !(py::is_space(c) || "\"'<>[]{}|\\^`".contains(c));
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if !b[i].is_ascii_lowercase() {
            i += 1;
            continue;
        }
        let mut j = i + 1;
        while j < b.len()
            && (b[j].is_ascii_lowercase() || b[j].is_ascii_digit() || b"+.-".contains(&b[j]))
        {
            j += 1;
        }
        if text[j..].starts_with("://") {
            let start_body = j + 3;
            let end = text[start_body..]
                .char_indices()
                .find(|&(_, c)| !body(c))
                .map_or(text.len(), |(k, _)| start_body + k);
            if end > start_body {
                out.push((i, end));
                i = end;
                continue;
            }
        }
        // every start inside this scheme run reaches the same place
        i = j.max(i + 1);
    }
    out
}

/// `_HOST_PATH_IN_TEXT.finditer`: an address written without its scheme (dotted labels, a letters-only last label,
/// then a path), not glued to a word, an @, a dot, a slash, a colon or a dash before it.
#[doc(hidden)]
pub fn host_path_spans(text: &str) -> Vec<(usize, usize)> {
    let b = text.as_bytes();
    let label = |x: u8| x.is_ascii_lowercase() || x.is_ascii_digit() || x == b'-';
    let path = |c: char| !(py::is_space(c) || "\"'<>[]{}|\\^`()".contains(c));
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let before_ok = i == 0 || {
            let prev = text[..i].chars().next_back().unwrap_or(' ');
            !(crate::rx::word(prev) || "@./:-".contains(prev))
        };
        if !before_ok || !label(b[i]) {
            i += text[i..].chars().next().map_or(1, char::len_utf8);
            continue;
        }
        // (?:[a-z0-9-]+\.)+ then [a-z]{2,} then /
        let mut j = i;
        let mut pieces = 0;
        let found = loop {
            let start = j;
            while j < b.len() && label(b[j]) {
                j += 1;
            }
            if j == start {
                break None;
            }
            if j < b.len() && b[j] == b'.' {
                pieces += 1;
                j += 1;
                continue;
            }
            let run = &b[start..j];
            if pieces >= 1
                && run.len() >= 2
                && run.iter().all(u8::is_ascii_lowercase)
                && j < b.len()
                && b[j] == b'/'
            {
                break Some(j + 1);
            }
            break None;
        };
        match found {
            Some(after_slash) => {
                let end = text[after_slash..]
                    .char_indices()
                    .find(|&(_, c)| !path(c))
                    .map_or(text.len(), |(k, _)| after_slash + k);
                out.push((i, end));
                i = end.max(i + 1);
            }
            None => i += 1,
        }
    }
    out
}

fn url_parts(t: &Text) -> &UrlParts {
    cache(t).url_parts.get_or_init(|| {
        let folded = &t.folded;
        let urls_found = url_spans(folded);
        let mut matches: Vec<((usize, usize), String)> = urls_found
            .iter()
            .map(|&(a, b)| ((a, b), folded[a..b].to_string()))
            .collect();
        for (a, b) in host_path_spans(folded) {
            if !urls_found.iter().any(|&(x, y)| x <= a && a < y) {
                matches.push(((a, b), format!("https://{}", &folded[a..b])));
            }
        }
        matches.sort_by_key(|((a, _), _)| *a);
        let mut links = Vec::new();
        let mut rest: Vec<&str> = Vec::new();
        let mut cut = 0;
        for ((a, b), address) in &matches {
            let addr = address.trim_end_matches(|c| TRAIL.contains(c));
            let parsed = urls::parse(addr);
            if parsed.is_some_and(|p| !p.host.is_empty()) && *a >= cut {
                links.push((page_scope(addr), value_forms(&folded[*a..*b], None)));
                rest.push(&folded[cut..*a]);
                cut = *b;
            }
        }
        rest.push(&folded[cut..]);
        (links, value_forms(&rest.join(" "), None))
    })
}

fn host_had_it(seen: &str, dest: &str) -> bool {
    let (seen, dest) = (seen.trim_end_matches('.'), dest.trim_end_matches('.'));
    let bare = seen.strip_prefix("www.").unwrap_or(seen);
    dest == seen
        || dest == bare
        || dest == format!("www.{bare}")
        || dest.ends_with(&format!(".{seen}"))
}

/// `page_scope` (GATE-528): the host and the page's path through its first segment that holds a value.
pub fn page_scope(url: &str) -> String {
    let address = if url.contains("://") {
        url.to_string()
    } else {
        format!("https://{url}")
    };
    let Some(parsed) = urls::parse(&address) else {
        return String::new();
    };
    if parsed.host.is_empty() {
        return String::new();
    }
    let mut kept: Vec<String> = Vec::new();
    for x in parsed.path.split('/').filter(|x| !x.is_empty()) {
        let seg = py::casefold(&pyurl::unquote_plus(x));
        let stop = is_value(&seg);
        kept.push(seg);
        if stop {
            break;
        }
    }
    format!("{}/{}", parsed.host.trim_end_matches('.'), kept.join("/"))
}

fn in_scope(scope: &str, dest: &str, dest_path: &str) -> bool {
    let (host, sep, path) = py::partition(scope, "/");
    if !sep || !host_had_it(host, dest) {
        return false;
    }
    let want: Vec<&str> = path.split('/').filter(|x| !x.is_empty()).collect();
    let have: Vec<String> = dest_path
        .split('/')
        .filter(|x| !x.is_empty())
        .map(|x| py::casefold(&pyurl::unquote_plus(x)))
        .collect();
    have.len() >= want.len() && have[..want.len()].iter().zip(&want).all(|(a, b)| a == b)
}

fn web_detail(o: &str) -> Option<&str> {
    let prefix = format!("{}:web:", origin::EXTERNAL);
    if o.starts_with(&prefix) && o.matches(':').count() >= 2 {
        Some(o.splitn(3, ':').nth(2).unwrap_or(""))
    } else {
        None
    }
}

fn read_from(labels: &HashSet<String>, dest: &str) -> bool {
    let web: Vec<&str> = labels.iter().filter_map(|o| web_detail(o)).collect();
    let prefix = format!("{}:web:", origin::EXTERNAL);
    !web.is_empty()
        && labels
            .iter()
            .all(|o| o == origin::PRIVATE || o.starts_with(&prefix))
        && web
            .iter()
            .all(|h| host_had_it(py::lower(h).trim_end_matches('.'), dest))
}

/// The records the sink reads: (span order, prepared text).
pub type Records = [(i64, Text)];
/// Each record's labels by span order.
pub type Origins = HashMap<i64, HashSet<String>>;
/// Values FERB's conversation stored as read on sites' pages, by the record's place in `private`.
pub type Sites = HashMap<usize, HashSet<String>>;
/// The page each record was read from, by span order.
pub type Pages = HashMap<i64, String>;

fn same_site_only(
    private: &Records,
    dest: &str,
    origins: Option<&Origins>,
    sites: Option<&Sites>,
    dest_path: &str,
    pages: Option<&Pages>,
) -> HashSet<String> {
    let mut inside: HashSet<String> = HashSet::new();
    let mut elsewhere: HashSet<String> = HashSet::new();
    let empty = HashSet::new();
    for (index, (order, t)) in private.iter().enumerate() {
        if let Some(origins) = origins {
            if read_from(origins.get(order).unwrap_or(&empty), dest) {
                let page = pages.and_then(|p| p.get(order)).map_or("", String::as_str);
                if !page.is_empty() && in_scope(&page_scope(page), dest, dest_path) {
                    inside.extend(toks(t).iter().cloned());
                    continue;
                }
            }
        }
        if let Some(read_on) = sites.and_then(|s| s.get(&index)).filter(|r| !r.is_empty()) {
            let target = if read_on.iter().all(|scope| in_scope(scope, dest, dest_path)) {
                &mut inside
            } else {
                &mut elsewhere
            };
            target.extend(toks(t).iter().cloned());
            continue;
        }
        let (links, rest) = url_parts(t);
        for (scope, forms) in links {
            if in_scope(scope, dest, dest_path) {
                inside.extend(forms.iter().cloned());
            } else {
                elsewhere.extend(forms.iter().cloned());
            }
        }
        elsewhere.extend(rest.iter().cloned());
    }
    inside.retain(|x| !elsewhere.contains(x));
    inside
}

fn value_labels(host: &str) -> Vec<String> {
    let labels: Vec<&str> = host.split('.').collect();
    if labels.len() < 3 {
        return Vec::new();
    }
    let reg = if TWO_LEVEL.contains(&labels[labels.len() - 2..].join(".").as_str()) {
        3
    } else {
        2
    };
    let mut out = Vec::new();
    for label in &labels[..labels.len() - reg] {
        out.push(label.to_string());
        if label.starts_with("xn--") {
            if let Ok(u) = idna::ulabel(label) {
                out.push(u);
            }
        }
    }
    out
}

/// `_url_pieces`: the pieces of a URL a value could ride in.
fn url_pieces(url: &str) -> Vec<String> {
    let Some(parts) = urls::parse(url) else {
        return Vec::new();
    };
    let mut pieces: Vec<String> = Vec::new();
    if !parts.username.is_empty() {
        pieces.push(pyurl::unquote_plus(&parts.username));
    }
    if !parts.password.is_empty() {
        pieces.push(pyurl::unquote_plus(&parts.password));
    }
    pieces.extend(value_labels(&parts.host));
    for seg in parts.path.split('/') {
        let seg = pyurl::unquote_plus(seg);
        let main = seg.split(';').next().unwrap_or("").to_string();
        pieces.push(seg.clone());
        pieces.push(main.clone());
        if main.contains('.') {
            pieces.push(
                main.rsplit_once('.')
                    .map_or(String::new(), |(a, _)| a.to_string()),
            );
        }
        for param in seg.split(';').skip(1) {
            pieces.extend(param.split('=').map(pyurl::unquote_plus));
        }
    }
    for pair in parts.query.split('&') {
        pieces.extend(pair.split('=').map(pyurl::unquote_plus));
    }
    let frag = pyurl::unquote_plus(&parts.fragment);
    pieces.push(frag.clone());
    for pair in frag.split('&') {
        for kv in pair.split('=') {
            pieces.push(kv.to_string());
            if kv.contains('/') {
                pieces.extend(kv.split('/').map(str::to_string));
            }
        }
    }
    let again: Vec<String> = pieces.iter().flat_map(|p| decoded_again(p)).collect();
    pieces.extend(again);
    pieces
}

fn decoded_again(piece: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut piece = piece.to_string();
    while piece.contains('%') && out.len() < 2 {
        let next = pyurl::unquote(&piece);
        if next == piece {
            break;
        }
        out.push(next.clone());
        piece = next;
    }
    out
}

fn variants(piece: &str) -> (HashSet<String>, HashSet<String>) {
    let mut shown = HashSet::new();
    let mut decoded = HashSet::new();
    let stripped = py::strip(piece);
    for base in [stripped, stripped.trim_end_matches(|c| TRAIL.contains(c))] {
        if py::char_len(base) >= MIN_LEN {
            let low = py::lower(base);
            if is_value(&low) {
                shown.extend(spellings_of(&low, false));
                shown.insert(low);
            }
            decoded.extend(decodings(base));
        }
    }
    (shown, decoded)
}

fn own_name(
    host: &str,
    private: &Records,
    outside: &Records,
    origins: Option<&Origins>,
) -> HashSet<String> {
    let host = py::lower(host.trim_end_matches('.'));
    let bare = host.strip_prefix("www.").unwrap_or(&host).to_string();
    if bare.is_empty() || !bare.contains('.') {
        return HashSet::new();
    }
    let pages: HashSet<String> = origins
        .into_iter()
        .flat_map(|o| o.values())
        .flatten()
        .filter_map(|o| web_detail(o))
        .map(|d| {
            let d = py::lower(d);
            let d = d.trim_end_matches('.');
            d.strip_prefix("www.").unwrap_or(d).to_string()
        })
        .collect();
    let named = |folded: &str| -> bool {
        // (?<![\w.-])(?:www\.)?BARE(?![\w-]|\.[\w-])
        let mut from = 0;
        while let Some(i) = folded[from..].find(bare.as_str()) {
            let at = from + i;
            let end = at + bare.len();
            let after_ok = match folded[end..].chars().next() {
                None => true,
                Some(c) if crate::rx::word(c) || c == '-' => false,
                Some('.') => !folded[end + 1..]
                    .chars()
                    .next()
                    .is_some_and(|c| crate::rx::word(c) || c == '-'),
                Some(_) => true,
            };
            let before_ok = |p: usize| {
                !folded[..p]
                    .chars()
                    .next_back()
                    .is_some_and(|c| crate::rx::word(c) || c == '.' || c == '-')
            };
            if after_ok
                && (before_ok(at)
                    || (at >= 4 && &folded[at - 4..at] == "www." && before_ok(at - 4)))
            {
                return true;
            }
            from = at + folded[at..].chars().next().map_or(1, char::len_utf8);
        }
        false
    };
    if !pages.contains(&bare) && !private.iter().chain(outside).any(|(_, t)| named(&t.folded)) {
        return HashSet::new();
    }
    let mut forms = HashSet::new();
    for label in host.split('.') {
        if !label.is_empty() && label != "www" {
            forms.insert(label.to_string());
            forms.extend(spellings_of(label, false));
            if label.starts_with("xn--") {
                if let Ok(u) = idna::ulabel(label) {
                    forms.insert(u);
                }
            }
        }
    }
    forms
}

/// A piece carrying the owner's data (GATE-507): the piece as it leaves, the token it matched, and the record that
/// token was first read from (span order).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrivateValue {
    pub piece: String,
    pub matched: String,
    pub first_read: i64,
}

/// `url_carries_private`.
pub fn url_carries_private(
    url: &str,
    private: &Records,
    outside: &Records,
    owner: &Text,
    origins: Option<&Origins>,
    sites: Option<&Sites>,
    pages: Option<&Pages>,
) -> Vec<String> {
    url_private_values(url, private, outside, owner, origins, sites, pages)
        .into_iter()
        .map(|v| v.piece)
        .collect()
}

/// `url_private_values`: the sink rule for navigation (S4, SQ6, N1, L4, GATE-506/528/536/570).
pub fn url_private_values(
    url: &str,
    private: &Records,
    outside: &Records,
    owner: &Text,
    origins: Option<&Origins>,
    sites: Option<&Sites>,
    pages: Option<&Pages>,
) -> Vec<PrivateValue> {
    if private.is_empty() {
        return Vec::new();
    }
    let known_to_site = match urls::parse(url) {
        Some(dest) if !dest.host.is_empty() => {
            let mut k = same_site_only(private, &dest.host, origins, sites, &dest.path, pages);
            k.extend(own_name(&dest.host, private, outside, origins));
            k
        }
        _ => HashSet::new(),
    };
    let hits = private_hits(&url_pieces(url), private, outside, owner, &known_to_site);
    if hits.is_empty() {
        return hits;
    }
    let earliest = hits.iter().map(|h| h.first_read).min().unwrap_or(0);
    let seen_url = [py::casefold(url), py::casefold(&url.replace('\\', "/"))];
    if outside.iter().any(|(order, t)| {
        *order < earliest && seen_url.iter().any(|u| t.folded.contains(u.as_str()))
    }) {
        return Vec::new();
    }
    if private
        .iter()
        .any(|(_, t)| link_appears(url, &t.folded) || broken_link_appears(url, &t.folded))
    {
        return Vec::new();
    }
    hits
}

fn link_appears(url: &str, folded: &str) -> bool {
    let forms = [
        url.to_string(),
        url.replace('\\', "/"),
        pyurl::quote(url, ""),
        pyurl::quote(url, ":/"),
        pyurl::quote_plus(url),
    ];
    forms
        .iter()
        .any(|f| folded.contains(py::casefold(f).as_str()))
}

fn broken_link_appears(url: &str, folded: &str) -> bool {
    let chars = py::casefold(url);
    if chars.is_empty() {
        return false;
    }
    // r"\s*".join(re.escape(c) for c in chars): a match counts only with whitespace inside it
    let pattern: Vec<String> = chars
        .chars()
        .map(|c| regex::escape(&c.to_string()))
        .collect();
    let re = crate::rx::plain(&pattern.join("[{S}]*"));
    let found = re
        .find_iter(folded)
        .any(|m| m.as_str().chars().any(py::is_space));
    found
}

/// `contacts_in`: the email addresses (lowercased) and phone numbers (digits, at least 7) written in `text`.
pub fn contacts_in(text: &str) -> HashSet<String> {
    let mut found: HashSet<String> = email_re().texts(text).into_iter().map(py::lower).collect();
    for m in phone_re().texts(text) {
        let d: String = m.chars().filter(|&c| py::is_digit(c)).collect();
        if py::char_len(&d) >= 7 {
            found.insert(d);
        }
    }
    found
}

/// `own_contact` (GATE-538): whether a typed value is one of the owner's own addresses or numbers.
pub fn own_contact<S: AsRef<str>>(value: &str, own: &[S]) -> bool {
    let mine: HashSet<&str> = own.iter().map(AsRef::as_ref).collect();
    if mine.contains(py::lower(py::strip(value)).as_str()) {
        return true;
    }
    let digits: Vec<char> = value.chars().filter(|&c| py::is_digit(c)).collect();
    let last10 = |v: &[char]| -> Vec<char> { v[v.len().saturating_sub(10)..].to_vec() };
    digits.len() >= 7
        && !value.chars().any(py::is_alpha)
        && mine.iter().filter(|d| py::all_digits(d)).any(|d| {
            let d: Vec<char> = d.chars().collect();
            d == digits || (d.len() >= 10 && digits.len() >= 10 && last10(&d) == last10(&digits))
        })
}

/// `text_carries_private`: the sink rule for a form.
pub fn text_carries_private(
    text: &str,
    private: &Records,
    outside: &Records,
    owner: &Text,
) -> Vec<String> {
    text_private_values(text, private, outside, owner)
        .into_iter()
        .map(|h| h.piece)
        .collect()
}

/// `text_private_values`.
pub fn text_private_values(
    text: &str,
    private: &Records,
    outside: &Records,
    owner: &Text,
) -> Vec<PrivateValue> {
    if private.is_empty() || py::strip(text).is_empty() {
        return Vec::new();
    }
    let mut pieces = vec![py::strip(text).to_string()];
    pieces.extend(
        fine_split(text)
            .into_iter()
            .filter(|t| !t.is_empty())
            .map(str::to_string),
    );
    private_hits(&pieces, private, outside, owner, &HashSet::new())
}

fn private_hits(
    pieces: &[String],
    private: &Records,
    outside: &Records,
    owner: &Text,
    known_to_site: &HashSet<String>,
) -> Vec<PrivateValue> {
    let mut first_private: HashMap<String, i64> = HashMap::new();
    for (order, t) in private {
        for tok in toks(t) {
            let e = first_private.entry(tok.clone()).or_insert(*order);
            if *order < *e {
                *e = *order;
            }
        }
    }
    let owner_toks = toks(owner);
    let outside_toks: Vec<(i64, &HashSet<String>)> =
        outside.iter().map(|(o, t)| (*o, toks(t))).collect();
    let mut hits: Vec<PrivateValue> = Vec::new();
    for piece in pieces {
        let value = py::strip(piece).to_string();
        let (shown, decoded) = variants(piece);
        let mut matched: Vec<String> = shown
            .union(&decoded)
            .filter(|m| first_private.contains_key(*m))
            .filter(|m| {
                let fp = first_private[*m];
                !owner_toks.contains(*m)
                    && !known_to_site.contains(*m)
                    && !outside_toks
                        .iter()
                        .any(|(o, ts)| *o < fp && ts.contains(*m))
                    && !is_year(m)
            })
            .cloned()
            .collect();
        if matched.is_empty() && !value.is_empty() {
            let mut groups: Vec<Vec<String>> = Vec::new();
            let mut forms: Vec<String> = Vec::new();
            for text in std::iter::once(&value).chain(decoded.iter()) {
                let (g, f) = candidate(text);
                groups.extend(g);
                for x in f {
                    if !forms.contains(&x) {
                        forms.push(x);
                    }
                }
            }
            let orders: Vec<i64> = private
                .iter()
                .filter(|(_, t)| in_runs(runs(t), &groups, &forms))
                .map(|(o, _)| *o)
                .collect();
            if let Some(&min) = orders.iter().min() {
                if !in_runs(runs(owner), &groups, &forms)
                    && !outside
                        .iter()
                        .any(|(o, t)| *o < min && in_runs(runs(t), &groups, &forms))
                {
                    first_private.insert(py::lower(&value), min);
                    matched = vec![py::lower(&value)];
                }
            }
        }
        if matched.is_empty() && !value.is_empty() {
            let orders: Vec<i64> = private
                .iter()
                .filter(|(_, t)| phrase(&value, t, known_to_site))
                .map(|(o, _)| *o)
                .collect();
            if let Some(&min) = orders.iter().min() {
                let none = HashSet::new();
                if !phrase(&value, owner, &none)
                    && !outside
                        .iter()
                        .any(|(o, t)| *o < min && phrase(&value, t, &none))
                {
                    first_private.insert(py::lower(&value), min);
                    matched = vec![py::lower(&value)];
                }
            }
        }
        if !matched.is_empty() && !value.is_empty() && !hits.iter().any(|h| h.piece == value) {
            let first = matched
                .iter()
                .min_by(|a, b| {
                    (
                        first_private[*a],
                        std::cmp::Reverse(py::char_len(a)),
                        a.as_str(),
                    )
                        .cmp(&(
                            first_private[*b],
                            std::cmp::Reverse(py::char_len(b)),
                            b.as_str(),
                        ))
                })
                .cloned()
                .unwrap_or_default();
            let order = first_private[&first];
            hits.push(PrivateValue {
                piece: value,
                matched: first,
                first_read: order,
            });
        }
    }
    // two neighbouring groups of a grouped value, each in its own piece (?a=7700&b=900123)
    let seq: Vec<String> = if hits.is_empty() {
        pieces
            .iter()
            .flat_map(|p| {
                fine_split(&py::lower(p))
                    .into_iter()
                    .filter(|w| !w.is_empty())
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .collect()
    } else {
        Vec::new()
    };
    let mut first_at: HashMap<&str, usize> = HashMap::new();
    let mut last_at: HashMap<&str, usize> = HashMap::new();
    for (i, w) in seq.iter().enumerate() {
        first_at.entry(w.as_str()).or_insert(i);
        last_at.insert(w.as_str(), i);
    }
    let mut ordered: Vec<&(i64, Text)> = private.iter().collect();
    ordered.sort_by_key(|(o, _)| *o);
    for (order, t) in ordered {
        for run in runs(t) {
            for pair in run.groups.windows(2) {
                let k = format!("{} {}", pair[0], pair[1]);
                let pair_v = vec![pair.to_vec()];
                if py::char_len(&(pair[0].clone() + &pair[1])) >= MIN_LEN
                    && first_at.contains_key(pair[0].as_str())
                    && last_at.contains_key(pair[1].as_str())
                    && first_at[pair[0].as_str()] < last_at[pair[1].as_str()]
                    && !hits.iter().any(|h| h.matched == k)
                    && !in_runs(runs(owner), &pair_v, &[])
                    && !outside
                        .iter()
                        .any(|(o, x)| o < order && in_runs(runs(x), &pair_v, &[]))
                {
                    hits.push(PrivateValue {
                        piece: k.clone(),
                        matched: k,
                        first_read: *order,
                    });
                    break;
                }
            }
        }
    }
    hits
}

/// `whole_values`: the owner-data-shaped whole values a text holds, lowercased, in order (GATE-519, GATE-529).
pub fn whole_values(text: &str) -> Vec<String> {
    let low = py::lower(text);
    let forms = value_forms(&low, None);
    let groups: HashSet<String> = grouped(&low).into_iter().flat_map(|r| r.groups).collect();
    let mut seen: Vec<String> = Vec::new();
    let candidates = provenance::tokens(&low).into_iter().chain(
        fine_split(&low)
            .into_iter()
            .filter(|x| !x.is_empty())
            .map(|x| x.trim_end_matches(|c| TRAIL.contains(c)).to_string()),
    );
    for tok in candidates {
        if py::char_len(&tok) >= MIN_LEN
            && is_value(&tok)
            && forms.contains(&tok)
            && !groups.contains(tok.trim_start_matches('+'))
            && !seen.contains(&tok)
        {
            seen.push(tok);
        }
    }
    for (a, b) in run_spans(&low) {
        let m = low[a..b].to_string();
        if !seen.contains(&m) {
            seen.push(m);
        }
    }
    seen
}

/// `spellings`: `value` and its other spellings as the sink matches them.
pub fn spellings(value: &str) -> HashSet<String> {
    let mut out = spellings_of(value, true);
    out.insert(value.to_string());
    out
}

/// `account_key`: spaces collapsed, ASCII letters lowercased, one leading "@" dropped.
pub fn account_key(value: Option<&str>) -> String {
    let joined = py::squeeze(value.unwrap_or(""))
        .chars()
        .map(|c| c.to_ascii_lowercase())
        .collect::<String>();
    joined
        .strip_prefix('@')
        .map(str::to_string)
        .unwrap_or(joined)
}

/// `site_key`: a site as the browser names it, a trailing dot and one leading "www." dropped.
pub fn site_key(value: Option<&str>) -> String {
    let text = py::strip(value.unwrap_or(""));
    let parsed = if text.is_empty() {
        None
    } else {
        urls::parse(&format!("https://{text}"))
    };
    let host = match parsed {
        Some(p) if !p.host.is_empty() => p.host,
        _ => py::lower(text),
    };
    let host = host.trim_end_matches('.');
    host.strip_prefix("www.").unwrap_or(host).to_string()
}

/// `known(kind, value)` or `approved(kind, value)`.
pub type Lookup<'a> = &'a dyn Fn(&str, &str) -> bool;

/// The facts `decide` reads besides the call itself.
pub struct Facts<'a> {
    pub read_private: bool,
    pub read_external: bool,
    /// `known(kind, value)`: a destination known for normal use.
    pub known: &'a dyn Fn(&str, &str) -> bool,
    /// `approved(kind, value)`: the sink rule's test (GATE-205); `known` when None.
    pub approved: Option<Lookup<'a>>,
    /// The outside content the tree read (origin labels).
    pub outside_read: &'a [String],
    pub place: Option<&'a str>,
    pub content: Option<&'a str>,
    /// The owner's own email addresses and phone numbers (GATE-538), read only when S4(b) would ask.
    pub own: &'a dyn Fn() -> Vec<String>,
}

/// `decide`: the needs of one call, in the order the owner reads them.
pub fn decide(
    kind: &str,
    declared: &[&str],
    target: &Target,
    arg_sources: &ArgSources,
    f: &Facts<'_>,
) -> Vec<Need> {
    let mut kinds: BTreeSet<&str> = declared.iter().copied().collect();
    kinds.insert(kind);
    let has = |k: &str| kinds.contains(k);
    let approved: &dyn Fn(&str, &str) -> bool = f.approved.unwrap_or(f.known);
    let who = who(target);
    let who_keys: Vec<String> = who.iter().map(|(k, _)| k.clone()).collect();
    let to_site = site_is_destination(kinds.iter().copied(), &who_keys);
    let where_ = f
        .place
        .filter(|p| !p.is_empty())
        .map(str::to_string)
        .or_else(|| target.site.clone().filter(|s| !s.is_empty()))
        .unwrap_or_default();
    let get = |k: &str| provenance::get(arg_sources, k);
    let mut needs: Vec<Need> = Vec::new();
    for k in [Kind::PAY, Kind::ACCOUNT_SECURITY] {
        if !has(k) {
            continue;
        }
        let rule = always_rule(k).unwrap_or_default();
        let amount = shown_amount(target.amount, target.currency.as_deref());
        let amount = if amount.is_empty() && k == Kind::PAY {
            UNREAD_AMOUNT.to_string()
        } else {
            amount
        };
        let values = Values::of(&[
            ("amount", &amount),
            ("site", &host_shown(&where_)),
            ("account", target.account.as_deref().unwrap_or("")),
            (
                "recipients",
                &who.iter()
                    .map(|(_, v)| v.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
        ]);
        let mut recipients: Vec<&str> = who.iter().map(|(_, v)| v.as_str()).collect();
        recipients.sort();
        let mut n = Need::new(
            rule,
            key(&[
                s(rule),
                opt(&target.site),
                opt(&target.account),
                target.amount.map_or(Value::Null, |a| json!(a)),
                json!(recipients),
            ]),
            values,
        );
        n.always = true;
        needs.push(n);
    }
    if has(Kind::DELETE_PERMANENT) && !owner(get("path")) {
        if let Some(path) = target.path.as_deref().filter(|p| !p.is_empty()) {
            needs.push(Need::new(
                "floor:delete_permanent",
                key(&[s("floor:delete_permanent"), s(path)]),
                Values::of(&[("path", path)]),
            ));
        } else {
            let on = target
                .page
                .clone()
                .filter(|p| !p.is_empty())
                .unwrap_or_else(|| where_.clone());
            let item = target.item.as_deref().filter(|i| !i.is_empty());
            let one_run = kind != Kind::DELETE_PERMANENT && item.is_none();
            let path_shown = item.map_or("an item it doesn't name".to_string(), shown_quoted);
            let place = if target.page.as_deref().is_some_and(|p| !p.is_empty()) {
                shown_quoted(&on)
            } else {
                on.clone()
            };
            let mut n = Need::new(
                "floor:delete_permanent",
                key(&[
                    s("floor:delete_permanent"),
                    Value::Null,
                    if on.is_empty() { Value::Null } else { s(&on) },
                    target.item.as_ref().map_or(Value::Null, |i| json!(i)),
                ]),
                Values::of(&[("path", &path_shown), ("place", &place)]),
            );
            n.always = one_run;
            needs.push(n);
        }
    }
    let sends: Vec<&str> = kinds
        .iter()
        .copied()
        .filter(|k| SEND_CLASS.contains(k))
        .collect();
    let calls = has(Kind::CALL) && sends.iter().all(|k| *k == Kind::CALL);
    if !sends.is_empty() {
        let new: Vec<String> = who
            .iter()
            .filter(|(k, v)| {
                let (dk, dv) = destination_of(v);
                !owner(get(k)) && !(f.known)(&dk, &dv)
            })
            .map(|(_, v)| v.clone())
            .collect();
        let verb = if calls { "calls" } else { "sends to" };
        let site_known = target.site.as_deref().is_some_and(|x| !x.is_empty());
        if !new.is_empty() {
            let mut sorted = new.clone();
            sorted.sort();
            let mut n = Need::new(
                "floor:new_recipient",
                key(&[s("floor:new_recipient"), json!(sorted)]),
                Values::of(&[("recipients", &new.join(", ")), ("verb", verb)]),
            );
            n.destinations = new.iter().map(|v| destination_of(v)).collect();
            needs.push(n);
        } else if to_site && (!SEND_CLASS.contains(&kind) || !site_known) {
            let site_named = site_known && owner(get("site"));
            let account = target.account.as_deref().filter(|a| !a.is_empty());
            if !(site_named && (account.is_none() || owner(get("account")))) {
                let mut parts = vec![
                    s("floor:new_recipient"),
                    Value::Null,
                    s(&site_key(target.site.as_deref())),
                ];
                if let Some(a) = account {
                    parts.push(s(&account_key(Some(a))));
                }
                needs.push(Need::new(
                    "floor:new_recipient",
                    key(&parts),
                    Values::of(&[
                        ("recipients", ""),
                        ("verb", verb),
                        ("place", &where_),
                        ("account", account.unwrap_or("")),
                    ]),
                ));
            }
            let site = target.site.as_deref().unwrap_or("");
            if target.page.as_deref().is_some_and(|p| !p.is_empty())
                && site_known
                && f.read_private
                && f.read_external
                && !site_named
                && !approved("host", site)
            {
                let mut n = Need::new(
                    "sink:new_destination",
                    key(&[s("sink:new_destination"), s(site)]),
                    Values::of(&[
                        ("destination", site),
                        (
                            "read",
                            &shown_origins(f.outside_read.iter().map(String::as_str)),
                        ),
                    ]),
                );
                n.destinations = vec![("host".to_string(), site.to_string())];
                needs.push(n);
            }
        } else if to_site
            && f.read_private
            && f.read_external
            && !owner(get("site"))
            && !approved("host", target.site.as_deref().unwrap_or(""))
        {
            let site = target.site.as_deref().unwrap_or("");
            let mut n = Need::new(
                "sink:new_destination",
                key(&[s("sink:new_destination"), opt(&target.site)]),
                Values::of(&[
                    ("destination", site),
                    (
                        "read",
                        &shown_origins(f.outside_read.iter().map(String::as_str)),
                    ),
                ]),
            );
            n.destinations = vec![("host".to_string(), site.to_string())];
            needs.push(n);
        }
        // S4 (c): a send to two or more recipients the owner didn't name
        let unnamed: BTreeSet<&str> = who
            .iter()
            .filter(|(k, _)| k.starts_with("recipient:") && !owner(get(k)))
            .map(|(_, v)| v.as_str())
            .collect();
        if unnamed.len() > 1 {
            let unnamed: Vec<&str> = unnamed.into_iter().collect();
            let content = f.content.unwrap_or("");
            needs.push(Need::new(
                BULK,
                key(&[
                    s(BULK),
                    json!(unnamed),
                    f.content.map_or(Value::Null, |c| json!(c)),
                ]),
                Values::of(&[
                    ("what", if calls { "call" } else { "send" }),
                    ("count", &unnamed.len().to_string()),
                    ("bytes", &counted(content.len() as i128, "byte")),
                    ("sample", &sample(&unnamed)),
                ]),
            ));
        }
    }
    let off = match f.content.filter(|c| !c.is_empty()) {
        Some(c) if kind == Kind::RUN_SHELL => tls::certificate_checks_off(c),
        Some(c) if kind == Kind::APPLESCRIPT => tls::applescript_checks_off(c),
        _ => None,
    };
    if let Some(off) = off {
        let content = f.content.unwrap_or("");
        needs.push(Need::new(
            TLS_OFF,
            key(&[s(TLS_OFF), json!(content)]),
            Values::of(&[
                ("command", &py::squeeze(content)),
                ("hosts", &off.hosts.join(" or ")),
                ("how", &off.how.join(", ")),
            ]),
        ));
    }
    let mut values: Vec<(String, String)> = who.clone();
    if let Some(a) = target.amount {
        if !has(Kind::PAY) {
            values.push(("amount".to_string(), ferb_py::json::python_float(a)));
        }
    }
    let mut mine: Option<Vec<String>> = None;
    for (k, value) in &values {
        let outside: Vec<&String> = get(k).iter().filter(|x| *x != "unknown").collect();
        let outside_owned: Vec<String> = outside.iter().map(|x| (*x).clone()).collect();
        if outside.is_empty()
            || owner(&outside_owned)
            || outside.iter().any(|x| *x == origin::PRIVATE)
        {
            continue;
        }
        if k != "amount" {
            let own = mine.get_or_insert_with(|| (f.own)());
            if own_contact(value, own) {
                continue;
            }
        }
        let dest = if k == "amount" {
            None
        } else {
            Some(destination_of(value))
        };
        if let Some((dk, dv)) = &dest {
            if (f.known)(dk, dv) {
                continue;
            }
        }
        let what = if k == "amount" {
            "amount"
        } else if !sends.is_empty() {
            "recipient"
        } else {
            "typed"
        };
        let shown_value = if k == "amount" {
            shown_amount(target.amount, None)
        } else {
            value.clone()
        };
        let mut n = Need::new(
            "sink:outside_value",
            key(&[s("sink:outside_value"), s(k), s(value)]),
            Values::of(&[
                ("what", what),
                ("value", &shown_value),
                ("origin", &shown_origins(outside.iter().map(|x| x.as_str()))),
                ("places", &shown_places(outside.iter().map(|x| x.as_str()))),
                ("site", &host_shown(&where_)),
            ]),
        );
        n.destinations = dest.into_iter().collect();
        needs.push(n);
    }
    for n in &mut needs {
        n.reaches = Some(target.clone());
    }
    needs
}

/// `bulk_delete` (S4 c): a permanent delete of a folder the owner didn't name, counted (`facts`: fs.usage).
pub fn bulk_delete(path: &str, facts: &serde_json::Map<String, Value>) -> Result<Need, Missing> {
    let listed: Vec<String> = facts
        .get("paths")
        .and_then(Value::as_array)
        .map(|a| a.iter().map(ferb_py::value::py_str).collect())
        .unwrap_or_default();
    let root = listed
        .first()
        .filter(|p| p.ends_with('/'))
        .cloned()
        .unwrap_or_default();
    let inside: Vec<String> = listed
        .iter()
        .skip(1)
        .map(|p| {
            if !root.is_empty() && p.starts_with(&root) {
                p[root.len()..].to_string()
            } else {
                p.clone()
            }
        })
        .collect();
    let counted_path = match facts.get("path") {
        Some(v) if ferb_py::value::truthy(v) => ferb_py::value::py_str(v),
        _ => path.to_string(),
    };
    let int = |k: &str| -> Result<i128, Missing> {
        let v = facts.get(k).ok_or_else(|| Missing(k.to_string()))?;
        match v {
            Value::Number(n) => n
                .as_i64()
                .map(i128::from)
                .or_else(|| n.as_f64().map(|f| f.trunc() as i128)),
            Value::Bool(b) => Some(i128::from(*b)),
            Value::String(t) => py::strip(t).parse().ok(),
            _ => None,
        }
        .ok_or_else(|| Missing(k.to_string()))
    };
    let mut n = Need::new(
        BULK,
        key(&[s(BULK), Value::Object(facts.clone())]),
        Values::of(&[
            ("what", "delete"),
            ("path", path),
            (
                "counted",
                if counted_path == path {
                    ""
                } else {
                    &counted_path
                },
            ),
            ("files", &counted(int("files")?, "file")),
            ("folders", &counted(int("directories")?, "folder")),
            ("bytes", &counted(int("bytes")?, "byte")),
            ("sample", &sample(&inside)),
            ("covers", "floor:delete_permanent"),
        ]),
    );
    n.reaches = Some(Target {
        path: Some(path.to_string()),
        ..Target::default()
    });
    Ok(n)
}

pub use crate::wording::*;
