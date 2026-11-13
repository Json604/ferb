//! FERB's daemon program (`ferb/daemon/main.py`, `ferb/daemon/__main__.py`, `ui/mac/FERBAgent/main.c`): the
//! composition root that builds every engine crate from FERB's home and serves the RPC socket, and the program's
//! other entry modes (the models' tool bridge, the internal commands FERB.app calls).
//!
//! - [`home`]: which FERB home, where output goes.
//! - [`host`]: inside FERB.app, what `ui/mac/FERBAgent/main.c` guaranteed (entry modes, home, environment).
//! - [`basics`]: the shared store connection, journal, events and the global stop.
//! - [`compose`]: [`compose::Daemon::start`] and `shutdown`, the gateway and its tools, the handler areas' seams,
//!   the runtime factory, the background loops.
//! - [`log`]: what the daemon writes to its own output (logs/daemon.err.log): its start and stop, refusals, panics.
//! - [`off`]: the seams deliberately left off, each named in `seams-off.txt` with why (the wiring guard).
//! - [`run`]: the program's modes (`daemon`, `mcp`, the internal commands).

pub mod areas_c;
pub mod basics;
pub mod cleanup_cards;
pub mod compose;
pub mod device;
pub mod home;
pub mod host;
pub mod log;
pub mod mainloop;
#[cfg(target_os = "macos")]
pub mod native;
pub mod off;
pub mod persona;
pub mod resources;
pub mod run;
pub mod setup_env;
#[cfg(feature = "stand-ins")]
pub mod standins;
#[cfg(feature = "stand-ins")]
pub mod standins_c;
#[cfg(feature = "stand-ins")]
pub mod standins_chat;
