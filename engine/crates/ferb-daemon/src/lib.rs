//! The daemon's RPC handlers (`ferb/daemon/areas/*.py`), registered on `ferb_rpc::Server`. Stage 1, part 12.
//!
//! - [`shared`]: what both handler groups use (`areas/shared.py`, plus the loop the handlers' state lives on).
//! - [`areas_a`]: memory, memory import, schedules and timers, retention, records, chat history, settings.
//! - [`areas_b`]: the other handler areas.

/// APP-1138, PHN-828: FERB.app's sign-in to the owner's FERB account, kept in the Keychain through FERB's signed helper.
pub mod account;
pub mod areas_a;
pub mod areas_b;
/// T-1274: insomnia mode (APP-980..APP-985): keeping the Mac working while the owner is away.
pub mod insomnia;
pub mod mac_now;
/// Stage 3 S3 (T-1202): the daemon's phone area: paired phones, challenges, the unlock window, proofs, the audit.
pub mod phone;
pub mod shared;
