//! FERB's Mac hands: a port of `ferb/act/mac/`, `ferb/act/listen.py`, `ferb/act/recording.py`, `ferb/act/point.py`,
//! the lock, after-check, ask and catalog tools, `ferb/act/evidence/`, `ferb/device/permissions.py`,
//! `ferb/device/autorelease.py` and `ferb/sanitize/images.py` (Python lines at `63369cb3`).
//!
//! The logic is pure and compiles anywhere; every Apple framework call is in [`native`] (macOS only) behind the
//! traits the logic takes (`driver::Ax`, `driver::Apps`, `driver::Shots`, `ocr::Recognize`, `ocr::Paint`,
//! `clipboard::Pasteboard`, `permissions::AutomationFn`, `listen::Capture`), so tests drive it with fakes.
//!
//! Rules kept from the Python (AGENTS.md rules 5, 7, 10): background first (input goes to one process or window;
//! the foreground route is taken only with the owner's Yes, waits for the owner to be quiet and stops when they take
//! over); secrets never leave (password fields report filled/empty, a capture whose secret fields can't be painted is
//! withheld, OCR masks what text recognition finds); tools report facts and never judge intent.

// Signatures follow the Python functions they port (their arguments and callbacks), so some are long.
#![allow(
    clippy::type_complexity,
    clippy::too_many_arguments,
    clippy::result_unit_err
)]

pub mod aftercheck;
pub mod applescript;
pub mod apps;
pub mod ax;
pub mod driver;
pub mod error;
#[cfg(target_os = "macos")]
pub mod evidence;
pub mod input;
pub mod keys;
pub mod launched;
pub mod listen;
pub mod locks;
pub mod mask;
pub mod ocr;
pub mod permissions;
pub mod point;
pub mod profile;
pub mod recording;
pub mod shots;
pub mod tools;
pub mod wav;

#[cfg(target_os = "macos")]
pub mod native;
