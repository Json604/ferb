//! FERB's time, ported from `ferb/clock/` and the calendar arithmetic of `ferb/orchestrator/schedules.py`
//! (Stage 1, part 9; decisions ORCH-320, ORCH-334, ORCH-511, ORCH-550..556, ORCH-631..633, ORCH-639, DMN-547,
//! DMN-548, TEST-508).
//!
//! - [`clock`]: the injected wall and monotonic clock ([`clock::SystemClock`], [`clock::FakeClock`]).
//! - [`zone`]: IANA zones on a built-in tz database, local wall times with Python's `fold=0`.
//! - [`when`]: the owner's "when" turned into an absolute time by code, never by a model.
//! - [`context`]: the time facts given to models.
//! - [`schedules`]: rules, several daily times, random windows, ends, the words code writes about them.
//! - [`wait`]: `wait.for`, waiting in code on named conditions.
//! - [`tools`]: `time.remind_at`, `time.set_timer`, `time.sleep_until`.
//! - [`timers`], [`schedule_store`] (feature `store`, Mac-side: bundled SQLite): the durable rows, byte for byte as
//!   Python writes them, and the claim-and-handoff that closes the scheduled-run crash window.
//! - [`py`]: Python's own behavior these lean on (rounding, `strftime`, `repr`, `casefold`, `random.sample`).
//!
//! Portable: nothing here calls an OS API beyond `std`; the Mac's presence comes in through
//! [`timers::Presence`], its zone through [`zone::Zone`].

pub mod clock;
pub mod context;
mod generated;
pub mod py;
pub mod pyrandom;
pub mod schedules;
pub mod tools;
pub mod wait;
pub mod when;
pub mod zone;

#[cfg(feature = "store")]
pub mod schedule_store;
#[cfg(feature = "store")]
pub mod timers;
