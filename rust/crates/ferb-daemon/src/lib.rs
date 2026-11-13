//! The daemon's RPC handlers (`ferb/daemon/areas/*.py`), registered on `ferb_rpc::Server`. Stage 1, part 12.
//!
//! - [`shared`]: what both handler groups use (`areas/shared.py`, plus the loop the handlers' state lives on).
//! - [`areas_a`]: memory, memory import, schedules and timers, retention, records, chat history, settings.
//! - [`areas_b`]: the other handler areas.

pub mod areas_a;
pub mod areas_b;
pub mod shared;
