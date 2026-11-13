//! FERB's device setup and its internal commands, ported from the Python build (ferb/device, ferb/cli) and checked
//! against fixtures the Python code wrote (rust/fixtures/device, ENG-3).
//!
//! - [`install`]: the background service's definition (the user LaunchAgent, or FERB.app's own registration),
//!   `service.json`, the native-messaging host's launcher and manifests, the secret helper, the pinned Codex, the
//!   `ferb` link, uninstall and purge. Every external program goes through a [`install::Runner`] seam.
//! - [`profile`], [`scan`], [`persona`], [`devtools`], [`extensions`]: the device profile, the app scan, the
//!   persona facts, the developer-tools facts and FERB's extension folder.
//! - [`cli`]: the `ferb` program's argument parser and the commands FERB.app runs (`setup --service`, `setup
//!   --cli`, `uninstall`, the sign-outs, the clean-up), plus the developer toolbox's pure RPC clients.
//! - `grant_probe` (Mac only): the fresh reading of Screen Recording and Input Monitoring (APP-662).
//!
//! Permission facts (ferb/device/permissions.py) and the autorelease pools are ported in `ferb-mac-hands`; this
//! crate takes them through [`cli::setup::PermissionFacts`].

pub mod cli;
pub mod devtools;
pub mod extensions;
#[cfg(target_os = "macos")]
pub mod grant_probe;
pub mod install;
pub mod persona;
pub mod plistlib;
pub mod profile;
pub mod py;
pub mod scan;
pub mod selftest;
