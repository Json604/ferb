//! FERB's browser hands: a port of `ferb/act/browser/` (Python lines at `fa2d8bb1`). The other end
//! is the unchanged Chromium extension (`extensions/chromium`, manifest 0.19.10, protocol 21,
//! minimum 0.19.0), reached through the native host (`ferb-native-host`) and the daemon's socket.
//!
//! - [`protocol`]: the wire schema, both HMACs, the signed daemon envelope (`ext_protocol.py`).
//! - [`lax`]: pydantic's lax validation of the extension's messages.
//! - [`pairing`]: bindings, single-use tokens, anti-replay counters (`pairing.py`), with the
//!   store's `ext_installs` table behind [`pairing::InstallStore`].
//! - [`seal`]: sealed fills (`ferb_identity::seal`, the same construction protocol.js opens).
//! - [`rules`]: the small rules and words (group labels, refusal reasons).
//! - [`keys`]: the extension's key table (`keyEvent`, `entersText`: HAND-218).
//! - [`money`]: amounts as a page writes them (GATE-578).
//! - [`mask`]: painting a capture's secret fields over (L13); the image library is the platform's.
//! - [`bridge`]: the extension connection: hub, bridges, commands, generations (`ext_bridge.py`
//!   `Hub` and `Bridge`).
//! - [`driver`], [`ops`], [`capture`], [`upload`], [`owner`]: the tab drivers (`_TabDriver`, `Router`,
//!   `FerbIdentity`): task-owned tabs and every page operation.
//! - [`tools`]: the model-facing tools, `browser.*` and `ferb_browser.*` (`tools.py`).
//! - [`profiles`], [`autoinstall`]: profiles and the resolver (`profiles.py`, `resolver.py`), the
//!   extension's install requests and package (`autoinstall.py`).
//! - [`mime`]: `mimetypes.guess_type` for an upload's file type.
//!
//! Portable: plain tokio and std. What belongs to another part comes in through a trait (listed in
//! each module); the Mac's SQLite and the Unix socket are behind `cfg`.
//!
//! AGENTS.md rule 7: pairing keys and sealed values are never printed (`Debug` shows a marker),
//! logged or put in an error.

pub mod autoinstall;
pub mod bridge;
pub mod capture;
pub mod driver;
pub mod error;
pub mod keys;
pub mod lax;
pub mod mask;
pub mod mime;
#[rustfmt::skip] // written by rust/fixtures/browser/hands.py
mod mime_table;
pub mod money;
pub mod ops;
pub mod owner;
pub mod pairing;
pub mod profiles;
pub mod protocol;
pub mod rules;
pub mod seal;
pub mod snapshot;
pub mod tools;

#[rustfmt::skip] // written by rust/fixtures/browser/make.py
mod generated;
pub mod upload;
pub mod url;

pub use error::{Error, Result};
