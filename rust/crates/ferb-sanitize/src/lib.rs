//! FERB's redaction and origin marking, ported from the Python daemon (`ferb/sanitize`, `ferb/core/origin.py`).
//!
//! AGENTS.md rule 7: secrets, OTPs and card details never reach a model provider, a trace, a log or the database;
//! code redacts at capture. This crate is that code. It never chooses an action and never judges intent.
//!
//! The port is checked against the Python build (ENG-3): `rust/fixtures/sanitize/make.py` runs the Python code on
//! invented inputs and `tests/parity.rs` asserts the same output for every case. Offsets here are bytes where
//! Python's are code points; outputs are the same text.
//!
//! Not ported (see rust/JOURNAL.md): `images.py` (Vision OCR, Mac-only), Python's `Redacted` mark, `bytes`
//! values in a structure, and integers beyond 64 bits.

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
pub use redact::{mask, Field, Redactor, EXCLUDED, NODE_TEXT, SAVED_SECRET};
pub use stream::{RedactionStream, MAX_PENDING, OVERSIZED};

/// Compile every pattern now (a daemon's start, a test) instead of at first use.
pub fn compile_patterns() {
    rx::compile_all();
}

/// Python's answers the port rests on, for the parity tests (`tests/parity.rs`): not part of the crate's use.
#[doc(hidden)]
pub mod parity {
    pub use crate::pyurl::hostname;
    pub use crate::pyvalue::{repr, to_str};
    pub use crate::unicode::{
        casefold, is_alnum, is_alpha, is_decimal, is_digit, is_space, lower, word_char,
    };
}
