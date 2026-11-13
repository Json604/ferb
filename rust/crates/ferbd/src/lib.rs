//! FERB's daemon program (`ferb/daemon/main.py`, `ferb/daemon/__main__.py`, `ui/mac/FERBAgent/main.c`): the
//! composition root that builds every engine crate from FERB's home and serves the RPC socket, and the program's
//! other entry modes (the models' tool bridge, the internal commands FERB.app calls).
//!
//! - [`home`]: which FERB home, where output goes.
//! - [`basics`]: the shared store connection, journal, events and the global stop.
//! - [`compose`]: [`compose::Daemon::start`] and `shutdown`, the gateway and its tools, the handler areas' seams,
//!   the runtime factory, the background loops.
//! - [`run`]: the program's modes (`daemon`, `mcp`, the internal commands).

pub mod basics;
pub mod compose;
pub mod device;
pub mod home;
pub mod run;
#[cfg(feature = "stand-ins")]
pub mod standins;
