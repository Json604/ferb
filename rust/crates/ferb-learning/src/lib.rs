//! FERB's memory beyond search: the shared save path for memory items, the core profile (Tier 1), learned nodes and
//! episode cards, the daily distill, playbooks, the Mac profile, the memory import job and the smaller stores around
//! them. Ported from `ferb/memory/` (items.py, profile.py, learning.py, lineage.py, distill.py, playbooks.py,
//! summary.py, questions.py, journal.py, calls.py, card_reader.py, export.py, import_job.py, mac_profile.py, tools.py,
//! upgrade.py, dashboard.py) and checked against what the Python code produced (`rust/fixtures/learning/make.py`,
//! ENG-3). Search, embeddings and the import readers are `ferb-memory`'s.
//!
//! Portable (no feature): [`text`] (word and clause rules, capture), [`lineage`], [`notes`] (how-notes, search text,
//! a node's prompt line), [`calls`] (one model call through the runtime trait), the journal's and summaries' origin
//! rules. Feature `store`: everything that reads and writes the Mac's SQLite store.
//!
//! Seams (other parts' code, passed in): [`Clock`], [`Ids`], the redactor, the local zone
//! (`ferb_memory::times::LocalZone`), the runtime factory ([`calls::MakeRuntime`]), spans ([`calls::Spans`]), the
//! owner's notice line (`Tell`), episodes and trail-source origins (`learning::Seams`), test runs and the security
//! digest (`summary::Seams`), the embedder (`ferb_memory::embeddings::Embedder`), the import's reader, similarity
//! search and busy check (`import_job::Seams`), and the Mac facts themselves (gathered natively, `mac_profile`).

// The seams are boxed callbacks and Python's keyword-heavy signatures are kept call for call.
#![allow(clippy::type_complexity, clippy::too_many_arguments)]

pub mod calls;
pub mod lineage;
pub mod notes;
pub mod py;
pub mod text;

#[cfg(feature = "store")]
pub mod dashboard;
#[cfg(feature = "store")]
pub mod distill;
#[cfg(feature = "store")]
pub mod export;
#[cfg(feature = "store")]
pub mod import_job;
#[cfg(feature = "store")]
pub mod items;
pub mod journal;
#[cfg(feature = "store")]
pub mod learning;
#[cfg(feature = "store")]
pub mod mac_profile;
#[cfg(feature = "store")]
pub mod playbooks;
#[cfg(feature = "store")]
pub mod profile;
#[cfg(feature = "store")]
pub mod questions;
#[cfg(feature = "store")]
pub mod store;
pub mod summary;
#[cfg(feature = "store")]
pub mod tools;
#[cfg(feature = "store")]
pub mod upgrade;

pub use ferb_py::time::DateTime;

/// The clock memory reads `now` from (`ferb.core.interfaces.Clock.now`).
pub trait Clock {
    fn now(&self) -> DateTime;
}

/// Injected ids (`IdSource.new_id`).
pub trait Ids {
    fn new_id(&self) -> String;
}

/// What a memory call refused or could not do, by the Python exception it stands for: `class()` is that class's
/// name and `Display` its `str(e)`, so the text FERB composes from it (`f"{type(e).__name__}: {e}"`) is the same.
#[derive(Debug, Clone, PartialEq)]
pub enum Error {
    /// `MemoryRefused`.
    MemoryRefused(String),
    /// `LearningRefused`.
    LearningRefused(String),
    /// `ValueError` (and `ImportBusy`, a ValueError, with its own class name).
    Value(String),
    ImportBusy(String),
    /// `ImportUnreadable`.
    ImportUnreadable(String),
    /// `InvalidAnswer` (a model's answer outside its schema).
    InvalidAnswer(String),
    /// `RuntimeError` (a runtime's error event).
    Runtime(String),
    /// A FERB error from another part (`InputTooLarge`, `ModelLimitsUnknown`, ...).
    Ferb(ferb_core::errors::FerbError),
    /// An SQLite error (`sqlite3.OperationalError` and the like).
    Sql(String, String),
    /// `json.JSONDecodeError` from reading a model's text.
    Json(String),
    /// An exception Python raises where nothing catches it (`TypeError`, `KeyError`, `AttributeError`, ...).
    Crash(&'static str, String),
}

impl Error {
    /// The Python class name.
    pub fn class(&self) -> &str {
        match self {
            Error::MemoryRefused(_) => "MemoryRefused",
            Error::LearningRefused(_) => "LearningRefused",
            Error::Value(_) => "ValueError",
            Error::ImportBusy(_) => "ImportBusy",
            Error::ImportUnreadable(_) => "ImportUnreadable",
            Error::InvalidAnswer(_) => "InvalidAnswer",
            Error::Runtime(_) => "RuntimeError",
            Error::Ferb(e) => e.class(),
            Error::Sql(class, _) => class,
            Error::Json(_) => "JSONDecodeError",
            Error::Crash(class, _) => class,
        }
    }

    /// `f"{type(e).__name__}: {e}"`.
    pub fn described(&self) -> String {
        format!("{}: {}", self.class(), self)
    }

    /// The error a refusal of `ValueError`'s family is: what callers catch as `(MemoryRefused, ValueError)`.
    pub fn is_value_error(&self) -> bool {
        matches!(
            self,
            Error::MemoryRefused(_)
                | Error::LearningRefused(_)
                | Error::Value(_)
                | Error::ImportBusy(_)
                | Error::ImportUnreadable(_)
                | Error::InvalidAnswer(_)
                | Error::Json(_)
        )
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::MemoryRefused(m)
            | Error::LearningRefused(m)
            | Error::Value(m)
            | Error::ImportBusy(m)
            | Error::ImportUnreadable(m)
            | Error::InvalidAnswer(m)
            | Error::Runtime(m)
            | Error::Json(m) => f.write_str(m),
            Error::Ferb(e) => write!(f, "{e}"),
            Error::Sql(_, m) | Error::Crash(_, m) => f.write_str(m),
        }
    }
}

impl std::error::Error for Error {}

impl From<ferb_memory::Error> for Error {
    fn from(e: ferb_memory::Error) -> Error {
        match e {
            ferb_memory::Error::Refused(m) => Error::Value(m),
            ferb_memory::Error::Unreadable(m) => Error::ImportUnreadable(m),
            ferb_memory::Error::Crash(kind) => {
                Error::Crash(kind, format!("{kind} on an unexpected shape"))
            }
            ferb_memory::Error::Io(e) => Error::Crash("OSError", e.to_string()),
            #[cfg(feature = "store")]
            ferb_memory::Error::Sql(e) => Error::from(e),
        }
    }
}

impl From<ferb_memory::pyval::Crash> for Error {
    fn from(c: ferb_memory::pyval::Crash) -> Error {
        Error::Crash(c.0, format!("{} on an unexpected shape", c.0))
    }
}

#[cfg(feature = "store")]
impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Error {
        let class = match &e {
            rusqlite::Error::SqliteFailure(f, _)
                if f.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                "IntegrityError"
            }
            _ => "OperationalError",
        };
        Error::Sql(class.into(), e.to_string())
    }
}

#[cfg(feature = "store")]
impl From<ferb_store::Error> for Error {
    fn from(e: ferb_store::Error) -> Error {
        match e {
            ferb_store::Error::Sqlite(e) => Error::from(e),
            other => Error::Crash("OSError", other.to_string()),
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;
