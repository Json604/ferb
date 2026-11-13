//! FERB's perception, ported from `ferb/sense` and checked against fixtures the Python code wrote
//! (`rust/fixtures/sense`, decision: ENG-3).
//!
//! The engine (any target): what a watcher saw ([`Observation`]), the owner's exclusions applied before anything is
//! kept ([`exclusions`]), who caused it ([`attribution`]), capture guards and typed-field bursts ([`capture`]), today's
//! activity file ([`daily`]), Jev's batches ([`batches`], switched off: SENSE-2, SENSE-3), the watcher manager
//! ([`manager`]), the trail tools' typed functions ([`tools`]), the front window at send ([`front_window`]), whether the
//! Mac has a route ([`network`]), installed-software facts ([`software`]) and each watcher's mapping from what macOS
//! reports to an observation ([`watchers`]).
//!
//! Mac-side (feature `store`): the trail itself ([`trail`]: the append-only rows, receipts, queries, purges),
//! notification context ([`notifications`]), perception's notification path and text expiry ([`perception`]) and the
//! daemon side of transcription ([`transcribe`]).
//!
//! macOS sources ([`mac`], `cfg(target_os = "macos")`): NSWorkspace notifications, AX observers on their own
//! CFRunLoop thread, FSEvents, the pasteboard's change count, the owner-input tap, the secure-input check, the session
//! and display state, window lists and process facts. Each thread drains its own autorelease pool.
//!
//! Rule 7 (AGENTS.md): text is redacted at capture through the [`Redact`] seam (ferb-sanitize's `Redactor`), and a
//! secure field is never read. Rule 10: nothing here brings a window forward, moves the cursor or types.

// The seams are closures and boxed traits, named where they are declared.
#![allow(clippy::type_complexity)]

pub mod attribution;
pub mod batches;
pub mod capture;
pub mod daily;
pub mod exclusions;
pub mod front_window;
pub mod manager;
pub mod network;
pub mod observation;
pub mod paths;
pub mod software;
pub mod tools;
pub mod watchers;

#[cfg(feature = "store")]
pub mod notifications;
#[cfg(feature = "store")]
pub mod perception;
#[cfg(feature = "store")]
pub mod trail;
#[cfg(feature = "store")]
pub mod transcribe;

#[cfg(target_os = "macos")]
pub mod mac;

pub use observation::{Dict, Observation, Redact};
