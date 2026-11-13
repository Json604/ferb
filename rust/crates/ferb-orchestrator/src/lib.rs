//! Orchestration's building blocks (rust/PLAN.md stage 1, part 8): everything in ferb/orchestrator except
//! supervisor.py and worker.py, which a later part ports on top of this crate, and ferb/decide. What these modules
//! write to the store, compose for a model or the owner, or answer about a model's answer is the contract (ENG-3):
//! `rust/fixtures/orchestrator/make.py` records what the Python build produces.
//!
//! Portable: the store-backed modules sit behind the `store` feature (ferb-store, bundled SQLite: Mac-side).

pub mod answer_marks;
pub mod attempts;
#[cfg(feature = "store")]
pub mod attention;
pub mod availability;
pub mod browsing;
#[cfg(feature = "store")]
pub mod cards;
pub mod context_layers;
#[cfg(feature = "store")]
pub mod continuity;
#[cfg(feature = "store")]
pub mod conversation_tools;
#[cfg(feature = "store")]
pub mod conversations;
#[cfg(feature = "store")]
pub mod decide;
#[cfg(feature = "store")]
pub mod episodes;
#[cfg(feature = "store")]
pub mod evidence;
pub mod guards;
pub mod language;
#[cfg(feature = "store")]
pub mod ledger;
pub mod leftovers;
pub mod lines;
pub mod loops;
pub mod models;
pub mod names;
pub mod parts;
#[cfg(feature = "store")]
pub mod plan_executor;
#[cfg(feature = "store")]
pub mod plan_store;
pub mod plans;
pub mod ports;
pub mod probes;
pub mod progress;
pub mod py;
mod pydantic;
pub mod reply_stream;
pub mod skills;
#[cfg(feature = "store")]
pub mod sql;
pub mod steering;
pub mod stop;
#[cfg(feature = "store")]
pub mod test_runs;
#[cfg(feature = "store")]
pub mod tree;
pub mod validate;
#[cfg(feature = "store")]
pub mod verify;

pub use models::*;
