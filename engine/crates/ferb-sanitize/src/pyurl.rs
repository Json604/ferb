//! `urllib.parse.urlsplit(url).hostname` as Python 3.12 computes it (Lib/urllib/parse.py:470-523, 165-217,
//! 421-465; Lib/ipaddress.py for a bracketed host), including every case where it raises ValueError: the redactor
//! skips such a URL for the exclusion list (redact.py:185-188) and origin labels fall back to the text
//! (origin.py:96-99), so the refusals are part of the behaviour.

//!
//! The one copy is ferb-py's (`ferb_py::url`); this module keeps the old paths.

pub use ferb_py::url::hostname;
