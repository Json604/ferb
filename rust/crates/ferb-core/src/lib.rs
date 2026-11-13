//! The engine's foundation, ported from the Python daemon: shared types, ids, errors, timing, call facts and the
//! typed interfaces (ferb/core), FERB's locations (ferb/paths.py), config.toml (ferb/config.py) and the event bus
//! (ferb/bus.py). Checked against fixtures the Python code wrote (rust/fixtures/core, ENG-3).
//!
//! `paths` and `config` are the daemon's own files (owner-only modes, the account database) and exist on Unix
//! only; everything else builds for any target.

pub mod bus;
pub mod call_facts;
#[cfg(unix)]
pub mod config;
pub mod errors;
pub mod ids;
pub mod interfaces;
#[cfg(unix)]
pub mod paths;
pub mod timing;
pub mod types;
pub mod when;

/// Origins (ferb/core/origin.py), ported in `ferb-sanitize`.
pub use ferb_sanitize::origin;
