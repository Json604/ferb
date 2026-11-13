//! FERB's file tools (ferb/act/fs): the operations behind `fs.list`, `fs.read`, `fs.write`, `fs.edit`, `fs.copy`,
//! `fs.move`, `fs.trash`, `fs.look`, `fs.disk_usage` and `recovery.snapshot`, S1 recovery (copies, Trash records,
//! restore, retention, snapshots) and the before-and-after change record a step's check reads (L39). Each tool is one
//! function with typed arguments returning the `ToolResult` the Python tool returned; registering them with the gateway
//! is another part.
//!
//! Seams (what another part supplies):
//! - the protection lists: `ToolContext.path_closed` (ferb-core's `PathCheck`) for the tools, [`Closed`] for the
//!   change record; [`identity`] holds the file-identity half of `ferb/policy/protection.py` they rest on;
//! - the Trash macOS names for a volume: [`files::TrashLocator`] (NSFileManager, in ferb-platform-macos);
//! - whether FERB has Full Disk Access: [`files::FsEnv::full_disk_access`];
//! - picture and PDF decoding for `fs.look`, PDF text for the change record: [`look::Renderer`], [`changes::PdfText`];
//! - `tmutil` for snapshots: [`recovery::Snapshotter`];
//! - the redactor for snapshot words: a closure.
//!
//! macOS-only calls are in [`sys::mac`] (`cfg(target_os = "macos")`); elsewhere they fail as Python's fallbacks did.

pub mod changes;
pub mod difflib;
mod docx;
pub mod files;
pub mod hooks;
pub mod identity;
pub mod look;
pub mod pypath;
pub mod recovery;
pub mod sys;
pub mod usage;

use std::path::Path;

/// The change record's view of the protection lists: whether FERB's shell sandbox denies reading a path
/// (`Protection.read_denied(path) is not None`).
pub type Closed = std::sync::Arc<dyn Fn(&Path) -> bool + Send + Sync>;

/// `repr(text)` as Python writes a str.
pub fn py_repr(text: &str) -> String {
    ferb_sanitize::parity::repr(&ferb_sanitize::Py::Str(ferb_sanitize::PyStr::new(text)))
}

/// `str.casefold()`.
pub fn casefold(text: &str) -> String {
    ferb_sanitize::parity::casefold(text)
}
