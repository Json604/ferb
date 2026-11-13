//! What Python's standard library would have produced: the few pieces of Python's own behavior the ports lean
//! on, in one place, so every crate writes and reads the bytes the Python build does (decision: ENG-3).
//!
//! - [`json`]: `json.dumps` byte for byte (separators, `ensure_ascii`, `sort_keys`, `indent`, `repr(float)`),
//!   `json.loads`' error words, `bytes.decode`'s error words.
//! - [`time`]: `datetime.fromisoformat`, `isoformat`, the move to UTC.
//! - [`value`]: `dict.get`, truthiness, `str()`, `==`, `str.split()` / `str.strip()` whitespace, slicing by
//!   character, on decoded JSON values and text.
//! - [`unicode`]: Python's Unicode tables where a port needs them (`\d`).
//!
//! No FERB rule lives here, and nothing that touches a file, a socket or a platform API.

pub mod json;
pub mod time;
pub mod value;

mod unicode_nd;

/// Python's Unicode tables (15.0.0), generated from Python by `rust/fixtures/store/make_tables.py`.
pub mod unicode {
    pub use crate::unicode_nd::is_decimal;
}
