//! FERB's redaction and origin marking, ported from the Python daemon (`ferb/sanitize`, `ferb/core/origin.py`).
//!
//! AGENTS.md rule 7: secrets, OTPs and card details never reach a model provider, a trace, a log or the database;
//! code redacts at capture. This crate is that code. It never chooses an action and never judges intent.
//!
//! The port is checked against the Python build (ENG-3): `rust/fixtures/sanitize/make.py` runs the Python code on
//! invented inputs and `tests/parity.rs` asserts the same output for every case. Offsets here are bytes where
//! Python's are code points; outputs are the same text.
//!
//! Not ported (see rust/JOURNAL.md): `images.py` (Vision OCR, Mac-only).
//!
//! Where Rust can fail and Python cannot: fancy-regex gives up on a text that needs more than a million saved
//! backtrack positions, and the system may give no random bytes. Neither panics. Every `Redactor` and
//! `RedactionStream` call then hides its whole input (`UNREADABLE`) or returns `Err(Unread)`; the functions of
//! `formats`, `labels`, `known` and `untrusted` read a pattern that could not be run as "no match", so a caller
//! of those wraps the call in `checked`, which returns `Err(Unread)` instead.

#[rustfmt::skip]
mod generated;
mod pyurl;
mod pyvalue;
mod rx;
mod unicode;

pub mod formats;
pub mod known;
pub mod labels;
pub mod origin;
pub mod redact;
pub mod stream;
pub mod terminal;
pub mod untrusted;

pub use formats::{
    first_format, format_hits, hidden_note, named_kinds, payment_card, secret_param, Hit,
};
pub use known::{
    escaped_forms, holds_sealed, id_bodies, known_spans, sealed_spans, KnownValues, Registry,
    REDACTED,
};
pub use labels::{
    code_words, label_class, password_field, secret_key, secret_words, LabelClass, CODE_REACH,
};
pub use pyvalue::{JsonError, Py, PyStr};
pub use redact::{mask, Field, Redactor, EXCLUDED, NODE_TEXT, SAVED_SECRET};
pub use rx::{checked, Unread, UNREADABLE};
pub use stream::{RedactionStream, MAX_PENDING, OVERSIZED};

/// Compile every pattern now (a daemon's start, a test) instead of at first use. `Err`: the names of the
/// patterns that do not compile (each would read every text as unreadable).
pub fn compile_patterns() -> Result<(), Vec<&'static str>> {
    let failed = rx::compile_all();
    if failed.is_empty() {
        Ok(())
    } else {
        Err(failed)
    }
}

/// Python's answers the port rests on, for the parity tests (`tests/parity.rs`): not part of the crate's use.
#[doc(hidden)]
pub mod parity {
    pub use crate::labels::{entropy_at_least_3, entropy_term};
    pub use crate::pyvalue::{repr, to_str};
    pub use crate::rx::{fail_from, pattern_names, run_pattern, set_backtrack_limit};
    // the helpers that moved to ferb-py (`ferb_py::text`, `ferb_py::url::hostname`, journal 44) are used from there
    pub use crate::unicode::{casefold, is_alpha, is_decimal, word_char};
}
