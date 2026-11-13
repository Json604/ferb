//! Everything of FERB's engine that touches Apple frameworks or macOS's own tools. The engine crates stay
//! portable (they build for iOS and Android too) and see this crate only through the traits they define:
//!
//! - [`security`]: `ferb_rpc::app_peer::Security` on the Security framework (SecCode, requirements).
//! - [`peer`]: a socket peer's pid and audit token from the kernel, for `ferb_rpc::Server::set_peer_lookup`.
//! - [`rpc`]: both of those put in place on a server.
//! - [`identity`]: `ferb_identity::helper::Launcher` on `/usr/bin/codesign` and a child process (the signed
//!   helper that keeps values in the login Keychain), and where Chrome keeps its profiles.
//!
//! The whole crate is `cfg(target_os = "macos")`: on any other target it is empty, so the workspace still
//! builds there.
#![cfg(target_os = "macos")]

pub mod identity;
pub mod peer;
pub mod rpc;
pub mod security;
