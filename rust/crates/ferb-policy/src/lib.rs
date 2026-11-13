//! FERB's policy, ported from the Python daemon (`ferb/policy`): when a consequential call must ask the owner first
//! (money, account security, permanent deletion, new recipients, terminal typing, protected folders, certificate
//! checks turned off, the sink rule), what is protected, and the owner-facing words of each question.
//!
//! AGENTS.md rules 3-5: code enforces where/what boundaries from structural facts; it never judges intent, and no
//! keyword list decides intent. This crate adds no fact the Python code didn't read.
//!
//! Checked against the Python build (ENG-3): `rust/fixtures/policy/make.py` runs `ferb/policy` on invented data and
//! `tests/parity.rs` replays every case. Python's own text handling (str methods, `unicodedata`, `re` classes,
//! `urllib.parse`, `ipaddress`, `idna`, `shlex`, `Decimal`) is reproduced from tables the Python build wrote
//! (`tables.rs`, made by `rust/fixtures/policy/gen_tables.py`).
//!
//! Seams (what other parts of FERB answer): the Mac's apps (`confirm::Apps`), the redactor
//! (`destinations::Redact`, implemented for `ferb_sanitize::Redactor`), whether a destination is known
//! (`confirm::Facts`), the owner's home folder, the clock (`ferb_core::interfaces::Clock`, and `payments::Payments`'s
//! `now`), ids (`IdSource`) and the file-identity syscalls (`ppath::Files`; POSIX in `ppath::PosixFiles`, the
//! name-only open with `F_GETPATH` in its macOS module). The stores are Mac-side (`stores`, on ferb-store's `Db`).

#[rustfmt::skip]
#[allow(clippy::all)]
mod tables;

pub mod carried;
pub mod confirm;
pub mod destinations;
pub mod idna;
pub mod norm;
pub mod payments;
pub mod ppath;
pub mod protection;
pub mod provenance;
pub mod py;
pub mod pyurl;
pub mod rules;
pub(crate) mod rx;
pub mod shlex;
pub mod small;
pub mod standing;
#[cfg(target_os = "macos")]
pub mod stores;
pub mod terminal;
pub mod tls;
pub mod urls;
mod wording;

/// Python's answers the port rests on, for the parity tests (`tests/parity.rs`): not part of the crate's use.
#[doc(hidden)]
pub mod parity {
    pub use crate::carried::joined;
    pub use crate::confirm::{
        b64decode, decodings, grouped_digits, host_path_spans, run_spans, spellings_of, url_spans,
        value_forms,
    };
}
