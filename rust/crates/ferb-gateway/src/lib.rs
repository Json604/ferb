//! FERB's gateway, ported from the Python daemon (`ferb/daemon/gateway.py` and its state files): the one path every
//! model tool call takes (AGENTS.md rules 3 to 7 and 12). It checks the arguments, the caller's grant and scope,
//! takes the facts a tool reports, decides through `ferb-policy` whether the owner must be asked, records the intent
//! before the effect and the outcome after, enforces stop and redacts what is kept.
//!
//! Portable: the catalog, argument checks, the tool trait and registry, pending questions, resource locks, stop.
//! Mac-side (`store`): the call pipeline itself, on the Mac's SQLite store (spans, confirmations, destinations).

pub mod admit;
pub mod args;
#[cfg(target_os = "macos")]
pub mod call_hooks;
pub mod catalog;
pub mod hidden_words;
pub mod listing;
pub mod locks;
pub mod pay_picture;
pub mod pydantic;
pub mod questions;
pub mod reply;
pub mod seams;
pub mod stop;
pub mod tool;
pub mod validators;
pub mod words;

pub mod adapters;
#[cfg(all(feature = "store", target_os = "macos"))]
pub mod gateway;
