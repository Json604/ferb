//! policy/destinations.py, the parts that need no store: a destination's kind by its shape, the normalized form it
//! is compared in, the destinations a text names, and the stable form a key or destination is stored in (rule 14).
//! The stores themselves (`KnownDestinations`, `Confirmations`) are in `stores` (Mac-side).

use sha2::{Digest, Sha256};

use crate::{provenance, py};

pub const KINDS: &[&str] = &["email", "phone", "host", "name"];
pub const HOW: &[&str] = &["named", "answered", "confirmed"];
/// How a destination the owner approved became known (GATE-205).
pub const APPROVED: &[&str] = &["answered", "confirmed"];

/// The redactor as the stores see it (a seam onto ferb-sanitize).
pub trait Redact {
    /// `redact({"value": text})["value"]`.
    fn redact(&self, text: &str) -> String;
    /// `redact.hit(text)` when the redactor has one: Some(true) when it finds a specific secret; None when it has
    /// no `hit` (then any change it would make counts).
    fn hit(&self, text: &str) -> Option<bool>;
}

impl Redact for ferb_sanitize::Redactor {
    fn redact(&self, text: &str) -> String {
        let out = self.value(&serde_json::json!({ "value": text }));
        out.get("value")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    fn hit(&self, text: &str) -> Option<bool> {
        // a text the redactor couldn't read counts as a hit: never stored, never matched
        Some(!matches!(
            ferb_sanitize::Redactor::hit(self, text),
            Ok(None)
        ))
    }
}

/// `stable`: the form a key or destination is stored and compared in: None when it holds a specific secret, the
/// text itself when the redactor leaves it alone, else "sha256:" and its digest.
pub fn stable(redact: &dyn Redact, text: &str) -> Option<String> {
    let hit = redact.hit(text);
    if hit == Some(true) {
        return None;
    }
    if redact.redact(text) == text {
        return Some(text.to_string());
    }
    hit?;
    Some(format!(
        "sha256:{}",
        hex::encode(Sha256::digest(text.as_bytes()))
    ))
}

/// `destination_of`: (kind, value) of a recipient or contact by its shape.
pub fn destination_of(value: &str) -> (String, String) {
    match provenance::destination_shape(value) {
        Some("url") => ("host".into(), provenance::host_of(py::strip(value))),
        Some(shape @ ("email" | "phone")) => (shape.into(), value.to_string()),
        _ => ("name".into(), value.to_string()),
    }
}

/// `KnownDestinations.normalize`.
pub fn normalize(kind: &str, value: &str) -> String {
    let text = py::strip(value);
    match kind {
        "phone" => text.chars().filter(|&c| py::is_digit(c)).collect(),
        "host" => py::lower(text).trim_end_matches('.').to_string(),
        "email" => py::lower(text),
        _ => py::split_ws(&py::casefold(text)).join(" "),
    }
}

/// `KnownDestinations.from_words`: the destinations a text names by shape, in order. Names never come from text.
pub fn from_words(words: &str) -> Vec<(String, String)> {
    let mut found: Vec<(String, String)> = Vec::new();
    for tok in provenance::raw_tokens(words) {
        if provenance::destination_shape(tok) == Some("email") {
            found.push(("email".into(), py::lower(tok)));
        } else {
            let host = provenance::host_of(tok);
            if !host.is_empty() {
                found.push(("host".into(), host));
            }
        }
    }
    found.extend(
        provenance::phone_runs(words)
            .into_iter()
            .map(|d| ("phone".to_string(), d)),
    );
    let mut seen = std::collections::HashSet::new();
    found.retain(|f| seen.insert(f.clone()));
    found
}

/// `KnownDestinations._stored`: the stored form of a destination, or None (never stored, never matched).
pub fn stored(redact: &dyn Redact, kind: &str, value: &str) -> Option<String> {
    let norm = normalize(kind, value);
    if KINDS.contains(&kind) && !norm.is_empty() {
        stable(redact, &norm)
    } else {
        None
    }
}
