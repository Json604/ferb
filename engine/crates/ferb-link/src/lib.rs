//! FERB's phone link: the connection between FERB on the owner's Mac and the phone app, over iroh.
//! CONTRACT.md of the phone lab (ferb-phone-link@57b3f34) is the description both sides follow; this crate is both
//! sides (README.md: what changed in the port).
//!
//! Phone side, built for Android and iOS as well as the Mac (kb ENG-5):
//! - `client`: the phone side (dial with backoff, calls, bulk); `ffi`: the app's door, explicit client handles.
//! - `net`, `relaytest`: the endpoint, the relay policy, the tier of a connection, the own-relay test.
//! - `wire`, `errors`, `limits`, `role`, `pairing`, `qr`, `diag`: the protocol both sides share.
//!
//! Mac side, built only on macOS: `bridge` (pairing through the daemon, the daemon's phones, tiers, limits, the daemon
//! as role `phone`), `daemon` (the daemon's socket, as roles `link` and `phone`), `store` and `secrets` (the state and
//! the keys FERB's daemon hands over), and the `ferb-link` program FERB's daemon starts (`src/bin`). Since S4 (kb
//! T-1204) the helper has no socket of its own: the owner reaches it only through FERB's daemon.

/// The feature level inside link version 2: raised whenever a build learns something the other side could ask for
/// (a method, an event, a form of the pairing code). Each side says its own in the hello and the welcome, so a
/// refusal can name the side that is behind. A side that says none is level 0.
/// 1: levels themselves, the short pairing code. 2: each side says which kinds of address it offers for a
/// direct connection and how often it tried (`link.note {net}`, `link.status`).
pub const LEVEL: u32 = 2;
/// Link version 2: hidden answers through the sealed path, relays with their own certificate. A version 1 app is
/// told to update.
pub const VERSION: u32 = 2;
pub const MIN_VERSION: u32 = 2;

/// Which build this is (the commit it was built from, set by the build scripts), for the owner to read.
pub fn build() -> &'static str {
    option_env!("FERB_LINK_BUILD").unwrap_or("dev")
}

pub mod client;
pub mod diag;
pub mod errors;
pub mod ffi;
pub mod limits;
pub mod net;
pub mod pairing;
pub mod qr;
pub mod relaytest;
pub mod role;
pub mod wire;

#[cfg(target_os = "macos")]
pub mod bridge;
#[cfg(target_os = "macos")]
pub mod daemon;
#[cfg(target_os = "macos")]
pub mod secrets;
#[cfg(target_os = "macos")]
pub mod store;
