//! The versioned extension wire schema and its authentication: a port of
//! `ferb/act/browser/ext_protocol.py` (the docstring there tells each protocol's history). The other
//! end is the unchanged extension (`extensions/chromium/protocol.js`, manifest 0.19.10, protocol 21).
//!
//! Extension messages carry `install_id`, `type`, `body_json`, a strictly increasing `seq` and an
//! HMAC-SHA256 over `install_id|seq|type|body_json`. The daemon's `welcome` and `cmd` carry
//! `{type, seq, body, mac}` with the HMAC over `nonce|seq|type|body` (protocol 18, decision:
//! HAND-222). Both HMACs use the pairing key. The signed body is checked as the very text that
//! arrived, never re-serialized.

use hmac::{Hmac, Mac};
use serde_json::{json, Map, Value};
use sha2::Sha256;

use crate::lax::{self, Invalid, Lax};

pub const PROTOCOL_VERSION: i64 = 21;
pub const MIN_EXTENSION_VERSION: &str = "0.19.0";
/// From the key in `extensions/chromium/manifest.json`.
pub const FERB_EXTENSION_ID: &str = "jaclnphefmmgmpofnabffahdkbjjompm";
/// Same cap as the daemon socket: one line, in bytes; a body_json, in code points.
pub const MAX_MESSAGE: usize = 1024 * 1024;

/// HAND-428: `pdf` needs this extension version; the connection minimum stays.
pub const PDF_EXTENSION_VERSION: &str = "0.19.3";
/// HAND-496: `html`.
pub const HTML_EXTENSION_VERSION: &str = "0.19.8";
/// HAND-497, GATE-579: `owner_view`.
pub const OWNER_VIEW_EXTENSION_VERSION: &str = "0.19.8";
/// HAND-500: an upload without file access hands the bytes through the page.
pub const UPLOAD_BYTES_EXTENSION_VERSION: &str = "0.19.9";
/// HAND-505..: paste, sideways and screen-sized scrolls, click modifiers.
pub const INPUT_EXTENSION_VERSION: &str = "0.19.10";
/// Raw bytes per `upload_part` (protocol.js UPLOAD_PART): one native message.
pub const UPLOAD_PART: usize = 256 * 1024;

/// What the extension sends (`ExtType`).
pub const EXT_TYPES: [&str; 6] = ["hello", "pair", "result", "error", "tab_event", "event"];

/// Every command the extension knows (protocol.js `OPS`). Python's `Op` type leaves out `html` and
/// `owner_view`, which `_TabDriver.html` and `owner_view` send all the same; this set is the
/// extension's.
pub const OPS: [&str; 42] = [
    "open",
    "navigate",
    "close",
    "list_tabs",
    "snapshot",
    "act",
    "wait",
    "screenshot",
    "screenshot_part",
    "record_frame",
    "wake_close",
    "frame_origin",
    "fill",
    "read_source",
    "window_secrets",
    "scroll",
    "find",
    "upload",
    "result_part",
    "command_part",
    "dialog",
    "ping",
    "point",
    "pointer",
    "pointer_cancel",
    "release_tab",
    "download_part",
    "disarm",
    "open_plain",
    "popup_ready",
    "open_seen",
    "media",
    "rest",
    "show_tab",
    "adopt_tab",
    "mute_tab",
    "popup_refused",
    "pdf",
    "pdf_part",
    "html",
    "owner_view",
    "upload_part",
];

/// Commands that never change the page (protocol.js READ_OPS): one that fails or times out is a
/// known failure (decision: HAND-224).
pub const READS: [&str; 18] = [
    "find",
    "frame_origin",
    "html",
    "list_tabs",
    "owner_view",
    "pdf",
    "pdf_part",
    "ping",
    "point",
    "read_source",
    "record_frame",
    "result_part",
    "screenshot",
    "screenshot_part",
    "snapshot",
    "upload_part",
    "wait",
    "window_secrets",
];

pub fn is_op(op: &str) -> bool {
    OPS.contains(&op)
}

pub fn is_read(op: &str) -> bool {
    READS.contains(&op)
}

/// One part of a version as Python's `int(digits)` reads it: its decimal digits without leading
/// zeros (compared by length, then text), so any length compares as Python's ints do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Part(String);

impl Part {
    fn of(digits: &str) -> Part {
        let trimmed = digits.trim_start_matches('0');
        Part(if trimmed.is_empty() {
            "0".into()
        } else {
            trimmed.into()
        })
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Ord for Part {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0
            .len()
            .cmp(&other.0.len())
            .then_with(|| self.0.cmp(&other.0))
    }
}

impl PartialOrd for Part {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// `version_tuple`: each dot-separated part's `str.isdigit()` characters as one integer (0 when
/// it has none). `Err` where Python's `int()` raises: a digit that isn't decimal ("²").
pub fn version_tuple(v: &str) -> Result<Vec<Part>, Invalid> {
    v.split('.')
        .map(|part| {
            let mut digits = String::new();
            for c in part.chars().filter(|c| lax::is_digit(*c)) {
                let d = lax::decimal_value(c).ok_or(Invalid)?;
                digits.push(char::from_digit(d, 10).ok_or(Invalid)?);
            }
            Ok(Part::of(&digits))
        })
        .collect()
}

/// `version_ok(v, minimum)`; `Err` as `version_tuple`.
pub fn version_ok(v: &str, minimum: &str) -> Result<bool, Invalid> {
    Ok(version_tuple(v)? >= version_tuple(minimum)?)
}

/// The kinds of extension message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExtType {
    Hello,
    Pair,
    Result,
    Error,
    TabEvent,
    Event,
}

impl ExtType {
    pub fn parse(s: &str) -> Option<ExtType> {
        Some(match s {
            "hello" => ExtType::Hello,
            "pair" => ExtType::Pair,
            "result" => ExtType::Result,
            "error" => ExtType::Error,
            "tab_event" => ExtType::TabEvent,
            "event" => ExtType::Event,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ExtType::Hello => "hello",
            ExtType::Pair => "pair",
            ExtType::Result => "result",
            ExtType::Error => "error",
            ExtType::TabEvent => "tab_event",
            ExtType::Event => "event",
        }
    }
}

/// One message from the extension (`ExtMessage`).
#[derive(Clone, PartialEq)]
pub struct ExtMessage {
    pub install_id: String,
    pub mtype: ExtType,
    /// The body as the text that arrived: the HMAC covers exactly this.
    pub body_json: String,
    pub seq: Option<i64>,
    pub mac: Option<String>,
}

impl std::fmt::Debug for ExtMessage {
    /// Never the body: it can hold page text.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExtMessage")
            .field("install_id", &self.install_id)
            .field("type", &self.mtype)
            .field("body_chars", &self.body_json.chars().count())
            .field("seq", &self.seq)
            .finish()
    }
}

impl ExtMessage {
    /// `body()`: the body object; `Err` where Python raises `ValueError` (not JSON, or not an
    /// object).
    pub fn body(&self) -> Lax<Map<String, Value>> {
        match serde_json::from_str::<Value>(&self.body_json) {
            Ok(Value::Object(m)) => Ok(m),
            _ => Err(Invalid),
        }
    }
}

/// `parse(raw)`: an outer frame as `json.loads` made it, validated as `ExtMessage`; `None` when it
/// isn't one.
pub fn parse(raw: &Value) -> Option<ExtMessage> {
    let m = raw.as_object()?;
    let install_id = lax::required(m, "install_id", lax::string).ok()?;
    let n = install_id.chars().count();
    if !(8..=64).contains(&n) {
        return None;
    }
    let mtype = lax::required(m, "type", |v| lax::literal(v, &EXT_TYPES)).ok()?;
    let body_json = lax::required(m, "body_json", |v| lax::string_max(v, MAX_MESSAGE)).ok()?;
    let seq = lax::optional(m, "seq", lax::int).ok()?;
    let mac = lax::optional(m, "mac", lax::string).ok()?;
    Some(ExtMessage {
        install_id,
        mtype: ExtType::parse(&mtype)?,
        body_json,
        seq,
        mac,
    })
}

fn opt<T: Into<Value>>(v: Option<T>) -> Value {
    v.map_or(Value::Null, Into::into)
}

fn maps(v: &[Map<String, Value>]) -> Value {
    Value::Array(v.iter().cloned().map(Value::Object).collect())
}

/// A hello (`Hello`).
#[derive(Clone, Debug, PartialEq)]
pub struct Hello {
    pub version: String,
    pub protocol: i64,
    /// What the extension believes (informational; the binding wins).
    pub browser: String,
    pub paired: bool,
    /// Resources it created, each tagged with its command id.
    pub owned: Vec<Map<String, Value>>,
    /// Recent command ids with started / done / failed / unknown (reconciliation).
    pub seen: Vec<Map<String, Value>>,
    /// Tabs holding downloads for FERB (D35).
    pub armed: Vec<i64>,
    /// Protocol 18: this connection's signing nonce.
    pub nonce: String,
    /// Protocol 18: the tabs FERB holds in this browser session.
    pub tabs: Vec<Map<String, Value>>,
    /// 0.19.9 (HAND-500): the owner allowed FERB access to file URLs.
    pub file_access: Option<bool>,
}

impl Hello {
    pub fn validate(b: &Map<String, Value>) -> Lax<Hello> {
        Ok(Hello {
            version: lax::required(b, "version", lax::string)?,
            protocol: lax::required(b, "protocol", lax::int)?,
            browser: lax::field(b, "browser", || Ok(String::new()), lax::string)?,
            paired: lax::field(b, "paired", || Ok(false), lax::boolean)?,
            owned: lax::field(b, "owned", || Ok(vec![]), |v| lax::list(v, lax::dict))?,
            seen: lax::field(b, "seen", || Ok(vec![]), |v| lax::list(v, lax::dict))?,
            armed: lax::field(b, "armed", || Ok(vec![]), |v| lax::list(v, lax::int))?,
            nonce: lax::field(b, "nonce", || Ok(String::new()), |v| lax::string_max(v, 64))?,
            tabs: lax::field(b, "tabs", || Ok(vec![]), |v| lax::list(v, lax::dict))?,
            file_access: lax::optional(b, "file_access", lax::boolean)?,
        })
    }

    /// `model_dump()`.
    pub fn dump(&self) -> Value {
        json!({"version": self.version, "protocol": self.protocol, "browser": self.browser, "paired": self.paired,
               "owned": maps(&self.owned), "seen": maps(&self.seen), "armed": self.armed, "nonce": self.nonce,
               "tabs": maps(&self.tabs), "file_access": opt(self.file_access)})
    }
}

/// A command's result (`Result`).
#[derive(Clone, Debug, PartialEq)]
pub struct CmdResult {
    pub id: String,
    pub gen: i64,
    /// ok | error | stale | duplicate
    pub status: String,
    pub value: Value,
    pub error: Option<String>,
    /// done | failed | unknown
    pub outcome: Option<String>,
    /// 0.19.6: the step a failed command was in.
    pub step: Option<String>,
    /// 0.19.6: how long each step took (ms).
    pub steps: Option<Map<String, Value>>,
    /// 0.19.7: the dialog that ended or refused it ({type}).
    pub dialog: Option<Map<String, Value>>,
}

impl CmdResult {
    pub fn validate(b: &Map<String, Value>) -> Lax<CmdResult> {
        Ok(CmdResult {
            id: lax::required(b, "id", lax::string)?,
            gen: lax::required(b, "gen", lax::int)?,
            status: lax::required(b, "status", |v| {
                lax::literal(v, &["ok", "error", "stale", "duplicate"])
            })?,
            value: b.get("value").cloned().unwrap_or(Value::Null),
            error: lax::optional(b, "error", lax::string)?,
            outcome: lax::optional(b, "outcome", |v| {
                lax::literal(v, &["done", "failed", "unknown"])
            })?,
            step: lax::optional(b, "step", |v| lax::string_max(v, 64))?,
            steps: lax::optional(b, "steps", lax::dict)?,
            dialog: lax::optional(b, "dialog", lax::dict)?,
        })
    }

    pub fn dump(&self) -> Value {
        json!({"id": self.id, "gen": self.gen, "status": self.status, "value": self.value, "error": opt(self.error.clone()),
               "outcome": opt(self.outcome.clone()), "step": opt(self.step.clone()),
               "steps": opt(self.steps.clone().map(Value::Object)), "dialog": opt(self.dialog.clone().map(Value::Object))})
    }
}

/// A browser tab event (`TabEvent`).
#[derive(Clone, Debug, PartialEq)]
pub struct TabEvent {
    /// created | updated | activated | removed | sound
    pub event: String,
    pub tab_id: i64,
    pub url: Option<String>,
    pub title: Option<String>,
    pub active: Option<bool>,
    pub incognito: Option<bool>,
    pub audible: Option<bool>,
    pub muted: Option<bool>,
    pub ferb: Option<bool>,
    pub changed: Vec<String>,
    pub muted_reason: Option<String>,
    pub same_document: Option<bool>,
    pub closed_by_page: Option<bool>,
}

impl TabEvent {
    pub fn validate(b: &Map<String, Value>) -> Lax<TabEvent> {
        Ok(TabEvent {
            event: lax::required(b, "event", |v| {
                lax::literal(v, &["created", "updated", "activated", "removed", "sound"])
            })?,
            tab_id: lax::required(b, "tab_id", lax::int)?,
            url: lax::optional(b, "url", lax::string)?,
            title: lax::optional(b, "title", lax::string)?,
            active: lax::optional(b, "active", lax::boolean)?,
            incognito: lax::optional(b, "incognito", lax::boolean)?,
            audible: lax::optional(b, "audible", lax::boolean)?,
            muted: lax::optional(b, "muted", lax::boolean)?,
            ferb: lax::optional(b, "ferb", lax::boolean)?,
            changed: lax::field(
                b,
                "changed",
                || Ok(vec![]),
                |v| lax::list(v, |x| lax::literal(x, &["audible", "muted"])),
            )?,
            muted_reason: lax::optional(b, "muted_reason", lax::string)?,
            same_document: lax::optional(b, "same_document", lax::boolean)?,
            closed_by_page: lax::optional(b, "closed_by_page", lax::boolean)?,
        })
    }

    pub fn dump(&self) -> Value {
        json!({"event": self.event, "tab_id": self.tab_id, "url": opt(self.url.clone()), "title": opt(self.title.clone()),
               "active": opt(self.active), "incognito": opt(self.incognito), "audible": opt(self.audible),
               "muted": opt(self.muted), "ferb": opt(self.ferb), "changed": self.changed,
               "muted_reason": opt(self.muted_reason.clone()), "same_document": opt(self.same_document),
               "closed_by_page": opt(self.closed_by_page)})
    }
}

/// An observed dialog, file chooser, debugger detachment, download, new tab or taken tab (`Event`).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Event {
    pub event: String,
    pub tab_id: i64,
    pub r#type: Option<String>,
    pub message: Option<String>,
    pub handled: Option<String>,
    pub reason: Option<String>,
    pub frame_id: Option<String>,
    pub session_id: Option<String>,
    pub backend_node_id: Option<i64>,
    pub mode: Option<String>,
    pub native_picker_shown: Option<bool>,
    pub transferred: Option<bool>,
    pub direct_upload_available: Option<bool>,
    pub detail: Option<String>,
    pub download_id: Option<String>,
    pub name: Option<String>,
    pub mime: Option<String>,
    pub size: Option<i64>,
    pub source_url: Option<String>,
    pub source: Option<String>,
    pub blank_tab: Option<bool>,
    pub uncaptured: Option<bool>,
    pub count: Option<i64>,
    pub url: Option<String>,
    pub opener_tab_id: Option<i64>,
    pub window_id: Option<i64>,
    pub background: Option<bool>,
    pub open_id: Option<String>,
}

impl Event {
    pub fn validate(b: &Map<String, Value>) -> Lax<Event> {
        let s = |name: &str| lax::optional(b, name, lax::string);
        let smax = |name: &str, max: usize| lax::optional(b, name, |v| lax::string_max(v, max));
        let flag = |name: &str| lax::optional(b, name, lax::boolean);
        let int = |name: &str| lax::optional(b, name, lax::int);
        let at_least = |name: &str, min: i64| {
            lax::optional(b, name, |v| {
                lax::int(v).and_then(|n| if n >= min { Ok(n) } else { Err(Invalid) })
            })
        };
        Ok(Event {
            event: lax::required(b, "event", |v| {
                lax::literal(
                    v,
                    &[
                        "dialog",
                        "detached",
                        "file_chooser",
                        "download",
                        "open",
                        "popup",
                        "taken",
                    ],
                )
            })?,
            tab_id: lax::required(b, "tab_id", lax::int)?,
            r#type: s("type")?,
            message: s("message")?,
            handled: s("handled")?,
            reason: s("reason")?,
            frame_id: s("frame_id")?,
            session_id: s("session_id")?,
            backend_node_id: int("backend_node_id")?,
            mode: s("mode")?,
            native_picker_shown: flag("native_picker_shown")?,
            transferred: flag("transferred")?,
            direct_upload_available: flag("direct_upload_available")?,
            detail: s("detail")?,
            download_id: smax("download_id", 64)?,
            name: smax("name", 4096)?,
            mime: smax("mime", 256)?,
            size: at_least("size", 0)?,
            source_url: smax("source_url", 8192)?,
            source: s("source")?,
            blank_tab: flag("blank_tab")?,
            uncaptured: flag("uncaptured")?,
            count: at_least("count", 1)?,
            url: smax("url", 8192)?,
            opener_tab_id: int("opener_tab_id")?,
            window_id: int("window_id")?,
            background: flag("background")?,
            open_id: smax("open_id", 64)?,
        })
    }

    /// `model_dump()`, every field in order.
    pub fn dump(&self) -> Value {
        let mut m = Map::new();
        for (k, v) in self.fields() {
            m.insert(k.into(), v);
        }
        Value::Object(m)
    }

    /// `model_dump(exclude_none=True)` (the `browser.file_chooser` event's data).
    pub fn dump_set(&self) -> Map<String, Value> {
        self.fields()
            .into_iter()
            .filter(|(_, v)| !v.is_null())
            .map(|(k, v)| (k.to_string(), v))
            .collect()
    }

    fn fields(&self) -> Vec<(&'static str, Value)> {
        vec![
            ("event", self.event.clone().into()),
            ("tab_id", self.tab_id.into()),
            ("type", opt(self.r#type.clone())),
            ("message", opt(self.message.clone())),
            ("handled", opt(self.handled.clone())),
            ("reason", opt(self.reason.clone())),
            ("frame_id", opt(self.frame_id.clone())),
            ("session_id", opt(self.session_id.clone())),
            ("backend_node_id", opt(self.backend_node_id)),
            ("mode", opt(self.mode.clone())),
            ("native_picker_shown", opt(self.native_picker_shown)),
            ("transferred", opt(self.transferred)),
            ("direct_upload_available", opt(self.direct_upload_available)),
            ("detail", opt(self.detail.clone())),
            ("download_id", opt(self.download_id.clone())),
            ("name", opt(self.name.clone())),
            ("mime", opt(self.mime.clone())),
            ("size", opt(self.size)),
            ("source_url", opt(self.source_url.clone())),
            ("source", opt(self.source.clone())),
            ("blank_tab", opt(self.blank_tab)),
            ("uncaptured", opt(self.uncaptured)),
            ("count", opt(self.count)),
            ("url", opt(self.url.clone())),
            ("opener_tab_id", opt(self.opener_tab_id)),
            ("window_id", opt(self.window_id)),
            ("background", opt(self.background)),
            ("open_id", opt(self.open_id.clone())),
        ]
    }
}

type HmacSha256 = Hmac<Sha256>;

fn hmac_b64(key: &[u8], text: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC takes a key of any length");
    mac.update(text);
    ferb_identity::py::b64encode(&mac.finalize().into_bytes())
}

/// What the extension's MAC covers: `install_id|seq|type|body_json` (UTF-8).
pub fn mac_text(install_id: &str, seq: i64, mtype: &str, body_json: &str) -> Vec<u8> {
    format!("{install_id}|{seq}|{mtype}|{body_json}").into_bytes()
}

/// `sign`: the extension's MAC of a message, base64.
pub fn sign(key: &[u8], install_id: &str, seq: i64, mtype: &str, body_json: &str) -> String {
    hmac_b64(key, &mac_text(install_id, seq, mtype, body_json))
}

/// `hmac.compare_digest` on two strings: equal or not, in time independent of where they differ.
/// Python raises `TypeError` when either holds a character beyond ASCII.
pub fn compare_digest(a: &str, b: &str) -> Result<bool, NonAscii> {
    if !a.is_ascii() || !b.is_ascii() {
        return Err(NonAscii);
    }
    let mut diff = u8::from(a.len() != b.len());
    for (x, y) in a.bytes().zip(b.bytes()) {
        diff |= x ^ y;
    }
    Ok(diff == 0)
}

/// `hmac.compare_digest` refused a text beyond ASCII (Python's `TypeError`, which ends the
/// connection: `Hub.serve` catches no `TypeError`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NonAscii;

/// `authentic`: the message carries a seq and a MAC, and the MAC is the key's.
pub fn authentic(key: &[u8], m: &ExtMessage) -> Result<bool, NonAscii> {
    match (m.seq, &m.mac) {
        (Some(seq), Some(mac)) => compare_digest(
            &sign(key, &m.install_id, seq, m.mtype.as_str(), &m.body_json),
            mac,
        ),
        _ => Ok(false),
    }
}

/// `command`: the body of a `cmd` envelope.
pub fn command(cid: &str, gen: i64, op: &str, args: Value, timeout_ms: i64) -> Value {
    json!({"id": cid, "gen": gen, "op": op, "args": args, "timeout_ms": timeout_ms})
}

/// `daemon_mac`: the daemon's MAC of a welcome or cmd over `nonce|seq|type|body`.
pub fn daemon_mac(key: &[u8], nonce: &str, seq: i64, mtype: &str, body: &str) -> String {
    hmac_b64(key, format!("{nonce}|{seq}|{mtype}|{body}").as_bytes())
}

/// A signed daemon message (`daemon_envelope`): the body as JSON text (relayed unchanged as a
/// string), the counter, and the MAC.
#[derive(Clone, PartialEq)]
pub struct Envelope {
    pub mtype: &'static str,
    pub seq: i64,
    pub body: String,
    pub mac: String,
}

impl std::fmt::Debug for Envelope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Envelope")
            .field("type", &self.mtype)
            .field("seq", &self.seq)
            .finish()
    }
}

impl Envelope {
    /// `json.dumps(envelope)`: the line as it is written.
    pub fn wire(&self) -> String {
        ferb_py::json::dumps(&self.value())
    }

    pub fn value(&self) -> Value {
        json!({"type": self.mtype, "seq": self.seq, "body": self.body, "mac": self.mac})
    }
}

/// `daemon_envelope(key, nonce, seq, type, body)`; `mtype` is "welcome" or "cmd".
pub fn daemon_envelope(
    key: &[u8],
    nonce: &str,
    seq: i64,
    mtype: &'static str,
    body: &Value,
) -> Envelope {
    let text = ferb_py::json::dumps(body);
    let mac = daemon_mac(key, nonce, seq, mtype, &text);
    Envelope {
        mtype,
        seq,
        body: text,
        mac,
    }
}
