//! FERB's memory: local embeddings, the fused search and the import readers, ported from the Python build
//! (`ferb/memory/embeddings.py`, `search.py`, `import_sources.py`, `import_extract.py`; MEM-6, MEM-25..27,
//! MEM-400..415). Checked against what the Python code produced (`rust/fixtures/memory/make.py`, ENG-3).
//!
//! - [`tokenizer`], [`embeddings`]: WordPiece, the static table read by mmap, int8 and sign-bit vectors, ranking.
//! - [`search`]: the pure helpers, and (feature `store`) [`search::MemorySearch`] over the store's connection.
//! - [`import_sources`]: the five readers (ChatGPT, claude.ai, Gemini, Claude Code, Codex), in memory only.
//! - [`import_extract`]: parts, chunks, the owner index, the checks of the model's recall and verify answers.
//!
//! Seams (other parts' code, passed in): the clock and [`search::FactLookup`] (items), the learned nodes' text
//! (`learning.search_text`), the redactor, a Zstandard decoder for compressed Codex rollouts
//! ([`import_sources::Unzstd`]), this device's zone ([`times::LocalZone`]), and the import's own save (items).
//! Python's text rules come from tables Python wrote ([`generated`]); Python's `json.dumps`, times and numbers
//! from `ferb-py`.

#[rustfmt::skip]
#[allow(clippy::all)]
mod generated;

pub mod embeddings;
pub mod html;
pub mod import_extract;
pub mod import_sources;
pub mod pymath;
pub mod pystr;
pub mod pyval;
pub mod search;
pub mod terms;
pub mod times;
pub mod tokenizer;
#[cfg(feature = "store")]
pub mod vectors;
mod zip;

/// What a memory call refused or could not do.
#[derive(Debug)]
pub enum Error {
    /// A refusal in Python's words (`ValueError` with a message the caller shows or logs).
    Refused(String),
    /// The export isn't what it should be (`ImportUnreadable`, said in the owner's terms).
    Unreadable(String),
    /// An exception Python raises there and nothing catches (its type name).
    Crash(&'static str),
    Io(std::io::Error),
    #[cfg(feature = "store")]
    Sql(rusqlite::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Refused(m) | Error::Unreadable(m) => f.write_str(m),
            Error::Crash(kind) => write!(f, "{kind} on an unexpected shape"),
            Error::Io(e) => write!(f, "{e}"),
            #[cfg(feature = "store")]
            Error::Sql(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<pyval::Crash> for Error {
    fn from(c: pyval::Crash) -> Error {
        Error::Crash(c.0)
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Error {
        Error::Io(e)
    }
}

#[cfg(feature = "store")]
impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Error {
        Error::Sql(e)
    }
}
