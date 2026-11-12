//! Port of ferb/sanitize/redact.py: secret redaction before model, trace, log and database boundaries (AGENTS.md
//! rule 7: secrets, OTPs and card details never reach a model provider, a trace, a log or the database). It never
//! uses secret content to decide an action; a gate asks only for a specific hit (`Redactor::hit`).
//!
//! Three sources of spans, all found on the text as given and masked once: known values (known.rs), formats a
//! value shows by its own shape (formats.rs) and labelled classes (labels.rs). The result is redacted again until
//! nothing changes, so `text(text(x)) == text(x)`; a mask is never split.
//!
//! Not ported: Python's `Redacted` mark (a str subclass that lets a later sink skip a string already redacted
//! under the same known values). Every call here redacts; see the crate's notes for what that changes.
//!
//! decision: GATE-572 (owner), GATE-573, GATE-501, GATE-502, GATE-206, GATE-211, GATE-212.

use std::collections::BTreeSet;
use std::sync::{Arc, RwLock};

use serde_json::{Map, Value};

use crate::formats::{first_format, format_hits, holds_value, payment_card, urls};
use crate::known::{known_spans, KnownValues, Registry, REDACTED};
use crate::labels::{
    self, code_alone, code_words, content_lines, hidden_answer, label_class, labelled_code,
    labelled_secret, labelled_spans, labelled_value, line_breaks, line_code_spans, list_line,
    password_field, LabelClass, FIELD_METADATA, INPUT_TYPES, INTERNAL_ID_KEYS,
};
use crate::pyurl;
use crate::pyvalue::{is_int, to_str, truthy};
use crate::rx::Span;
use crate::stream::RedactionStream;
use crate::unicode as u;

pub const EXCLUDED: &str = "[excluded]";
/// redact.py:33: `Redactor::kinds`' name for a known value: a key saved in FERB.
pub const SAVED_SECRET: &str = "saved secret";
const INPUT_TEXT_FIELDS: &[&str] = &["text", "unmodifiedText", "key", "code"];
/// redact.py:43: where a page or accessibility node holds its visible text, in order of preference.
pub const NODE_TEXT: &[&str] = &["value", "text", "name", "label", "title", "content"];
const SIBLING_ID_EXTRA: &[&str] = &["id", "intent_id", "episode", "evidence"];
const FIXED_POINT: usize = 8;

fn part_of(kind: &str) -> Option<&'static str> {
    // redact.py:36 `_PART_OF`
    match kind {
        "card expiry" | "CVV" => Some("payment card"),
        "Twilio auth token" => Some("Twilio key"),
        _ => None,
    }
}

/// `_MASKS.finditer(s)` (redact.py:38): every "[redacted]" and "[excluded]".
fn mask_spans(s: &str) -> Vec<Span> {
    let (mut out, mut pos) = (Vec::new(), 0);
    while let Some(i) = s[pos..].find('[') {
        let at = pos + i;
        if s[at..].starts_with(REDACTED) || s[at..].starts_with(EXCLUDED) {
            out.push((at, at + REDACTED.len())); // both masks are 10 bytes
            pos = at + REDACTED.len();
        } else {
            pos = at + 1;
        }
    }
    out
}

/// redact.py:59: `s` with each span replaced; overlapping or adjacent spans as one (EXCLUDED wins), a mask already
/// there never split. Offsets are bytes and must lie on character boundaries.
pub fn mask(s: &str, spans: &[(usize, usize, &str)]) -> String {
    let mut found: Vec<(usize, usize, &str)> =
        spans.iter().copied().filter(|&(a, b, _)| b > a).collect();
    if found.is_empty() {
        return s.to_string();
    }
    found.sort();
    let masks = mask_spans(s);
    let mut out = String::with_capacity(s.len());
    let mut kept = 0;
    let mut cur: Option<(usize, usize, &str)> = None;
    for (mut a, mut b, rep) in found {
        if !masks.is_empty() {
            // a span reaching into a mask takes the whole mask
            let i = masks.partition_point(|&(start, _)| start <= a);
            if i > 0 && masks[i - 1].1 > a {
                a = masks[i - 1].0;
            }
            let j = masks.partition_point(|&(start, _)| start < b);
            if j > 0 && masks[j - 1].1 > b {
                b = masks[j - 1].1;
            }
        }
        if let Some((ca, cb, crep)) = cur {
            if a <= cb {
                // overlapping or adjacent: one mask
                cur = Some((
                    ca,
                    cb.max(b),
                    if crep == EXCLUDED || rep == EXCLUDED {
                        EXCLUDED
                    } else {
                        crep
                    },
                ));
                continue;
            }
            out.push_str(&s[kept..ca]);
            out.push_str(crep);
            kept = cb;
        }
        cur = Some((a, b, rep));
    }
    if let Some((ca, cb, crep)) = cur {
        out.push_str(&s[kept..ca]);
        out.push_str(crep);
        out.push_str(&s[cb..]);
    }
    out
}

/// redact.py:88: the stretches of `s` a later pass reads: each line holding a mask, with the lines that aren't
/// blank just above and below it (and up a list to its label), merged.
fn around_masks(s: &str) -> Vec<Span> {
    let found = mask_spans(s);
    if found.is_empty() {
        return Vec::new();
    }
    let mut starts = vec![0];
    starts.extend(line_breaks(s).into_iter().map(|(_, e)| e));
    let lines: Vec<Span> = starts
        .iter()
        .enumerate()
        .map(|(i, &a)| (a, starts.get(i + 1).copied().unwrap_or(s.len())))
        .collect();
    let content = |i: usize| !u::strip(&s[lines[i].0..lines[i].1]).is_empty();
    let mut out: Vec<Span> = Vec::new();
    for (pos, _) in found {
        let i = (starts.partition_point(|&x| x <= pos) - 1) as isize;
        let (mut up, mut down) = (i - 1, i + 1);
        while up > 0 && !content(up as usize) {
            up -= 1;
        }
        while up > 0
            && list_line(
                s[lines[up as usize].0..lines[up as usize].1].trim_end_matches(['\r', '\n']),
            )
        {
            up -= 1;
        }
        let n = lines.len() as isize;
        while down < n - 1 && !content(down as usize) {
            down += 1;
        }
        let (a, b) = (
            lines[up.max(0) as usize].0,
            lines[down.min(n - 1) as usize].1,
        );
        match out.last_mut() {
            Some(last) if a <= last.1 => last.1 = last.1.max(b),
            _ => out.push((a, b)),
        }
    }
    out
}

/// A field's facts at a capture boundary (redact.py:243, `Redactor.field`'s keyword arguments).
#[derive(Clone, Copy, Debug, Default)]
pub struct Field<'a> {
    pub role: &'a str,
    pub label: &'a str,
    pub secure_input: bool,
    pub concealed: bool,
    pub metadata: &'a [&'a str],
    /// The field's input type or role for the password rule; None: its role.
    pub kind: Option<&'a str>,
}

struct Exclusions {
    domains: BTreeSet<String>,
    apps: BTreeSet<String>,
}

/// redact.py:117.
pub struct Redactor {
    registry: Registry,
    exclusions: RwLock<Arc<Exclusions>>,
}

impl std::fmt::Debug for Redactor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Redactor({} known values)", self.registry.lasting()) // how many, never which
    }
}

impl Default for Redactor {
    fn default() -> Redactor {
        Redactor::new(&[], &[])
    }
}

impl Redactor {
    pub fn new(exclude_domains: &[&str], exclude_apps: &[&str]) -> Redactor {
        let r = Redactor {
            registry: Registry::new(),
            exclusions: RwLock::new(Arc::new(Exclusions {
                domains: BTreeSet::new(),
                apps: BTreeSet::new(),
            })),
        };
        r.configure(exclude_domains, exclude_apps);
        r
    }

    // ---------- configuration and known values ----------

    /// redact.py:128: change the exclusions in place (the journal, spans and gateway hold this instance).
    pub fn configure(&self, exclude_domains: &[&str], exclude_apps: &[&str]) {
        let new = Exclusions {
            domains: exclude_domains
                .iter()
                .map(|d| u::lower(d).trim_start_matches('.').to_string())
                .collect(),
            apps: exclude_apps.iter().map(|a| u::lower(a)).collect(),
        };
        *self
            .exclusions
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Arc::new(new);
    }

    fn exclusions(&self) -> Arc<Exclusions> {
        Arc::clone(
            &self
                .exclusions
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }

    /// redact.py:134: one of FERB's own credentials (an API key, a proxy token, a sign-in code): hidden by its
    /// value wherever it appears. Nothing else is known by value. decision: GATE-572.
    pub fn add_secret_value(&self, value: &str) {
        self.registry.add(value);
    }

    /// redact.py:139: the API keys read at start; how many were added. decision: GATE-573.
    pub fn preload<'a, I: IntoIterator<Item = &'a str>>(&self, values: I) -> usize {
        let before = self.registry.lasting();
        for value in values {
            self.registry.add(value);
        }
        self.registry.lasting() - before
    }

    pub fn forget_secret_value(&self, value: &str) {
        self.registry.forget(value);
    }

    /// redact.py:149: every known value and its escaped forms.
    pub fn secret_values(&self) -> Arc<KnownValues> {
        self.registry.values()
    }

    /// redact.py:153: a host on the owner's exclusion list (a domain covers its subdomains).
    pub fn excluded_host(&self, host: &str) -> bool {
        let host = u::lower(host);
        self.exclusions()
            .domains
            .iter()
            .any(|d| host == *d || host.ends_with(&format!(".{d}")))
    }

    /// redact.py:158: a separate decoder and buffer for each process; raw partial secrets never leave it.
    pub fn stream(&self) -> RedactionStream<&Redactor> {
        RedactionStream::new(self)
    }

    fn excluded_app(&self, s: &str) -> bool {
        let ex = self.exclusions();
        !ex.apps.is_empty() && ex.apps.contains(&u::lower(s))
    }

    // ---------- text ----------

    /// redact.py:177: every span to mask in `s`, found on `s` as given: (start, end, mask), byte offsets.
    pub fn spans(&self, s: &str, keyed: bool) -> Vec<(usize, usize, &'static str)> {
        let found = urls(s);
        let mut out: Vec<(usize, usize, &'static str)> =
            known_spans(s, &self.registry.values(), keyed)
                .into_iter()
                .map(|(a, b)| (a, b, REDACTED))
                .collect();
        out.extend(
            format_hits(s, Some(&found))
                .into_iter()
                .map(|h| (h.start, h.end, REDACTED)),
        );
        out.extend(
            labelled_spans(s, Some(&found))
                .into_iter()
                .map(|(a, b)| (a, b, REDACTED)),
        );
        if !self.exclusions().domains.is_empty() {
            for &(a, b) in &found {
                // a URL urlsplit refuses is skipped (redact.py:187)
                let Ok(host) = pyurl::hostname(&s[a..b]) else {
                    continue;
                };
                if self.excluded_host(&host.unwrap_or_default()) {
                    out.push((a, b, EXCLUDED));
                }
            }
        }
        out
    }

    /// redact.py:193: `s` masked, then masked again until nothing changes; each later pass reads only the lines a
    /// mask can change (`around_masks`).
    fn fixed(&self, s: &str, keyed: bool) -> String {
        let mut s = s.to_string();
        let mut out = mask(&s, &self.spans(&s, keyed));
        for _ in 0..FIXED_POINT {
            if out == s {
                break;
            }
            s = out;
            let mut found: Vec<(usize, usize, &str)> = Vec::new();
            for (a, b) in around_masks(&s) {
                found.extend(
                    self.spans(&s[a..b], keyed)
                        .into_iter()
                        .map(|(x, y, rep)| (a + x, a + y, rep)),
                );
            }
            out = mask(&s, &found);
        }
        out
    }

    /// redact.py:206: free text: an exception message, a page, a command's output.
    pub fn text(&self, s: &str) -> String {
        if self.excluded_app(s) {
            return EXCLUDED.to_string();
        }
        self.fixed(s, false)
    }

    /// redact.py:215: a value under one of FERB's own id keys: any generated id body keeps its form. An excluded
    /// app's name stays excluded.
    pub fn ident(&self, s: &str) -> String {
        if self.excluded_app(s) {
            return EXCLUDED.to_string();
        }
        self.fixed(s, true)
    }

    /// redact.py:222: a specific secret in `s`, by kind, for a gate: a known value or a vendor format; None
    /// otherwise. Never "redaction would change it".
    pub fn hit(&self, s: &str) -> Option<&'static str> {
        if !known_spans(s, &self.registry.values(), false).is_empty() {
            return Some("a saved secret");
        }
        first_format(s)
    }

    /// redact.py:229: each kind of specific secret in `s`, once, in the order they stand; names only, never a
    /// value. decision: GATE-501, GATE-502.
    pub fn kinds(&self, s: &str) -> Vec<&'static str> {
        let mut taken: Vec<(usize, usize, &'static str)> =
            known_spans(s, &self.registry.values(), false)
                .into_iter()
                .map(|(a, b)| (a, b, SAVED_SECRET))
                .collect();
        for h in format_hits(s, None) {
            if holds_value(s, &h) && !taken.iter().any(|&(a, b, _)| a <= h.start && h.end <= b) {
                taken.push((h.start, h.end, h.kind));
            }
        }
        taken.sort();
        let found: Vec<&'static str> = taken.into_iter().map(|(_, _, kind)| kind).collect();
        let mut out: Vec<&'static str> = Vec::new();
        for &k in &found {
            let whole = part_of(k).filter(|w| found.contains(w));
            if matches!(k, "card expiry" | "CVV") && whole.is_none() {
                continue; // a card's part alone names no kind
            }
            let name = whole.unwrap_or(k);
            if !out.contains(&name) {
                out.push(name);
            }
        }
        out
    }

    /// redact.py:243: capture boundary for AX/DOM/clipboard fields; metadata never grants action permission.
    /// decision: GATE-206.
    pub fn field(&self, text: &str, f: &Field<'_>) -> String {
        let kind = f.kind.unwrap_or(f.role);
        let role = u::casefold(f.role);
        let mut meta: Vec<&str> = vec![f.label];
        meta.extend_from_slice(f.metadata);
        if f.secure_input
            || f.concealed
            || role.contains("secure")
            || role == "password"
            || (!kind.is_empty() && password_field(kind, &meta))
            || labelled_code(f.label, text)
            || labelled_secret(kind, f.label, text)
        {
            return if text.is_empty() {
                String::new()
            } else {
                REDACTED.to_string()
            };
        }
        self.text(text)
    }

    /// redact.py:254: the owner's typed answer to a question (no field): to a sign-in password, passcode or PIN
    /// question it is hidden whole; to a question with a code word its 4-8 digit codes are masked. `reply`: a chat
    /// message while the question is open.
    pub fn answer(&self, text: &str, question: &str, reply: bool) -> String {
        if u::strip(text).is_empty() {
            return text.to_string();
        }
        if hidden_answer(question, text, reply) {
            return REDACTED.to_string();
        }
        let out = self.text(text);
        if code_words(question).is_empty() {
            out
        } else {
            labels::mask_code_values(&out)
        }
    }

    // ---------- structures ----------

    /// redact.py:267 `Redactor.__call__`: a JSON value redacted, its shape kept.
    pub fn value(&self, obj: &Value) -> Value {
        self.walk(obj)
    }

    fn walk(&self, obj: &Value) -> Value {
        match obj {
            Value::String(s) => Value::String(self.text(s)),
            Value::Object(d) => Value::Object(self.dict(d)),
            Value::Array(items) => Value::Array(self.list(items)),
            Value::Number(n) if is_int(obj) => {
                let digits = n.to_string();
                let digits = digits.trim_start_matches('-');
                if payment_card(digits)
                    || !known_spans(digits, &self.registry.values(), false).is_empty()
                {
                    Value::String(REDACTED.to_string())
                } else {
                    obj.clone()
                }
            }
            _ => obj.clone(),
        }
    }

    /// redact.py:350: `value` under `k` redacted; two keys that redact alike both stay ("#2" on the later one).
    fn put(&self, out: &mut Map<String, Value>, k: &str, value: Value) -> String {
        let mut key = self.text(k);
        if out.contains_key(&key) {
            let mut n = 2;
            while out.contains_key(&format!("{key}#{n}")) {
                n += 1;
            }
            key = format!("{key}#{n}");
        }
        out.insert(key.clone(), value);
        key
    }

    /// redact.py:361: a value a secret key names: every text and number in it masked, its shape kept.
    fn hidden(&self, value: &Value) -> Value {
        match value {
            Value::Object(d) => {
                let mut out = Map::new();
                for (k, v) in d {
                    let v = self.hidden(v);
                    self.put(&mut out, k, v);
                }
                Value::Object(out)
            }
            Value::Array(items) => Value::Array(items.iter().map(|v| self.hidden(v)).collect()),
            Value::Null | Value::Bool(_) => value.clone(),
            Value::String(s) if s.is_empty() => value.clone(),
            _ => Value::String(REDACTED.to_string()),
        }
    }

    fn dict(&self, d: &Map<String, Value>) -> Map<String, Value> {
        let or_empty = |key: &str| d.get(key).filter(|v| truthy(v));
        let get_str = |v: Option<&Value>| v.map(to_str).unwrap_or_default();
        let kind_type = get_str(or_empty("type"));
        let kind_type_lower = u::lower(&kind_type);
        // an input type before a role
        let kind = match or_empty("input_type") {
            Some(v) => to_str(v),
            None if INPUT_TYPES.contains(&kind_type_lower.as_str()) && !kind_type.is_empty() => {
                kind_type.clone()
            }
            None => get_str(or_empty("role")),
        };
        let autocomplete = get_str(d.get("autocomplete"));
        let metadata: Vec<&str> = FIELD_METADATA
            .iter()
            .filter_map(|k| d.get(*k).and_then(Value::as_str))
            .collect();
        let secret_field = kind_type_lower == "password"
            || u::lower(&autocomplete).contains("one-time-code")
            || labels::PASSWORD_AUTOCOMPLETE
                .search(&autocomplete, 0)
                .is_some()
            || labels::CARD_AUTOCOMPLETE.search(&autocomplete, 0).is_some()
            || password_field(&kind, &metadata);
        let input_event = d
            .get("method")
            .and_then(Value::as_str)
            .is_some_and(|m| m.starts_with("Input."));
        let mut out = Map::new();
        let mut order: Vec<(&str, String)> = Vec::new();
        for (k, v) in d {
            let ks = k.as_str();
            let cls = label_class(ks);
            let value = if cls == Some(LabelClass::Secret) {
                self.hidden(v)
            } else if cls.is_some()
                && (v.is_string() || is_int(v))
                && labelled_value(cls, &to_str(v))
            {
                Value::String(REDACTED.to_string())
            } else if INTERNAL_ID_KEYS.contains(&ks) && (v.is_string() || v.is_array()) {
                match v {
                    Value::String(s) => Value::String(self.ident(s)),
                    Value::Array(items) => Value::Array(
                        items
                            .iter()
                            .map(|item| match item {
                                Value::String(s) => Value::String(self.ident(s)),
                                other => self.walk(other),
                            })
                            .collect(),
                    ),
                    _ => unreachable!("a string or an array"),
                }
            } else if secret_field && ks == "value" && !v.is_null() && v.as_str() != Some("") {
                Value::String(REDACTED.to_string())
            } else if let (true, "params", Value::Object(params)) = (input_event, ks, v) {
                let mut inner = Map::new();
                for (pk, pv) in params {
                    let pv = if INPUT_TEXT_FIELDS.contains(&pk.as_str()) {
                        Value::String(REDACTED.to_string())
                    } else {
                        self.walk(pv)
                    };
                    self.put(&mut inner, pk, pv);
                }
                Value::Object(inner)
            } else if let (true, Value::String(text)) =
                (matches!(ks, "value" | "text" | "content"), v)
            {
                let role = get_str(d.get("role").or_else(|| d.get("type")));
                let label = get_str(
                    d.get("label")
                        .or_else(|| d.get("name"))
                        .or_else(|| d.get("autocomplete")),
                );
                let f = Field {
                    role: &role,
                    label: &label,
                    kind: Some(&kind),
                    ..Field::default()
                };
                Value::String(self.field(text, &f))
            } else {
                self.walk(v)
            };
            let key = self.put(&mut out, k, value);
            order.push((ks, key));
        }
        if d.len() > 1 {
            self.dict_siblings(d, &mut out, order);
        }
        out
    }

    /// redact.py:336: a code its previous value labels (a body's "PIN" then "4821"): an object's neighbouring
    /// scalar values, in key order, read as sibling lines; FERB's own ids are neither read nor masked.
    fn dict_siblings(
        &self,
        d: &Map<String, Value>,
        out: &mut Map<String, Value>,
        mut order: Vec<(&str, String)>,
    ) {
        order.sort_by(|a, b| a.1.cmp(&b.1)); // by the redacted key, as text; stable
        let id_key = |ks: &str| INTERNAL_ID_KEYS.contains(&ks) || SIBLING_ID_EXTRA.contains(&ks);
        let lines: Vec<String> = order
            .iter()
            .map(|(ks, key)| match out.get(key) {
                Some(v) if (v.is_string() || is_int(v)) && !id_key(ks) => to_str(v),
                _ => String::new(),
            })
            .collect();
        if !lines
            .iter()
            .any(|t| !u::longer_than(t, 63) && t.bytes().any(|b| b.is_ascii_digit()))
        {
            return;
        }
        let raw: Vec<&str> = order
            .iter()
            .map(|(ks, _)| d.get(*ks).and_then(Value::as_str).unwrap_or(""))
            .collect();
        for i in self.sibling_hits(&lines, &raw) {
            if let Some(v) = out.get_mut(&order[i].1) {
                if v.is_string() || is_int(v) || v.is_boolean() {
                    *v = Value::String(REDACTED.to_string()); // Python's isinstance(…, (str, int)) takes a bool too
                }
            }
        }
    }

    fn list(&self, raw: &[Value]) -> Vec<Value> {
        let mut out: Vec<Value> = raw.iter().map(|v| self.walk(v)).collect();
        if raw.len() < 2 {
            return out;
        }
        // redact.py:377: a list with no text and no object relates nothing (there, every text it holds is already
        // marked redacted: none is)
        if !raw.iter().any(|v| v.is_string() || v.is_object()) {
            return out;
        }
        self.siblings(raw, &mut out);
        out
    }

    /// redact.py:381: secrets that only neighbouring items show: a code split from its label across sibling nodes,
    /// read as lines; and a private key whose lines are separate items.
    fn siblings(&self, raw: &[Value], out: &mut [Value]) {
        let mut keys: Vec<Option<&'static str>> = Vec::new();
        let mut lines: Vec<String> = Vec::new();
        for item in out.iter() {
            let key = NODE_TEXT.iter().copied().find(|k| {
                item.as_object()
                    .and_then(|o| o.get(*k))
                    .and_then(Value::as_str)
                    .is_some_and(|t| !u::strip(t).is_empty())
            });
            keys.push(key);
            lines.push(match item {
                Value::String(s) => s.clone(),
                v if is_int(v) => to_str(v),
                v => key
                    .and_then(|k| v.get(k))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
            });
        }
        let raw_lines: Vec<&str> = raw.iter().map(|v| v.as_str().unwrap_or("")).collect();
        for i in self.sibling_hits(&lines, &raw_lines) {
            let item = &mut out[i];
            if item.is_string() || is_int(item) || item.is_boolean() {
                *item = Value::String(REDACTED.to_string());
            } else if let (Some(key), Value::Object(o)) = (keys[i], item) {
                o.insert(key.to_string(), Value::String(REDACTED.to_string()));
            }
        }
    }

    /// redact.py:401: which of neighbouring texts read as lines hold a secret only together.
    fn sibling_hits(&self, lines: &[String], raw: &[&str]) -> BTreeSet<usize> {
        let mut hits = BTreeSet::new();
        if lines
            .iter()
            .any(|line| !line.is_empty() && code_alone(line))
        {
            let (mut joined, mut starts) = (String::new(), Vec::new());
            for line in lines {
                starts.push(joined.len());
                joined.push_str(&u::squeeze(line));
                joined.push('\n');
            }
            for (a, _) in line_code_spans(&joined, &content_lines(&joined)) {
                hits.insert(starts.partition_point(|&x| x <= a) - 1);
            }
        }
        if raw.iter().any(|line| line.contains("-----BEGIN")) {
            let (mut joined, mut starts) = (String::new(), Vec::new());
            for line in raw {
                starts.push(joined.len());
                joined.push_str(line);
                joined.push('\n');
            }
            for (a, b, _) in self.spans(&joined, false) {
                let first = starts.partition_point(|&x| x <= a) - 1;
                let last = starts.partition_point(|&x| x < b) - 1; // bisect_right(starts, b - 1) - 1
                if first != last {
                    hits.extend(first..=last);
                }
            }
        }
        hits
    }
}
