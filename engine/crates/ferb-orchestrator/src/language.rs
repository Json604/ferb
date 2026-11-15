//! ferb/orchestrator/language.py: P27 audit hints only, never a gate.

use std::sync::OnceLock;

use regex::Regex;

fn internal() -> &'static Regex {
    static RX: OnceLock<Regex> = OnceLock::new();
    RX.get_or_init(|| {
        Regex::new(
            r"(?i)\b(?:harness|orchestrator|context envelope|scope grant|tool schema|gateway|worker runtime)\b",
        )
        .expect("valid pattern")
    })
}

/// language.py:7 `internal_terms`: the internal words a text uses, lowercased, sorted, once each.
pub fn internal_terms(text: &str) -> Vec<String> {
    let mut found: Vec<String> = internal()
        .find_iter(text)
        .map(|m| crate::py::lower(m.as_str()))
        .collect();
    found.sort();
    found.dedup();
    found
}
