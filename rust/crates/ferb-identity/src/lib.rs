//! FERB's identity storage and sealed secrets: a port of `ferb/identity/` (Python lines at
//! `fa2d8bb1`). It must not expose credentials or choose accounts (`ferb/identity/__init__.py`).
//!
//! - [`helper`]: the caller of the pinned, signed Swift secrets helper (`helper.py`). The helper
//!   itself stays Swift and unchanged (decisions: GATE-548, REL-404). Verifying and running it is
//!   the platform's part, behind [`helper::Launcher`] (`ferb-platform-macos`).
//! - [`owner_secrets`]: the owner's saved secrets, their names and places (`owner_secrets.py`).
//! - [`secrets`] and [`origin`]: FERB's own configured secrets and origin comparison (`secrets.py`).
//! - [`seal`]: sealed fills and the sealed mail feed (`seal.py`).
//! - [`mailbox`]: the unread-mail code inventory (`mailbox.py`).
//! - [`seed`]: the check of FERB's dedicated Chrome profile (`seed.py`).
//! - [`secret_tools`]: what the `secrets.list` tool answers (`secret_tools.py`).
//!
//! AGENTS.md rule 7: a secret never reaches a model, a trace, a log or the database. Every value
//! is a [`Secret`] from the moment this crate holds it: zeroed on drop, printed as a fixed
//! marker, absent from every error.
//!
//! Compared with the Python code through `rust/fixtures/identity/make.py` (decision: ENG-3); the
//! tests never touch the Keychain, the installed helper or `~/.ferb`.

pub mod helper;
pub mod mailbox;
pub mod origin;
pub mod owner_secrets;
pub mod py;
pub mod seal;
pub mod secret_tools;
pub mod secrets;
pub mod seed;

mod secret;

use std::{fmt, io};

pub use helper::{Helper, HelperFacts, Launcher, Service};
pub use owner_secrets::{Keychain, OwnerSecrets, Places};
pub use secret::{Secret, MARKER};

/// Why a call gave nothing. No variant ever holds a secret's value, a request or a Keychain
/// account.
#[derive(Debug)]
pub enum Error {
    /// Python's `Refused` (`ferb/core/errors.py:61`): a safety check said no. The message is safe
    /// to show a worker.
    Refused(String),
    /// Python's `HelperFailed` (`helper.py:21-26`), a `Refused`: the helper ran and the operation
    /// failed; `facts` is what is known about why (decision: HAND-410).
    HelperFailed { message: String, facts: HelperFacts },
    /// Not a refusal: the helper, the verifier or a file could not be started, read or written
    /// (Python lets the `OSError` through).
    Io(io::Error),
}

impl Error {
    /// The refusal's words, for the two variants Python raises as `Refused`.
    pub fn refusal(&self) -> Option<&str> {
        match self {
            Error::Refused(message) | Error::HelperFailed { message, .. } => Some(message),
            Error::Io(_) => None,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Refused(message) | Error::HelperFailed { message, .. } => f.write_str(message),
            Error::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Io(e)
    }
}

pub type Result<T> = std::result::Result<T, Error>;
